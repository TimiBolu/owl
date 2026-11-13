# Owl

Owl is an object-graph database with a programmer-native query language, a
schema-compiled storage engine, and first-class reference traversal. The entire
system — language, runtime, and storage — is implemented in Rust with no
external database or FFI dependency.

Owl is under active development and not yet suitable for production data.

## What makes Owl different

Most databases force you to reconstruct relationships by hand. Owl treats
references between objects as first-class navigable connections, so traversing
`orders.customer.name` is part of the query model, not an afterthought bolted
onto a key-value or document store.

The core idea: if your data is already modeled as an object graph, the database
should let you query it that way.

## Query language (OQL)

OQL is a collection pipeline language. Anything that behaves like a collection —
a top-level table, a nested array, a reverse relationship — supports the same
operations: `where`, `select`, `sort`, `limit`, `group_by`, `count`, `sum`,
`avg`, `min`, `max`, `nearest`.

Two syntactic forms lower to the same execution path:

```oql
// Chained form
events
    .where { .level == "error" && .occurred_at >= $from }
    .sort { .occurred_at desc }
    .limit(100)
    .select {
        id,
        occurred_at,
        message,
        application: .application.name
    }
```

```oql
// Scoped block form (same semantics, better for composition)
events {
    where { .level == "error" }
    select { id, message, app: .application.name }
    limit 100
}
```

References resolve naturally through the object graph:

```oql
// Forward traversal — null-safe, no joins
orders.select {
    id,
    customer_name: .customer.name,
    items: .customer.orders.where { .total > 100 }
}
```

```oql
// Reverse traversal — walk back across a reference field
companies.select {
    id,
    order_count: .findings.where { .status == "open" }.count()
}
```

Mutations use the same collection roots:

```oql
users.insert {
    id: "user1",
    name: "Alice",
    age: 28,
    profile: { bio: "Engineer", preferences: { theme: "dark" } }
}

users.where { .id == "user1" }.update { age: 29 }
users.where { .id == "user1" }.delete()
```

Reusable pipelines via bindings:

```oql
open_findings = findings.where { .workflow.status == "OPEN" }

open_findings
    .select { id, severity }
    .limit(20)
```

User-defined functions, correlated subqueries via collection-name scope anchors,
match expressions, conditional expressions, and vector `nearest` search are all
part of the current language surface.

### Using OQL from Rust

```rust
use owl_db::create_oql_runner;

let runner = create_oql_runner("./my_db")?;

runner.execute_content(r#"
    schema users {
        id: String,
        name: String,
        age: Integer
    }

    users.insert { id: "alice", name: "Alice", age: 30 }
    users.where { .age > 25 }.select { id, name }
"#)?;
```

OQL files can define schemas, bindings, functions, and transaction blocks that
execute as a script.

## Storage engine

The storage engine is entirely Rust-owned. There is no RocksDB, no Zig, no FFI.
Rust owns the full correctness boundary: identity, records, WAL, indexes,
checkpoints, compaction, and recovery.

### Object identity

Every object has a stable `ObjRef` — a packed 64-bit identity that never
encodes a file offset:

```
+-----------------+----------------------------------+------------------+
| collection (16) |          object id (32)          | generation (16)  |
+-----------------+----------------------------------+------------------+
```

An object directory maps `ObjRef` to the current physical record location.
Compaction can move records without rewriting references.

### Schema-compiled records

Documents are not stored as serialized field-name maps. The `RecordCodec`
compiles each schema into fixed slots, a null bitmap, and a checked variable
region. Top-level field names are stored once in the compiled schema, not
repeated per record. Stable field IDs survive schema evolution, so old and new
record versions coexist.

Projection decoding reads only the requested fields without materializing the
full document. This is the primary mechanism for selective field reads.

### Write path

- One serialized writer, atomic mutation batches
- Checksummed WAL frames with contiguous commit LSNs
- One durability sync (`fsync`) before publication
- Group commit: concurrent producers share one WAL batch and sync
- Local transactions with begin/commit/rollback and read-your-writes
- Unique constraint checks before WAL commit (no partial batch on conflict)

### Recovery and checkpoints

- Restart recovery loads the latest valid checkpoint, then replays later WAL
- Torn-tail removal: incomplete WAL frames at the end are detected and removed
- Alternating manifests (`OWLM`) with crash-safe publication ordering:
  segment, manifest, then WAL truncation
- Full checkpoints write a complete snapshot
- Incremental checkpoints fold the delta since the last checkpoint
- Compaction folds flush generations into a new base
- Automatic checkpoint scheduling by WAL byte threshold or cumulative delta
- Integrity checker validates checkpoint, WAL, indexes, and live state
- Obsolete file reclamation for old checkpoint and WAL segments

### Indexes

