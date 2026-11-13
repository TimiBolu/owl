# Owl Core Technical Description

## Purpose

This document describes Owl as a single technical system.

Its job is to connect:
- query semantics
- document and reference semantics
- storage semantics
- distribution and sharding semantics

without collapsing into parser internals or implementation trivia.

It should answer, in one place:

1. what Owl is trying to be
2. what data model it exposes
3. what a query means
4. what the storage engine must uphold
5. what distribution must preserve

This document is not a benchmark note, not a marketing note, and not a dump of source files.
It is the current system description and the intended system contract.

---

## 1. What Owl Is

Owl is a database and query system for modern application workloads.

Its central idea is that the way programmers naturally think about application data should be close to the way the database lets them query and traverse it.

The dominant shapes of modern application data include:
- objects with fields
- nested values
- arrays of values
- references between entities
- collections of related entities
- repeated traversal across those relationships

Owl is designed around those shapes directly.

It is therefore not best understood as:
- a relational system with object conveniences
- a document system with ad hoc joins
- a key-value system with query features on top

It is better understood as:

> a persistent object-graph database with collection semantics, reference semantics, and programmer-native query composition

That description is important because it explains why Owl's language, storage, and distribution models must be designed together.

---

## 2. Core Design Principles

### 2.1 Programmer-native

Owl is for programmers.

Its semantics should align with how programmers already think:
- values
- fields
- nested objects
- references
- collections
- filtering
- mapping
- grouping
- local bindings
- scoped computation

It should not force programmers to reconstruct object relationships through unnatural mechanics.

### 2.2 One dominant mental model

Owl should have one dominant mental model rather than separate mini-languages for:
- top-level collections
- nested arrays
- reverse relationships
- grouped data
- distributed data

Anything that behaves like a collection should feel like a collection.

### 2.3 Least effort

If the data is already modeled in a way that expresses a relationship, the query language should use that model directly rather than force the user to restate it manually.

### 2.4 Least surprise

The same ideas should behave the same way everywhere.

The cost model may differ internally, but the semantics should not fracture.

### 2.5 Storage should honor semantics

The storage engine is not just a persistence detail.

It exists to physically realize Owl's semantics:
- nested resolution
- reference traversal
- predictable identity
- locality where beneficial
- scalable placement and routing

### 2.6 Distribution must preserve conceptual integrity

Local Owl and distributed Owl must still feel like Owl.

Sharding must not create a second language.

---

## 3. Core Data Model

Owl is built on five primary entities:

1. schema
2. collection
3. document
4. value
5. query

### 3.1 Schema

A schema defines the intended shape of a collection's documents.

A schema may define:
- scalar fields
- object fields
- list fields
- vector fields
- reference fields
- optional fields
- constraints

The schema is a language-level and runtime-level contract. It is not merely advisory metadata.

### 3.2 Collection

A collection is a named set of documents of a schema.

It is also the normal root of a query.

Examples:
- `users`
- `orders`
- `companies`

### 3.3 Document

A document is an object with:
- identity
- schema membership
- versioning metadata
- named fields

Current Rust model:
- `/Users/timiadesina/Documents/owl/src/document.rs:101`

### 3.4 Value

Owl values include:
- `Null`
- `Boolean`
- `Integer`
- `Float`
- `String`
- `DateTime`
- `Reference`
- `GlobalReference`
- `List`
- `Object`

Current Rust model:
- `/Users/timiadesina/Documents/owl/src/document.rs:183`

### 3.5 Reference

A reference is not a textual join key disguised as a relationship.

In Owl, a reference is intended to be a first-class navigable connection between documents.

Locally, this is represented by a physical object reference.
In distributed mode, this is represented by a shard-aware global object reference.

The language-level consequence is:

```oql
orders.select {
  id,
  customer_name: .customer.name
}
```

should feel natural because the reference is part of the model, not an afterthought.

---

## 4. Query Semantics

## 4.1 Query roots

An Owl query normally starts from a collection-valued thing.

That may be:
- a top-level collection
- a list field
- a reverse relationship
- the result of an earlier binding
- a nested query result

This is one of Owl's core simplifications.

### 4.2 Collection operations

Collection-valued things should support the same family of operations where meaningful:
- `where`
- `select`
- `sort`
- `limit`
- `group_by`
- `having`
- `count`
- `sum`
- `avg`
- `min`
- `max`
- `nearest`

The goal is not that every collection-valued thing supports every operation identically in implementation, but that the semantic model remains consistent.

