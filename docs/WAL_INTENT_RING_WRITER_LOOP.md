# WalIntentRing Writer Loop Execution Note

## Purpose

This document defines the first execution model for the `WalIntentRing` writer loop.

The design stack now is:
- fit and adoption analysis:
  `/Users/timiadesina/Documents/owl/docs/RINGMPSC_FOR_OWL_STORAGE_ENGINE.md`
- boundary architecture:
  `/Users/timiadesina/Documents/owl/docs/WAL_INTENT_RING_DESIGN.md`
- protocol semantics:
  `/Users/timiadesina/Documents/owl/docs/WAL_INTENT_RING_PROTOCOL.md`
- concrete layout:
  `/Users/timiadesina/Documents/owl/docs/WAL_INTENT_RING_STRUCTS.md`
- writer execution model:
  this document

This note is intentionally operational. It answers:
- how the writer polls rings
- when an intent becomes written
- when an intent becomes durable
- how batching works
- when fsync happens
- how failures are published
- which transitions are legal

It should be detailed enough that the first Zig prototype can follow it closely.

## Writer Role

There is exactly one writer loop per WAL stream.

That writer owns:
- global WAL order
- final LSN assignment
- final byte offset assignment
- WAL framing
- file writes
- fsync / flush policy
- completion publication

Producers do not participate in any of those responsibilities.

## Core Rule

The writer loop is the only place where an intent crosses from staged work into durable log state.

That gives us three distinct milestones:
1. `published`
   - producer placed the intent in its ring
2. `written`
   - writer assigned order and copied the framed record into the WAL sink
3. `durable`
   - writer completed the durability guarantee required by the intent's durability class

Only `durable` should be treated as success by default.

## Intent Lifecycle

### States

For the first implementation, the internal lifecycle should be:
- `empty`
- `published`
- `claimed`
- `batched`
- `written`
- `durable`
- `failed`

Not all of these need to be exposed to producers.

### Producer-visible terminal states

The first external contract should keep producers focused on:
- `durable`
- `failed`

A debug mode may expose `written`, but it should not be necessary for correctness.

## State Transition Rules

### 1. `empty -> published`
Producer reserves a ring slot, writes the `WalIntent`, then publishes it by advancing the ring tail.

### 2. `published -> claimed`
Writer observes the slot during ring drain and claims it for batch construction.

### 3. `claimed -> batched`
Writer normalizes the slot into a writer-owned `WalBatchEntry` and appends it to the current batch.

### 4. `batched -> written`
Writer frames the batch and successfully appends it to the WAL file.

### 5. `written -> durable`
Writer performs the durability action required by the batch and/or intent, then publishes successful completion.

### 6. `published|claimed|batched|written -> failed`
Writer encounters an error and publishes failure with an error code.

No other transitions are valid.

## Writer Loop Outline

The first writer loop should follow this shape:

1. scan producer rings in fair order
2. claim available intents
3. normalize intents into writer-owned `WalBatchEntry`s
4. stop drain when any batch threshold fires
5. write the batch to the WAL file
6. decide whether fsync is required now
7. publish completions for success or failure
8. recycle batch memory
9. sleep or back off only when no work is available

## Polling Policy

### Fairness Rule

The writer should not always start at producer ring zero.

Use a round-robin starting cursor:
- maintain `rr_cursor`
- each batch drain starts from `rr_cursor`
- after each drain cycle, advance it

This prevents producer zero from winning every tie.

### Per-Ring Drain Rule

The writer should not fully drain one hot producer before checking others.

Use a per-ring cap:
- `per_ring_batch_cap`
- example first value: `32` intents per ring per sweep

This gives:
- better fairness
- lower starvation risk
- more stable multi-producer tail latency

### Idle Rule

If no ring has work:
- perform short spin for a bounded count
- then back off with a small sleep or yield
- then resume polling

The first prototype should keep this simple and measurable.

## Batch Construction

### Why batch at all

The single writer exists partly to assemble coherent writes.

Batching improves:
- fewer syscalls
- fewer fsync decisions
- better disk throughput
- lower per-intent overhead

### Batch stop conditions

The writer should stop claiming more intents when any of these conditions is met:
- `batch_entry_count >= max_entries`
- `batch_bytes >= max_bytes`
- at least one claimed intent requires `immediate` durability and the writer already has a non-empty batch
- elapsed wait since first claimed intent exceeds `max_wait_ns`

These conditions should be explicit config values.

### Immediate durability interaction

If an `immediate` intent arrives and the batch already contains buffered intents:
- include it in the current batch if it was claimed before the flush decision
- write once
- fsync once
- publish durability for all batch members together

That is the cleanest first behavior.

The writer should not split a batch after claiming unless correctness requires it.

## Normalization Step

The writer should copy each claimed slot into a writer-owned `WalBatchEntry` before any file write.

That normalized entry should include:
- producer id
- completion slot
- local sequence
- final LSN placeholder
- final offset placeholder
- durability class
- payload pointer and length inside writer-owned batch memory

### Why normalize

Because after normalization:
- later file formatting no longer depends on the producer ring slot
- the producer ring can be consumed cleanly
- payload lifetime becomes explicit

This is also where external payloads should be copied into writer-owned batch memory in the first prototype.

## Order Assignment

### LSN assignment

The writer assigns final LSNs only after batch order is fixed.

Recommended first rule:
- batch order is the order intents are appended into `WalBatchEntry[]`
- `next_lsn` advances monotonically through that batch order

This keeps ordering simple and observable.

### Byte offset assignment

