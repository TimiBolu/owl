# Storefront

A commerce workload for Owl, checked and timed against SQLite.

It exists to answer two questions with evidence: does Owl return the right
answers to realistic queries, and how fast is it compared with a mature
embedded engine given the same data, the same indexes and the same
durability?

## What is in it

| File | Purpose |
|---|---|
| `schema.oql` | The data model in OQL: seven collections, typed line items inside orders, constraints and indexes declared with the schema |
| `schema.sql` | The same model for SQLite. Every reference Owl indexes automatically has an explicit index here |
| `queries/*.oql`, `queries/*.sql` | Sixteen queries, each written in both languages |
| `src/data.rs` | A deterministic generator: the same order count always yields the same data |
| `src/engines.rs` | Loading and querying for each engine, and the result comparison |

## Running it

```bash
cargo run --release -p storefront -- check --orders 2000
```

loads both engines with identical data and requires all sixteen queries to
return the same rows. Numbers match within a relative tolerance of one part
in a million, because the engines add floats in different orders.

```bash
cargo run --release -p storefront -- bench --orders 10000,100000 --samples 3
```

times loading and each query. Each query runs once untimed, then `--samples`
times; the median is reported. `--output report.json` saves everything.

```bash
cargo run --release -p storefront -- time --orders 10000 'orders.count()'
```

times ad-hoc OQL against a loaded dataset.

## What the comparison holds equal

- **Data.** One generator feeds both engines.
- **Durability.** Owl syncs each batch before returning. SQLite runs in WAL
  mode with `synchronous=FULL` and one transaction per batch of the same 500
  rows.
- **Indexes.** The same secondary indexes on both sides, and an index for
  every relationship.
- **Statistics.** SQLite runs `ANALYZE` after loading.
- **Result handling.** Both sides convert every row to JSON inside the timed
  region.
- **Loading.** Both sides are handed values, not text: SQLite through
  prepared statements, Owl through `QueryExecutor::insert_documents`, which
  applies the same typing, defaults, checks and constraints as
  `collection.insert [ .. ]` and commits each batch with one durable write.

## Results

Run on 2026-11-15 on an 8-core Apple Silicon machine with 8 GB of memory, at
100,000 orders (247,048 rows in total). Times are medians of five runs, in
milliseconds. The machine was not idle, and figures move by tens of percent
between runs, so treat a ratio near 1 as a tie.

| Query | Owl | SQLite | Owl ÷ SQLite |
|---|---:|---:|---:|
| q01 one order with customer and items | 0.06 | 0.03 | 1.6× |
| q02 twenty most recent paid orders | 0.23 | 0.04 | 5.6× |
| q03 large orders from one country | 75.66 | 16.44 | 4.6× |
| q04 customers by lifetime spend | 199.92 | 208.90 | 1.0× |
| q05 three latest orders per gold customer | 7.03 | 1.65 | 4.3× |
| q06 orders and revenue by status | 75.95 | 125.81 | 0.6× |
| q07 paid revenue by month | 126.62 | 61.87 | 2.0× |
| q08 paid revenue by customer country | 116.66 | 135.83 | 0.9× |
| q09 best-selling products from line items | 1029.41 | 1079.66 | 1.0× |
| q10 paid customers with no ticket | 92.77 | 17.66 | 5.3× |
| q11 orders above their customer's average | 1468.26 | 1409.25 | 1.0× |
| q12 category tree with statistics | 4.32 | 0.76 | 5.7× |
| q13 customers per country | 5.40 | 3.34 | 1.6× |
| q14 countries buying in one month | 28.46 | 9.80 | 2.9× |
| q15 payments by method in a quarter | 36.81 | 16.08 | 2.3× |
| q16 best-reviewed products | 39.12 | 46.95 | 0.8× |

Loading took 18 s for Owl (13,791 rows/s, 120 MiB on disk) and 9 s for
SQLite (26,264 rows/s, 54 MiB).

