# WalIntentRing Protocol

## Purpose

This document defines the first concrete protocol for `WalIntentRing`.

`/Users/timiadesina/Documents/owl/docs/WAL_INTENT_RING_DESIGN.md` explains why a
ring-decomposed ingress path is a good fit for Owl's WAL boundary.

This document goes one level deeper. It defines:
- the first protocol messages
- ownership rules
- completion semantics
- state transitions
- a first writer loop model

The goal is not to overdesign. The goal is to remove ambiguity before implementation.

## Design Goals

The first protocol should optimize for:

1. correctness
2. explicit ownership
3. bounded memory behavior
4. simple producer hot path
5. clear durability semantics

The first protocol should not optimize for every future use case.
It should be narrow, easy to reason about, and easy to benchmark.

## Protocol Model

There are three actors:

1. **producer lane**
- owns one dedicated intent ring
- publishes append requests

2. **WAL writer**
- owns the WAL file
- drains all producer rings
- assigns final order, LSNs, and offsets
- writes and flushes
- publishes completion

3. **completion observer**
- usually the producer itself
- waits for or polls completion state when required

## First Protocol Rule

The protocol should be understood as:

> publish intent, not durability.

A producer publishing a `WalIntent` has only staged a request.
Nothing is committed until the WAL writer publishes successful completion.

## Producer Registration Model

The first implementation should use explicit producer registration.

### Rule 1. Producer lanes are bounded and stable

- the WAL ingress path has a configured maximum producer count
- each registered producer is assigned exactly one ring
- the ring remains owned by that producer for its lifetime

This preserves the main advantage of ring decomposition: no producer-producer contention
on enqueue.

### Rule 2. Registration returns a producer handle

The producer handle should contain at least:
- producer id
- pointer or reference to its ring
- pointer or reference to its completion state area
- local sequence counter

## Message Types

The first protocol only needs one producer-to-writer message type and one writer-to-
producer completion type.

## `WalIntent`

A `WalIntent` is a producer-published request for one WAL append operation.

### First recommended shape

```text
WalIntent {
  producer_id: u32,
  sequence_local: u64,
  entry_type: u8,
  flags: u32,
  payload_mode: u8,
  durability_class: u8,
  objref_kind: u8,
  reserved: u8,
  objref_local_offset: u64,
  objref_shard_id: u32,
  payload_len: u32,
  inline_len: u16,
  reserved2: u16,
  payload_ptr: u64,
  inline_payload: [N]u8,
  completion_slot: u32,
  reserved3: u32,
}
```

This exact binary shape may change, but the semantic fields should remain close to this.

### Field meaning

#### `producer_id`
Stable producer lane id.

#### `sequence_local`
Monotonic sequence within one producer lane.
Used for:
- completion matching
- debugging
- detecting protocol violations

#### `entry_type`
Logical WAL entry kind:
- insert
- update
- delete
- commit
- abort
- checkpoint
- others later if needed

The first implementation should keep this aligned with current WAL entry semantics.

#### `flags`
Protocol flags for special handling.
Examples later may include:
- urgent flush requested
- payload externally staged
- no completion wait expected

The first implementation should keep flags minimal.

#### `payload_mode`
How payload bytes are supplied.

Recommended first values:
- `inline_small`
- `external_ptr`

#### `durability_class`
How durable completion should be interpreted.

Recommended first values:
- `buffered`
- `immediate`

#### `objref_kind`
Whether the object identity is:
- no object ref
- local `ObjRef`
- `GlobalObjRef`

This keeps the protocol future-safe for distributed WAL cases.

#### `objref_local_offset`
Local object offset when relevant.

#### `objref_shard_id`
Shard id when relevant.

#### `payload_len`
Total payload length in bytes.

#### `inline_len`
Number of valid inline bytes if `payload_mode == inline_small`.

#### `payload_ptr`
Pointer to producer-owned payload buffer if `payload_mode == external_ptr`.

#### `inline_payload`
Fixed inline payload area for very small entries.

#### `completion_slot`
Index into the producer's completion area for this intent.

