# Owl Scale-Out Storage Architecture Review

This document reviews Owl's current storage engine and distributed/sharding story through the lens that matters most:

- what an Owl query naturally expects
- what the higher-level Rust runtime currently provides
- what the lower-level Zig storage substrate currently provides
- what is already integrated
- what still exists as parallel infrastructure rather than the main serving path

The central conclusion is:

> Owl already has a meaningful local storage engine and a substantial distributed substrate, but the scale-out query path is not yet fully unified. The local Rust executor is real and increasingly strong; the distributed Zig and Rust layers are substantial; the remaining work is convergence.

---

## 1. What an Owl query expects from storage

An Owl query is not asking for a generic key-value store. The language already implies a richer execution contract.

At a practical level, Owl queries expect:

1. collection access
2. path traversal across nested values
3. forward reference navigation
4. reverse relationship navigation or at least reverse-reference acceleration
5. schema-aware field and reference interpretation
6. grouping, projection, ordering, and limiting over collection-valued things
7. vector search where applicable
8. durability and restart recovery
9. eventually, the same semantics whether the data is local or distributed

This is important because it means we should not judge the storage layer only by raw persistence mechanics. We should judge it by whether it can uphold Owl's query semantics without forcing the language to become more mechanical.

---

## 2. Current system shape

Today Owl has three meaningful layers:

1. a real local serving path
2. a real distributed substrate
3. a partially integrated distributed execution path

The right way to understand the repo is not "local or distributed", but:

- what already serves ordinary queries
- what already exists as lower-level distributed capability
- what still needs to be unified into the ordinary execution path

---

## 3. Higher-level Rust parts

The Rust layer is where Owl's query semantics, storage contract, and distributed coordination are expressed at the system level.

### 3.1 The active local serving contract: `DocumentStore`

The central runtime abstraction is:

- `/Users/timiadesina/Documents/owl/src/storage.rs`

This trait is the real contract ordinary Owl queries depend on.

It already includes:

- document CRUD
- schema CRUD and listing
- forward reference navigation
- object reference lookup
- reverse-reference lookup
- scalar index metadata ownership
- vector index metadata and status/build hooks
- native ANN hook

This is a strong sign that Owl is not architected around "storage first, semantics later". The query model is already influencing the storage contract.

### 3.2 The current main runtime backend: `ZigStore`

The default runtime path is implemented in Rust as a Zig-backed storage adapter:

- `/Users/timiadesina/Documents/owl/src/storage.rs`

This backend is already real and used by ordinary Owl query execution.

It currently provides:

- arena-backed object storage
- metadata WAL and restart recovery
- document index
- reverse reference index
- scalar index metadata catalog
- runtime scalar index manager hydration
- vector index metadata catalog
- persisted HNSW vector files
- storage-native vector ANN hook

This is the most mature serving path today.

### 3.3 The current executor

The main query executor lives here:

- `/Users/timiadesina/Documents/owl/src/executor.rs`

This is important because it tells us what query execution actually does today, not just what the architecture aspires to do.

The current model is mostly:

1. identify the working collection
2. optionally prefilter documents through reverse-reference analysis
3. scan candidate documents
4. evaluate conditions, projections, grouping, aggregates, and nearest-neighbor logic

This means current Owl query execution is:

- local-store first
- scan-oriented with targeted accelerators
- aware of refs
- aware of reverse refs
- aware of vectors

That is already meaningful, but it also means distributed execution is not yet the dominant ordinary path.

### 3.4 Reverse-reference acceleration is one of Owl's strongest current runtime advantages

The executor already detects certain relationship filters and converts them into reverse-reference prefiltering.

Examples of the shape it can exploit:

- `.org.id == "org_1"`
- `.team.id == "team_payments"`
- `.company.org.id == "org_1"`

That work is rooted in:

- `/Users/timiadesina/Documents/owl/src/executor.rs`
- `/Users/timiadesina/Documents/owl/src/storage.rs`

This is one of the clearest places where the engine is already aligning with Owl's graph-native query semantics.

### 3.5 Vector search is more integrated than most young systems manage

The current Rust runtime already coordinates:

- vector field typing
- index metadata
- persisted ANN artifacts
- storage-native ANN search
- exact fallback reranking

Relevant files:

- `/Users/timiadesina/Documents/owl/src/storage.rs`
- `/Users/timiadesina/Documents/owl/src/executor.rs`

This matters because vector capability is not just bolted on as a separate service. It is already entering the same query model as structured filtering and refs.