### 4.3 Scope

Owl uses a scoped evaluation model:
- `.` refers to the current row/value in the current scope
- collection names may anchor outer scope
- local bindings may be introduced for reuse

This allows nested computation without inventing awkward outer-query syntax.

### 4.4 Chained and scoped-block equivalence

Owl has two main surface forms:
- chained form
- scoped block form

These are intended to be equivalent query semantics with different presentation and reuse affordances.

That means:

```oql
stores
  .where { .region == "north" }
  .select { id, name }
```

and:

```oql
stores {
  where { .region == "north" }
  select { id, name }
}
```

should mean the same kind of thing.

Scoped blocks add local bindings and clearer composition; they do not introduce a second semantic universe.

### 4.5 Nested and reference traversal

Owl path resolution supports:
- nested object access
- reference traversal
- global reference traversal
- graceful null propagation

Current runtime resolution begins in:
- `/Users/timiadesina/Documents/owl/src/executor.rs:1494`

This is a key part of Owl's language identity. A path should resolve as a path, not force the user into a different relationship language.

### 4.6 Null and missing-field semantics

Owl prefers graceful resolution over brittle failure for missing fields and nulls.

This is important for:
- schema evolution
- nested optional data
- uneven application payloads

That does not mean "anything goes." It means absence is a first-class runtime reality and should compose predictably.

---

## 5. Query Execution Model

Today, Owl query semantics are primarily executed in Rust.

Main executor:
- `/Users/timiadesina/Documents/owl/src/executor.rs`

At a high level, the current execution shape is:

1. identify the root collection or collection-valued source
2. obtain candidate documents
3. evaluate filters
4. resolve nested values and references as needed
5. apply transforms such as select, sort, group, aggregate, nearest
6. return shaped results

This is the current semantic center of Owl.

### 5.1 Storage abstraction boundary

The executor does not directly depend on Zig.

It depends on the storage contract:
- `/Users/timiadesina/Documents/owl/src/storage.rs:41`

This is important.

Owl's language semantics should not depend on one specific storage implementation. The storage layer must uphold the semantics the query layer expects.

### 5.2 Reverse-reference acceleration

Owl already performs useful graph-native optimization through reverse-reference prefiltering.

This matters because it shows the engine is not only scan-first in a naive sense; it is beginning to exploit the object graph model directly.

### 5.3 Vector-aware execution

Vector search is already part of the ordinary query model rather than a separate service universe.

That is good and should be preserved.

---

## 6. Storage Model

The storage model is where Owl's semantics must become physical.

This is the most delicate part of the system because it must balance:
- simplicity
- speed
- nested resolution
- reference traversal
- recovery correctness
- future sharding

### 6.1 What the storage layer must provide

The storage layer must support:
- durable document persistence
- stable identity lookup
- schema persistence
- reference navigation
- reverse-reference support or acceleration
- vector artifact support where relevant
- restart recovery
- distributed extension without conceptual breakage

### 6.2 Current architecture

Today Owl is roughly:

- Rust semantic runtime
- Zig physical storage substrate

The current persistence path is closer to:

```text
Rust Document -> serialized blob -> Zig arena
```

than to:

```text
Rust Document -> fully native Zig semantic object graph
```

This is the current truth.

It is not a failure. It is an architectural stage.

### 6.3 Current write path

At a high level:

1. Rust executor creates a `Document`
2. storage adapter wraps it as a stored form
3. Rust serializes the payload
4. Zig-backed arena allocates space
5. payload bytes are written into the arena
6. Rust metadata structures are updated
7. durability boundaries are enforced

Key code:
- storage trait: `/Users/timiadesina/Documents/owl/src/storage.rs:41`
- ZigStore write entry: `/Users/timiadesina/Documents/owl/src/storage.rs:2198`
- core blob write path: `/Users/timiadesina/Documents/owl/src/storage.rs:2038`

### 6.4 Current read path

At a high level:

1. lookup or scan finds an object reference
2. bytes are read from the Zig-backed arena
3. payload header and checksum are validated
4. Rust deserializes the stored document
5. query execution continues over the Rust `Document`

Key code:
- get path: `/Users/timiadesina/Documents/owl/src/storage.rs:2165`
- scan path: `/Users/timiadesina/Documents/owl/src/storage.rs:2296`
- core blob read path: `/Users/timiadesina/Documents/owl/src/storage.rs:2070`

### 6.5 Current strengths of this storage approach

