# Owl Scale-Out Convergence Roadmap

This document turns the architecture review into an implementation roadmap.

It is focused on one goal:

> make scale-out Owl feel like ordinary Owl.

That means:

- the same query language
- the same intuitive semantics
- the same graph-native expectations
- different physical execution paths only where the system needs them

The work is therefore not "add distributed features".
It is:

- converge the higher-level Rust execution path
- converge the lower-level Zig storage substrate
- close the serving gaps between local Owl and distributed Owl

Related review:

- `/Users/timiadesina/Documents/owl/docs/OWL_SCALE_OUT_STORAGE_ARCHITECTURE_REVIEW.md`

---

## 1. Primary architectural objective

Today Owl has:

1. a strong local serving engine
2. a meaningful distributed substrate
3. a partially separate distributed execution path

The objective of this roadmap is to eliminate the separation that matters to users.

The end state should be:

- one query language
- one dominant execution model
- one `DocumentStore` contract that works locally and across shards
- one planner/runtime story
- graph-native and vector-native behavior preserved at scale

---

## 2. Core principles for the scale-out path

The scale-out design must still obey OQL's language principles.

### 2.1 Least effort

Users should not have to write a "distributed version" of an Owl query.

If a query works locally, the scale-out path should preserve:

- shape
- meaning
- mental model

### 2.2 Least surprise

Sharding should not change query meaning unexpectedly.

A distributed query may differ in:

- latency
- cost
- execution strategy

but not in:

- basic semantics
- reference behavior
- projection semantics
- null-safe traversal behavior

### 2.3 Model once, navigate both ways

If refs remain central locally, they must remain central across shards.

This means scale-out Owl must preserve:

- forward ref navigation
- implicit reverse traversal
- reverse-reference acceleration where possible

### 2.4 One mental model

The user must not feel that:

- local Owl is one product
- distributed Owl is another

This is the central test for every step in this roadmap.

---

## 3. The three convergence fronts

There are three fronts to this work.

### Front A: Higher-level Rust query-serving convergence

This is about:

- executor
- planner
- `DocumentStore` semantics
- distributed execution coordination

### Front B: Lower-level Zig storage-substrate convergence

This is about:

- arena
- refs
- WAL
- MVCC
- distributed object identity
- shard-aware storage mechanics

### Front C: Semantic preservation

This is about ensuring that:

- the language semantics users expect
- still hold when execution becomes shard-aware

This includes:

- ref traversal
- reverse traversal
- null-safe reads
- vector search
- grouping and projection

---

## 4. Phase 1: Make distributed `DocumentStore` sufficient for ordinary Owl queries

This is the highest-leverage convergence phase.

The current distributed `DocumentStore` path is real, but still incomplete relative to what ordinary Owl queries already require locally.

Key files:

- `/Users/timiadesina/Documents/owl/src/distributed/storage_adapter.rs`
- `/Users/timiadesina/Documents/owl/src/distributed/wal_first_storage.rs`
- `/Users/timiadesina/Documents/owl/src/storage.rs`

### 4.1 Goal

The distributed `DocumentStore` must become sufficient for the normal query executor to operate against it without major semantic degradation.

### 4.2 Required capabilities

The distributed `DocumentStore` must fully support:

1. `get_document(...)`
2. `put_document(...)`
3. `delete_document(...)`
4. `scan_documents(...)` across all relevant shards
5. `navigate_reference(...)` across local and remote refs
6. `get_objref(...)` with remote-awareness
7. reverse-reference operations across shards
8. schema access with a coherent replication/authority model
9. vector index metadata access and status where applicable

### 4.3 Immediate gaps to close

#### Gap 1: distributed `scan_documents(...)` is incomplete

Current state:

- local shard scan works
- remote scan is explicitly incomplete

This is the first practical blocker to ordinary distributed query execution.

#### Gap 2: reverse-reference operations are not yet fully distributed-first

Current Owl query performance increasingly depends on reverse-reference acceleration.

Distributed `DocumentStore` must decide:

- whether reverse refs are maintained shard-locally only
- whether cross-shard reverse-ref lookup is federated
- whether some reverse traversals can be resolved by shard routing plus local reverse index scans

#### Gap 3: schema handling is too thin in `WALFirstStorage`

Current state:

- schema operations are intentionally incomplete there

That is acceptable for a durability-focused subsystem, but not for a dominant ordinary serving path.