### 3.6 Rust distributed components already exist

There is a serious distributed subtree in Rust:

- `/Users/timiadesina/Documents/owl/src/distributed/mod.rs`

It includes:

- consistent hashing
- global distributed refs
- shard manager
- distributed storage adapter
- distributed query planner
- distributed WAL
- WAL-first storage
- Raft and cluster coordination components
- distributed OQL execution layer

This is real architecture work, not merely placeholders.

### 3.7 `ShardManager`

The shard and cluster control layer is here:

- `/Users/timiadesina/Documents/owl/src/distributed/shard_manager.rs`

It models:

- nodes
- shards
- shard status
- node status
- capacity
- rebalancing operations
- cluster health

This gives Owl a meaningful Rust-side cluster control plane.

### 3.8 `DistributedStorage`

The distributed storage adapter is here:

- `/Users/timiadesina/Documents/owl/src/distributed/storage_adapter.rs`

This is significant because it actually implements `DocumentStore`.

That means, in principle, Owl can route the same query abstraction over:

- local storage
- distributed storage

It already handles:

- local vs remote get/put/delete
- global object reference navigation
- shard routing
- remote document caching
- object reference caching

But this adapter is still only partially complete as a full serving layer.

The most important current limitation is explicit in the implementation:

- distributed `scan_documents(...)` currently only scans the local shard and leaves remote shard scanning as TODO

That is a major indicator that the distributed `DocumentStore` path is real, but not yet complete enough to stand in for ordinary multi-shard query execution across the board.

### 3.9 `WALFirstStorage`

The WAL-first distributed storage path is here:

- `/Users/timiadesina/Documents/owl/src/distributed/wal_first_storage.rs`

This layer is important because it captures a better story for atomic writes:

- reserve
- WAL
- commit to arena
- replay on recovery

It implements `DocumentStore`, and it is a serious attempt at aligning distributed durability with Owl's write semantics.

Its strengths:

- explicit WAL-first durability boundary
- reservation and commit pattern
- recovery via WAL replay
- `GlobalObjRef` indexing

Its current limitation:

- cross-shard navigation is not fully implemented at this layer
- schema handling is intentionally incomplete
- it is more focused on durability and atomicity than on becoming the full general serving engine

### 3.10 `DistributedOQLEngine`

The distributed OQL execution layer is here:

- `/Users/timiadesina/Documents/owl/src/distributed/oql_distributed.rs`

This layer is valuable because it shows the intended direction:

- route writes by shard
- allow cluster-manager-mediated remote execution
- preserve transaction semantics across shards

But it is not yet equivalent in maturity to the main local `QueryExecutor`.

The key limitation is that many richer query operations still do not receive the same depth of execution as the local engine. The file itself makes that visible: some branches effectively degrade to simple storage scans or note that a fuller engine would still be needed.

### 3.11 `DistributedQueryPlanner`

The planner is here:

- `/Users/timiadesina/Documents/owl/src/distributed/query_planner.rs`

It already models:

- distributed scan
- distributed join
- navigate
- aggregate
- project
- sort
- limit
- union
- network hop cost
- distributed cost estimation

This is substantial planning work.

But today it is best understood as a serious distributed planning subsystem that is not yet the normal everyday path ordinary Owl queries take.

### 3.12 High-level Rust assessment

The Rust side is strongest when:

- expressing the query/storage contract
- serving the local engine
- handling reverse-reference and vector coordination
- defining distributed control-plane abstractions

The Rust side is weaker where:

- distributed scan and distributed query completeness are required
- planner and serving path need to become one unified system
- local and distributed execution still feel like parallel worlds

---

## 4. Lower-level Zig parts

The Zig layer is where the lower-level storage substrate, concurrency primitives, distributed object identity, and durability structures live.

This layer is richer than a plain allocator or arena. It already has the beginnings of a real distributed object store substrate.

### 4.1 Arena and object substrate

The local object substrate includes:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/arena.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/object.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/object_header.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/objref.zig`

This is the base object memory model Owl's higher-level storage relies on.

### 4.2 WAL and lock-free WAL

The Zig durability layer includes:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/wal.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`

The lock-free WAL in particular is serious work:

- lock-free LSN generation
- lock-free append coordination
- group commit coordination
- concurrent append intent management

This is one of Owl's strongest low-level performance-oriented areas.

The newer ring-based work we did separately strengthens the ingress story even further, but the broader conclusion remains:

> Owl's Zig layer is already thinking in terms of high-throughput concurrent durability, not just correctness.

### 4.3 MVCC and transaction primitives

These live here:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/mvcc.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/transaction.zig`

These modules already define:

- transaction IDs
- transaction state
- isolation levels
- snapshots
- version chains
- version visibility rules
- transaction manager

This is a meaningful concurrency foundation.

However, the current implementation is still not complete in the production sense:

- version-chain walking is still shallow
- comments note where full storage-backed chain traversal would still be needed
- garbage collection behavior is still simplified

So this is real and useful, but not yet final-form MVCC.

### 4.4 Concurrency primitives

These are in:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/concurrency_primitives.zig`

This module contains:

- atomic reference counting
- shared-object support
- hazard pointers
- safe memory reclamation machinery

This matters because it shows the Zig layer is being designed as a long-lived concurrent engine, not just a single-threaded persistence substrate.

### 4.5 `GlobalObjRef`

This is one of the most important distributed Zig primitives:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/global_objref.zig`

It provides:

- 128-bit global object references
- shard id
- local offset
- flags
- local vs remote distinction
- migration and replication metadata

This is exactly the kind of primitive Owl needs if distributed object navigation is going to be a native language concern rather than an application-level convention.

### 4.6 Distributed arena management

This lives in:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/distributed_arena.zig`

It already models:

- shard registry
- shard state
- allocation into a chosen shard
- locality-aware allocation
- migration tracking
- remote navigation results

That is a serious distributed object-store substrate.

But it is important to be precise:

- it is a lower-level distributed arena manager
- it is not yet obviously the universal storage engine ordinary Rust query serving uses every day

### 4.7 Distributed B-tree support

This is in:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/btree.zig`

The B-tree already includes:

- distributed keys
- shard-aware insert/search/delete
- cross-shard routing
- shard map
- some cross-shard range coordination

This is substantial and valuable.

But it is also still best understood as distributed-capable index substrate rather than fully mature distributed query-serving infrastructure.

### 4.8 Distributed WAL

This lives in:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/distributed_wal.zig`

It models:

- distributed begin/prepare/commit/abort
- shard membership events
- replication
- catch-up synchronization
- replication manager
- synchronization manager

That is a meaningful durability and coordination substrate for sharded storage.

### 4.9 Lower-level Zig assessment

The Zig side is strongest when:

- modeling efficient local object storage
- modeling high-throughput WAL and ingress paths
- defining distributed object identity
- defining shard-aware durability and navigation primitives
- providing concurrency and memory-reclamation building blocks

The Zig side is weaker where:

- those distributed primitives still need to be promoted into the ordinary serving path
- some concurrency and MVCC machinery is still simplified relative to a final production engine
- lower-level distributed substrate and higher-level distributed query runtime are not yet one continuous path

---

## 5. Current architecture split: what is integrated vs what is parallel

This is the most important distinction in the whole review.

### 5.1 What is already integrated

These are part of the current real serving path:

- Rust `DocumentStore`
- Rust `ZigStore`
- executor-driven query execution
- reverse-reference prefiltering
- scalar index metadata ownership
- vector ANN support
- WAL-backed metadata recovery

If you run ordinary Owl today, this is the world you are really in.

### 5.2 What is real but parallel

These are real systems, but not yet the same dominant path as the current local executor:

- Rust `DistributedStorage`
- Rust `WALFirstStorage`
- Rust `DistributedOQLEngine`
- Rust `DistributedQueryPlanner`
- Zig `DistributedArenaManager`
- Zig distributed WAL machinery
- Zig distributed B-tree machinery

These are not fake. But they are not yet a single unified "this is how Owl queries work in distributed mode" path.

### 5.3 Why this matters

This split is why the right conclusion is not:

- "Owl does not have sharding"

and also not:

- "Owl already has fully unified scale-out query execution"

The honest conclusion is:

> Owl already has a strong distributed substrate and several distributed runtime layers, but they still need convergence into the main serving path.

---

## 6. Scalability analysis

We should separate scalability into scale-up and scale-out.

### 6.1 Scale-up

Owl is already making real moves toward good scale-up characteristics.

The strongest pieces are:

- lock-free WAL work
- ring-based WAL ingress prototype
- reverse-reference acceleration
- storage-owned vector ANN
- local object graph navigation
- concurrency primitives in Zig

So for single-node growth, the architecture is promising and increasingly real.

### 6.2 Scale-out

