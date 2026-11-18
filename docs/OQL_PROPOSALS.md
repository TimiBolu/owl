# OQL proposals: the remaining feature gaps

Five gaps remain between OQL and what analytical queries need. Each is a
design choice, so each is written here as a question with options and a
recommendation, for a decision before any code.

Every proposal was judged by the rule the specification already states: make
an existing concept work in more places before adding a word. Each one says
what it adds to the language and what it lets us take out.

Everything marked "works today" was run against the current build.

| | Gap | Recommendation | New words | Words retired |
|---|---|---|---|---|
| P1 | Standard deviation, variance, mode | A standard library written in OQL. **Done.** | 0 built in | 0 |
| P2 | Running values and moving windows | Name the rows so far; add `last(n)`. **Done.** | 2 | 5 |
| P3 | Pairs of rows | Let a function be a step in a chain. **Done.** | 0 | 0 |
| P4 | A value carried along a walk | Each walked row carries its path; `routes` for every route. **Done.** | 1 field, 1 aggregate, 1 operation | 0 |
| P5 | Rollup, cube, pivot | Leave out | 0 | 0 |

---

## P1. Statistical aggregates as a library written in OQL

> **Done.** Option B was built: `src/std.oql` holds `variance`, `stddev`,
> `variance_pop`, `stddev_pop` and `mode`. A function defined in a script now
> lasts for the connection, and a function can end a chain
> (`orders.total.stddev()`). The specification has the details in section
> 15.1.

**The question.** Standard deviation, variance and mode are missing. Should
they be built in, like `median`?

**What works today.** They can already be written in the language, because a
function can take a collection:

```oql
fn variance(values) { values.select { sq: . * . }.sq.avg() - values.avg() * values.avg() }
fn stddev(values)   { sqrt(variance(values)) }
fn spread(values)   { values.max() - values.min() }
fn mode(values)     { values.group_by { v: . }.select { v, n: count() }.sort { .n.desc }.first().v }
```

These give the right answers through a relationship and as a method:
`stddev(.staff.pay)`, `.staff.pay.variance()`.

**Options.**

| | Option | For | Against |
|---|---|---|---|
| A | Build each in, as `median` is | Fastest to run | Every statistic anyone wants becomes a change to the engine. The core grows without limit. |
| B | Ship a standard library of functions written in OQL, loaded when a database opens | The core stays small. The library is readable, and users extend it the same way we do. | Slower than a built-in: each is several passes over the values. |
| C | Document the definitions and leave users to add them | No work | Everyone writes `stddev` slightly differently. |

**Recommendation: B.** It is the rule we agreed, applied to ourselves: a new
word must justify why it cannot be a function. If one library function proves
too slow in practice, the engine can recognise it and run it directly, as it
already does for `.field.distinct().count()`, without the language changing.

**What needs deciding.**

- Where the library lives: one `.oql` file compiled into the engine.
- Whether a user may redefine a library function. Today a later definition
  replaces an earlier one and a built-in cannot be redefined. Library
  functions should behave as user functions: replaceable.
- Whether `median` and `percentile` move into the library too. They need a
  sort the language cannot express as cheaply, so I would leave them built in.

**Work.** Small. One thing must be fixed first: a function defined in a script
is not visible to a later query on the same connection, which I noticed while
testing and have not looked into.

---

## P2. The rows so far

> **Done.** Option B was built: `so_far()` and `last(n)`, and the five
> `running_*` words are removed. A session number, which was
> `running_sum(if .. then 1 else 0)`, is now a count with a filter:
> `so_far().where { .. }.count()`. A reduction of every row so far is carried
> from row to row and a reduction of `last(n)` reads those rows; any other
> chain copies the rows so far for each row. The specification has the
> details in section 11.1.

**The question.** `running_sum`, `running_avg`, `running_min`, `running_max`
and `running_count` are five words for one idea: an aggregate over the rows
up to this one. A moving window ("the last 7 rows") cannot be written at all.
Can one concept replace the five and cover moving windows?

**What works today.** After `partition_by`, an aggregate with no receiver is
over the row's whole partition. That rule is one step from this one. A running
total can also be written as a correlated query, but it rereads the collection
for every row:

```oql
staff.sort { .id.asc }.select { id, upto: staff.where { .id <= staff.id }.sum(.pay) }
```

**Options.**

| | Option | Running total | Last 7 rows |
|---|---|---|---|
| A | Keep the `running_*` family, add `moving_*` | `running_sum(.amount)` | `moving_avg(.amount, 7)`: five more words |
| B | One collection, "the rows so far", and one operation, `last(n)` | `so_far().amount.sum()` | `so_far().last(7).amount.avg()` |
| C | A frame clause, as SQL has | `sum(.amount).over { rows: unbounded }` | `avg(.amount).over { rows: 7 }` |

**Recommendation: B.** `so_far()` is the rows of the partition from its first
row to this one, in order. Everything else is operations OQL already has:

```oql
events.sort { .at.asc }.partition_by { .person }.select {
  id,
  total:   so_far().amount.sum(),
  average: so_far().last(7).amount.avg(),
  highest: so_far().amount.max(),
  seen:    so_far().count(),
  busy:    so_far().last(30).where { .kind == "deploy" }.count()
}
```

The last line is something neither SQL's frames nor the current functions can
say directly: a filter inside a moving window.

It adds two things: `so_far()`, and `last(n)` as the counterpart of
`limit(n)`. It removes five. `prev`, `next`, `row_number`, `rank` and
`dense_rank` stay; they are about one other row or a position, not a set.

**What needs deciding.**