### 4.4 Deliverables for Phase 1

1. distributed `scan_documents(...)` implementation
2. remote scan extension in the network layer
3. distributed reverse-reference lookup contract
4. schema authority model for distributed stores
5. integration tests showing the ordinary query executor can run against the distributed store with the same core semantics as local execution

### 4.5 Acceptance standard

At the end of Phase 1, the ordinary executor should be able to run these query families against a distributed store:

1. simple collection scans
2. ordinary `where`
3. path-based `select`
4. forward ref navigation
5. reverse-ref prefilter cases

If that does not work cleanly, the distributed `DocumentStore` is still not converged enough.

---

## 5. Phase 2: Unify planner and runtime

Right now Owl has:

- a mature-ish ordinary executor
- a separate distributed planner
- a separate distributed OQL engine

That is too many execution stories.

Key files:

- `/Users/timiadesina/Documents/owl/src/executor.rs`
- `/Users/timiadesina/Documents/owl/src/distributed/query_planner.rs`
- `/Users/timiadesina/Documents/owl/src/distributed/oql_distributed.rs`

### 5.1 Goal

There should be one dominant execution architecture.

The planner should produce different physical strategies for:

- local-only work
- single-shard work
- multi-shard work

But it should not feel like a separate product branch.

### 5.2 What should be unified

#### A. Query representation

The distributed planner should operate on the same real OQL query structures used by the main executor, not a conceptually separate query model.

#### B. Physical execution decisions

The ordinary executor should be able to consume plans that include:

- local scan
- distributed scan
- navigate
- remote fetch
- local aggregation
- shard-local partial aggregation + merge

#### C. Graph-native optimization

The planner should understand:

- reverse-reference prefilter opportunities
- local vs remote reference hops
- shard fanout risk
- when a query is naturally scoped to one shard

This is one of Owl's best chances to outperform more mechanical distributed systems.

### 5.3 Immediate gaps

1. distributed planner currently feels more like a parallel subsystem than the source of actual runtime plans
2. distributed OQL execution is not yet equivalent in richness to the local executor
3. local graph-native accelerators are not yet clearly promoted into the distributed planning story

### 5.4 Deliverables for Phase 2

1. planner integration strategy document
2. one real physical plan type shared by local and distributed execution
3. executor support for shard-aware plan nodes
4. planner support for reverse-ref-aware routing
5. planner support for local-partial and global-merge aggregate patterns

### 5.5 Acceptance standard

At the end of Phase 2, an ordinary Owl query should:

- enter one planning path
- produce local or distributed physical operations as needed
- not require a separate distributed query engine to feel complete

---

## 6. Phase 3: Preserve graph-native semantics across shards

This is where Owl must avoid becoming a generic distributed document system.

If scale-out Owl loses its graph-native feel, then sharding will have weakened the language.

### 6.1 Goal

Preserve these strengths across shards:

1. forward ref navigation
2. implicit reverse traversal
3. reverse-ref acceleration
4. path-based projection
5. graph-scoped vector queries

### 6.2 Required design decisions

#### A. Cross-shard ref navigation

Need a clear policy for:

- one-hop remote navigation
- multi-hop remote navigation
- migrated refs and stale refs
- caching and invalidation

#### B. Implicit reverse traversal at scale

If `businesses.account: Ref(accounts)` implies:

- `business.account`
- `account.businesses`

then the distributed runtime must decide how `account.businesses` is resolved:

- local shard only
- via shard-local reverse indexes plus routing
- via coordinator fanout

#### C. Reverse-reference acceleration

This should become a distributed optimization, not remain local-only.

The planner/runtime should exploit:

- shard routing by target object
- shard-local reverse index scans
- intersection of distributed reverse-ref candidate sets

### 6.3 Deliverables for Phase 3

1. distributed reverse-reference model
2. distributed implicit reverse traversal model
3. remote reference navigation policy
4. cache invalidation and migration semantics for global refs

### 6.4 Acceptance standard

At the end of Phase 3, users should be able to write graph-native Owl queries without thinking about shard boundaries.

The engine may still pay coordination cost, but the semantics should remain intact and intuitive.

---

## 7. Phase 4: Make vector and structured querying scale together

Owl should not split vector behavior into a separate system when data becomes distributed.

### 7.1 Goal

Preserve Owl's strongest hybrid pattern:

- structured filtering
- graph scoping
- vector search
- shaped output

### 7.2 Required work

1. shard-aware vector index metadata
2. shard-local ANN execution
3. candidate merge across shards
4. optional graph-scoped prefilter before shard-local ANN
5. ranking merge semantics across shards

### 7.3 Acceptance standard

At the end of Phase 4, a query shaped like:

- filter by org/team/status
- then nearest on embedding

should work coherently whether the data is single-shard or multi-shard.

---

## 8. Phase 5: Choose the dominant distributed write path

Owl currently has several meaningful write-oriented distributed pieces:

- distributed WAL work
- WAL-first storage
- transaction coordination work

The architecture will become much clearer when one dominant write path is chosen.

### 8.1 Goal

Choose one canonical distributed write story.

### 8.2 Strong candidate

The strongest current candidate is:

- WAL-first distributed write path

because it is already aligned around:

- reserve
- WAL
- commit
- replay

That is a clearer foundation than a looser multi-path write strategy.

### 8.3 Required work

1. choose the dominant distributed write path explicitly
2. align transaction and recovery semantics around it
3. align shard-local and cross-shard durability semantics around it
4. decide how lower-level Zig WAL ingress improvements plug into that write path

### 8.4 Acceptance standard

At the end of Phase 5, there should be one authoritative answer to:

> how does a distributed Owl write become durable, visible, and recoverable?

---

## 9. Lower-level Zig convergence work

The Zig side should not be treated as a separate future database.

It should be treated as the storage substrate that the higher-level runtime increasingly adopts.

Key files:

- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/global_objref.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/distributed_arena.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/distributed_wal.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/btree.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/mvcc.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/transaction.zig`
- `/Users/timiadesina/Documents/owl/owl_storage_zig/src/lockfree_wal.zig`

### 9.1 What Zig already gives us

1. object substrate
2. distributed object identity
3. shard-aware distributed arena concepts
4. shard-aware distributed B-tree concepts
5. distributed durability concepts
6. WAL concurrency and ingestion work
7. concurrency primitives

### 9.2 What Zig still needs relative to convergence

1. clearer promotion of distributed primitives into the dominant runtime path
2. stronger direct support for the selected distributed write path
3. deeper productionization of MVCC/version-chain behavior
4. closer coupling between distributed refs, migration semantics, and serving/runtime expectations

### 9.3 Lower-level success criterion

The Zig substrate is "converged enough" when:

- the higher-level Rust runtime is no longer treating distributed Zig capabilities as mostly parallel experiments
- and instead relies on them directly for ordinary distributed serving

---

## 10. Suggested implementation order

This is the practical order I would follow.

### Step 1

Finish distributed `DocumentStore` sufficiency:

- remote scan support
- distributed reverse-ref contract
- schema handling clarity

### Step 2

Run the ordinary executor against distributed storage in integration tests until core local semantics hold.

### Step 3

Unify planner/runtime:

- one plan story
- one execution story

### Step 4

Promote reverse-ref and graph-native optimizations into the distributed path.

### Step 5

Unify vector + graph + structured execution across shards.

### Step 6

Choose and lock the dominant distributed write path.

### Step 7

Then optimize:

- ingress
- batching
- remote fetch latency
- merge phases
- routing heuristics

Optimization should come after convergence, not before.

---

## 11. Risks to avoid

### 11.1 Building a second product inside the same repo

The biggest risk is continuing to evolve:

- local Owl
- distributed Owl

as separate mental models and code paths.

### 11.2 Letting planner and runtime drift

If planning grows richer without serving-path adoption, the planner becomes a beautiful sidecar.

### 11.3 Losing graph-native advantages in distributed mode

If sharding forces users back into mechanical query patterns, then scale-out will have made Owl worse.

### 11.4 Over-investing in lower-level sophistication before path convergence

Owl already has many good lower-level primitives.

The next bottleneck is not lack of clever primitives.
It is making one coherent serving system out of them.

---

## 12. Final recommendation

The best next engineering move is not "more distributed research modules".

It is:

1. make distributed `DocumentStore` sufficient for ordinary Owl queries
2. unify planner and runtime
3. preserve graph-native semantics across shards
4. choose one dominant distributed write path

In short:

> converge the system around the query model Owl already deserves.

That is the path from "strong distributed ingredients" to "a genuinely scale-out Owl query engine."