Owl also has a meaningful scale-out story in terms of substrate:

- shard identity
- global object references
- distributed durability
- shard manager
- distributed storage adapter
- distributed query planner

But current scale-out serving has these gaps:

1. distributed scans are incomplete
2. distributed query execution is not yet equivalent in richness to the local executor
3. planner and serving path are not yet unified
4. reverse-reference and graph-native acceleration are not yet fully promoted into the distributed path
5. distributed transaction machinery exists, but the full ordinary query-serving path is not yet built on top of it end-to-end

---

## 7. What this means for Owl query semantics

### 7.1 What Owl queries can rely on today

Today, an Owl query can rely fairly well on:

- local graph-style traversal
- local reverse-reference acceleration
- local document scans with schema-aware path handling
- local vector + structured query combination
- local durability and restart recovery

### 7.2 What Owl queries cannot yet fully rely on in distributed mode

An Owl query cannot yet fully assume that:

- local and multi-shard execution have the same execution richness
- cross-shard collection operations are complete
- distributed planning is the same as ordinary planning
- every graph-native optimization available locally is equally available across shards

That is the key gap between substrate maturity and language/runtime maturity.

---

## 8. Main strengths

The strongest parts of the current architecture are:

1. the language and storage contract are aligned
2. refs and reverse refs are treated as core storage/runtime concerns
3. vector support is already entering the same query model as structured navigation
4. distributed object identity is modeled correctly
5. lower-level concurrency and WAL thinking are serious
6. there is already a real distributed Rust layer, not merely future ideas

This is a very good foundation.

---

## 9. Main weaknesses and risks

The biggest current weaknesses are:

1. duplicated local and distributed runtime worlds
2. incomplete distributed collection semantics
3. planner and executor split across parallel architectures
4. partial distributed scan behavior
5. lower-level distributed substrate not yet fully adopted by the normal serving path
6. MVCC and distributed transaction depth not yet fully unified into one trusted execution stack

The main architectural risk is not lack of good ideas.

The main architectural risk is:

> divergence between a strong local serving engine and a separately strong distributed substrate, without a single dominant execution path that unifies them.

---

## 10. Recommended convergence plan

The next architectural steps should not be "add more distributed modules".

They should be about convergence.

### 10.1 Make the distributed `DocumentStore` path a first-class ordinary serving target

The local executor should be able to operate against a distributed store path without feeling like it entered a different product.

That means:

- making remote scan semantics complete
- making cross-shard document access ordinary
- ensuring the same query operations behave coherently

### 10.2 Unify the executor and distributed planner

The planner should not remain a parallel subsystem.

The goal should be:

- one query language
- one dominant execution story
- different physical plans depending on locality and shard distribution

### 10.3 Preserve Owl-native accelerators across shards

The local engine already benefits from:

- reverse-reference acceleration
- graph-style navigation
- vector ANN integration

These should not be local-only conveniences.

They should become part of the distributed strategy too.

This is especially important because these are some of Owl's most distinctive advantages.

### 10.4 Choose the dominant distributed write path

There are several meaningful write-oriented distributed components:

- distributed WAL work
- WAL-first storage
- distributed transaction coordination

The system will become much clearer once one of these is made the canonical distributed write path and the others become support layers around it.

### 10.5 Promote the Zig distributed substrate more directly into the active serving path

The Zig layer already has:

- global refs
- distributed arena concepts
- distributed B-tree support
- distributed WAL concepts

The next step is less new invention and more adoption:

- make these the actual substrate ordinary distributed serving relies on

---

## 11. Final assessment

Owl is in a strong but transitional place.

### Higher-level Rust

Rust currently provides:

- the active local query-serving contract
- the main local runtime backend
- reverse-reference query acceleration
- vector coordination
- distributed control-plane and execution abstractions

Rust is where Owl's serving semantics are most mature today.

### Lower-level Zig

Zig currently provides:

- the object substrate
- durability substrate
- concurrency substrate
- distributed object identity
- distributed storage and WAL building blocks

Zig is where Owl's long-term scale-up and scale-out storage foundation is strongest.

### Overall

The system already has enough pieces that the correct next move is not broad invention.

The correct next move is architectural convergence:

- unify local and distributed serving
- unify planner and runtime
- promote graph-native and vector-native capabilities into the distributed path
- make the sharded path feel like ordinary Owl rather than a parallel mode

That is the path from "promising distributed substrate" to "a scale-out Owl query engine worthy of Owl's language model."
