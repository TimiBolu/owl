# WalIntentRing Design For Owl

## Purpose

This document defines a narrow replacement candidate for the current append-ingress side
of Owl's lock-free WAL path.

It does **not** redesign Owl's full durability subsystem.
It does **not** replace WAL framing, superblock persistence, crash recovery, or recovery
replay semantics.

Instead, it proposes a more mechanically appropriate ingress path for many concurrent
producers feeding one WAL file owner.

The central idea is simple:

> producers do not append directly to the WAL file and do not contend on a shared
> append queue; they publish `WalIntent` messages into dedicated rings, and a single
> WAL writer drains those rings and performs the actual durable write protocol.

This design should be evaluated first against the current implementation in:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`

## Scope

### In scope

- producer-to-WAL-writer handoff
- intent message format
- producer registration model
- batch drain and flush behavior
- completion notification model
- durability boundary
- benchmark comparison against current `LockFreeWAL`

### Out of scope

- changing on-disk WAL format
- changing crash-recovery semantics
- changing checkpoint semantics
- replacing the entire transaction system
- replacing arena allocation
- replacing distributed WAL design at the protocol level

This is intentionally a narrow design.

## Problem With The Current `LockFreeWAL`

The current lock-free WAL implementation improves some contention paths, but it still has
an awkward shape.

### What it does now

In `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`:
- producers reserve LSNs atomically
- producers reserve file offsets atomically
- append operations may be enqueued in a lock-free linked queue
- flushing swaps the queue out and writes buffered entries
- final durability still depends on one sink interacting with one file

### Why that is not ideal

1. **The file sink is still single-owner in reality**
- only one entity can sensibly own write ordering and fsync policy for a WAL file
- pretending every producer participates equally in the final sink path creates extra
  coordination complexity

2. **The queue is pointer-heavy**
- `AppendOp` nodes require allocation and linked-queue maintenance
- this adds allocation churn and CAS traffic to the hot path

3. **Reservation and sink behavior are mixed too early**
- producers know too much about append mechanics
- the final sink still has to batch, write, and sync coherently anyway

4. **It is only partially lock-free in the part that matters**
- the queue may be lock-free, but the durable I/O boundary is still serialized

This makes it a strong candidate for ring-decomposed ingress.

## Design Overview

`WalIntentRing` uses a ring-decomposed MPSC shape:

- each producer or execution lane gets one dedicated SPSC ring
- a single WAL writer owns the WAL file
- the WAL writer polls all active producer rings
- the WAL writer builds batches, assigns write order, writes framed records, and decides
  when to fsync

This is not just a faster queue. It is a better ownership model for a single WAL sink.

## Core Architecture

```text
Producer 0 ──► [Intent Ring 0] ──┐
Producer 1 ──► [Intent Ring 1] ──┼──► WAL Writer ──► WAL file ──► fsync / checkpoint
Producer 2 ──► [Intent Ring 2] ──┤
Producer N ──► [Intent Ring N] ──┘
```

### Ownership rule

- producers own only publication into their ring
- the WAL writer owns:
  - final write order
  - byte offset assignment
  - batch construction
  - file writes
  - fsync policy
  - commit visibility of durable completion

This is the most important rule in the design.

## Why Ring Decomposition Fits This Boundary

A WAL file is fundamentally a single ordered stream.

That means the storage engine already wants:
- one durable stream owner
- one place where flush policy lives
- one place where ordering is finalized

Ring decomposition matches that reality by removing producer-producer contention before
that sink.

This is the same lesson from `/Users/timiadesina/Documents/owl/ringmpsc-main/src/channel.zig`:
- many producers should not fight over one shared enqueue structure when a single sink
  already exists

## Proposed Types

## `WalIntent`

A `WalIntent` is a producer-published description of a WAL append request.

Suggested first shape:

```text
WalIntent {
  producer_id
  sequence_local
  requested_lsn_mode
  entry_type
  objref_or_global_ref
  payload_ptr_or_inline
  payload_len
  durability_class
  completion_token
}
```

### Fields

- `producer_id`
  - stable ring owner id
- `sequence_local`
  - monotonically increasing sequence inside that producer ring
  - useful for debugging, completion matching, and correctness checks
- `requested_lsn_mode`
  - usually "assign by writer"
  - future-proofing if special cases ever need explicit ordering hints
- `entry_type`
  - insert/update/delete/checkpoint/commit/etc.
- `objref_or_global_ref`
  - object identity relevant to the entry
- `payload_ptr_or_inline`
  - pointer to payload memory or small inline payload area
- `payload_len`
  - byte size
- `durability_class`
  - immediate, buffered, checkpoint-coupled, or similar
- `completion_token`
  - how the writer reports success/failure back

The first implementation should keep this small and fixed-size where possible.

## `WalIntentCompletion`

A producer needs to know whether the intent has:
- been accepted into the ring
- been written to the WAL file
- become durable if required

Suggested first completion record:

```text
WalIntentCompletion {
  producer_id
  sequence_local
  final_lsn
  status
  durable
}
```

### Completion status

At minimum:
- `accepted`
- `written`
- `durable`
- `failed`

For the first version, producers do not need all intermediate states if the API is kept
simple. It may be enough that the producer blocks or waits only until `durable` or
`failed`.

## Producer Model

### Registration

The producer set should be explicit and relatively stable.

Recommended first rule:
- pre-register a bounded set of producer lanes for the WAL writer
- each lane owns one ring

This matches the `ringmpsc` design and avoids dynamic ring churn.

### Publication path

Producer hot path:
1. reserve one or more slots in its private ring
2. write `WalIntent` directly into the reserved slot(s)
3. commit publication
4. optionally wait for completion if required by durability semantics

This gives Owl:
- no producer-producer queue contention
- no append-node allocation on the hot path
- stable memory layout

## WAL Writer Model

The WAL writer is a dedicated owner thread or dedicated execution lane.

### Responsibilities

The WAL writer alone:
- drains producer rings
- assigns final ordering
- assigns file offsets
- serializes/framed-writes WAL records
- decides when to flush and fsync
- publishes completion state back to producers

### Polling strategy

First version recommendation:
- round-robin drain across active rings
- bounded per-ring drain to avoid starvation
- immediate flush when required by durability class
- buffered batch flush when thresholds are met

This should be simple first. If fairness becomes a real problem, improve it after
measurement.

## Ordering Semantics

This is where discipline matters.

### What must be true

1. WAL file order is the order chosen by the WAL writer.
2. Per-producer local order must be preserved.
3. Global total order is defined only by the final WAL stream.

This fits ring decomposition well:
- each producer ring naturally preserves FIFO
- the writer determines the interleaving between rings

### Important consequence

The writer, not the producer, should assign final WAL sequence ordering.

That means the cleanest design is:
- producer publishes intent without a final LSN
- writer assigns the final LSN during drain or batch assembly

This is cleaner than having producers fetch a global LSN first and then race toward a
single sink.

## Offset Assignment

The same logic applies to byte offsets.

Recommended rule:
- producers do **not** reserve file offsets
- the writer reserves or computes offsets while assembling the batch

This centralizes file layout policy in one place and removes another shared atomic from
the producer hot path.

## Batch And Flush Policy

This is one of the main reasons to adopt this design.

### Writer-side batch policy

The WAL writer should build batches using these thresholds:
- `max_entries`
- `max_bytes`
- `max_wait_ns` or `max_wait_ms`
- durability-class overrides

### Suggested first durability classes

1. `immediate`
- write this intent and fsync before acknowledging durable completion

2. `buffered`
- write in batch and acknowledge when durable under normal batch flush policy

3. `checkpoint_linked`
- allowed to be coalesced with checkpoint-oriented flush policy if Owl later wants this

The first implementation can even start with just:
- `immediate`
- `buffered`

That is enough to test the model.

## Durability Boundary

This must stay explicit.

A producer publishing into a ring has **not** made anything durable.

A durable completion only occurs after:
1. writer serializes the intent into framed WAL bytes
2. writer writes bytes to the WAL file
3. writer performs the required sync for that durability class
4. writer publishes a successful completion

This line must stay sharp.

## Failure Semantics

A `WalIntent` can fail at several points:

1. publication failure
- ring full or closed

2. write failure
- serialization or file write error

3. durability failure
- fsync or file sync error

4. shutdown interruption
- writer exits before publishing success

Recommended first rule:
- producer-visible success means durable success if the API requested durable completion
- otherwise the producer must treat the operation as not committed

That keeps semantics strict.

## Payload Ownership

This is an important engineering decision.

### Options

1. **Inline small payloads**
- good for tiny entries
- avoids extra indirection
- makes ring slot larger

2. **Pointer to producer-owned payload buffer**
- keeps ring slots small
- requires ownership and lifetime discipline until completion

3. **Pointer to arena or shared staging region**
- more complex but can align with zero-copy goals

### Recommendation

For the first prototype:
- small fixed inline payload for very small entries
- pointer-plus-length for larger payloads
- producer must keep payload valid until completion

This keeps the first prototype practical without prematurely over-abstracting.

## Completion Signaling

There are several ways to signal completion.

### Option A. Atomic completion slots per producer

Each producer maintains a completion array or completion cursor.
The writer stores completion results into producer-owned completion memory.

Pros:
- simple
- cache-local to each producer
- no extra queue needed

Cons:
- completion protocol must be designed carefully

### Option B. One completion ring back to each producer

Symmetric design, but probably heavier than needed.

### Recommendation

Start with **atomic completion slots or a per-producer completion sequence**.
This is enough for a first prototype and avoids adding a second queueing system.

## Interaction With Existing WAL Framing

The `WalIntentRing` design should preserve the current durable format work.

That means:
- reuse the framed record shape already evolving in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/c_api.zig`
- reuse recovery logic in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/wal.zig`
  and `/Users/timiadesina/Documents/owl/owl_storage_zig/src/crash_recovery.zig`
- do not change recovery semantics in the first prototype

This is important. We are changing the ingress, not the storage contract.

## Interaction With Transactions

This design does not replace transaction semantics.

What it changes:
- how transaction-related WAL entries reach the file owner

What it does not change:
- prepare/commit/abort meaning
- visibility rules
- MVCC semantics

If transaction commit currently depends on WAL durability, then the producer-side commit
path should wait for the corresponding durable completion from the WAL writer.

## Why This Is Better Than The Current `LockFreeWAL`

### 1. Less allocation churn

Current `LockFreeWAL` uses append operations as heap-managed linked nodes.

`WalIntentRing` uses bounded ring slots.

That should reduce:
- allocation traffic
- pointer chasing
- queue-node cache misses

### 2. Better ownership

The final WAL sink is single-owner anyway.
This design makes that explicit instead of partially pretending otherwise.

### 3. Cleaner producer hot path

Producer work becomes:
- reserve
- write intent
- commit

That is much closer to the `ringmpsc` fast path and much cheaper than queue-node
construction plus shared-queue coordination.

### 4. Better batching control

The writer can batch naturally across rings while preserving per-producer FIFO.

That should make it easier to optimize:
- flush cadence
- batch size
- fairness
- fsync rate

## Important Risks

### 1. Fairness across rings

If one producer is hot and others are cold, naive polling may distort latency.

Mitigation:
- per-ring drain cap
- round-robin cursor
- explicit max batch size before rotating

### 2. Ring saturation

If producers outpace the writer, rings fill.

Mitigation:
- bounded backoff
- explicit full-ring metrics
- configurable ring size per lane
- producer-side fallback or pressure signaling

### 3. Payload lifetime bugs

If payload pointers outlive their buffers incorrectly, correctness breaks.

Mitigation:
- strict ownership rules in first prototype
- start with conservative lifetime discipline even if it costs a bit of performance

### 4. Latency regression under tiny workloads

A dedicated writer thread and ring polling can be worse for very small or lightly
concurrent workloads.

Mitigation:
- benchmark honestly against current `LockFreeWAL`
- do not assume the ring path wins universally

## Proposed Benchmark Plan

Compare current `LockFreeWAL` with `WalIntentRing` under identical conditions.

### Workload dimensions

1. producer count
- 1, 2, 4, 8, 16

2. entry payload size
- tiny
- medium
- large

3. durability mode
- buffered
- immediate

4. batch thresholds
- small batch
- medium batch
- large batch

### Metrics

- throughput (entries/s)
- bytes/s
- p50/p95/p99 append latency
- fsync frequency
- producer-side stall rate
- queue/ring full rate
- allocation count on hot path
- CPU utilization of producers and writer

### Success criteria

A strong win would look like:
- less producer contention
- less allocation pressure
- better p95/p99 under many producers
- equal or better throughput under real durability settings

Not just synthetic no-op wins.

## Rollout Plan

### Phase 1. Narrow prototype

Build a `WalIntentRing` implementation behind an experiment flag.
Keep the existing `LockFreeWAL` intact.

### Phase 2. One-file WAL comparison

Use the same framed record logic and compare:
- correctness
- recovery compatibility
- benchmark behavior

### Phase 3. Integrate with one narrow commit path

Use it only for one storage-engine WAL owner path first.
Do not generalize immediately.

### Phase 4. Generalize to shard-local apply if justified

Only after the WAL ingress path proves itself.

## Recommendation

This is a strong next experiment.

It has the right characteristics:
- narrow boundary
- real contention problem
- naturally single sink
- clear benchmark target
- low risk to overall storage semantics if kept behind a flag

If it works, it will give Owl a cleaner write-ingress architecture and a pattern that can
later be reused for shard-local apply loops and replication shipping.

That is the right way to adopt ringmpsc ideas here.
