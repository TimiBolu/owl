# RingMPSC And Owl Storage Engine Design

## Purpose

This document records a careful reread of the current Zig storage engine in
`/Users/timiadesina/Documents/owl/owl_storage_zig` and the ring-decomposed MPSC
implementation in `/Users/timiadesina/Documents/owl/ringmpsc-main`.

The goal is not to force a queueing primitive into the storage engine. The goal is to
answer a narrower and more useful question:

> Where, if anywhere, does a ring-decomposed MPSC approach materially improve Owl's
> storage-engine design?

The answer is: yes, but only at specific boundaries.

RingMPSC is not a replacement for Owl's arena, WAL, MVCC, indexing, or recovery
semantics. It is a strong candidate for a few coordination paths where Owl currently
mixes concurrency, batching, and I/O in ways that can be made mechanically cleaner.

## What The Owl Zig Storage Engine Looks Like Today

After rereading the current codebase, the storage engine is best understood as a broad
low-level storage toolkit rather than one single tight datapath.

### 1. The engine is object-arena centric

The center of the design is still the physical object graph model:

- `ObjRef` is a 64-bit offset-based physical pointer in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/objref.zig`
- `ObjectHeader` packs hot metadata for deletion, type dispatch, MVCC, LSN, and checksum in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/object_header.zig`
- `Object` carries fixed bytes, variable data, and explicit `refs` in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/object.zig`
- `Arena` is the main storage substrate in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/arena.zig`

The design intent is very clear:
- object references should be cheap physical navigation,
- co-location matters,
- allocation strategy matters,
- durability is layered beneath the Rust plane.

### 2. The arena is already multi-strategy, not minimal

`Arena` is not a simple bump allocator anymore. It composes several mechanisms:

- slab allocation for small objects via
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/slab_allocator.zig`
- page allocation for medium objects via
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/page_allocator.zig`
- bump allocation for very large objects in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/arena.zig`
- object pooling, compression, and decompression caching also live in the arena

This means Owl already has multiple hot paths with different contention and memory
behavior. Any queueing integration has to respect that. There is no single producer
path to wrap blindly.

### 3. Durability is split across multiple layers

The storage engine has several durability-related components:

- regular WAL in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/wal.zig`
- lock-free WAL attempt in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`
- crash recovery in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/crash_recovery.zig`
- backup/recovery in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/backup_recovery.zig`
- framed superblock and WAL persistence logic in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/c_api.zig`

This part of the engine is already trying to solve batching, fsync, recovery, and
consistency. That is the first place where a queueing design can matter.

### 4. Concurrency is mixed: some modules are lock-free, some are lock-based

The engine has a mix of concurrency styles:

Lock-free or mostly lock-free primitives:
- slab allocator free lists using `LockFreeStack`
- page allocator bitmap CAS loops
- hazard pointers and atomic refcounting in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/concurrency_primitives.zig`
- lock-free WAL append queue in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`

Lock-based coordination still exists in major structures:
- `BTree` uses a mutex in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/btree.zig`
- `TransactionManager` uses mutexes around transaction maps/stats in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/transaction.zig`
- `VersionStore` uses a mutex in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/mvcc.zig`
- `LRUCache` uses a mutex in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/cache.zig`

So the engine is not philosophically pure lock-free. That is important. RingMPSC should
be judged as a practical mechanical fit, not as a purity move.

### 5. Distributed support is already influencing design