Owl is level with or ahead of SQLite on six queries, within about three
times on five, and four to six times behind on five. All sixteen return the
same rows as SQLite.

## How far it scales on this machine

```bash
cargo run --release -p storefront -- memory --orders 100000
```

reports Owl's memory alone at each stage. Measured on 2026-11-15 at 100,000
orders (247,048 rows, 134 MiB on disk):

| Stage | Memory | Rows held decoded |
|---|---:|---:|
| Loaded | 737 MiB | 247,048 |
| Checkpointed | 683 MiB | 247,048 |
| Compacted | 546 MiB | 112,000 |
| Closed and reopened | 426 MiB | 112,000 |

Owl holds about 2.8 KB of memory per row while loading, five times what the
row takes on disk, and the whole benchmark process (with SQLite and the
generated data) peaks at 1.2 GB for 100,000 orders and 2.1 GB for 200,000.
A million orders would need about 10 GB, which this 8 GB machine cannot
hold; at 200,000 both engines were already slowed by memory pressure.

The ceiling is memory, not time, and the table shows why. A row lives in
memory as a decoded document until a compaction moves it into a column
segment. Compaction can only do that for collections whose fields are all
plain values: the 112,000 rows it leaves decoded are the orders and the
products, which hold a list of line items and a vector.

### A budget on decoded documents

Those rows no longer have to stay decoded. Opened with a budget
(`RustStore::with_decoded_budget`, `owl-server --decoded-documents`, or the
`OWL_DECODED_DOCUMENTS` environment variable), Owl holds at most that many
documents decoded. The rest are kept as their encoded records and decoded
when read, and a scan decodes only the fields it filters on or returns.
Every answer is the same: the whole test suite passes with a budget of zero,
and so does the check against SQLite.

Measured at 100,000 orders:

| Budget | After compaction | After reopening | q06 | q04 | q10 |
|---|---:|---:|---:|---:|---:|
| None | 608 MiB | 538 MiB | 79 ms | 206 ms | 106 ms |
| 20,000 documents | 475 MiB | 299 MiB | 214 ms | 463 ms | 242 ms |
| 0 documents | 348 MiB | 350 MiB | 238 ms | 763 ms | 496 ms |

A budget of 20,000 roughly halves the memory of an opened database and
makes scans of the affected collections two to three times slower. Queries
that touch a few rows are unaffected.

It does not remove the ceiling. With nothing held decoded Owl still uses
about 350 MiB for these 247,048 rows, around 1.4 KB a row, and loading peaks
far higher. What is left is everything else held in memory for every row:
the records themselves, the indexes, and three directories keyed by id. A
million orders is still out of reach of this machine until those can live
on disk too.

## Where the time goes

Most queries run on the general executor, which represents a row as a map of
field names to values and builds one for every row it reads. That is what
the first benchmark measured, and what the work since has been removing.

1. **Aggregating without building rows.** A query that filters and then
   counts, aggregates, or groups is folded straight from storage: each row is
   visited once and none is kept. Storage evaluates the filters it can. The
   rest (a filter through a reference, a computed group key such as
   `format_date(.placed, "%Y-%m")`, a key reached through a reference such as
   `.customer.country`, an aggregate of an expression, an aggregate over part
   of a group, a `having`) is evaluated by the same evaluator the general
   path uses, against the stored document. A value reached through a
   reference is looked up once per distinct reference. This is q06, q07,
   q08, q13, q15 and q16.
2. **Shaping rows as they are read.** `where { .. }.select { .. }` builds
   each result row from the stored document, with no intermediate row.
3. **One pass for a question asked of every row.** When many rows of one
   statement ask the same thing of their related rows (`.orders.count()`,
   `.orders.where { .. }.sum(.total)`, `.tickets.exists()`), Owl reads the
   referring collection once and answers each row from the tally. The first
   64 rows still use the index, so a query about a few rows does not pay for
   a scan. This is q04 and q10.
4. **Starting from the smaller side.** A leading filter on a field of a
   referenced document (`.customer.country == "NG"`) starts from the
   documents that pass and follows the relationship back. This is q03.
