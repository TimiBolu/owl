# Owl

**An object-graph database written in Rust.** Your data is already a graph of
objects that point at each other. Owl lets you query it that way: you follow a
reference with a dot, and you never write a join.

[Website](https://owldb.tech) ·
[The language in seven chapters](https://owldb.tech/#language) ·
[Run it](#run-it) ·
[Help Owl grow](#help-owl-grow)

> Owl is under active development and not yet suitable for production data.

If Owl looks useful, or just interesting, a star on this repository helps other
people find it.

## A first look

```oql
orders
  .where { .status == "paid" }
  .sort { .total.desc }
  .limit(3)
  .select { id, who: .customer.name, total }
```

```json
[
  { "id": "o1", "who": "Ada",  "total": 125.0 },
  { "id": "o6", "who": "Ada",  "total": 97.0 },
  { "id": "o2", "who": "Bola", "total": 85.0 }
]
```

`.customer` is a reference from an order to a customer. `.customer.name` walks
it. There is no join to write and no key to match.

## Run it

Owl needs only a Rust toolchain.

```bash
# 1. Start a server
cargo run --release -p owl-server -- --data shop.owl

# 2. Declare two collections and add a row to each
curl 127.0.0.1:7878/v1/script --json '{"source": "schema customers { id: String, name: String }\nschema orders { id: String, customer: Ref(customers), total: Float }\ncustomers.insert { id: \"c1\", name: \"Ada\" }\norders.insert { id: \"o1\", customer: \"c1\", total: 12.5 }"}'

# 3. Follow the reference from order to customer
curl 127.0.0.1:7878/v1/query --json '{"query": "orders.select { id, who: .customer.name }"}'
```

```json
{"rows":[{"id":"o1","who":"Ada"}]}
```

[apps/owl-server](apps/owl-server) documents the HTTP interface, and
[drivers/go](drivers/go) is a client for it. To embed Owl in a Rust program,
see [Using OQL from Rust](#using-oql-from-rust).

## What makes Owl different

Most databases make you reconstruct relationships by hand. Owl treats a
reference between two objects as a connection you can walk, in either
direction, as part of the query model itself.

- **No joins.** `orders.customer.name` is a path, not a join condition.
- **The way back is free.** Declare `customer: Ref(customers)` on an order and
  every customer has `.orders`, with an index Owl keeps for you.
- **One set of steps.** A collection, a list inside a document and a
  relationship followed backwards all take the same `where`, `select`, `sort`
  and `count`.
- **One language for everything.** Reads, writes, rules, functions and
  transactions are all OQL.
- **Its own engine.** There is no other database underneath: no RocksDB and no
  FFI.

## The language (OQL)

The [website](https://owldb.tech/#language) walks through each of these with a
drawing. Every example below runs against the same small shop.

### A schema

```oql
schema customers {
  id: String,
  name: String,
  email: String unique
}

schema orders {
  id: String,
  customer: Ref(customers),
  status: String,
  total: Float,
  items: List({ product: Ref(products), qty: Integer })
}
```

### Following references, both ways

```oql
// Forward: a field on the far side of a reference
orders.where { .customer.country == "NG" }.select { id, who: .customer.name }

// Back: every order that points at this customer
customers.select {
  name,
  orders: .orders.count(),
  spent: .orders.where { .status == "paid" }.sum(.total)
}
```

A path that cannot be followed gives `null`. It is not an error.

### Grouping, recursion and positions

```oql
// One pile per status, measured
orders.group_by { .status }.select { status, n: count(), revenue: sum(.total) }

// A tree walked to any depth, then across to its products
categories.where { .id == "home" }.walk(.children).products.select { name, price }

// Each customer's largest order
orders
  .sort { .customer.id.asc, .total.desc }
  .partition_by { .customer }
  .select { id, who: .customer.name, total, place: row_number() }
  .where { .place == 1 }
```

### Two forms of the same query

Any query can be written as a chain or as a block. They mean the same thing. A
block can also name something once and use it again.

```oql
customers {
  paid = .orders.where { .status == "paid" }
  spent = paid.sum(.total)

  where { spent > 80 }
  sort { .id.asc }
  select { name, orders: paid.count(), spent: spent }
}
```

### Functions

Functions are written in OQL and kept in the database, so every program that
talks to it shares one definition.

```oql
fn spent(c: customers) {
  c.orders.where { .status == "paid" }.sum(.total)
}

fn tier(spent) {
  if spent >= 200 then "gold" else if spent >= 80 then "silver" else "bronze"
}

customers.select { name, tier: tier(spent(*)) }
```

A function can also return a collection, and then it chains like a built-in
step. With an `also_ordered` function of your own that returns a customer's
other orders, you can write `orders.where { .id == "o1" }.also_ordered()`.

### Writing, and the rules that guard it

```oql
customers.insert { id: "chen", name: "Chen", email: "chen@shop.test" }
products.where { .id == "kettle" && .stock > 0 }.update { stock -= 1 }
customers.upsert on id { id: "chen", name: "Chen W." }
orders.where { .status == "cancelled" }.delete()
```

A schema declares what must always be true: `unique`, `check(.price >= 0)`,
`Ref(customers)` and `on delete cascade`. Owl refuses any write that would
break one, and says why in terms of your schema:

```
'customers' document 'eve' has email "ada@shop.test", which must be unique,
and another document already has it
```

One statement is all or nothing. A `transaction` block makes several
statements one:

```oql
transaction {
  orders.insert { id: "o4", customer: "ada", product: "kettle", qty: 2, status: "paid" }
  products.where { .id == "kettle" }.update { stock -= 2 }
}
```

### Parameters

Values enter a query as `$name` parameters. A parameter is never read as query
text, whatever it holds.

```rust
use owl_db::{bind_parameters, parse_oql, Value};
use std::collections::HashMap;

let query = parse_oql(r#"events.where { .level == $level && .attempts > $min }"#)?;
let query = bind_parameters(&query, &HashMap::from([
    ("level".to_string(), Value::String("error".into())),
    ("min".to_string(), Value::Integer(3)),
]))?;
```

A query means the same thing however the engine runs it. Comparison, null
and ordering rules are defined once (`src/value_semantics.rs`) and shared by
the general executor and the physical read pipeline;
`tests/oql_language_conformance_test.rs` checks that both agree.

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

The storage engine is entirely Rust-owned. There is no RocksDB and no FFI.
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

## Around the core

| | |
|---|---|
| [owl-server](apps/owl-server) | Owl as a network service: JSON over HTTP, transactions, backups |
| [Go driver](drivers/go) | A client for owl-server, standard library only |
| [Loggr](apps/loggr) | An error investigation workbench built on Owl's Rust API. It exercises ingestion, batch commit, indexes, references and checkpoints end to end |
| [Storefront](apps/storefront) | A commerce workload: sixteen queries, each checked against SQLite for the same rows and then timed against it |
| [Gauntlet](apps/gauntlet) | Queries built to be hard (deep hierarchies, many-to-many, skew), checked and timed against SQLite the same way |
| [Website](website) | The site at [owldb.tech](https://owldb.tech) |

## Build and test

Owl requires only a Rust toolchain.

```bash
cargo check --all-targets
cargo test --lib storage_engine
cargo test --test oql_language_conformance_test --test oql_dual_representation_test
```

The focused storage invariant tests and the OQL conformance suites are the
release gate. The wider historical
test suite contains known failures in experimental features (distributed,
simulation, columnar) that are outside the trusted single-node path.

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
| `apps/` | owl-server, Loggr, and the Storefront and Gauntlet suites |
| `drivers/go/` | Go client for owl-server |
| `website/` | The site at owldb.tech |
| `oql/` | OQL example files and syntax fixtures |
| `benches/` | Index scan, ingest, navigation benchmarks |
| `docs/` | Design documents and architecture specs |

## Key design documents

- [Storage Engine Architecture](docs/STORAGE_ENGINE_ARCHITECTURE.md) — canonical storage design, invariants, disk layout
- [OQL Language Description](docs/OWL_LANGUAGE_DESCRIPTION.md) — language contract and design principles
- [OQL Specification](docs/OQL_SPECIFICATION.md) — the language in full
- [Scoped Block Semantics](docs/OWL_SCOPED_BLOCK_SEMANTICS.md) — what the block form means, rule by rule
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

## Help Owl grow

Owl is early, and it is open source under the [MIT license](LICENSE). There
are three ways to help, from ten seconds to a weekend.

**Star it.** A star costs nothing, and it is how other people come across a
project this new.

**Try it and say what broke.** Run it against data shaped like yours. A query
Owl gets wrong, or cannot express, is the most useful report there is.
[Open an issue](https://github.com/TimiBolu/owl/issues/new) with the schema, the
query, and what you expected.

**Help build it.** The current priority is storage correctness and a single
connected query path. Useful contributions include:

- deterministic crash tests
- format fuzzing
- record codec work
- typed key encoding
- differential index tests
- reproducible workload benchmarks
- a query that Storefront or Gauntlet should cover and does not

Before you start on something large, open an issue to say what you plan, so
the work is not duplicated. Before you open a pull request, run the release
gate:

```bash
cargo check --all-targets
cargo test --lib storage_engine
cargo test --test oql_language_conformance_test --test oql_dual_representation_test
```

A change to what a query means should come with a test in
`tests/oql_language_conformance_test.rs`. A change to the HTTP interface
should come with one in `apps/owl-server/tests`.

## License

Owl is released under the [MIT license](LICENSE). You may use, change and
share it, including in commercial work, as long as the license text stays with
it.
