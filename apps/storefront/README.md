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

What it does not hold equal: Owl loads through OQL text, which it parses for
every batch, while SQLite uses prepared statements. Owl has no prepared
insert yet.

## Results

Run on 2026-11-14 on an 8-core Apple Silicon machine with 8 GB of memory, at
100,000 orders (247,048 rows in total). Times are medians in milliseconds.

| Query | Owl | SQLite | Owl is slower by |
|---|---:|---:|---:|
| q01 one order with customer and items | 0.06 | 0.04 | 1.5× |
| q02 twenty most recent paid orders | 482.76 | 0.05 | 10,495× |
| q03 large orders from one country | 518.43 | 16.84 | 30.8× |
| q04 customers by lifetime spend | 1184.34 | 198.98 | 6.0× |
| q05 three latest orders per gold customer | 30.50 | 1.69 | 18.1× |
| q06 orders and revenue by status | 302.77 | 125.65 | 2.4× |
| q07 paid revenue by month | 405.99 | 55.19 | 7.4× |
| q08 paid revenue by customer country | 520.52 | 137.63 | 3.8× |
| q09 best-selling products from line items | 1426.55 | 1064.98 | 1.3× |
| q10 paid customers with no ticket | 755.52 | 18.15 | 41.6× |
| q11 orders above their customer's average | 1576.65 | 1431.69 | 1.1× |
| q12 category tree with statistics | 14.06 | 0.77 | 18.2× |
| q13 customers per country | 41.05 | 3.29 | 12.5× |
| q14 countries buying in one month | 243.21 | 9.75 | 24.9× |
| q15 payments by method in a quarter | 167.01 | 16.25 | 10.3× |
| q16 best-reviewed products | 245.16 | 46.84 | 5.2× |

Loading took 58.8 s for Owl (4,199 rows/s, 120 MiB on disk) and 10.0 s for
SQLite (24,683 rows/s, 54 MiB). Both are dominated by the cost of a durable
sync per batch on this machine, which varied between runs.

Owl is slower on every query. All sixteen return the same rows as SQLite.

## Why Owl is slower

Profiling points at three causes, in order of how much they explain.

1. **Rows are allocated one at a time.** Fifteen of the sixteen queries run on
   the general executor, which represents each row as a map of field names to
   values and builds a new one at every stage. A profile of a plain filtered
   scan shows most of the time inside the memory allocator. SQLite reads
   fields in place from pages.
2. **An index order with a tie-breaker is not used.** q02 sorts by
   `placed` then `id` and takes twenty rows. The index on `(status, placed)`
   could supply them in order, but the extra sort key makes the planner fetch
   every matching order and sort. Without the tie-breaker the same query
   takes 0.6 ms at 10,000 orders, against 52 ms with it.
3. **A filter on a timestamp does not use its index.** q14 and q15 filter
   on `placed` and `paid_at`, which are indexed, but a timestamp compared with
   timestamp text is kept off the planner and scanned instead.

## What this work already fixed

Measured at 10,000 orders unless noted.

| Change | Effect |
|---|---|
| A filter on `id` is a lookup, not a scan | q01: 58 ms to 0.05 ms |
| A group's rows are shared by its aggregates instead of copied per aggregate | q06: 240 ms to 31 ms |
| A scan decodes only the fields the query reads | q13: 20 ms to 3.5 ms |
| A batch of scattered records is read in nearby runs, not one span from first to last | q02 at 100,000 orders: 3.9 s to 0.48 s; q04: 10.3 s to 1.2 s |

The last of these was a quadratic cost in the storage engine: every batch of
index-ordered lookups read nearly the whole segment.
