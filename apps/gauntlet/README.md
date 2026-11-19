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

2,000 people, 74,612 rows, on a quiet laptop. Times are the median of 21 runs
in milliseconds; the ratio is Owl over SQLite, so below 1 is faster. A second
run agreed with this one to within a few percent. All 44 queries return the
same rows: 12 are faster on Owl, 30 are within five times, and 2 are beyond.

| Query | Owl | SQLite | Ratio |
|---|---:|---:|---:|
| `g01_distinct_per_group` | 8.0 | 2.7 | 3.0x |
| `g02_salary_spread` | 2.0 | 2.2 | 0.9x |
| `g03_unit_medians` | 2.4 | 2.8 | 0.9x |
| `g04_owners_or_managers` | 0.28 | 0.32 | 0.9x |
| `g05_owner_and_manager` | 1.2 | 0.28 | 4.3x |
| `g06_subtree_headcount` | 1.1 | 0.23 | 5.0x |
| `g07_management_chain` | 0.05 | 0.22 | 0.2x |
| `g08_transitive_blockers` | 0.07 | 1.5 | 0.0x |
| `g09_two_levels_of_reports` | 0.28 | 0.06 | 4.6x |
| `g10_running_total` | 1.5 | 5.7 | 0.3x |
| `g11_salary_rank` | 6.9 | 3.2 | 2.2x |
| `g12_sessions` | 188.3 | 105.8 | 1.8x |
| `g13_longest_gap` | 157.7 | 82.1 | 1.9x |
| `g14_unwind_with_parent` | 229.6 | 78.4 | 2.9x |
| `g15_share_of_unit` | 10.6 | 2.8 | 3.7x |
| `g16_pay_spread` | 1.2 | 0.34 | 3.5x |
| `g17_pairs_at_one_level` | 1.0 | 0.26 | 4.0x |
| `g18_moving_window` | 1.9 | 0.94 | 2.0x |
| `g19_budget_down_the_tree` | 0.42 | 0.08 | 5.4x |
| `g20_routes_to_blockers` | 1.7 | 0.24 | 7.1x |
| `h01_org_rollup` | 8.0 | 7.9 | 1.0x |
| `h02_unit_tree` | 0.26 | 0.27 | 1.0x |
| `h03_deep_forward_filter` | 11.2 | 3.7 | 3.0x |
| `h04_colleagues` | 1.0 | 0.31 | 3.3x |
| `h05_cross_tenant` | 1.9 | 3.5 | 0.5x |
| `h06_overallocated` | 3.2 | 1.4 | 2.2x |
| `h07_no_local_assignee` | 1.7 | 0.83 | 2.1x |
| `h08_above_unit_average` | 1.4 | 8.0 | 0.2x |
| `h09_outearned_managers` | 0.26 | 0.09 | 3.0x |
| `h10_states_by_org` | 9.8 | 11.4 | 0.9x |
| `h11_monthly_flow` | 44.6 | 19.7 | 2.3x |
| `h12_top_earners_per_unit` | 1.8 | 1.1 | 1.7x |
| `h13_late_limit` | 1.2 | 0.47 | 2.6x |
| `h14_skewed_tenant` | 2.4 | 2.0 | 1.2x |
| `h15_heavy_worklogs` | 15.6 | 5.8 | 2.7x |
| `h16_worklog_by_author` | 116.0 | 55.4 | 2.1x |
| `h17_label_histogram` | 23.9 | 9.6 | 2.5x |
| `h18_distinct_assignees` | 9.9 | 3.5 | 2.8x |
| `h19_deep_offset` | 3.0 | 6.8 | 0.4x |
| `h20_blocked_two_hops` | 33.0 | 8.6 | 3.8x |
| `h21_rollup_fixed_depth` | 0.31 | 0.09 | 3.5x |
| `h22_null_logic` | 0.43 | 1.1 | 0.4x |
| `h23_or_across_relationships` | 1.7 | 0.99 | 1.8x |
| `h24_everything_at_once` | 16.5 | 8.5 | 1.9x |

### With a memory budget

Everything above is with every row held decoded in memory. With a budget on
decoded rows (`OWL_DECODED_DOCUMENTS`), all 44 queries still return the same
rows. Measured in one sitting, on a laptop in low power mode, so the ratios
are comparable and the times are not those of the table above:

| Budget, of 74,612 rows | Faster | Within 5x | Beyond | Owl time over SQLite's, whole suite |
|---|---:|---:|---:|---:|
| None | 13 | 29 | 2 | 2.0 |
| 20,000 | 7 | 26 | 11 | 3.4 |
| 20,000, compacted first | 6 | 27 | 11 | 3.1 |
| 500 | 3 | 12 | 29 | 5.2 |

The first time this was measured, the budget of 20,000 left 10 queries within
five times and the suite took six times as long as with no budget. What
closed most of that:

- A row decoded from a checkpoint file is held, within the budget. It used
  not to be held at all, so every statement that wanted it decoded it again.
- Following a relationship decodes each row it reaches once, not twice,
  passes over rows of other collections without reading them, and for a
  count or a sum decodes only the fields read.
- A page of a checkpoint file is read and checked once and kept for a while,
  and records that lie one after another are read in one stretch.
- A field's column is built in one pass that decodes a row, takes its fields
  and lets it go, so filters, cut sorts and aggregates run on columns whether
  or not the rows are held, and only the rows a filter leaves are decoded.
- A scan that names its fields decodes only those from a record on file, and
  skips over a row's long lists.

Owl is fastest when the data fits in memory. With a quarter of it in memory
it is under twice as slow as that; with under one percent, about two and a
half times. What remains is mostly queries that need whole rows or the lists
inside them.

### Memory and disk

`gauntlet memory` counts the bytes Owl has allocated and still holds, and
lists its files. For the same 74,612 rows, about 25 MB as text:

| | Size |
|---|---:|
| Held in memory after loading | 73 MB |
| Held after a checkpoint | 61 MB |
| Held after compaction | 33 MB |
| Held after every query has run | about 50 MB |
| Most held at once while a query runs | up to 115 MB |
| On disk after a checkpoint | 36.0 MB |
| On disk after compaction | 40.2 MB |
| On disk after compaction, with the copy it replaced removed | 22.4 MB |
| SQLite's files for the same data | 19.3 MB |

A database opened and told nothing takes a budget of a quarter of the
machine's memory (`OWL_DECODED_BYTES` sets it; 0 for none). The budget is one
figure for everything the store keeps so as not to read it again: half for
the rows held decoded, a quarter for column segments held decoded, and a
quarter for the columns and row lists kept between queries. While the data
fits within it nothing is let go and nothing is slower. Past it the oldest
are let go and read again when wanted.

### Larger than memory

Not yet. At ten times the data (746,012 rows, 228 MB on disk) under a 16 MB
budget, measured with `gauntlet memory --people 20000`:

| | |
|---|---|
| At most while reopening | 47 MB |
| Held after reopening, before any query | 37 MB |
| Held after every query has run | 61 MB |
| At most during the first compaction | 249 MB |
| At most during a compaction of the compacted database | 126 MB |

What is held is near the budget plus about 45 MB the budget does not cover:
bookkeeping for every row outside the column segments, which grows with the
data. Opening a database does not take the records of its checkpoint into
memory; each is read from its file when it is wanted. A compaction reads the
rows it compacts the same way, keeps the records it writes in a working
file, sorts its index entries on disk a bounded amount at a time, and builds
smaller segments when memory is short. Its column segments go to a working file as each
is built. What still grows with the data is the bookkeeping for every row
outside the column segments: its id, its schema version, its index and
change entries. A database not yet compacted has most of its rows in that
form (114 MB held at this size, against 54 MB once compacted), and a
compaction holds a second copy of it in the base it rebuilds. 

Queries are not there either. Grouping the elements of a list every row
holds (`g14`, `h16`, `h17`) reads the rows a bounded number at a time and no
longer needs the collection in memory; at ten times the data those three
had needed 637, 515 and 341 MB. A sort cut to a few rows (`h19`) reads only the
rows that can be among those kept, and a count of rows passing a filter
(`h15`) keeps none: 49 MB and 36 MB at twice the data, now 6 MB and 2 MB.
What needs the most now are the window queries (`g13`, `g12`, `g18`), about
60 MB each at twice the data whatever the budget: the rows are copied once
in order and again as they are shaped, and a partition's rows once more for
`so_far()`. With so small
a share for kept columns, the whole run also took several times as long as
with the earlier, looser limits.

A compaction keeps the copy it replaced, as something to fall back to;
`compact_and_reclaim` removes it once the new one has been read back and
checked.

## What the results say

Owl gets every answer right, including the ones SQL needs four nested
subqueries or a recursive CTE for. Twelve queries are faster than SQLite,
thirty are within five times, and two are further behind. On a busy machine
the ratios move by a point or more between runs, and should not be trusted.

The first run of this suite had sixteen queries more than five times slower,
the worst by 900 times. What closed most of that:

- A filter on plain fields is decided from the fields' columns, built in
  memory from the held rows and kept until the collection is written, so only
  the rows that pass are read at all. Cut sorts, aggregates and groups read
  from the same columns.
- The elements of a list each row holds, grouped (`tasks.worklog.group_by
  {..}`), are folded as they are read. A row used to be made of every
  element first, which for one query took as much memory again as the whole
  database; those queries are now faster than SQLite's.
- A sort of a whole collection reads its keys where the rows hold them.
- A row holds its values in order and shares its field names with other rows
  of the same shape, so reading a field is an index and not a hash.
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