5. **Using the order an index gives.** A sort with a tie-breaker takes its
   first page in index order and re-sorts only the rows tied at the page
   boundary (q02), and timestamp text compared with a `DateTime` field is
   typed when the query is planned, so the index applies (q14, q15).

Each of these returns `None` when a query does not fit and leaves it to the
general executor, which defines what every query means. The conformance
suite runs each shape both ways and requires identical rows.

What is still behind, and why:

- **q10** reads every order once to learn which customers have a paid one.
  SQLite answers from an index and reads no order.
- **q03** fetches every order of the country's customers through the
  relationship before it can tell which are large; SQLite joins on an index
  and filters as it goes.
- **q12** and **q05** are a handful of rows with nested results. Their cost
  is the fixed work of each nested query, not data volume.
- **q02** is a 0.2 ms query; what remains is planning it.

## What this work already fixed

Measured at 10,000 orders unless noted.

| Change | Effect |
|---|---|
| A filter on `id` is a lookup, not a scan | q01: 58 ms to 0.05 ms |
| A group's rows are shared by its aggregates instead of copied per aggregate | q06: 240 ms to 31 ms |
| A scan decodes only the fields the query reads | q13: 20 ms to 3.5 ms |
| A batch of scattered records is read in nearby runs, not one span from first to last | q02 at 100,000 orders: 3.9 s to 0.48 s; q04: 10.3 s to 1.2 s |
| A sort with a tie-breaker takes its first page from the index | q02: 52 ms to 0.6 ms; at 100,000 orders 483 ms to 1.2 ms |
| A timestamp filter uses the field's index | q14: 27 ms to 4 ms; q15: 24 ms to 0.8 ms |
| Simple aggregates and groupings read column batches without building rows | q06: 31 ms to 5 ms |
| A referenced document is fetched once per statement | q08: 44 ms to 31 ms |
| Rows reached through a relationship are shared and trimmed to the fields read | q04: 105 ms to 62 ms; q10: 62 ms to 40 ms |
| A filter on a referenced document's field starts from that document | q03 at 100,000 orders: 403 ms to 112 ms |
| Per-row counts and existence tests over a relationship are answered from one pass | q10 at 100,000 orders: 566 ms to 219 ms |
| Grouping and aggregating fold storage directly for computed keys, keys through a reference, filtered aggregates and `having` | At 100,000 orders: q07 398 ms to 130 ms; q08 419 ms to 100 ms; q13 82 ms to 9 ms; q16 328 ms to 58 ms |
| A scan reads rows in the id order they are already kept in, instead of copying and sorting every id | q06 at 100,000 orders: 104 ms to 58 ms |
| A per-row aggregate over a relationship is answered from one pass | q04 at 100,000 orders: 706 ms to 298 ms |
| Reading a field no longer times itself: a built-in profiler took two clock readings and a process-wide lock on every field read, and is now off unless asked for | At 30,000 orders, with the two changes below: q03 77 ms to 20 ms |
| A lookup by id searches the ordered directory by borrowed text instead of building a key from two new strings | (with the above) |
| Rows reached through a relationship are filtered before they are copied, and name themselves without copying | q12 at 100,000 orders: 9.4 ms to 4.3 ms; q05: 9.7 ms to 7.0 ms |
| A relationship tally accumulates per referenced document directly, building no group rows | q10 at 100,000 orders: 142 ms to 93 ms; q04: 298 ms to 200 ms |
| A unique index is consulted for a new key, instead of reading every document in the database | Loading 100,000 orders: 42 s to 18 s |
| A reference is checked by looking its target up rather than copying it, a `check` constraint is parsed once, and loading hands Owl values through a prepared insert | Loading 100,000 orders: 18 s to 16 s |

Loading 100,000 orders took 98 s in the first benchmark.

The fourth of these was a quadratic cost in the storage engine: every batch of
index-ordered lookups read nearly the whole segment.