The engine also includes:
- `GlobalObjRef` in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/global_objref.zig`
- distributed B-tree support in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/btree.zig`
- distributed WAL in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/distributed_wal.zig`
- deterministic/simulation-oriented variants

This matters because ring decomposition becomes more attractive once a system becomes
shard-oriented. A shard-local single consumer is much easier to justify than a single
consumer for the entire database.

## What RingMPSC Actually Is

After rereading `/Users/timiadesina/Documents/owl/ringmpsc-main`, the design is much
narrower and cleaner than Owl storage.

### 1. RingMPSC is really "many SPSC rings plus one polling consumer"

The core idea is stated consistently in:
- `/Users/timiadesina/Documents/owl/ringmpsc-main/README.md`
- `/Users/timiadesina/Documents/owl/ringmpsc-main/ALGORITHM.md`
- `/Users/timiadesina/Documents/owl/ringmpsc-main/src/channel.zig`

Each producer gets a private ring.
The consumer polls all active rings.

That means it eliminates producer-producer contention by construction.
It does not solve arbitrary many-to-many scheduling.

### 2. The optimizations are very mechanical and very specific

RingMPSC's advantages come from a small set of disciplined choices:

- 128-byte separation of producer and consumer hot fields
- cached remote sequence numbers
- reserve/commit zero-copy producer path
- batch consumption with one consumer-side update
- adaptive backoff
- fixed power-of-two ring sizes

This is not just a queue. It is a queue shape designed for:
- bounded messages,
- stable hot loops,
- a dedicated draining consumer,
- low coordination complexity.

### 3. The correctness contract is intentionally small

The tests in:
- `/Users/timiadesina/Documents/owl/ringmpsc-main/src/test_fifo.zig`
- `/Users/timiadesina/Documents/owl/ringmpsc-main/src/test_chaos.zig`
- `/Users/timiadesina/Documents/owl/ringmpsc-main/src/test_determinism.zig`

show the intended guarantees:
- per-producer FIFO
- no loss
- no duplication
- deterministic behavior under the chosen model

It does **not** give:
- global ordering,
- random-access shared work stealing,
- general transactional coordination,
- persistence.

That is why it can be so fast.

### 4. Even the alternate implementations reinforce the same lesson

The Rust and C ports show the same structure:
- `/Users/timiadesina/Documents/owl/ringmpsc-main/rust_impl/src/lib.rs`
- `/Users/timiadesina/Documents/owl/ringmpsc-main/rust_impl/src/stack_ring.rs`
- `/Users/timiadesina/Documents/owl/ringmpsc-main/c_impl/ringmpsc.h`

The point is not the language. The point is the boundary shape:
- many producers,
- one consumer,
- bounded ring,
- batch drain.

## Where RingMPSC Can Help Owl

The right question is not "should Owl use ringmpsc?"
The right question is "which Owl boundaries are actually MPSC-to-single-consumer?"

There are several.

### 1. WAL append staging is the strongest fit

This is the clearest place ring decomposition can help.

#### Why

Owl currently has two WAL stories:
- a normal WAL with buffered pending entries and mutex-driven flush coordination in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/wal.zig`
- a `LockFreeWAL` in `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`

But the current lock-free WAL is only partially convincing as a true lock-free I/O path:
- it still reserves offsets and then performs file seek/write/sync against one file handle
- the append queue is lock-free, but the final file interaction is still naturally
  single-sink work
- the current design mixes reservation, queuing, writing, and flushing in one object

That is exactly where a ring-decomposed ingress can help.

#### Better design

Use **one ring per producer thread** or **one ring per execution lane** feeding a
single WAL writer thread per WAL file.

That gives:
- producers write WAL intents into their private ring
- the WAL writer drains rings in batches
- the WAL writer alone performs offset assignment, write ordering, fd writes, and fsync

#### Why this is a good fit

Because the WAL sink is already logically single-consumer at the file level.
You do not gain much by pretending concurrent writers are all equal at the final sink.
You gain more by:
- eliminating producer contention before the sink,
- batching writes coherently,
- keeping file I/O ownership single-threaded.

#### Concrete benefit

This would likely be a better design than the current `LockFreeWAL` append queue for:
- predictable write batching,
- removing queue-node allocation pressure,
- reducing CAS traffic under many producers,
- simplifying the sink's durability protocol.

### 2. Shard-local commit pipelines are a strong fit

`GlobalObjRef`, distributed B-tree work, and distributed WAL make shard-local execution
more important.

A very good storage-engine use of ring decomposition is:

