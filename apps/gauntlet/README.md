# Gauntlet

Hard queries over a data model built to be hard, each checked against SQLite
for the same answer and timed against it. Where the storefront shows what Owl
is like on an ordinary application, this is where Owl is supposed to struggle.

## The data

A workspace of tenants, units, people, projects, tasks and activity, generated
from one number (`--people`, default 2000, which gives about 75,000 rows).

| What makes it hard | Where |
|---|---|
| Hierarchies of unknown depth | units under units, people under managers, tasks under tasks |
| Many-to-many relationships | `assignments` (people and projects), `links` (tasks blocking tasks) |
| Skew | the first tenant holds about a third of everything; a few people and projects hold most assignments and tasks |
| Optional references | a quarter of tasks have no assignee; most have no parent |
| Lists of text | skills, tags, labels |
| A wide embedded list | each task's work log: usually under five entries, fifty to two hundred for one task in fifty |
| Bursty time series | each person's events come in sessions separated by hours or days |

The schema is [schema.oql](schema.oql); SQLite gets the same model in
[schema.sql](schema.sql), with lists as child tables and an index on every
reference.

## The queries

Each query in [queries](queries) is a pair. The `.sql` file defines the answer
and the `.oql` file has to produce the same rows.

- **`h` queries** use what OQL had before this suite existed: deep traversal,
  nested results, correlated conditions, anti-joins, grouping through
  references, aggregates over embedded lists.
- **`g` queries** need what the suite showed was missing: distinct counts and
  percentiles, set operations, recursion (`walk`), position functions
  (`partition_by`, `prev`, `so_far()`, `rank`), routes and paths, and reaching the parent row
  from a list element.

An `.oql` file that holds only comments marks a query OQL cannot express; the
run reports it as `NO OQL`. There are none at present.

## Running it

```bash
cargo build --release -p gauntlet
```

```bash
./target/release/gauntlet check
```

```bash
./target/release/gauntlet bench --samples 5
```

```bash
./target/release/gauntlet show h04
```

`check` fails if any query differs. `bench` adds the median time on each
engine. `show` prints both answers for a named query, or runs an OQL statement
given in full on Owl alone, which is the quick way to try a new one. All three
take `--people` to change the size and `check` and `bench` take `--only` to
pick queries by name.

## Results

2,000 people, 74,612 rows, on a laptop with other work running. Times are the
median of three runs in milliseconds; the ratio is Owl over SQLite, so below 1
is faster. All 38 queries return the same rows.