## Why the protocol needs both inline and pointer payload modes

Because Owl has two competing needs:
- small WAL records should avoid extra pointer chasing
- large WAL records should not blow up ring slot size

So the first protocol should support both.

### Recommended first sizing rule

Use inline payload only for very small entries, for example up to 32 or 64 bytes.
Everything larger uses pointer-plus-length.

That keeps ring slots reasonably compact while still handling small intents efficiently.

## `WalIntentCompletion`

A `WalIntentCompletion` is the writer-published result for one producer intent.

### First recommended shape

```text
WalIntentCompletion {
  producer_id: u32,
  sequence_local: u64,
  final_lsn: u64,
  final_offset: u64,
  status: u8,
  durable: u8,
  error_code: i32,
  reserved: u16,
}
```

### Field meaning

#### `producer_id`
Producer lane id.

#### `sequence_local`
Must match the original intent.
This is the primary correctness key.

#### `final_lsn`
Assigned by the writer.
Zero if not assigned due to failure.

#### `final_offset`
Assigned by the writer.
Zero if not assigned due to failure.

#### `status`
Recommended first statuses:
- `pending`
- `written`
- `durable`
- `failed`

In the first implementation, producers may only care about:
- `durable`
- `failed`

#### `durable`
Boolean-like field saying whether durability guarantee has been achieved.

#### `error_code`
Storage-layer or WAL-layer error code on failure.

## Completion Memory Model

The first protocol should avoid a second queue.

### Recommended design

Each producer owns a small completion array or completion table.
The writer writes completion records into the producer-owned slot indicated by
`completion_slot`.

This has several advantages:
- cache-local for the producer
- no completion queue contention
- easy matching using `(producer_id, sequence_local)`

### Rule 1. Producer owns slot allocation

The producer chooses the completion slot and must not reuse that slot for another live
intent until the previous intent has reached a terminal state.

### Rule 2. Writer owns completion publication

Only the writer may move a completion from:
- `pending` to `written`
- `pending` to `durable`
- `pending` to `failed`

### Rule 3. Terminal states are final

Once `durable` or `failed` is published, that completion is terminal.

## Producer-Side Protocol

The first producer-side protocol should be intentionally small.

### Publish algorithm

1. reserve one ring slot
2. allocate or choose one completion slot
3. populate `WalIntent`
4. ensure payload ownership is valid
5. publish the intent by committing the ring slot
6. if synchronous durability is required, wait until completion becomes terminal

### Producer invariants

The producer must ensure:

1. `sequence_local` strictly increases
2. the completion slot is not reused too early
3. external payload memory remains valid until terminal completion
4. ring publication uses release semantics

## Writer-Side Protocol

The WAL writer is the sole owner of final stream construction.

### Writer algorithm, high level

1. poll active rings in round-robin order
2. read available intents from one ring
3. for each intent:
   - validate message shape
   - assemble framed WAL record
   - assign `final_lsn`
   - stage for batch write
4. flush batch according to policy
5. publish completion for each batch member

### Writer invariants

The writer must ensure:

1. per-producer FIFO is preserved
2. final stream order is exactly the order of writer assembly
3. `final_lsn` is unique and monotonic in final WAL order
4. completion is published only after the protocol's durability boundary is satisfied

## First State Machine

## Intent lifecycle

```text
producer constructs intent
  -> reserved in ring slot
  -> published
  -> observed by writer
  -> assigned final_lsn/final_offset
  -> serialized into batch
  -> written
  -> synced if required
  -> completion published
  -> terminal
```

## Completion lifecycle

```text
empty
  -> pending
  -> durable
```

or

```text
empty
  -> pending
  -> failed
```

The first implementation does not need more states than this unless debugging proves it
necessary.

## Ordering Rules

### Rule 1. Per-producer FIFO is strict

Within one producer ring, intents must be observed and handled in publication order.

### Rule 2. Global order is writer-defined

Across producers, the global WAL order is whatever order the writer actually drains and
commits.

### Rule 3. Final LSN follows global writer order

A later-writer-drained intent must not receive an earlier LSN.