The writer assigns final WAL byte offsets when framing the batch.

Recommended first rule:
- `next_offset` is monotonically advanced by actual framed entry sizes
- offset is the start position of the framed record in the WAL stream

Only the writer owns this decision.

## WAL Framing And Write Step

The writer should reuse the current WAL record framing logic as much as possible.

The first prototype should not redesign:
- record format
- record checksum semantics
- superblock interaction
- recovery parser expectations

The change is ingress ownership, not WAL encoding.

### Write step

For each `WalBatchEntry`:
1. frame the record into the writer batch buffer
2. assign final offset
3. assign final LSN
4. append framed bytes to the batch write region

After framing all entries in the batch:
5. perform one write call or as few calls as practical
6. treat batch write failure as batch failure

## Durability Policy

### First recommended durability classes

The first implementation should keep just:
- `buffered`
- `immediate`

### `buffered`
The batch may be written without immediate fsync.
Completion may still be delayed until a later durability event if the public contract requires durable completion only.

### `immediate`
The batch must reach the durable boundary before completion is published as success.

### First external contract

To keep the first prototype honest and simple:
- producers should only receive success once durability has actually been achieved
- even buffered intents should complete only when their durability checkpoint has happened

That makes the API clearer, even if it leaves some latency on the table in the first version.

Later, if needed, Owl can add an explicit `written-but-not-durable` completion mode.

## Flush And Sync Decision

The writer should decide to sync when any of these is true:
- current batch contains at least one `immediate` intent
- accumulated unsynced bytes exceed threshold
- accumulated unsynced entries exceed threshold
- unsynced age exceeds threshold
- shutdown-drain mode is active

This creates a single policy location for durability behavior.

## Completion Publication

Completion publication happens only after the batch reaches terminal success or failure.

### Success publication

For each batch entry, the writer stores into that producer's completion slot:
- `sequence_local`
- `final_lsn`
- `final_offset`
- `status = durable`
- `durable = 1`
- `error_code = 0`

Then the writer performs the release-store needed for the producer to observe terminal completion.

### Failure publication

For each batch entry that failed:
- `sequence_local`
- `final_lsn = 0` if not assigned safely
- `final_offset = 0` if not assigned safely
- `status = failed`
- `durable = 0`
- `error_code = <wal-file-os code>`

Failure publication must still be terminal and observable.

## Failure Handling

### Claim-time failure

If normalization fails before file write:
- mark each affected entry failed
- do not advance WAL stream state
- do not publish partial success

### Write failure

If file append fails:
- mark all batch entries failed
- keep WAL stream position unchanged if the underlying write semantics guarantee that
- if partial write semantics are possible, escalate to a writer-fatal state and force restart-recovery handling

The first prototype should prefer write APIs and framing discipline that avoid ambiguous partial progress.

### Sync failure

If fsync fails:
- treat the entire not-yet-completed batch as failed
- writer should enter a fatal error state unless storage policy explicitly allows retries

The first version should be conservative here.

## Ring Consumption Rule

The writer should only advance the consumer head for a slot after the slot has been normalized into writer-owned state.

That rule matters because once the head advances:
- the producer may reuse the slot later
- the writer must no longer depend on slot contents

This is another reason to normalize eagerly.

## Shutdown And Drain

On requested shutdown:
1. stop accepting new producer registration
2. keep polling active rings
3. drain all published work
4. write remaining batch
5. sync if required by policy
6. publish final completions
7. close the writer cleanly

If shutdown is forced and safe drain is impossible:
- all not-yet-terminal intents should fail explicitly where possible
- crash recovery should remain the source of truth after process restart

## Observability

The writer loop should expose at least:
- batches written
- intents written
- bytes written
- fsync count
- average batch size
- max batch size
- producer starvation count or long-wait count
- failure count by category
- completion latency histogram if feasible

These are necessary if we want an honest comparison with the current `LockFreeWAL`.

## First Prototype Defaults

Reasonable first defaults:

```text
max_entries = 256
max_bytes = 1 MiB
max_wait_ns = 200_000
per_ring_batch_cap = 32
sync_unsynced_bytes_threshold = 1 MiB
sync_unsynced_entries_threshold = 1024
sync_unsynced_age_ns = 2_000_000
```

These are not sacred. They should be benchmark-tuned later.

## Pseudocode

```text
loop:
  batch.clear()
  start = now()

  drain_rings_round_robin_until_threshold(batch)

  if batch.empty():
    idle_backoff()
    continue

  normalize_payloads_into_writer_memory(batch)
  assign_lsns_and_frame_records(batch)

  if write_batch(batch) fails:
    publish_batch_failure(batch)
    maybe_enter_fatal_state()
    continue

  if sync_required(batch, policy):
    if fsync() fails:
      publish_batch_failure(batch)
      enter_fatal_state()
      continue

  publish_batch_success(batch)
  reclaim_writer_batch_memory(batch)
```

## First Implementation Recommendation

The first Zig prototype should implement exactly this shape:
- fair round-robin polling
- bounded per-ring drain
- eager normalization into writer-owned batch entries
- writer-owned LSN and offset assignment
- success only at durable completion
- conservative fatal handling on sync ambiguity

That gives Owl the clearest, safest first evaluation of whether ring-decomposed WAL ingress is actually better than the current lock-free queueing path.

## What Comes After This Note

If we stay disciplined, the next step is implementation behind a feature flag, then benchmark against:
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`

Only after that should we decide whether to:
- replace the current ingress path
- keep both modes
- extend the ring model to shard-local apply loops or replication shipping
