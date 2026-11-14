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
The machine was not idle, and figures move by tens of percent between runs
(far more under memory pressure), so treat a ratio near 1 as a tie.

| Query | Owl | SQLite | Owl ÷ SQLite |
|---|---:|---:|---:|
| q01 one order with customer and items | 0.07 | 0.04 | 1.9× |
| q02 twenty most recent paid orders | 0.25 | 0.04 | 5.6× |
| q03 large orders from one country | 111.71 | 17.68 | 6.3× |
| q04 customers by lifetime spend | 741.96 | 304.77 | 2.4× |
| q05 three latest orders per gold customer | 15.73 | 2.29 | 6.9× |
| q06 orders and revenue by status | 103.65 | 133.19 | 0.8× |
| q07 paid revenue by month | 345.11 | 52.43 | 6.6× |
| q08 paid revenue by customer country | 366.57 | 142.63 | 2.6× |
| q09 best-selling products from line items | 1171.16 | 1131.27 | 1.0× |
| q10 paid customers with no ticket | 219.25 | 18.77 | 11.7× |
| q11 orders above their customer's average | 1872.19 | 1502.57 | 1.2× |
| q12 category tree with statistics | 6.61 | 0.76 | 8.7× |
| q13 customers per country | 41.14 | 3.46 | 11.9× |
| q14 countries buying in one month | 43.69 | 10.24 | 4.3× |
| q15 payments by method in a quarter | 63.93 | 16.42 | 3.9× |
| q16 best-reviewed products | 190.35 | 50.00 | 3.8× |

Loading took 94 s for Owl (2,636 rows/s, 120 MiB on disk) and 9 s for
SQLite (26,123 rows/s, 54 MiB). Both are dominated by the cost of a durable
sync per batch on this machine, which varied between runs.

Owl is slower on thirteen of sixteen queries and level on three. All sixteen
return the same rows as SQLite.

## Why Owl is slower

The first benchmark pointed at three causes. Two are fixed; the first is
reduced.

1. **Rows are allocated one at a time.** Most queries run on the general
   executor, which represents each row as a map of field names to values.
   Two things now avoid that. Filters followed by `count()`, a single-field
   aggregate, or a `group_by` on plain fields with simple aggregates read
   storage's column batches and build no rows at all (q06, q15). And rows
   reached through a relationship (`.orders` on a customer) are fetched under
   one lock, shared rather than copied, and carry only the fields the query
   reads (q04, q05, q10, q12). Everything else still pays for rows: a grouping
   key that is computed (q07), an aggregate over a filtered group (q13), and
   any scan whose filter storage cannot evaluate.
2. **An index order with a tie-breaker was not used.** Fixed: q02 reads the
   first page in index order and re-sorts only the rows tied at the page
   boundary.
3. **A filter on a timestamp did not use its index.** Fixed: timestamp text
   compared with a `DateTime` field is typed when the query is planned, so
   q14 and q15 use the indexes on `placed` and `paid_at`.

A fourth cause turned up afterwards, about planning rather than per-row
cost, and is fixed for one shape:

4. **A filter through a reference scanned the wrong side.** q03 filters
   orders by `.customer.country`. Owl scanned orders and followed each
   reference. Now, when a leading filter tests a field of a referenced
   document for equality and a minority of those documents pass, Owl starts
   from them and follows the relationship back, then returns the rows in the
   order a scan would (by id).

5. **A question about related rows was answered one row at a time.** q10
   asks, per customer, whether a paid order exists and whether a ticket
   exists. When many rows of one statement ask the same
   `.relationship.where { .. }.count()` or `.exists()`, Owl now reads the
   referring collection once, counts per referenced document, and answers
   the rest from that tally. The first 64 rows still use the index, so a
   query about a few rows does not pay for a scan.

q10 is still the furthest behind. Its remaining cost is the single pass over
orders: SQLite answers from an index and never reads an order row. The q10
figure in the table comes from a later run than the other rows.

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

The fourth of these was a quadratic cost in the storage engine: every batch of
index-ordered lookups read nearly the whole segment.
