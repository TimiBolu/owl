# WalIntentRing Struct Layouts

## Purpose

This document defines the first concrete data layouts for `WalIntentRing`.

The design stack now looks like this:
- fit analysis:
  `/Users/timiadesina/Documents/owl/docs/RINGMPSC_FOR_OWL_STORAGE_ENGINE.md`
- boundary architecture:
  `/Users/timiadesina/Documents/owl/docs/WAL_INTENT_RING_DESIGN.md`
- protocol semantics:
  `/Users/timiadesina/Documents/owl/docs/WAL_INTENT_RING_PROTOCOL.md`
- concrete layouts:
  this document

This is the bridge from protocol to implementation.

The purpose here is to decide:
- exact first structs
- field order
- alignment
- cache-line separation
- ownership boundaries
- FFI exposure boundaries

The first implementation should follow this document closely unless benchmarking or
correctness work gives a compelling reason to adjust it.

## Design Principles For The Layout

The first layout should optimize for:

1. explicit ownership
2. stable producer hot path
3. bounded slot size
4. low producer cache traffic
5. single-writer-friendly batching

The first layout should not attempt to encode every future feature.

## First Important Choice

The first prototype should use **fixed-size intent slots**.

That means:
- no heap allocation for queue nodes
- no variable-sized ring entries
- one predictable ring slot layout

Payload itself may be:
- inline for very small records
- external for larger records

That lets the ring stay bounded and mechanically simple.

## Recommended Constants

These are first-pass recommendations, not eternal truths.

```text
WAL_INTENT_INLINE_BYTES = 64
WAL_RING_ALIGNMENT = 128
WAL_COMPLETION_ALIGNMENT = 64
WAL_DEFAULT_RING_BITS = 12   // 4096 slots per producer ring
WAL_MAX_PRODUCERS = configurable, first default 16
```

### Why 64 inline bytes

This is a good first compromise:
- large enough for small control-heavy WAL records
- small enough to avoid bloating every ring slot too much

### Why 128-byte ring metadata alignment

This follows the strongest lesson from `ringmpsc-main`:
- producer-hot and consumer-hot fields should be isolated from false sharing and adjacent
  prefetch behavior

## Core Enums

The first implementation should keep enums compact.

### `WalDurabilityClass`

```text
enum(u8) {
  buffered = 1,
  immediate = 2,
}
```

### `WalPayloadMode`

```text
enum(u8) {
  inline_small = 1,
  external_ptr = 2,
}
```

### `WalObjRefKind`

```text
enum(u8) {
  none = 0,
  local_objref = 1,
  global_objref = 2,
}
```

### `WalCompletionStatus`

```text
enum(u8) {
  empty = 0,
  pending = 1,
  durable = 2,
  failed = 3,
}
```

The first implementation does not need `written` as a separately visible terminal state.
If internal instrumentation wants it later, it can be added without changing the first
API contract.

## `WalIntent`

## Recommended semantic fields

The first `WalIntent` should carry:
- producer identity
- producer-local sequence
- entry type
- payload mode
- durability class
- object reference identity
- payload description
- completion slot id

## Recommended first binary layout

```text
WalIntent (aligned to 64 bytes or 128 bytes depending on benchmarking)

u32 producer_id
u32 completion_slot
u64 sequence_local
u8  entry_type
u8  payload_mode
u8  durability_class
u8  objref_kind
u32 flags
u32 payload_len
u32 inline_len
u64 objref_local_offset
u32 objref_shard_id
u32 reserved0
u64 payload_ptr
u8  inline_payload[64]
u64 reserved1
```

This should be rounded to a stable alignment boundary.

### Notes on ordering

This ordering is chosen so that:
- producer identity and completion routing are near the front
- small control fields are grouped tightly
- payload pointer and payload bytes are contiguous in the latter half
- object reference fields are explicit rather than union-packed in a confusing way

### Why not over-pack immediately

Because the first prototype values clarity and correctness over shaving a few bytes from
slot size. We should benchmark first before collapsing the layout into a harder-to-read
packed form.

## `WalIntentCompletion`

The completion structure should be small and cheap to poll.

## Recommended first binary layout

```text
WalIntentCompletion (aligned to 64 bytes)

u64 sequence_local
u64 final_lsn
u64 final_offset
i32 error_code
u8  status
u8  durable
u16 reserved0
u32 producer_id
u32 reserved1
u8  padding[32]
```

### Why pad it this much

Because completion slots are hot for the producer and written by the writer.
Keeping one completion in its own cache-friendly region reduces accidental contention.

This is one of the few places where mild space waste is justified by clarity and
mechanical stability.

## Producer Completion Area

Each producer should own a completion array.

### Recommended first shape

```text
ProducerCompletionArea {
  slots: [COMPLETION_SLOTS]WalIntentCompletion
  next_slot: u32
}
```

### Recommended first rule

Use a power-of-two slot count so wrapping is trivial.

Suggested first constant:

```text
WAL_COMPLETION_SLOTS = 1024
```

That is enough for a first prototype while staying bounded.

### Slot reuse rule

The producer must not reuse a slot until the previous intent occupying that slot is in a
terminal state.