This approach is strong because:
- language/runtime evolution remains fast
- storage and semantic concerns are separated cleanly
- Zig stays focused on storage primitives
- the system avoids premature semantic over-coupling

### 6.6 Current limitations

This approach is limited because:
- the primary stored form is largely opaque to the storage engine
- selective field reads are harder from the main representation
- full-document materialization is often required
- storage-native semantic execution is limited

This is why a hybrid future is likely better than a permanently blob-only future.

### 6.7 Likely desired storage direction

Owl likely wants a hybrid physical representation:
- native hot metadata
- native typed references
- native null/presence structure
- optional native hot scalar fields
- flexible nested payload tail

This would preserve Owl's semantic flexibility while allowing the storage engine to participate more directly in performance-critical paths.

---

## 7. Distribution and Sharding Model

Owl is intended to scale through sharding while preserving a single conceptual model.

That means the distributed system is not a separate product layered awkwardly on top of Owl semantics.

It is the distributed form of the same model.

### 7.1 What distribution must preserve

Sharding must preserve:
- collection semantics
- reference semantics
- identity semantics
- query composition semantics
- predictable null and traversal behavior

Users should not feel that local Owl is one language and distributed Owl is another.

### 7.2 Current distributed building blocks

The Rust distributed subtree already includes:
- consistent hashing
- shard manager
- global object references
- distributed storage adapter
- distributed WAL-first storage
- distributed OQL execution layer

Relevant path:
- `/Users/timiadesina/Documents/owl/src/distributed/`

### 7.3 Global references

Distributed Owl requires a reference representation that can survive across shards.

That is the role of `GlobalObjRef`.

This is essential because a system that wants reference-native querying cannot treat distributed identity as an afterthought.

### 7.4 Shard-local vs cross-shard

The system should strongly prefer shard-local work where possible.

That means:
- local reads should remain cheap
- local traversal should remain simple
- cross-shard traversal should be explicit in implementation cost even if not ugly in language

This is one of the most important design constraints for future performance.

### 7.5 Conceptual integrity under sharding

The biggest distributed risk is conceptual fracture.

That happens when:
- local reference traversal feels natural but cross-shard traversal becomes mechanical
- certain query forms mean different things in distributed mode
- sharding leaks too much infrastructure complexity into the user model

Owl should resist that strongly.

---

## 8. Reliability Model

Owl will only become seriously respected if its semantics and storage model are matched by strong reliability discipline.

This means:
- explicit invariants
- explicit commit ordering
- explicit recovery behavior
- measurable failure handling

### 8.1 Required reliability properties

At a minimum Owl needs:
- durable committed writes
- deterministic restart recovery
- corruption detection
- coherent identity lookup
- coherent reference structures
- coherent index structures
- explicit behavior under distributed partial failure

### 8.2 Reliability is part of the architecture, not a later feature

Because Owl's value proposition depends on semantic coherence, reliability failures are not just storage failures. They can become language-trust failures.

If a programmer cannot trust traversal, identity, or updates, Owl loses its core advantage.

---

## 9. What Owl Should Optimize For

Owl should optimize first for modern application workloads where the dominant concerns are:
- object and relationship traversal
- nested document access
- natural transformation and shaping
- strong write/read correctness
- scalable operational workloads

It should not initially optimize first for:
- maximum SQL feature parity
- maximum analytics specialization at all costs
- maximum benchmark theater across every category

Its best path is focused excellence.

---

## 10. The Current Technical Position

The most honest current summary is:

> Owl already has a meaningful language, a real runtime, a real storage engine, and substantial distributed infrastructure. The remaining technical challenge is convergence: making storage, distribution, and query semantics increasingly reinforce one another without breaking Owl's conceptual simplicity.

That is where Owl stands today.

---

## 11. The Desired Technical Position

The target state is something like:

> Owl lets programmers work with data the way they naturally think about it, while the storage engine and distributed system quietly make that model fast, correct, and scalable.

That means:
- query semantics stay elegant
- storage semantics stay disciplined
- distribution semantics stay coherent
- internal sophistication does not leak upward unnecessarily

That is the standard Owl should design toward.

---

## 12. Bottom Line

Owl is best understood as a unified technical idea:

- a programmer-native query language
- over a persistent object graph model
- backed by a storage system designed for fast nested and reference resolution
- and extended by sharding without sacrificing conceptual integrity

That is the system.

Everything else should be judged by whether it strengthens or weakens that unity.