- one per-shard WAL consumer,
- multiple producer lanes sending shard-local work to that shard,
- shard-local batch application.

This fits Owl because distributed design usually converges on shard-local serialization
for durability anyway.

RingMPSC helps here by turning:
- many concurrent shard-local writers
into
- one deterministic per-shard commit/apply loop.

That improves:
- batching,
- ordering discipline,
- cache locality,
- observability.

### 3. Background work submission is a good fit

Several Owl subsystems naturally look like many producers feeding one worker:
- decompression cache fill requests
- compaction work requests
- GC retire/reclaim requests
- backup snapshot chunk emission
- recovery rebuild tasks into a dedicated applier

Not all of these need ring decomposition, but some do.

For example:
- many threads discovering cold compressed objects could enqueue decompression work to a
  single decompression worker or a small number of workers
- many transactional actors could enqueue reclaim requests to a single hazard-pointer or
  epoch cleanup worker

The fit is good when the sink is inherently single-owner or sequential by policy.

### 4. Replication and distributed WAL shipping are a good fit

The distributed WAL path in
`/Users/timiadesina/Documents/owl/owl_storage_zig/src/distributed_wal.zig`
already implies stream-like work:
- shard replication,
- replica catch-up,
- ordered shipment of log frames.

RingMPSC can help as the local handoff structure from many producers of replication work
into a single stream owner.

Again, the fit is strongest when the stream must be emitted in one place anyway.

## Where RingMPSC Should Not Be Forced Into Owl

This matters just as much.

### 1. It should not replace the arena allocator itself

The arena allocator is about:
- address-space management,
- page/slab placement,
- physical offsets,
- co-location,
- reclamation.

RingMPSC is a transport/coordination structure, not an allocator.

It can help feed allocation requests into a single allocator lane, but it is not a
replacement for slab/page/bump design.

### 2. It should not replace MVCC data structures directly

`VersionStore`, snapshots, and transaction visibility logic are stateful structures with
lookup/update semantics, not a streaming producer-consumer boundary.

You can use a queue to send version-install intents to a version applier, but you should
not confuse that with MVCC itself.

### 3. It should not replace the B-tree as a data structure

B-tree mutation may eventually benefit from a single-writer-per-shard policy.
But RingMPSC is not an index structure.

It is only useful if you decide to route index mutations through a single owner.
That is an architectural choice above the tree itself.

### 4. It should not be used where multiple consumers or work stealing are essential

RingMPSC is explicitly many-producer, single-consumer.
If Owl needs:
- many parallel consumers,
- fair work stealing,
- dynamic fan-out,
- arbitrary DAG scheduling,

then RingMPSC is the wrong primitive.

## The Most Useful Design Translation For Owl

The most productive way to use ringmpsc ideas in Owl is this:

> use ring decomposition at ownership boundaries, not inside every data structure.

That means:
- one owner per WAL file,
- one owner per shard apply loop,
- one owner per sequential maintenance stream,
- many producers feeding that owner through dedicated rings.

This aligns with the storage engine's real structure.

## Concrete Proposal For Owl

### Proposal A. Replace append-node queueing in `LockFreeWAL` with ring-decomposed ingress

Current state:
- `LockFreeWAL` uses dynamically allocated `AppendOp` nodes and a CAS-linked queue in
  `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`

Suggested redesign:
- pre-register N producers for the WAL lane
- each producer owns an SPSC ring of `WalIntent`
- one WAL thread drains all rings and writes framed records
- fsync and group commit remain owned by that WAL thread

Recommended message shape:
- object ref / global ref
- entry type
- payload pointer or small inline payload descriptor
- completion token / sequence / promise handle

This is the highest-confidence adoption point.

### Proposal B. Introduce shard-local "apply loops"

For each shard:
- many producer threads emit logical write/apply intents
- a shard-local single consumer applies:
  - arena write
  - index mutation
  - WAL staging if not already separated
  - version publication