- The name. `so_far()` reads well in a sentence. Alternatives: `upto()`,
  `preceding()`. It needs the parentheses so that a field called `so_far`
  still works.
- Whether to remove the `running_*` words at once or keep them for a time.
  They are days old and nothing outside this repository uses them, so I would
  remove them, as with `count_distinct`.

**Work.** Moderate. The engine must not build the prefix afresh for every row:
`so_far().amount.sum()` has to run as the incremental sum it replaces. The
same recognition that runs `.field.distinct().count()` as one aggregate
applies here.

---

## P3. Pairs of rows

> **Done.** Option A was built. A function whose first parameter is declared
> as a row of a collection is a step in a chain. The specification has the
> details in section 15, and the gauntlet checks it against a SQL self-join.

**The question.** "People on the same team", "products bought together",
"events within an hour of each other": each row matched with other rows, as a
flat result. OQL has no join syntax by design. A nested select gives a tree,
not pairs.

**What works today.** A function can return the matching rows for one row, and
used inside a select it gives the tree:

```oql
fn mates(s: staff) { staff.where { .team == s.team && .id > s.id } }

staff.select { id, mates: mates(*).select { id } }     // works: a tree
staff.mates().select { a: staff.id, b: .id }           // does not parse
```

The second line is the flat form. Every piece of it already exists: a path
step flattens, the collection's name is the row each result came from, and a
function can be called as a method. They are not connected.

**Options.**

| | Option | Looks like | Against |
|---|---|---|---|
| A | Let a function that returns rows be a step in a chain | `staff.mates().select { a: staff.id, b: .id }` | The pairing rule lives in a function, away from the query. |
| B | An inline step: a block that returns rows for each row | `staff.each { staff.where { .team == each.team } }` | A new operation and a new name for the current row. |
| C | Join syntax | `staff.join(staff, on: ..)` | Against "relationships are navigated, never restated". |

**Recommendation: A.** It adds no word. A step is already "replace every row
by the rows something leads to"; today that something must be a path, and this
lets it be a function. The function names the relationship (`mates`,
`bought_with`, `near`), which is what OQL does with every other relationship.

Where the rule is used once and a function feels heavy, the tree form remains.

**What needs deciding.** Whether a pair should appear once or twice. With
`.id > s.id` in the function each pair appears once; that is the author's
choice and needs no language rule.

**Work.** Small to moderate: the parser must accept a call as a step, and the
step must keep the originating row reachable by name, which nested and
top-level steps already do.

---

## P4. A value carried along a walk

> **Done.** Option A was built: `._path`, and `product`. The open question
> was settled by adding `routes(.path)`, which gives one row for every route
> to a row and never passes through a row twice; `walk` is unchanged. One
> thing was decided while building: when a step is through several
> relationships, the rows passed through are on the path, because that is
> where a bill of materials keeps its quantities. The specification has the
> details in section 6.2.

**The question.** `walk` returns the rows reached and how deep each is. It
cannot answer "how many of this part does the whole machine need", which
multiplies quantities down a bill of materials, or "what does this route cost",
which adds along it. SQL's recursive query carries such a value.

**What works today.**

```oql
staff.where { .id == "d" }.walk(.boss).select { id, depth: ._depth }
```

**Options.**

| | Option | Looks like | Against |
|---|---|---|---|
| A | Each walked row carries the rows on the way to it, as a collection | `parts.walk(.components).select { id, needed: ._path.qty.product() }` | Memory: a path per row. |
| B | An accumulator argument to `walk` | `walk(.components, needed: .qty * needed)` | A small language inside one operation. |
| C | Recursive functions | `fn needed(p) { p.qty * needed(p.parent) }` | Works for a tree with one parent, not for a graph. Already possible today. |

**Recommendation: A.** `._path` is the rows from the starting row to this one,
in order. It is a collection, so everything that works on a collection works
on it: `._path.cost.sum()`, `._path.qty.product()`, `._path.select { id }`
for the route itself, `._path.count()` in place of `._depth`.

It needs one new aggregate, `product`, which is a plain aggregate like `sum`
and belongs in the core for that reason.

**What needs deciding.**

- When a row can be reached by more than one path, `walk` keeps the shortest.
  For quantities that is wrong: a part used in two assemblies is needed for
  both. `walk` would need a way to say "every path", which changes what it
  returns from a set to a list. I would add that as an argument only when
  `._path` is used, and I am least sure of this part of the five proposals.
- Whether `._depth` stays once `._path.count()` exists. I would keep it; it is
  cheaper and already used.

**Work.** Moderate, and the every-path question could make it larger.

---

## P5. Rollup, cube and pivot

**The question.** Subtotals at several levels in one result, and turning the
values of a field into columns.

**Recommendation: leave out.** Each is a large feature. Subtotals are several
`group_by` queries joined with `+`, which now works. A pivot is a `group_by`
followed by reshaping in the application, where the set of columns is known.
Neither blocks a query from being answered; they save typing.

---

## Suggested order

1. **P1, the library.** Smallest, and it sets the pattern for everything
   after: new capability arrives as OQL, not as engine code.
2. **P3, a function as a step.** Small, and it makes the library more useful.
3. **P2, the rows so far.** Removes five words. Worth doing before anyone
   depends on them.
4. **P4, the path of a walk.** Last, because of the open question in it.

## One observation from testing

Inside a query on `staff`, the text `staff.pay.sum()` gives 0. The name
`staff` there means the enclosing row, so `staff.pay` is one value, and summing
it should either give that value or be an error. It is not part of any
proposal above, but it is the kind of quiet answer the last round of fixes was
meant to remove.