- Scalar indexes: ordered, unique, composite, memcomparable typed keys
- B-tree with in-memory overlay over checkpoint-backed file pages
- Copy-on-write index pages with independent checksums
- Composite-prefix ranges with early limits and keyset cursors
- Reverse reference indexes: auto-derived from schema, used for graph-aware
  query prefiltering
- Vector: exact top-k brute-force fallback and HNSW ANN with rerank
  (cosine, L2, dot product)
- Index definitions and postings are durable in checkpoints and validated
  against authoritative records on open

### Query execution

- Physical plan pipeline with indexed reads and sequential scans
- Index scan with typed inclusive/exclusive bounds
- OR-branch deduplication and bounded ordered top-K unions
- Projection pushdown: storage decodes only requested fields from compiled
  records
- Storage-owned filtered scans fuse predicate evaluation with materialization
- Batch execution returns move-owned chunks for streaming workloads
- Per-operator profiling with execution stats
- Reverse reference prefiltering exploits the object graph model directly

## Build

Owl requires only a Rust toolchain.

```bash
cargo check --all-targets
cargo test --lib storage_engine
```

The focused storage invariant tests are the release gate. The wider historical
test suite contains known failures in experimental features (distributed,
simulation, columnar) that are outside the trusted single-node path.

## Loggr — first application

[Loggr](apps/loggr/) is a local-first error investigation workbench built on
Owl's public Rust API. It stores structured logs, exceptions, stack traces, and
trace context, and serves an HTTP query interface.

```bash
cargo run -p loggr -- serve --db ./loggr.owl
cargo run -p loggr -- demo --count 240
cargo run -p loggr -- check --db ./loggr.owl
```

Loggr exercises the ingestion, batch commit, index, reference, and checkpoint
paths end to end. It is the forcing workload that keeps the storage engine
honest for real application shapes, not just microbenchmarks.

## What is not supported

These exist in the repository as experimental code but are not part of the
trusted single-node engine:

- Distributed consensus (Raft), sharding, and cross-node transactions
- General multi-writer MVCC
- Automatic graph partitioning
- General analytical column store
- Schema alteration/migration syntax (not yet settled in the language contract)

Do not interpret their presence as a supported capability.

## Repository map

| Path | Contents |
|---|---|
| `src/storage_engine/` | Physical storage: WAL, checkpoints, compaction, records, indexes, vector |
| `src/storage.rs` | `DocumentStore` trait, `RustStore`, scalar/vector index definitions, projection types |
| `src/parser.rs` | OQL tokenizer, parser, file execution, script bindings |
| `src/executor.rs` | Query executor: filters, projections, aggregates, reference resolution, vector search |
| `src/planner/` | Logical and physical plan types |
| `src/execution/` | Physical operators: seq scan, index scan, join, aggregate, parallel scan |
| `src/optimizer/` | Statistics, histograms, selectivity estimation |
| `src/schema.rs` | Schema definitions, field types, reference types, constraints, evolution |
| `src/document.rs` | `Document` and `Value` types (the in-memory data model) |
| `src/oql_runner.rs` | `OQLRunner` for executing `.oql` files |
| `src/admin/` | Admin CLI: backup, cluster, health, index management |
| `src/distributed/` | Experimental: consistent hashing, shard manager, Raft, global ObjRef |
| `apps/loggr/` | First application: error investigation workbench |
| `oql/` | OQL example files and syntax fixtures |
| `benches/` | Index scan, ingest, navigation benchmarks |
| `docs/` | Design documents and architecture specs |

## Key design documents

- [Storage Engine Architecture](docs/STORAGE_ENGINE_ARCHITECTURE.md) — canonical storage design, invariants, disk layout
- [OQL Language Description](docs/OWL_LANGUAGE_DESCRIPTION.md) — language contract and design principles
- [OQL Implementation Status](docs/OQL_IMPLEMENTATION_STATUS.md) — what parses, executes, and is tested
- [Owl Core Technical Description](docs/OWL_CORE_TECHNICAL_DESCRIPTION.md) — system-level design connecting query, storage, and distribution
- [Loggr Product and Architecture](docs/LOGGR_PRODUCT_AND_ARCHITECTURE.md) — first application spec
- [Improved Syntax Guide](oql/IMPROVED_SYNTAX_GUIDE.md) — bindings, functions, correlation, method calls

## Performance philosophy

Owl does not claim universal superiority over mature storage engines. Its
advantage comes from information a generic KV engine does not have:

- No repeated field names in schema-compiled records
- Selective field decoding and projection pushdown
- Stable forward references and auto-derived reverse reference indexes
- Relationship-aware query planning and prefiltering
- Collection-specific append and retention policies

Performance claims require reproducible comparisons with equal durability,
batching, compression, cache, hardware, and dataset settings. The benchmark
suite targets Log, Ledger, Game, and Commerce workload profiles.

## Contributing

The current priority is storage correctness and a single connected query path.
Useful contributions include deterministic crash tests, format fuzzing, record
codec work, typed key encoding, differential index tests, and reproducible
workload benchmarks.