This means producer-side pre-allocation of LSNs is not part of the first protocol.

## Offset Assignment Rules

### Rule 1. Offsets are writer-owned

Only the writer computes or reserves final file offsets.

### Rule 2. Producers are offset-agnostic

A producer does not need to know the final offset to publish an intent.

This makes the producer hot path smaller and more stable.

## Batch Policy

The writer should build batches with these decision rules:

1. flush if an `immediate` intent requires durable completion now
2. flush if `max_entries` is reached
3. flush if `max_bytes` is reached
4. flush if `max_wait` expires

### First recommended fairness rule

When polling rings:
- drain at most `per_ring_batch_cap` items from one ring before rotating

This avoids a single hot producer starving everyone else.

## Payload Ownership Rules

This is a correctness-critical section.

### Inline payload

If `payload_mode == inline_small`:
- payload bytes live entirely in the ring slot
- no external ownership issue exists

### External payload

If `payload_mode == external_ptr`:
- the producer owns the payload memory until terminal completion
- the writer may read from that payload only before terminal completion is published
- the producer must not mutate or free the payload before terminal completion

### First implementation recommendation

Be conservative:
- prefer copying into writer-owned batch memory before writing
- optimize later only after correctness and lifetime behavior are proven

That is still a win if ring ingress eliminates most enqueue contention.

## Failure Semantics

A failure can occur at these stages:

1. ring reserve failed
2. writer validation failed
3. batch serialization failed
4. file write failed
5. sync failed
6. writer shutdown interrupted progress

### Protocol rule

If any failure occurs before terminal success:
- completion must move to `failed`
- producer must treat the request as not durable
- any partially assigned writer-local metadata must not be treated as committed state

## Memory Ordering Requirements

The exact atomics can be tuned later, but the semantic requirements are clear.

### Producer publication

Producer must publish the fully initialized intent before making it visible in the ring.

### Writer observation

Writer must observe a fully published intent before reading its fields.

### Completion publication

Writer must publish completion only after all corresponding writer-side state changes for
that completion are visible.

### Producer completion observation

Producer must observe completion with acquire semantics before trusting terminal fields.

The implementation can map these to the weakest correct atomics later.

## Writer Loop Pseudocode

```text
loop:
  made_progress = false

  for ring in active_rings round-robin:
    drained = 0

    while drained < per_ring_batch_cap:
      intent = try_read_next_intent(ring)
      if none:
        break

      made_progress = true
      validate(intent)
      assign final_lsn
      assemble framed wal record
      append to current batch
      remember completion target
      drained += 1

      if intent.durability_class == immediate:
        flush_and_sync_batch()
        publish_terminal_completions()
        reset batch

      else if batch_limits_hit:
        flush_and_sync_batch_if_policy_requires()
        publish_completions_per_policy()
        reset batch

  if !made_progress:
    if batch_wait_expired and batch_not_empty:
      flush according to policy
      publish completions
      reset batch
    backoff_or_sleep_briefly()
```

The first implementation should stay close to this.

## Observability Requirements

Even the first prototype should expose:
- ring full count per producer
- intents published
- intents completed
- intents failed
- average and max batch size
- fsync count
- writer idle time
- writer drain time
- completion wait time

Without this, the benchmark will not be trustworthy.

## Compatibility Requirements

The first protocol must preserve:
- current on-disk WAL framing compatibility
- current crash-recovery assumptions
- current durability meaning for commit-related entries

If the protocol changes these, it has gone too far.

## First Prototype Constraints

To keep the first prototype honest, constrain it deliberately:

1. one WAL file
2. bounded producers
3. fixed ring size
4. only `buffered` and `immediate`
5. only one completion mechanism
6. no dynamic ring creation after start

This is enough to test the architecture.

## Recommendation

This protocol is a good first implementation target because it is:
- narrow
- benchmarkable
- compatible with current WAL semantics
- much clearer about ownership than the current append-node queue path

If it proves itself, then the same protocol style can later be adapted for:
- shard-local apply loops
- replication shipping
- other sequential ownership boundaries

But this one should come first.