For the first prototype, if the producer is synchronous and waits for durability, this is
simple. More advanced asynchronous reuse can come later.

## Producer Handle

The producer handle should be a small struct used only on the producer side.

## Recommended first shape

```text
WalIntentProducer {
  producer_id: u32
  local_sequence: u64
  ring: *WalIntentRing
  completion_area: *ProducerCompletionArea
}
```

### Responsibilities

The producer handle is responsible for:
- reserving ring slots
- incrementing `sequence_local`
- allocating completion slots
- publishing intents
- optionally waiting for completion

It should not know about:
- final WAL offsets
- final LSN assignment policy
- file writing
- fsync policy details

## Per-Producer Ring Metadata

Following the ringmpsc lessons, each ring should separate producer-hot and
consumer-hot fields.

## Recommended first shape

```text
WalIntentRing {
  // producer hot
  tail: atomic(u64)        align(128)
  cached_head: u64

  // consumer hot
  head: atomic(u64)        align(128)
  cached_tail: u64

  // cold
  active: atomic(bool)     align(128)
  closed: atomic(bool)
  dropped_intents: atomic(u64)
  producer_id: u32
  reserved: u32

  // slots
  buffer: [CAPACITY]WalIntent align(64)
}
```

### Important note

The ring should be SPSC in actual use:
- one producer writes it
- one writer thread drains it

That is why the ring can stay extremely simple.

## Writer-Side State

The writer needs a structure separate from the rings themselves.

## Recommended first shape

```text
WalWriterState {
  wal_file_handle
  next_lsn: u64
  next_offset: u64
  ring_count: u32
  rr_cursor: u32
  batch_entry_count: u32
  batch_bytes: u32
  batch_buffer_ptr
  batch_buffer_len
  max_entries
  max_bytes
  max_wait_ns
  per_ring_batch_cap
  producer_rings: []*WalIntentRing
  completion_areas: []*ProducerCompletionArea
}
```

### Responsibilities

The writer state owns:
- polling order
- batch assembly
- final LSN assignment
- final offset assignment
- file writes
- flush/sync policy
- completion publication

## Writer Batch Entry

The writer will likely need an internal batch descriptor separate from the ring slot.

## Recommended first shape

```text
WalBatchEntry {
  producer_id: u32
  completion_slot: u32
  sequence_local: u64
  final_lsn: u64
  final_offset: u64
  entry_type: u8
  durability_class: u8
  reserved: u16
  payload_ptr: *const u8
  payload_len: u32
}
```

### Why this exists

Because once the writer has consumed an intent, it should normalize it into a writer-
owned descriptor for batch assembly.

That keeps the later sink path clean.

## Payload Ownership Boundary

This is the part that must be explicit.

### For inline payloads

Ownership is entirely inside the ring slot until the writer consumes the intent.

### For external payloads

The producer owns the payload memory until completion is terminal.

### Conservative first implementation

The writer should copy external payloads into writer-owned batch memory before writing.

That means the first prototype has:
- ring enqueue advantage
- cleaner ownership
- fewer lifetime bugs

Later, if measurements justify it, this copy can be optimized.

## FFI Boundary Recommendation

Not everything should be exposed over FFI immediately.

### Keep Zig-only first

These should remain Zig-internal initially:
- `WalIntent`
- `WalIntentRing`
- `WalIntentCompletion`
- `WalWriterState`
- `WalBatchEntry`

### Expose only higher-level operations if needed

Rust does not need direct access to the raw ring structs in the first iteration.
It only needs operations such as:
- register producer lane
- submit WAL intent
- wait for completion
- shutdown writer
- query stats

That keeps the ABI smaller and safer.

## Cache-Line Intent

The whole point of this design is to make hot paths predictable.

So the first implementation should preserve these layout intentions:

1. producer tail and cached head isolated from writer head and cached tail
2. completion slot publication cheap for producer polling
3. batch writer state separate from producer ring state
4. ring slot size stable and not dependent on payload length

## Recommended First Struct Strategy In Zig

The first implementation should prefer:
- `extern struct` or explicitly laid-out structs where ABI matters
- explicit `align(128)` on producer/consumer hot sections
- explicit padding fields when needed
- compile-time assertions for struct size and alignment

This is worth doing carefully because layout is part of the design, not an incidental
compiler outcome.

## Compile-Time Assertions To Add Later

The eventual implementation should include compile-time checks for:
- intent slot size
- completion slot size
- producer-hot / consumer-hot alignment
- ring capacity power-of-two assumptions
- inline payload capacity

That will help keep later edits honest.

## First Implementation Recommendation

Start with:
- fixed-size `WalIntent`
- fixed-size `WalIntentCompletion`
- one completion area per producer
- SPSC rings with 128-byte producer/consumer hot separation
- writer-owned `WalBatchEntry` normalization
- conservative payload copy into writer batch memory

This is not the smallest possible implementation, but it is the most responsible first
one.

It keeps the shape clear while still preserving the main performance reason for doing
this at all.

## What To Do Next

The next best step after this document is:

1. write a **writer loop execution note** with exact state transitions and flush timing
2. then implement the first prototype in Zig behind an experiment flag
3. then benchmark directly against current `LockFreeWAL`

That is the safest path from design to code.
