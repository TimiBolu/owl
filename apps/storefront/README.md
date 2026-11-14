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

Run on 2026-11-14 on an 8-core Apple Silicon machine with 8 GB of memory, at
100,000 orders (247,048 rows in total). Times are medians of five runs, in
milliseconds. The machine was not idle, and figures move by tens of percent
between runs, so treat a ratio near 1 as a tie.

| Query | Owl | SQLite | Owl ÷ SQLite |
|---|---:|---:|---:|
| q01 one order with customer and items | 0.07 | 0.04 | 1.6× |
| q02 twenty most recent paid orders | 0.21 | 0.04 | 6.0× |
| q03 large orders from one country | 103.41 | 18.66 | 5.5× |
| q04 customers by lifetime spend | 297.71 | 329.15 | 0.9× |
| q05 three latest orders per gold customer | 9.74 | 1.25 | 7.8× |
| q06 orders and revenue by status | 58.30 | 126.77 | 0.5× |
| q07 paid revenue by month | 130.47 | 47.28 | 2.8× |
| q08 paid revenue by customer country | 100.33 | 137.25 | 0.7× |
| q09 best-selling products from line items | 970.60 | 952.70 | 1.0× |
| q10 paid customers with no ticket | 142.46 | 13.68 | 10.4× |
| q11 orders above their customer's average | 1336.98 | 1788.91 | 0.7× |
| q12 category tree with statistics | 9.42 | 0.85 | 11.0× |
| q13 customers per country | 8.64 | 3.97 | 2.2× |
| q14 countries buying in one month | 39.27 | 12.90 | 3.0× |
| q15 payments by method in a quarter | 45.14 | 20.17 | 2.2× |
| q16 best-reviewed products | 58.04 | 55.48 | 1.0× |

Loading took 16 s for Owl (15,294 rows/s, 120 MiB on disk) and 11 s for
SQLite (23,269 rows/s, 54 MiB).

Owl is level with or ahead of SQLite on six queries, within about three
times on five, and five to eleven times behind on five. All sixteen return
the same rows as SQLite. The q02 row is from a rerun: in the main run one
slow sample put its median at 1.7 ms.

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
- **q12** and **q05** are a handful of rows with nested results. Their cost
  is fixed overhead per nested query, not data volume.
- **q03** still sorts every large order from the country before taking 25.
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
| A unique index is consulted for a new key, instead of reading every document in the database | Loading 100,000 orders: 42 s to 18 s |
| A reference is checked by looking its target up rather than copying it, a `check` constraint is parsed once, and loading hands Owl values through a prepared insert | Loading 100,000 orders: 18 s to 16 s |

Loading 100,000 orders took 98 s in the first benchmark.

The fourth of these was a quadratic cost in the storage engine: every batch of
index-ordered lookups read nearly the whole segment.