| Query | Owl | SQLite | Ratio |
|---|---:|---:|---:|
| `g01_distinct_per_group` | 17.9 | 4.3 | 4.2x |
| `g02_salary_spread` | 2.8 | 3.5 | 0.8x |
| `g03_unit_medians` | 3.6 | 4.4 | 0.8x |
| `g04_owners_or_managers` | 4.3 | 0.5 | 8.3x |
| `g05_owner_and_manager` | 6.2 | 0.4 | 13.7x |
| `g06_subtree_headcount` | 1.8 | 0.4 | 5.0x |
| `g07_management_chain` | 0.1 | 0.3 | 0.2x |
| `g08_transitive_blockers` | 0.1 | 2.5 | 0.1x |
| `g09_two_levels_of_reports` | 0.5 | 0.1 | 4.5x |
| `g10_running_total` | 28.9 | 8.9 | 3.3x |
| `g11_salary_rank` | 12.4 | 5.0 | 2.5x |
| `g12_sessions` | 320.3 | 185.7 | 1.7x |
| `g13_longest_gap` | 280.9 | 128.6 | 2.2x |
| `g14_unwind_with_parent` | 490.9 | 133.6 | 3.7x |
| `h01_org_rollup` | 23.6 | 12.4 | 1.9x |
| `h02_unit_tree` | 0.5 | 0.4 | 1.1x |
| `h03_deep_forward_filter` | 16.6 | 5.8 | 2.9x |
| `h04_colleagues` | 2.4 | 0.5 | 4.7x |
| `h05_cross_tenant` | 4.4 | 5.6 | 0.8x |
| `h06_overallocated` | 6.4 | 2.3 | 2.8x |
| `h07_no_local_assignee` | 3.4 | 1.3 | 2.5x |
| `h08_above_unit_average` | 2.9 | 12.8 | 0.2x |
| `h09_outearned_managers` | 1.0 | 0.1 | 6.8x |
| `h10_states_by_org` | 21.5 | 17.9 | 1.2x |
| `h11_monthly_flow` | 121.9 | 30.8 | 4.0x |
| `h12_top_earners_per_unit` | 3.5 | 1.7 | 2.1x |
| `h13_late_limit` | 2.6 | 0.8 | 3.5x |
| `h14_skewed_tenant` | 16.0 | 3.1 | 5.1x |
| `h15_heavy_worklogs` | 41.8 | 9.2 | 4.5x |
| `h16_worklog_by_author` | 246.2 | 85.3 | 2.9x |
| `h17_label_histogram` | 58.1 | 17.8 | 3.3x |
| `h18_distinct_assignees` | 26.3 | 6.3 | 4.2x |
| `h19_deep_offset` | 30.5 | 10.8 | 2.8x |
| `h20_blocked_two_hops` | 56.0 | 13.6 | 4.1x |
| `h21_rollup_fixed_depth` | 0.6 | 0.2 | 3.7x |
| `h22_null_logic` | 13.5 | 1.8 | 7.4x |
| `h23_or_across_relationships` | 3.0 | 1.6 | 1.9x |
| `h24_everything_at_once` | 38.0 | 13.5 | 2.8x |

## What the results say

Owl gets every answer right, including the ones SQL needs four nested
subqueries or a recursive CTE for. Six queries are faster than SQLite,
twenty-six are within five times, and six are further behind. Ratios move by
up to a point between runs on a busy machine.

The first run of this suite had sixteen queries more than five times slower,
the worst by 900 times. What closed most of that:

- Rows are filtered, counted and stepped through where the store holds them,
  and only the rows a query keeps are copied. Following a path used to copy
  the whole starting row, work log and all, before reading one field of it.
- A filter on related rows, or on the elements of an embedded list, is
  evaluated on them in place, whatever the filter is.
- A question about a referenced row (`.project.assignments.where {..}.exists()`)
  is answered once per referenced row in a statement, not once per row that
  points at it.
- "Which of this project's assignments are this person's" is answered from
  the person's side, where there are few rows, for every project at once.
- A filtered count or existence test asked by most rows of a collection is
  answered from one pass over the related collection.
- A costly filter ahead of a sort and a limit runs only until the limit is met.
- A limit written after a costly `select`, or after position functions that
  only look back, is applied before it.
- A sort cut by an offset and a limit copies only the rows it returns.
- A nested operation that does not read the enclosing row is prepared once,
  not once per row, and a plain field is compared with a constant without
  being copied out first.

What is still slow, and why:

- **`g05`, `g04` (14 and 8 times).** 4 to 6 ms against half a millisecond.
  Each side of the set operation evaluates its filter for every person and
  copies the rows that pass. Starting from the relationship's index instead
  was tried and cost more than it saved at this size.
- **`h22_null_logic` (7 times).** Four cheap conditions over 20,000 rows, one
  through a reference: the cost is the interpreter, about 0.7 microseconds a
  row.
- **`h09`, `h14`, `g06` (5 to 7 times).** The same per-row overhead, on
  queries that take 1 to 16 ms.

What remains is mostly the cost of evaluating one row at a time through an
interpreter. Closing it means compiling conditions or evaluating rows in
bulk, and a planner that can choose which side of a query to start from.

These gains depend on the rows being held decoded in memory. Under a memory
budget that evicts rows, queries take the earlier routes.