This is especially attractive because Owl is already moving toward shard-aware physical
references and distributed WAL.

### Proposal C. Use ring decomposition for replication shipping

Distributed WAL replication can adopt the same pattern:
- many local producers may generate frames to ship,
- one shard-local shipper drains and emits them in order.

This gives cleaner network batching and clearer ownership.

### Proposal D. Use rings for background maintenance requests, not maintenance logic itself

Good candidates:
- GC retire notifications
- decompression warm-up requests
- compaction task intake
- backup chunk intake

The queue helps the boundary. It should not swallow the subsystem.

## What Owl Would Need To Borrow From RingMPSC Specifically

If Owl adopts this approach, it should borrow more than the idea of "a ring".
The important parts are:

### 1. Dedicated producer rings

Do not use one shared CAS-heavy ring if the design goal is to remove producer
contention. The decomposition is the whole point.

### 2. 128-byte producer/consumer separation

This is one of the strongest mechanical-sympathy lessons in `ringmpsc-main`.
If Owl builds shard-local or WAL-local rings, the hot producer and consumer indices
should be separated similarly.

### 3. Cached remote cursors

The producer-side cached consumer cursor and consumer-side cached producer cursor are a
big part of why the rings stay cheap. Owl should copy this if it adopts ring ingress.

### 4. Reserve/commit instead of extra copying

Owl already cares about zero-copy and explicit control. Reserve/commit fits that ethos
very well for WAL intent staging or replication staging.

### 5. Batch drain semantics

The most important API shape is not single-message pop. It is batch drain with one sink
commit step.

That is exactly what Owl needs for:
- WAL write batches,
- shard apply batches,
- replication batches.

## Important Risks And Limits

### 1. RingMPSC does not solve durability on its own

A ring only stages work in memory.
It does not make a write durable.

So if Owl uses ring ingress for WAL or shard apply, the durable boundary must still be
clearly defined after the consumer has written and synced the batch as required.

### 2. It changes latency shape

Ring-based batching usually improves throughput but can worsen tail latency if batch
formation or polling policy is wrong.

For Owl this means:
- you need explicit flush thresholds,
- explicit max wait time,
- explicit fairness policy across rings.

### 3. Polling many rings can become unfair

The `ringmpsc-main` design notes already admit fairness limitations.
If Owl has unbalanced producers, naive round-robin drain may not be enough.

This is manageable, but it must be treated as an engineering requirement, not an
implementation detail.

### 4. It works best with stable producer registration

Ring decomposition prefers a fixed or slowly changing producer set.
That is a natural fit for:
- worker threads,
- shard executors,
- WAL lanes,

and a weaker fit for arbitrary dynamic user threads unless registration discipline is
added.

## My Recommendation

### High-confidence recommendation

Adopt a ring-decomposed ingress path for:

1. WAL staging and flush ownership
2. shard-local apply loops
3. replication shipping

That is where the fit is strongest and the payoff is most believable.

### Medium-confidence recommendation

Use the same pattern for some background maintenance queues, especially where there is
already a natural single owner.

### Low-confidence recommendation

Do not try to "ringmpsc-ify" the whole storage engine.
Do not replace allocators, MVCC structures, or the B-tree with ring logic.

That would be architectural overreach and would likely make the design worse.

## The Most Important Design Insight

The biggest value from `ringmpsc-main` is not that it is lock-free.
It is that it forces a good ownership shape:

- many producers,
- one clear sink owner,
- bounded staging,
- batched publication.

Owl can benefit from that where its current design already wants a single owner, but has
not yet fully embraced it.

That is the right way to use ringmpsc ideas here.

## Suggested Next Steps

1. Design a `WalIntentRing` prototype for one WAL file owner.
2. Compare it directly against the current
   `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig` path.
3. If it wins, generalize the same ownership model to shard-local apply loops.
4. Keep the rest of the engine unchanged until that boundary proves itself.

That sequence is much safer than trying to refactor the whole storage engine around a
queue primitive.
