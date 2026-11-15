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
| q01 one order with customer and items | 0.04 | 0.02 | 1.7× |
| q02 twenty most recent paid orders | 0.15 | 0.03 | 5.8× |
| q03 large orders from one country | 53.36 | 13.12 | 4.1× |
| q04 customers by lifetime spend | 152.55 | 135.83 | 1.1× |
| q05 three latest orders per gold customer | 6.41 | 1.03 | 6.3× |
| q06 orders and revenue by status | 52.47 | 78.34 | 0.7× |
| q07 paid revenue by month | 95.48 | 40.02 | 2.4× |
| q08 paid revenue by customer country | 77.05 | 63.68 | 1.2× |
| q09 best-selling products from line items | 694.18 | 663.30 | 1.0× |
| q10 paid customers with no ticket | 28.14 | 5.66 | 5.0× |
| q11 orders above their customer's average | 963.10 | 895.59 | 1.1× |
| q12 category tree with statistics | 2.70 | 0.48 | 5.7× |
| q13 customers per country | 4.70 | 2.10 | 2.2× |
| q14 countries buying in one month | 19.86 | 6.32 | 3.1× |
| q15 payments by method in a quarter | 32.96 | 10.11 | 3.3× |
| q16 best-reviewed products | 31.34 | 29.69 | 1.1× |

Loading took 15 s for Owl (16,828 rows/s, 123 MiB on disk) and 15 s for
SQLite (16,538 rows/s, 56 MiB) in this run; SQLite is usually nearer 10 s.

Owl is level with or ahead of SQLite on six queries, within about three
times on five, and four to six times behind on five. All sixteen return the
same rows as SQLite. This run was on a quieter machine than the earlier
ones, so both engines are faster than in the table of fixes below.

## How far it scales on this machine

```bash
cargo run --release -p storefront -- memory --orders 100000
```

reports Owl's memory alone (its physical footprint) at each stage. A row
with a list, an object or a vector in it lives outside the column segments,
and by default stays decoded in memory for as long as the database is open.
A budget on decoded documents (`RustStore::with_decoded_budget`,
`owl-server --decoded-documents`, or the `OWL_DECODED_DOCUMENTS` environment
variable) keeps the rest as their encoded records, decoded when read; a scan
then decodes only the fields it filters on or returns. Every answer is the
same: the test suites pass with a budget of zero, and so does the check
against SQLite.

Measured on 2026-11-15 at 100,000 orders (247,048 rows, 137 MiB on disk):

| Budget | Loaded | Checkpointed | Compacted | Reopened |
|---|---:|---:|---:|---:|
| None | 941 MiB | 600 MiB | 502 MiB | 404 MiB |
| 20,000 documents | 624 MiB | 322 MiB | 255 MiB | 360 MiB |
| 0 documents | 654 MiB | 283 MiB | 219 MiB | 281 MiB |

With a budget the settled database takes about half the memory, around 1 KB
a row instead of 2. The price is speed on large scans of the affected
collections: measured before the latest query work, a 20,000-document
budget took q06 from 79 ms to 214 ms and q04 from 206 ms to 463 ms, while
queries that touch a few rows were unaffected.

What is left with nothing decoded, in a reopened database:

- about 85 MiB is the encoded records of the rows outside the segments,
  which are also in the checkpoint file and could be read from there;
- about 35 MiB is one allocation of 112 bytes per row, not yet identified;
- the rest is id directories, indexes and allocator overhead.

The peak while loading barely moves, because everything written since the
last checkpoint is held until the next one. A million orders would settle
near 2.5 GB with a budget and peak far higher during the load, so it is
still not a comfortable fit for this 8 GB machine.

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

- **q10** now counts from `index(.status, .customer)` without reading an
  order, as SQLite does. What remains is evaluating two conditions for each
  of 10,000 customers through the general evaluator.
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
| A relationship count whose filters are equalities is answered from an index on those fields and the reference, reading no row; the index was added to both engines | q10 at 100,000 orders: 93 ms to 28 ms (SQLite: 18 ms to 6 ms) |
| A unique index is consulted for a new key, instead of reading every document in the database | Loading 100,000 orders: 42 s to 18 s |
| A reference is checked by looking its target up rather than copying it, a `check` constraint is parsed once, and loading hands Owl values through a prepared insert | Loading 100,000 orders: 18 s to 16 s |

Loading 100,000 orders took 98 s in the first benchmark.

The fourth of these was a quadratic cost in the storage engine: every batch of
index-ordered lookups read nearly the whole segment.
