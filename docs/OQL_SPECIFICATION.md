# OQL Specification

This is the complete list of what Owl Query Language is meant to do, feature by
feature, with the current state of each one. It is the working contract for
finishing the language.

It is derived from the design documents (`OQL_PRINCIPLES.md`,
`OWL_LANGUAGE_DESCRIPTION.md`, `OWL_SCOPED_BLOCK_SEMANTICS.md`,
`OWL_NULL_AND_EXISTENCE_SEMANTICS.md`, `OWL_LANGUAGE_COVERAGE_REVIEW.md`,
`REFERENCE_WORKLOAD_SHAPES.md`). Where those documents leave a question open,
this one proposes an answer and marks it as a proposal.

## How to read the status column

Every status below was established by running the feature against the engine
on 2026-11-14 (about 190 probes), not by reading code or older documents.

| Mark | Meaning |
|---|---|
| **Works** | Parses, executes, and returned the right answer in the probe. |
| **Wrong** | Parses and executes but returns an incorrect result, or accepts something it must reject. These are the most urgent: they fail silently. |
| **Partial** | Works in some positions or forms and not others. |
| **Missing** | Does not parse or does not exist. |
| **Proposed** | Not settled by the design documents. The form shown is this document's recommendation and needs a decision (see section 23). |
| **Out** | Deliberately outside the core language. |

Items marked *(not probed)* were inferred from the grammar and still need a
test.

---

## 1. Principles and the tests they imply

The ten principles in `OQL_PRINCIPLES.md` reduce to five tests that any
feature in this document must pass. They are how gaps and proposals below were
judged.

1. **One model.** Anything collection-shaped (a top-level collection, an array
   field, a reverse relationship, a binding, a nested query) accepts the same
   operations with the same meaning. A feature that works on one source and
   not another is incomplete.
2. **One meaning.** The same text gives the same answer in every position and
   on every execution path.
3. **Graceful reads, strict writes.** A read never fails because data is
   absent; it yields `null`. A write never silently does less than it says.
4. **Model once.** A relationship declared once is navigable both ways. The
   user never restates a join.
5. **Small core.** A capability is added by making an existing concept work in
   more places before a new keyword is considered.

A sixth rule follows from the review of the implementation and is not in the
principles document, but everything else depends on it:

6. **No silent wrong answers.** If the engine cannot do what the text says, it
   reports an error. It never returns a plausible-looking result for a
   construct it does not implement.

---

## 2. Lexical structure

| Feature | Form | Status |
|---|---|---|
| Line comment | `// to end of line` | Works |
| Block comment | `/* ... */` | Works |
| SQL line comment | `-- to end of line` | Works. The two dashes are followed by a space or the end of the line, so `a --b` is still arithmetic. |
| String in single quotes | `'paid'` | Works, with the same escapes as double quotes |
| A closing `;` | `orders.count();` | Works: allowed after a statement, and ignored |
| Identifier | `[A-Za-z_][A-Za-z0-9_]*` | Works |
| Integer, float | `42`, `-7`, `3.14`, `-0.5` | Works |
| String with escapes | `"a \"b\" \n \u{1F989}"` | Works |
| Boolean, null | `true`, `false`, `null` | Works |
| Query parameter | `$name` | Works |
| Word operators | `and`, `or`, `not` alongside `&&`, `\|\|`, `!` | Works |
| Keywords as field names | `.count`, `.first`, `{ limit: 9 }`, `select { min: .x, avg: avg(.amount) }` | Works |
| Timestamp literal | see 5.2 | Not needed: timestamp text is accepted |

**Contextual keywords.** A keyword is only a keyword where the grammar expects
one. After a `.`, before a `:` in an object, insert, update or select block,
and as a shorthand select field, any identifier is a name: `.count` is the
field `count`, while `.count()` counts. Real data has fields called `count`,
`type`, `order` and `first`, and the language does not forbid them.

Not yet covered: a keyword as a *function parameter* name, and the structural
words (`if then else match and or not in`) as bare names without a leading
`.`.

---

## 3. Scripts and statements

| Feature | Status |
|---|---|
| A script is a sequence of statements, evaluated top to bottom | Works |
| Statement kinds: schema, function, binding, query (chained or scoped), transaction | Works |
| Free layout: statements end at a line break outside brackets unless they visibly continue; blank lines, comments and braces in strings have no effect | Works |
| A failed statement fails the script; a per-statement report is available | Works |
| A script stops at the first statement that fails; the report says how many later statements were not run | Works |

---

## 4. Types and schemas

### 4.1 Field types

| Type | Status |
|---|---|
| `String`, `Integer`, `Float`, `Boolean` | Works |
| `DateTime` | Works. Written from timestamp text (5.2). |
| `Object` (free-form nested value) | Works |
| `List(T)` | Works |
| `Optional(T)` and the `T?` shorthand | Works |
| `Ref(schema)` | Works |
| `Vector(n)` | Works |
| Aliases (`Int`, `Bool`, `Double`, `Timestamp`, `Array(T)`, `Map`, `Json`, `Reference(x)`) | Works. Proposed: keep them accepted but document only the canonical names. |
| Typed nested object: `address: { city: String, zip: String?, geo: { lat: Float, lng: Float } }` | Works, to any depth, and as list elements: `items: List({ sku: Ref(skus), qty: Integer })`. Required, unknown-field and type rules apply inside, and values are coerced as at the top level (timestamp text, ids to references). Plain `Object` stays free-form. |

### 4.2 Schema declaration

| Feature | Status |
|---|---|
| `schema name { field: Type, ... }`, one line or many, with comments | Works |
| Forward references between schemas in one script | Works |
| Unknown type is an error | Works |
| Collections without a schema (schemaless) | Works |

### 4.3 What a schema enforces on write

| Rule | Status |
|---|---|
| Required field must be present | Works |
| Unknown field is rejected | Works |
| Value must match the declared type | Works |
| `Integer` given for `Float` is widened | Works *(not probed for every path)* |
| A `Ref` must point at an existing document | Works |
| A `Ref` may point at a document written earlier in the same statement or transaction | Works: the rows of a tree can be inserted in one batch, parents before children, and a transaction can insert a row and then rows that refer to it. |
| `id` identifies a document uniquely within its collection | Works. An insert with an existing `id` is rejected, including within one batch; `id` cannot be changed by an update. |

### 4.4 Identity and system fields

Every document has `id`, `created_at`, `updated_at` and `version`. The `id` a
user writes is the document's identity: the key it is stored under and the
target references resolve to. It is unique per collection; `insert` of an
existing `id` is an error; `upsert on id` is the way to replace. An `id` may be
a non-empty `String` or an `Integer`. If `id` is omitted on insert, one is
generated.

Databases written before this rule stored documents under a generated key
with `id` as an ordinary field. They still read correctly, but uniqueness is
only enforced against documents written since.

### 4.5 Constraints, defaults and indexes

A constraint or an index is a fact about the collection, so it is declared in
the schema block next to the fields it concerns. There is no separate
statement kind.

```oql
schema accounts {
  id: String,
  email: String unique,
  tenant: String,
  handle: String,
  age: Integer,
  plan: String = "free",
  joined: DateTime = now(),
  unique(tenant, handle),
  check(.age >= 0 && .age < 150),
  index(.plan, .age)
}
```

| Feature | Form | Status |
|---|---|---|
| Unique field | `email: String unique` | Works. Backed by a unique index, so it is enforced without a scan. |
| Composite unique | `unique(tenant, handle)` | Works |
| Check | `check(.age >= 0 && .ends > .starts)` | Works: any condition over the document, enforced on insert and update |
| Default | `plan: String = "free"`, `joined: DateTime = now()`, `label: String = upper(.handle)` | Works. A default is an expression evaluated against the fields the insert wrote; a written value wins. |
| Index | `index(.status, .placed)` | Works. Reads that match use it. |
| Vector index | `index(.embedding, metric: "cosine")` | Works. Declares an exact index; approximate search remains an engine task. |
| The declared set is the set | | Works. An index or unique rule dropped from the block is removed on redeclaration. |

Field modifiers are written in this order after the type:
`name: Type [as reverse] [on delete rule] [from old] [unique] [= default]`.

### 4.6 Schema evolution

Redeclaring a schema is how it is changed. The schema file stays the single
source of truth, and there is no `ALTER` command family.

| Change | Result | Status |
|---|---|---|
| Add an optional field | Applied | Works |
| Add a field with a default | Applied; existing documents receive the default, which may be computed from their other fields | Works |
| Add a required field with no default | Refused while documents exist, with the remedy | Works |
| Make a required field optional | Applied | Works |
| Make an optional field required | Refused if any document lacks a value | Works |
| Widen `Integer` to `Float` | Applied; stored values are converted | Works |
| Any other type change | Refused while documents exist | Works |
| Rename | `full_name: String from name` | Works |
| Remove | `legacy: removed` | Works. Simply omitting a field that documents still carry is refused, so data is never dropped by accident. |
| Change a reference's delete rule or reverse name | Applied | Works |
| Change a reference's target | Refused | Works |
| Reshape an empty collection | Always applied | Works |
| A refused change writes nothing | | Works |
| Drop a collection | `drop books` removes documents, indexes and schema; refused while another schema refers to it | Works |

Not included: a general `migrate` block for arbitrary per-row transformation.
A change the rules above cannot express is done in two steps with an ordinary
`update` between two declarations.

---

## 5. Values and expressions

### 5.1 Paths

| Feature | Form | Status |
|---|---|---|
| Current row field | `.name` | Works |
| Nested object field | `.profile.city` | Works |
| Reference traversal, any depth | `.team.org.name` | Works |
| Null-safe: an unresolved path is `null` | | Works, with one exception below |
| Index into a list | `.tags[0]` | Works |
| Index into an absent list | `.profile.langs[1]` when `langs` is missing | Works: yields `null`. |
| Index from the end | `.tags[-1]`, `.tags[-2]` | Works |
| Out-of-range index is `null` | | Works |
| The current value itself | `.` as in `.tags.where { . == "beta" }`, `upper(.)`, `sort { . desc }` | Works. On an object row, `.` is the whole row. |
| A path through a collection | `.orders.total`, `.items.qty`, `.team.users.id` | Works: the list of what each row reached gives, flattened when it passes through more than one collection, with absent values left out. A relationship reached by name alone (`.orders`) is the list of its rows. References in such a list match the id they point at. |
| Bare name as a field | `select { id, name }`, `where { status == "x" }` | Works. Resolution order: function parameter, query parameter, script binding, block local, scope anchor, then field of the current row. |

### 5.2 Timestamps

| Feature | Status |
|---|---|
| Compare a `DateTime` with an RFC 3339 string, by instant | Works |
| `now()`, `today()`, `days_ago(n)`, `hours_ago(n)`, `minutes_ago(n)` | Works |
| `year month day hour minute second weekday`, `format_date`, `parse_date`, `add_days`, `add_hours`, `diff_days` | Works on `DateTime` values |
| `seconds_until(from, to)` | Works: whole seconds between two timestamps, for gaps too short to count in days. Null when either is absent. |
| Write a `DateTime` field from text | Works: `events.insert { at: "2026-01-05T10:00:00Z" }`, in inserts, updates and lists. Unparseable text is an error. |
| Date-only text `"2026-03-01"` | Works: midnight UTC on that day, for writes and comparisons. |
| Time functions on a timestamp stored as `String` | Works |
| Method form `.joined.year()` | Works (5.6) |

Wherever a `DateTime` is expected (a typed field on write, an argument of a
time function, a comparison with a `DateTime`), text in RFC 3339 form or
`YYYY-MM-DD` form is accepted and means that instant. There is no separate
literal syntax, and reads and writes are symmetric.

### 5.3 Operators

| Feature | Status |
|---|---|
| Arithmetic `+ - * / %`, usual precedence, left-associative | Works |
| Integer arithmetic is checked (overflow is an error) | Works |
| Unary minus | Works |
| String concatenation with `+` | Works |
| List concatenation with `+` | Works. An absent list counts as empty, so `tags: .tags + ["new"]` appends even to a row with no `tags`. |
| Comparison `== != < <= > >=` | Works |
| Numeric equality across `Integer` and `Float` | Works |
| Null and mismatched types never order and never fail | Works |
| A reference equals the id of its target | Works for `==`, `!=` and `in`: `orders.where { .user == "u1" }`. Served by the reverse-reference index when it is the leading filter, as `.user.id == "u1"` already was. |
| `&&`, `\|\|`, `!` with short-circuit | Works |
| `a \|\| b` yields the first present, truthy operand | Works |
| Membership `x in [..]` | Works |
| `list contains x`, `text contains part` | Works |
| `starts_with`, `ends_with` infix | Works |
| Range `x between a and b`, `x not between a and b` | Works. Both ends are included. |
| `x not in [..]`, `x is null`, `x is not null` | Works |
| `text like "A%"`, `ilike`, `not like` | Works. `%` is any run of characters and `_` is any one; `ilike` ignores case. |
| `list contains_all [..]`, `list contains_any [..]` | Works |
| Several values compared in order | Works: `(.price, .id) > (12.5, "p2")` compares the first pair and goes on to the next only when they are equal, which is how a page starts after the last row seen. |
| A field alone as a condition | Works: `where { .email }` keeps the rows where it is present and not empty, false or zero; `where { !.email }` the rest. A field declared `Boolean` is read as itself. |
| A field compared with a literal of another kind | An error on a collection with a schema: `.total > "50"` on `total: Float`, `.id == 1`, `.placed > "soon"`. It could never hold, and used to answer with no rows. Text that is a timestamp may be compared with a `DateTime`. |
| A reference compared with a row or a query | Works: `.customer == customers.where { .name == "Bob" }` when the query gives one row, and `.customer in customers.where { .city == "Lagos" }` for any number. |
| A value added to or taken from a list | Works: `.tags + "gold"`, `.tags - "vip"`, `.tags - ["vip", "early"]` |
| Timestamps and lengths of time | Works: `.joined + days(30)`, `.joined - weeks(1)`, and `a - b` for two timestamps is a length of time. Lengths of time compare, add, multiply by a number and divide by one another: `(.b - .a) / days(1)` is the days between. `weeks days hours minutes seconds` make one; `in_days in_hours in_minutes in_seconds` read one as a number. |
| `\|\|` between two texts | "Or", as everywhere: the first that is present. It never joins them, as SQL's `\|\|` does. Text is joined with `+` or `concat`. |
| Division of two whole numbers | A whole number: `7 / 2` is `3`. With a `Float` on either side it is `3.5`. |
| Parentheses for conditions and for operands | Works |
| A comparison is an ordinary boolean expression | Works |
| A path through a collection compared with one value | An error, for `== != < <= > >=`: `.orders.total > 100` has a total for every order, and whether any or all must exceed 100 is not for the engine to guess. The message gives the spelling for "any": `.orders.where { .total > 100 }.exists()`, or `.orders.total contains 100` for equality. It used to be false for every row. `contains`, `in` and comparison with a list are unaffected. |
| A field declared as a list compared with one value | An error, for `== != < <= > >=`: `.tags == "red"` on `tags: List(String)`. It is a mismatch of types and would be false for every row, when what was meant is `.tags contains "red"`, which the message gives. `.tags == ["red"]` and `.tags == null` are comparisons of like with like and are unaffected. On a collection with no schema nothing says the field is a list, so the quiet rule above still holds there. |

### 5.4 Conditionals

| Feature | Status |
|---|---|
| `if c then a else b` | Works |
| `match x { "a" -> 1, _ -> 0 }` | Works |
| Match guards | Works: `match .status { "paid" if .amount > 200 -> "big", _ -> "other" }` |
| `coalesce(a, b, ...)`, `if_null(a, b)` | Works |
| `case { cond -> a, cond -> b, _ -> c }` | Works: the first condition that holds, as SQL's searched `CASE` |

### 5.5 Literals for structured values

Object literals `{ a: 1, b: { c: 2 } }` and list literals `[1, 2, 3]` work in
every expression position.

### 5.6 Function calls

| Feature | Form | Status |
|---|---|---|
| Call | `upper(.name)` | Works |
| Method form on a path | `.name.upper()`, `.name.starts_with("A")`, `.joined.year()`, `.tags.contains("beta")` | Works, including chains (`.name.trim().upper()`) and user-defined functions |
| Method form on a parenthesised expression | `(.a \|\| .b).days_until(now())` | Works |

`x.f(args)` means `f(x, args)` for any expression `x` and any function `f`,
built in or user-defined. After a path, a name followed by `(` is a call,
never a field.

### 5.7 Built-in functions

Present today:

- **Text:** `trim upper lower length substring concat replace split starts_with ends_with contains pad_left pad_right`
- **Math:** `abs round floor ceil sqrt pow min max sin cos tan log ln exp random`
  (`min(a, b)` and `max(a, b)` are the lesser and greater of two values; with
  one argument they are the aggregates of section 11)
- **Time:** see 5.2
- **Utility:** `coalesce if_null to_string to_int to_float type_of`
- **Existence:** `exists is_null is_missing is_broken_ref`

Gaps found:

| Gap | Status |
|---|---|
| `length(list)` | Works. `length` counts the characters of text and the elements of a list. |
| Regular expression match | Works: `matches(text, pattern)`. One function, no operator. The pattern is found anywhere in the text unless anchored with `^` and `$`; `(?i)` at its start ignores case. Matching time grows with the text only: there are no back-references or look-around. Null, or a value that is not text, matches nothing; a pattern that does not parse is an error. A leading `where { matches(.field, "^prefix..") }` reads only the range of the field that starts with the prefix, through an index when the field has one. |
| `to_bool`, `to_datetime`, `days_until(from, to)`, `days_since(as_of, earlier)` | Works |
| List helpers beyond collection operations | Works: `join(list, sep)`, `reverse(list)`, `index_of(list, value)` (null when it is not there), `keys(object)`, and slices `.tags[1:]`, `.tags[:2]`, `.name[0:2]`, which also cut text. |
| `round(x, places)`, `repeat(text, n)`, `truncate(text, n)`, `truncate(when, "month")`, `when.format("%Y-%m")` | Works |

---

## 6. Collections and their operations

### 6.1 What counts as a collection

| Source | Example | Status |
|---|---|---|
| Top-level collection | `orders` | Works |
| Binding | `paid = orders.where {..}` then `paid.select {..}` | Works |
| Array field of objects | `.items` | Works |
| Array field of scalars | `.tags` | Works, using `.` for the element (5.1) |
| Reverse relationship | `.orders` on a user | Works |
| Nested query | `orders.where { .user.id == users.id }` | Works |
| List literal or list binding | `names = ["Ada", "Cy"]` | Works for `in`; other operations *(not probed)* |
| A single referenced or embedded object | `.team.select { name }`, `.profile.select { city }` | Works: yields one object, and shaping can traverse onward (`.team.select { name, org: .org.name }`). |
| A collection reached through a collection | `.teams.users`, `.teams.users.orders` on an org | Works: each step maps over the rows reached so far and flattens. |
| A path step in a chain | `orders.where { .status == "paid" }.items.group_by { .product.id }`, `orgs.teams.users.count()` | Works, at the top level and between operations in an expression. Every row is replaced by the rows its path leads to, which is how a query reaches the line items inside its orders. |
| The row an element came from | `orders.items.group_by { status: orders.status, .product.id }`, `users.orders.items.select { sku, status: orders.status }`, `.orders.items.select { sku, status: orders.status }` | Works for the collection a query starts from and for any collection its steps reach or pass through, at the top level and in a nested expression. After a path step, the collection's name is the row each element was reached from, until a `select` or `group_by` reshapes the rows. In a nested expression the enclosing row's own collection still means the enclosing row. With no step and no enclosing row, the collection's name is the row in hand: `staff.select { staff.pay }` is `staff.select { .pay }`. The name of a collection with no row in reach is that collection, so `units.budget.sum()` in a query on `staff` is the total over all units. This is what `$unwind` and a join back to the parent are for elsewhere. |

### 6.2 Operations

Each operation works on every source in 6.1. A conformance test runs 23
operations against the same rows reached five ways (top-level collection,
reverse relationship, correlated nested query, path through a collection,
embedded array) and requires identical answers, and a shorter set against a
list of scalars.

An operation written with no receiver applies to the rows in hand: the rows of
the group in a grouped `select`, or every input row in an aggregate `select`.
That is what `count()` and `sum(.x)` already did; it now holds for every
operation, which gives conditional aggregates and top-k per group with no new
construct:

```oql
orders.group_by { .status }.select {
  status,
  n: count(),
  big: where { .amount > 100 }.count(),
  top: sort { .amount.desc }.limit(3).select { id }
}
```

| Operation | Form | Status |
|---|---|---|
| Filter | `.where { cond }` | Works |
| Project | `.select { ... }` | Works (see section 7) |
| Order | `.sort { .a.asc, .b.desc }`, `.sort { .a desc }` | Works |
| Order by an expression | `.sort { (.price * .qty).desc }`, `.sort { abs(.x - 5).asc }` | Works. The direction after a computed key used to be read as a field of the value, which left the rows unsorted. |
| Limit, offset | `.limit(n)`, `.offset(n)` | Works with any expression: a literal, arithmetic, a `$parameter`, or a value computed from another collection. A negative or non-numeric count is an error. |
| The final rows | `.last(n)` | Works: the last `n` rows, in the order they were in. The counterpart of `limit(n)`; takes any expression `limit` does. `last` is a word only when applied, so a field may be called `last`. |
| First row | `.first()` | Works. Follow-on path works (`.first().amount`). |
| First row, then shape it | `.first().select { id, amount }` | Works: one object, or `null` when there is no row. |
| Non-emptiness | `.exists()` | Works |
| Count | `.count()`, `count(.field)` | Works |
| Sum, average, min, max of a path | `.sum(.amount)` | Works. One implementation for the terminal and the in-`select` form. |
| Aggregate of an expression | `.sum(.price * .qty)`, `sum(if .a then .b else 0)` | Works in the terminal form, in `select`, per group, and in `having` |
| Group | `.group_by { .a, .b }` | Works (section 11) |
| Filter groups | `.having { count() > 1 }` | Works |
| Nearest neighbours | `.nearest { field, query, metric, k }` | Works (section 18) |
| Distinct | `.distinct()` | Works. Rows are equal when all their fields are equal by value (`1` and `1.0` are one value; an object's field order is irrelevant). The first of each set is kept, so input order survives. |
| Order of operations is the order written | `limit(2).where {..}` filters the first two rows | Works |
| Join two collections, keeping every row | `(a + b).sort { .id.asc }`, `(.incoming + .outgoing).count()` | Works. `+` already joined two lists; two collections are the rows of the first followed by the rows of the second, duplicates kept, each still a row of its own collection. A query may start with such a sum in parentheses. The collections may be of different kinds. Where `union` gives a set, `+` gives everything. |
| Set operations | `.union(query)`, `.intersect(query)`, `.except(query)` | Works. The argument is a query, a collection name or a list. Rows are equal as for `distinct()`, and the result is a set: a row appears once. The order is that of the rows the operation is called on, then, for a union, the other query's. Rows keep their collection and identity, so relationships can be followed from them. |
| Walk a relationship to any depth | `.walk(.children)`, `.walk(.manager)`, `.walk(.blocked_by.blocker)`, `.walk(.children, 2)` | Works, at the top level and from a row (`below: .walk(.children).count()`). Every row reached by following the path one or more times, each once, nearest first, so a cycle ends the walk. The starting rows are included only if the path leads back to them. `._depth` is the number of steps that reached a row. A second argument limits the steps. |
| The way to a walked row | `._path` | Works: the rows from the row the walk started at to the row itself, in order. It is a collection, so `._path.cost.sum()` adds along the way, `._path.count()` counts it, `._path.select { id }` lists it. When one step is through several relationships (`.uses.part`), the rows passed through are on the way too, so a quantity held on the row between two parts is read as `._path.qty`. A row on the way that lacks a field has no value for it. The way is kept only when the statement reads `_path`. |
| Every route to a row | `.routes(.uses.part)`, `.routes(.blocked_by.blocker, 5)` | Works: as `walk`, but one row for every route to a row, not one row for each row reached. `walk` answers "what is reachable"; `routes` answers "in how many ways, and what does each cost". A route never passes through the same row twice, which is what ends it in a cycle. Rows carry `._depth` and `._path`. A bill of materials: `parts.where { .id == "bike" }.routes(.uses.part).select { id, needed: ._path.qty.product() }.group_by { .id }.select { id, needed: sum(.needed) }`. The number of routes can grow very fast with depth; past a million it is an error that asks for a depth. |
| The row a walk started from | `staff.where { .id == "e" }.walk(.boss).select { from: staff.id, id }`, `links.where { .blocked == "t4" }.blocker.walk(.blocked_by.blocker).select { task: links.blocked, far: id }` | Works, for `walk` and `routes`: as after a path step, the collection's name is the row each row was reached from, not the row reached. When the walk follows a step, the name is still the row the query began at. Naming the starting row changes what a walk from several rows gives: without the name, a row reached from two of them is one row; with it, the row is given once for each starting row that reaches it, since each has a starting row of its own to name. So `tasks.walk(.blocked_by.blocker).select { task: tasks.id, behind: id }` is every pair of a task and a task behind it. |
| Multiply | `product(.qty)`, `.uses.qty.product()` | Works: an aggregate like `sum`. Whole numbers stay whole and overflow is an error. Of no values it is 1, as a sum of none is 0. `product` is a word only when applied. |
| Partition for position functions | `.partition_by { .person }` | Works (section 11.1). Rows and their order are unchanged. |

### 6.3 Aggregate results on empty input

`count` is `0`, `sum` is `0`, and `avg`, `min`, `max` are `null`. Nulls and
unresolved values are ignored as inputs. A non-numeric input to `sum` or `avg`
is an error. All of this works and is the same in every form.

### 6.4 Text that cannot mean anything is an error

| Text | Result | Status |
|---|---|---|
| `users.where { count() > 0 }` | Error: an aggregate has no collection here | Works |
| `users.select { n: .orderz.count() }` on a collection with a schema | Error naming the unknown field or relationship | Works. No spelling suggestion yet. |
| The same on a schemaless collection | `0`: nothing can be ruled out | Works |
| `users.where { exists(.orders) }` | True when the relationship has rows, the same as `.orders.exists()` | Works |
| `staff.pay.sum()` in a query on `staff`, where `staff.pay` is the pay of the row being read | Error: it is one value, and the message gives the form for every row, `staff.sum(.pay)` | Works |

| `users.where { .team.nmae == "x" }`, `.orders.where { .totl > 1 }.exists()`, `.items.sum(.qyt)` on collections with schemas | Error naming the collection, the unknown name and the closest declared one | Works. Every name along a path is checked by following the schema through references, relationships and embedded objects of declared shape, wherever the path is written: a filter, a projection, a sort, a group key, a nested collection or a nested query. A name that is declared and has no value is still `null`. |
| `.children*.id`, `.orders.total * 2` | Error: a path through a collection has many values and arithmetic needs one. For `*` between two paths the message points at `walk` | Works |
| `sort { nonesuch(.x).desc }` | Error: unknown function | Works. A sort key that cannot be worked out for one row is still `null` for that row. |

After a `select`, a name is one the `select` produced or a relationship of the
row; any other is the same error. With `select { * }` nothing can be ruled out.

---

## 7. Projection

| Feature | Form | Status |
|---|---|---|
| Field list | `select { id, name }` | Works |
| Alias | `select { display: .profile.name }` | Works |
| Computed field | `select { vat: .amount * 0.075 }` | Works |
| Nested object | `select { who: { name: .name, city: .profile.city } }` | Works |
| List literal | `select { pair: [.age, .score] }` | Works |
| Nested collection | `select { books: .items.where {..}.select { sku, price } }` | Works, to at least three levels |
| Related object | `select { team: .team.select { name } }` | Works |
| Chained projections | `.select {..}.where { .computed > 1 }.sort { .computed.desc }` | Works. Later operations see the projected fields. |
| All fields | `select { * }` | Works |
| All fields plus more | `select { *, older: .age + 1 }` | Works. A later entry with the same name replaces the field. |
| All fields except some | `select { *, -profile, -tags }` | Works. Entries apply in order, so a later entry can add a field back. |

Rule to implement: in a `select` block, `*` stands for every field of the
current row. A later entry with the same name replaces it, and `-name` removes
it. Outside a `select` block `*` remains "the current row" as a function
argument.

---

## 8. Relationships

| Feature | Form | Status |
|---|---|---|
| Forward traversal | `.team.name` | Works |
| Deep forward traversal | `.user.team.org.tier` | Works, in `where` and `select` |
| Reverse traversal | `.orders` on a user, because `orders.user: Ref(users)` | Works |
| Filter, aggregate, project, sort, limit a reverse set | `.orders.where {..}.sum(.amount)` | Works |
| Reverse after forward | `.user.team.users.count()` | Works |
| Reverse inside `where` | `users.where { .orders.count() > 1 }` | Works |
| Existence and anti-existence | `.orders.where {..}.exists()`, `!.orders.exists()` | Works |
| Reverse through a collection | `.teams.users` | Works |
| Top-k within scope | `.orders.sort { .placed.desc }.limit(3)` | Works |
| Reference equals id | `.user == "u1"` | Works |
| A list of references | `members: List(Ref(users))` | *(not probed)* |

### 8.1 The name of a reverse relationship

The reverse of `orders.user: Ref(users)` is reached from a user as `.orders`,
the name of the referring collection. When two fields refer to the same
target, each can be named where it is declared:

```oql
schema transfers {
  id: String,
  from: Ref(accounts) as outgoing,
  to: Ref(accounts) as incoming,
  amount: Integer
}

accounts.select { id, sent: .outgoing.sum(.amount), received: .incoming.sum(.amount) }
```

| Rule | Status |
|---|---|
| `.transfers` is every transfer that refers to the account through any field | Works |
| `.outgoing` and `.incoming` are the transfers that refer to it through that one field | Works |
| A stored field of the same name always wins over a reverse relationship | Works |
| A name declared with `as` wins over a collection name | Works |

The form `.transfers(from)` considered earlier was not adopted: after a path,
a name followed by `(` is a function call (5.6), and the two readings would
collide.

---

## 9. Correlation and scope

| Feature | Status |
|---|---|
| A collection name used as a scope anchor: `orders.where { .user.id == users.id }` inside a `users` query | Works |
| Nested queries in `select` and in `where` | Works |
| An uncorrelated nested query as a value: `orders.where { .amount > orders.avg(.amount) }` | Works |
| Anchors through several nesting levels | Works *(two levels probed)* |
| The same collection nested inside itself | Works. The collection name is the enclosing row and `.` is the row being considered: `users.select { teammates: users.where { .team == users.team && .id != users.id }.count() }`. |
| An anchor that is a reference, a list, an object, or absent | Works. A reference is carried as the id of its target; an absent value is null and matches nothing. |

---

## 10. Scoped query blocks

| Feature | Status |
|---|---|
| `collection { where {..} sort {..} select {..} }` is the same query as the chained form | Works |
| Block locals, visible from their definition onward, including inside nested queries | Works. Inside a nested query or the operations of a path, a local still means what it meant for the block's row: `people { mine = .assignments.project.id  select { n: projects.where { .id in mine }.count() } }`. A local written as a bare field name (`boss = manager`) is the exception and is read against the inner row; write it as a path (`boss = .manager`). |
| A local may be a collection, an aggregate, or any value | Works |
| No shadowing of the root name or of an earlier local | Works |
| `group_by`, `limit`, `nearest` inside a block | Works |
| Object-valued select on a local bound to a reference | Works |
| A block ends in exactly one result entry | Works. The entry is a `select` or any terminal the chained form allows: `count()`, `first()`, `exists()`, an aggregate, `update {..}`, `delete()`. Nothing may follow it, and a block with no result entry is rejected. |
| A block rooted in a binding, an array field or a reverse relationship | *(not probed)* |

### 10.1 Captures: one row for every combination

A block names things in two ways. `name = value` names a whole value: the
block still has one row for each row it started from. `name in collection`
names each member of the collection in turn: the block then has one row for
every member, and the name is that member.

```oql
tasks {                                   -- three tasks that block one another in a ring
  b in .blocked_by.blocker
  c in b.blocked_by.blocker
  where { c.blocked_by.blocker contains tasks.id }
  select { a: id, b: b.id, c: c.id }
}

people {                                  -- the row between two others, named and read
  a in .assignments
  t in a.project.tasks
  where { a.role == "dev" }
  select { person: name, role: a.role, task: t.id }
}

tasks {                                   -- what blocks a task, and everything behind that
  where { id == "t4" }
  near in .blocked_by.blocker
  far in near.walk(.blocked_by.blocker)
  select { task: id, near: near.id, far: far.id, steps: far._depth }
}
```

| Rule | Status |
|---|---|
| `name in collection` is an entry of a block, anywhere before the result entry. The name is visible from its line onward, as a local is; it may not be the root's name, an earlier name, or a name already used above it | Works |
| The collection is anything collection-shaped, as it stands for the row in hand: a relationship or a path (`.assignments`, `b.blocked_by.blocker`), a list field (`tag in .tags`, `n in .notes`), a walk or routes (`near.walk(..)`), a path with operations (`.orders.where {..}`), a whole collection or a query on one (`p in people`). A single reference is one member, or none | Works |
| The row in hand stays the row the block started from: `.field`, a bare field and the collection's name mean that row. A captured row is read by its name: `b.id`, `b.project.name`, `b.blocked_by.blocker`, `b.select {..}`, `b.walk(..)` | Works. A captured row that is a stored row is everything a row reached by a path is: its fields, its references, and the rows that refer to it. |
| After a capture every entry works on combinations: `where`, `sort`, `limit`, `group_by`, `count()`, an aggregate and `select` mean what they always mean, over one row for every combination | Works. `group_by { a.role }` is keyed `role`. |
| A `where` may sit between two captures, and filters before the next one multiplies the rows | Works |
| A row with no member is not in the answer | Works |
| `name? in collection` keeps a row with no member: it is in the answer once, and the name is null. The `?` is the one that marks an optional field in a schema | Works. Every path from an empty name is absent (`m.name` is null), a capture from it has no members, and an aggregate over it is what it is over nothing (`sum(a.hours)` is 0). `where { a == null }` is the rows that have none: `people { a? in .assignments  where { a == null }  select { name } }`. The mark belongs to a capture; `name? = value` is not a form. |
| Nothing is left out unasked: two captures may land on the same row. `x != y` says otherwise; two captured rows are compared by which row each is | Works |
| Order: the rows the block started from in their order, and for each, its members in theirs | Works |
| Inside a nested query the name is still the row captured for the block's row: `links.where { .blocker == b.id }` | Works |
| What a walk notes on a row is read of the captured row: `far._depth`, `far._path` | Works |
| A name the captured row's collection does not have (`b.nmae`) | Error, as on any declared row, for a captured row that is a stored row |
| A write after a capture (`update`, `delete()`, an insert) | Error: there is one row for every combination, and a row would be written once for each |
| A capture outside a block, in a chain | Not a form. As with locals, a name belongs to a block. |
| A captured row in a `select` by its name alone (`select { b }`) | The reference to the row, as a reference field is. `b.select { .. }` gives its fields. |
| `explain()` on a block with a capture | Works: every entry from the first capture on, with the rows that went into it and the rows that came out (section 20). This is where a block that multiplies too far shows it. |

Captures run as written: each is a loop over the rows in hand, in the order
the entries stand. A block is fast when it starts from the selective end and
filters early. The engine does not yet move a filter ahead of a capture or
choose which end to start from, and a capture of a whole collection is every
pair until a later `where` narrows it.

---

## 11. Grouping and aggregation

| Feature | Form | Status |
|---|---|---|
| Group by one or several keys | `.group_by { .status }`, `.group_by { .user.id, .status }` | Works |
| Group by a path through a reference | `.group_by { .user.team.name }` | Works |
| Key naming in the group row | `.status` is `status`; `.user.id` is `user_id` | Works |
| Aggregates per group | `select { status, n: count(), total: sum(.amount) }` | Works |
| An alias that is a keyword | `select { avg: avg(.amount) }` | Works |
| `having` with aggregate expressions | `.having { count() >= 2 && sum(.amount) > 100 }` | Works |
| Sort and limit groups by an aggregate | `.select {..}.sort { .total.desc }.limit(1)` | Works |
| Aggregate arithmetic without grouping | `select { mean: sum(.amount) / count(.amount) }` | Works |
| Group by a function call | `.group_by { year(.placed) }` | Works. The key is named after the function (`year`). |
| Name a group key | `.group_by { month: format_date(.placed, "%Y-%m"), big: .amount > 100 }` | Works. Any other expression must be named, except a path from a named row (`a.role`, `orders.status`), which is keyed by its last name. Two keys with one name are an error. |
| Count distinct | `.select { status }.distinct().count()` | Works |
| An aggregate after a path of values | `.orders.total.sum()`, `people.salary.max()`, `.salary.median()`, `.salary.percentile(0.9)` | Works. With nothing named, an aggregate takes each value it follows: `.x.sum()` is `sum(.x)`. A path picks the values and a method reduces them, at the top level, through a relationship, in a filter and in a group. |
| Count distinct | `.customer.distinct().count()`, `.orders.status.distinct().count()` | Works, by composition, in every position. There is no word for it: `count_distinct` was added and then removed once the chain worked everywhere and ran as fast. |
| Median and percentiles | `.salary.median()`, `.salary.percentile(0.9)`, `median(.salary)`, `percentile(.salary, 0.9)` | Works in every form an aggregate takes. The fraction is between 0 and 1; a value that falls between two rows is interpolated between them. Null for no rows. |
| The rows of a group as a collection | `top: sort { .amount.desc }.limit(3).select { id }`, `big: where { .amount > 100 }.count()` | Works (6.2) |
| A path in a grouped `select` that is not a key | `buyers: .customer.distinct().count()`, `amounts: .amount` | Works: it is read from the group's rows, as a path through any other collection is. |
| Grouping an empty collection gives no rows | | Works |
| `1` and `1.0` are one group | | Works |

Time-bucketed reporting, the main reporting gap named in the coverage review,
needed no new feature: `orders.group_by { month: format_date(.placed, "%Y-%m") }`.

Rollups, cubes and histogram families stay out of the core (section 22).
Top-k per parent is already expressible (section 8).

### 11.1 Position functions

A `select` shapes its rows in order, and these functions answer for a row by
its position among them. They are what window functions are for in SQL.

```oql
events
  .sort { .person.id.asc, .at.asc }
  .partition_by { .person }
  .select {
    id,
    n: row_number(),
    total: so_far().amount.sum(),
    recent: so_far().last(7).amount.avg(),
    gap: seconds_until(prev(.at), .at),
    session: so_far().where { prev(.at) == null || seconds_until(prev(.at), .at) >= 21600 }.count()
  }
```

| Function | Meaning | Status |
|---|---|---|
| `row_number()` | The row's place, from 1 | Works |
| `prev(value)`, `next(value)`, `prev(value, n)` | The value for the row `n` places before or after, null past the edge | Works |
| `so_far()` | The rows of the partition from its first row to this one, in order, as a collection | Works |
| `rank(value)`, `dense_rank(value)` | Rows that follow one another with equal values share a rank; `rank` then skips, `dense_rank` does not | Works |

| Rule | Status |
|---|---|
| The order is the order of the rows: `sort` before the `select` | Works |
| `partition_by { keys }` makes the functions of the next `select` start again for each distinct key. A partition is all the rows with that key, wherever they fall | Works |
| With no `partition_by` the rows are one sequence | Works |
| After `partition_by`, an aggregate or an operation with no receiver is over the row's partition, and every row is kept | Works: `select { id, total: sum(.hours), share: .hours / sum(.hours), first: sort { .at.asc }.first().id }`. This is the partition's rows as a collection, the same rule a group's rows follow, so a partition total, a share and a first or last value need no functions of their own. `partition_by { all: 1 }` makes one partition of everything. |
| The rows of a relationship are their own sequence: `.events.sort { .at.asc }.select { so_far().amount.sum() }` | Works |
| One function may be used inside another | Works |
| A computed position can be filtered, grouped and sorted on by what follows the `select` | Works |
| Outside a `select` a position function is an error | Works |

**The rows so far.** A running total and a moving window are not functions of
their own. `so_far()` is a collection, and what is asked of it is asked with
the operations every collection has.

| Wanted | Written | Status |
|---|---|---|
| A running total, average, least, greatest, count | `so_far().amount.sum()`, `.avg()`, `.min()`, `.max()`, `so_far().count()` | Works |
| The same of a computed value | `so_far().sum(.price * .qty)` | Works |
| A moving window | `so_far().last(7).amount.avg()` | Works |
| A count that meets a condition | `so_far().where { .kind == "deploy" }.count()` | Works |
| A filter inside a window | `so_far().last(30).where { .amount > 100 }.count()` | Works |
| The last few that meet a condition | `so_far().where { .kind == "deploy" }.last(5).amount.avg()` | Works |
| Anything else a collection takes | `so_far().amount.median()`, `so_far().amount.stddev()`, `so_far().sort { .amount.desc }.first().id` | Works |

| Rule | Status |
|---|---|
| `so_far()` takes nothing and starts a chain; it is narrowed by what follows | Works |
| It includes the row being shaped, and reaches back to the start of that row's partition | Works |
| In a filter on it, `prev` and `next` are about the row being tested, so `so_far().where { prev(.at) == null }` finds the first row | Works |
| Outside a `select` it is an error | Works |

How each form runs is not part of its meaning, but it decides the cost. A
reduction of every row so far, with or without filters before it, is carried
from one row to the next: each row adds itself. A reduction of `last(n)` reads
those `n` rows. Any other chain is given a copy of the rows so far for every
row, so its cost grows with the square of the partition's size.

A query that uses these runs on the general executor from the first such
operation on; what precedes it still uses indexes and column reads.

---

## 12. Ordering and pagination

| Feature | Status |
|---|---|
| Multi-key sort with per-key direction | Works |
| A total, deterministic order: nulls first, then booleans, numbers, strings, timestamps | Works |
| Nulls first in ascending order, last in descending | Works |
| Stable sort | Works |
| `limit` and `offset` | Works, parameterisable (6.2) |
| Keyset or cursor pagination | The engine has keyset cursors; the language has no form. Proposed: none needed. `where { .placed < $after }.sort {..}.limit($n)` is keyset pagination, and works today. |

---

## 13. Mutation

| Feature | Form | Status |
|---|---|---|
| Insert one | `users.insert { id: "u1", ... }` | Works |
| Insert many, atomically | `users.insert [ {..}, {..} ]` | Works |
| Insert an existing `id` | | Rejected (4.3) |
| Update matching rows | `.where {..}.update { name: "x" }` | Works |
| Update from the row's own values | `update { score: .score * 2 }` | Works |
| Arithmetic sugar | `update { balance += 100 }` and `-= *= /=` | Works |
| Nested path | `update { profile.city: "Lagos" }` | Works |
| Missing intermediate object is created only for a declared `Object` field | | Works *(per existing tests)* |
| Writes never traverse a reference | `update { team.name: "x" }` is rejected | Works |
| Reshape a list | `update { items: .items.select { sku, qty: .qty + 1 } }` | Works |
| Append to a list | `update { tags: .tags + ["new"] }` | Works |
| Remove one element | `update { tags: .tags.where { . != "old" } }` | Works |
| Clear a field | `update { nickname: null }` | Works for optional fields |
| Delete matching rows | `.where {..}.delete()` | Works |
| Update or delete every row | `docs.update {..}`, `kw.delete()` with no `where` | Works. See decision D11. |
| Upsert | `users.upsert on id { id: "u9", ... }` | Works. Fields omitted on update are preserved. |
| A mutation returns the rows it affected | `.update {..}.select { id }`, `.insert {..}.select { id }` | Works |
| Type, required-field, unknown-field and reference checks | | Works |
| A failed multi-row mutation writes nothing | | Works |
| Unique and check constraints | | Works, declared in the schema (4.5) |
| `DateTime` from text | | Works (5.2) |
| Deleting a referenced document | `author: Ref(users) on delete cascade` | Works. `restrict` (the default) refuses the delete and names the referring document; `cascade` deletes the referrers, following chains; `nullify` clears an optional reference or removes the element from a list of references. The whole delete is decided before anything is written, so a `restrict` anywhere leaves everything untouched. |

---

## 14. Transactions

| Feature | Status |
|---|---|
| `transaction { ... }` applies all statements or none | Works |
| A failing statement rolls the block back | Works |
| Statements inside see earlier writes in the same block | Works |
| Bindings made inside are kept only on commit | Works |
| Nested transaction blocks | Rejected with an error. Out. |
| One active transaction per store; a second concurrent begin is rejected | Works |
| Atomicity comes from the storage engine's own transaction | Works. The whole-database copy is used only for a store that has no transaction mechanism. |
| A failure during commit rolls the block back and is reported | Works *(not exercised by a test: it needs an injected storage fault)* |
| Functions defined inside a rolled-back block are removed with it | Works |
| A block answers with the rows of its last statement | Works, as a script answers with the rows of its last statement. A block that is dropped answers with its error alone. |

---

## 15. Functions

| Feature | Status |
|---|---|
| `fn name(p: Type, ...) -> Type { expression }` | Works |
| Return type is optional | Works |
| Parameters are in scope throughout the body | Works |
| Row parameters (`m: members`, passed with `*`) keep full path semantics | Works |
| Functions call functions; a row parameter can be passed on | Works |
| A row parameter given by method form | Works: `.is_boss()` is `is_boss(*)`, and `.manager.is_boss()` gives the function the row the reference points at. |
| A function body may contain a nested query over other collections | Works |
| Arguments are type-checked; an `Integer` widens to `Float` | Works |
| Recursion is bounded with a clear error | Works |
| Local names inside a body | Works: `name = expression` lines before the value, each able to use the ones before it. A local may not hide a parameter. |
| A function that returns a collection | Works: `fn seniors(least: Integer) { staff.where { .pay > least } }`, used as `seniors(70).count()` inside an expression. |
| A function at the start of a query | Works: `seniors(70).sort { .id.asc }.select { id }`. When the body is a query it runs as if written out, and the rows are still rows of their collection, so relationships can be followed from them. |
| A function that takes a collection | Works: `fn spread(rows) { rows.max(.pay) - rows.min(.pay) }`, called `spread(.reports)` or `.reports.spread()`. A relationship arrives as its rows, which the function can filter, aggregate and follow relationships from; a path of values arrives as those values. This is how a word that might have been built in is written in the language: `fn distinct_count(values) { values.distinct().count() }`. |
| A path from a parameter that holds rows | Works: `rows.reports.count()`, `rows.pay.max()`, and `rows.pay` as the list of values. |
| A function at the end of a chain | Works: `orders.total.stddev()`, `orders.where { .. }.headcount()`, `staff.paid_over(85).sort { .id.asc }`. The function is given the rows in hand as its first argument, and what is in the parentheses are its others. One that gives rows back gives the chain its rows to carry on with; any other value is the answer, in a row named after the function. |
| A function of a row as a step | Works: with `fn mates(s: staff) { staff.where { .team == s.team && .id > s.id } }`, `staff.mates().select { a: staff.id, b: .id }` gives every pair once. A function whose first parameter is declared as a row of a collection is called for each row in hand, and the rows it gives for all of them carry on down the chain, still rows of their collection. The collection's name is the row the function was called for, as after a path step. This is how rows are paired with other rows; there is no join syntax. A function whose first parameter is not a row is given the rows together, as above. |
| A function defined in a script | Works for every query on the connection after it. It is stored in the database as any write is, so it is there after a restart, and a transaction that is rolled back does not keep the definitions made inside it. A connection that was already open sees a function another connection defines only once it is opened again. |
| A parameter without a type | Works: `fn f(rows)` takes whatever it is given. |
| A default value | Works: `fn raise(pay: Integer, by: Float = 0.1)`. Parameters with defaults may be left out from the end. |
| Redefining a function | Works: the later definition replaces the earlier. A built-in cannot be redefined. |

---

### 15.1 The standard library

`variance`, `stddev`, `variance_pop`, `stddev_pop` and `mode` are not built
in. They are functions written in OQL, in `src/std.oql`, loaded with every
database. Each takes a collection of values, so each works wherever a
function does: `.orders.total.stddev()`, `stddev(.orders.total)`,
`orders.total.stddev()`, and in a grouped `select`.

| Rule | Status |
|---|---|
| The library is there with no setup | Works |
| A library function is an ordinary function: defining one of the same name replaces it | Works |
| `variance` and `stddev` estimate from a sample (divide by one less than the number of values) and are null for fewer than two; the `_pop` forms divide by the number and are null for none | Works |
| `mode` is the value that occurs most often, the least of several that tie | Works |

A statistic OQL lacks is added here, or by a user in the same way, before it
is considered for the engine. `median` and `percentile` stay built in: they
need the values sorted, which the language cannot yet say as cheaply.

---

## 16. Bindings

| Feature | Status |
|---|---|
| Query binding, reused and extended: `paid = orders.where {..}` then `paid.select {..}`, `paid.sum(.amount)` | Works |
| Scalar binding: `cutoff = 100` | Works |
| Object binding with path access: `p = { city: "Lagos" }` then `p.city` | Works |
| List binding: `names = ["Ada", "Cy"]` then `.name in names` | Works |
| Bindings resolve in script order and reject cycles | Works |
| Binding to an aggregate used as a value: `mean = orders.avg(.amount)` then `orders.where { .amount > mean }` | Works |

A binding is a name for an expression. Using the name gives the same result
as writing the expression in its place.

Within one statement a nested query runs once per distinct question. After
the enclosing row's values are substituted, an uncorrelated nested query is
the same for every row and runs once; a correlated one runs once per distinct
value it is correlated on. Nothing is remembered between statements. A nested
query that reads a function's parameters is never shared.

---

## 17. Parameters

| Feature | Status |
|---|---|
| `$name` anywhere an expression is allowed | Works |
| Values are bound as data and cannot change query structure | Works |
| Scalar, list and object values | Works |
| Unbound parameter is an error | Works |
| A script value satisfies the parameter of the same name | Works |
| `limit($n)`, `offset($n)` | Works |
| A parameter as the query vector of `nearest` | *(not probed)* |

---

## 18. Vector search

| Feature | Status |
|---|---|
| `Vector(n)` fields | Works |
| `.nearest { field: .embedding, query: [...], metric: "cosine" \| "l2" \| "dot", k: n }` | Works |
| `._score` in the following `select` | Works |
| `nearest` after `where` (filtered search) and inside a scoped block | Works |
| Options `num_candidates`, `exact` | Works *(per existing tests)* |
| Dimension mismatch is an error | Works |
| `nearest` on a reverse relationship or array field (search within a parent's related rows) | *(not probed)* |
| The index behind it | Exact brute force in the storage engine. An approximate index is an engine task, not a language one. |

---

## 19. Errors

The language distinguishes three outcomes, and each construct must land in the
right one.

| Outcome | When |
|---|---|
| A value, possibly `null` | Any read of data that is absent, null, or behind a broken reference |
| A non-match | A comparison that cannot be decided |
| An error | Text that cannot mean anything (unknown function, unknown field on a schema'd collection, aggregate with no collection, unbound parameter), a write that cannot be done exactly as written, arithmetic that overflows or divides by zero |

A parse error names what was expected in the language's own symbols and
shows where:

```text
Unexpected token: Expected '}', found 'roughly' at line 1, column 20:
  users.where { .age roughly 1 }
                     ^
```

In a script, the message also says which statement, by the line it starts on.
Nesting deeper than 64 levels is an error rather than a crash.

On a collection with a declared schema, a path whose first name is not a
field, a system field or a relationship is an error with a suggestion:
`'users' has no field or relationship 'nmae'. Did you mean 'name'?`

---

## 20. Introspection

A terminal `.explain()` returns one row describing how the query would run,
without running it:

| Field | Meaning |
|---|---|
| `path` | `physical pipeline` or `general executor` |
| `uses_index` | whether an index serves the read |
| `plan` | the physical plan as text |

`explain()` must be the last operation. In a block it is the last entry, and
may come after the block's result entry: `tasks { where {..}  select {..}  explain() }`.

A block with a capture is the one query `explain()` runs. What such a block
costs is decided by how many rows each capture makes, which cannot be known
without making them, and nothing after a capture can write. The row then
also holds `steps`, and `plan` ends with the same as lines:

| Field of a step | Meaning |
|---|---|
| `step` | the entry as written: `b in .blocked_by.blocker`, `c? in b.blocked_by.blocker`, `far in c.walk(.blocked_by.blocker)`, `where { .. }`, `select { .. }`. The first step is the rows the block starts from, with how many entries stand before the first capture: `tasks, 1 entry` |
| `rows_in` | the rows that went into the entry; null for the first step |
| `rows_out` | the rows that came out |
| `ms` | the time the entry took |

```
tasks, 1 entry: 3 rows
b in .blocked_by.blocker: 3 rows in, 3 out
c? in b.blocked_by.blocker: 3 rows in, 3 out
far in c.walk(.blocked_by.blocker): 3 rows in, 9 out
where { .. }: 9 rows in, 3 out
select { .. }: 3 rows in, 3 out
```

`path`, `uses_index` and the start of `plan` say how the block's first rows
are read, as for any other query.

---

## 20a. Forms that read as a SQL or MongoDB user expects

Added after the completeness matrix showed where a query took more words in
OQL than elsewhere, or was accepted and answered wrongly. Every row here is
held by `tests/oql_language_gaps_test.rs`. The conditions, operators and
functions among them are listed in section 5; these are the rest.

| Feature | Form | Notes |
|---|---|---|
| Any, all, none of related rows or a list | `.orders.any { .status == "paid" }`, `.items.all { .qty >= 2 }`, `.orders.none { .total > 100 }` | `all` holds for a row with none, as it does in SQL. At the top level `orders.any { .. }` asks whether any row matches. |
| The first rows, the last row | `.first(2)`, `.last()` | `first(n)` is `limit(n)`. |
| The row with the most or least of something | `products.max_by { .price }`, `min_by { .. }` | Also inside a group: `group_by { .customer }.select { top: max_by { .total }.id }`. |
| A random sample | `products.sample(2)` | |
| Delete by a condition | `orders.delete { .status == "cancelled" }` | The same as `where { .. }.delete()`. |
| Insert the rows of a query | `archive.insert(orders.where { .status == "cancelled" })` | |
| What a write gives back | `orders.where { .. }.update { .. }.select { id, status }` | Insert, update, upsert and delete give back the rows they wrote or removed, so what follows reads them. |
| Take a field off a row | `update { -email }` | `exists(.email)` is then false. `email: null` keeps the field, holding null. |
| Add to and take from a list | `update { tags += "gold" }`, `update { tags -= "vip" }` | |
| Change one element of a list | `update { tags[0]: "gold" }`, `update { items[0].qty += 1 }` | |
| Change the elements that match | `update { items.where { .price < 5 }.qty += 1 }` | |
| Set a reference from a query | `update { customer: customers.where { .name == "Ada" } }` | The query gives one row. |
| The fields of an object set beside the others | `select { id, ...address }` | |
| A list of plain values reshaped | `.tags.select { upper(.) }` | Gives a list of values, not of rows. |
| A total beside each row's own values | `select { id, share: .total / sum(.total) }` | Every row is given, with the total over all of them. A select of totals alone is still one row. |
| An aggregate named the SQL way | `avg(orders.total)` | The same as `orders.avg(.total)`. `avg(.total)` in a filter is an error, because it is given one row; so is a total over a path that holds a list on every row, as `sum(.items.qty)`: the message gives `.items.sum(.qty)`. |
| Count of distinct values, count and total of some rows | `count(distinct .customer)`, `count(where .status == "paid")`, `sum(.total where .status == "paid")`, `count(.total > 50)` | |
| Filter on a group before shaping it | `group_by { .customer }.having { count() > 1 }` | |
| A key through a reference names itself | `group_by { .customer.city, .status }.select { city, status }` | |
| The ids of a group's rows | `group_by { .status }.select { status, ids: .id }` | `.id` on a group used to be the group's own made-up id. |
| Subtotals and grand totals | `group_by { rollup(.city, .status) }`, `group_by { cube(.a, .b) }` | `rollup` adds a row for each leading run of the keys and one for the whole; `cube` adds one for every combination. A key a subtotal leaves out is null in its row. `rollup` and `cube` take every key of the grouping. |
| Buckets of equal size, and rank as a share | `ntile(4)`, `percent_rank(.total)`, `cume_dist(.total)` | Position functions of section 11.1: over the sorted rows, or each partition. |
| Sort keys | `sort { .city, -.joined }`, `.email.nulls_last`, `.email.nulls_first` | Ascending when no direction is given; a leading `-` is descending. |
| A name nothing has | `nosuch.count()`, `where { .status == open }`, `.joined.year` | Errors, each saying what was probably meant: a collection that was never written to (one that was emptied still answers), a bare word that is not a field (text goes in quotes), and a field read off a plain value (`year` is a function: `.joined.year()`). All three used to answer with nothing. |

---

## 21. Filled-in comparison checklist

The blank Owl column of `Owl_SQL_MQL_gap_checklist.md`, summarised. Y works,
P partial or wrong in some form, N missing.

| Area | Y | P | N |
|---|---|---|---|
| Query shape | chained and scoped queries, bindings, functions, parameters, comments | | |
| Schema | typed fields, typed nested objects, references, vectors, optional fields, constraints, defaults, indexes, alter by redeclaration, drop | | |
| Mutation | insert, batch insert, update, delete, upsert, returning affected rows, unique identity | | |
| Filtering | equality, comparison, boolean logic, null checks, membership, existence, prefix and suffix and substring | | regex |
| Projection | fields, aliases, computed, object and list construction, nested projection, all-fields `*`, exclusion | | |
| Relationships | forward, reverse, deep traversal, nested property access, collection through collection, object-valued select | | |
| Aggregation | count, sum, avg, min, max, group by, having, sort by aggregate, distinct, expression keys, aggregates over expressions, conditional aggregates, top-k per group, distinct count, median and percentiles, position (window) functions, set operations, recursive traversal | | |
| Ordering | sort, limit, offset, parameterised paging | | |
| Transactions | atomic blocks, rollback, read-your-writes | | isolation levels (out) |
| Update semantics | partial, nested, increment, list append and removal | | |
| Specialised | vector nearest, graph plus vector | | full text, geospatial (out) |
| Explain | `.explain()` | | |

---

## 22. Deliberately out of the core

Stated so that their absence is a decision, not an oversight.

- Join syntax. Relationships are navigated.
- A separate array language (`map`, `unwind`, `$elemMatch`). `any`, `all` and
  `none` were on this list and are now words over the same collections
  everything else works on (section 20a).
- Update operator families (`$set`, `$push`, `$inc`). Shaped updates and `+=` cover them.
- Facets and histogram families. Window functions, set operations, `rollup`
  and `cube` were on this list and no longer are: analytical queries cannot
  be written without them (sections 6.2, 11.1 and 20a).
- Regex as an operator. One function at most.
- Nested transactions, isolation-level syntax. `between` and block comments
  were on this list; leaving them out saved nothing and cost every reader
  who came from SQL a moment's doubt.
- Full-text and geospatial search.

---

## 23. Decisions needed

Each has a recommendation, already written into the sections above. Anything
not overridden will be implemented as recommended.

| | Question | Recommendation |
|---|---|---|
| D1 | Typed nested objects in schemas? | Yes: `address: { city: String }`. Nested data is central to the model and is untyped today. **Implemented.** |
| D2 | Syntax for constraints, defaults and indexes | Inside the schema block: `unique`, `= default`, `unique(a, b)`, `check(..)`, `index(..)`. **Implemented; `check` takes any condition.** |
| D3 | Schema evolution | Redeclare the schema; compatible changes apply, incompatible ones need a `migrate` block; `from old_name` for renames. **Implemented with `from` and `removed`; no general `migrate` block.** |
| D4 | Distinct | `.distinct()` as a collection operation. **Implemented.** |
| D5 | All-fields and exclusion in `select` | `*` for all fields, `-name` to drop one. **Implemented.** |
| D6 | Reverse names when two fields reference the same target | `.coll` is the union, `.coll(field)` selects one; optional `as name` in the schema. **Implemented as `as name` only; `.coll(field)` was dropped because it collides with method calls.** |
| D7 | Self-nested scope anchors | Innermost scope wins. **Resolved the other way: the name is the enclosing row; the inner row is `.`. That is the only reading that lets a collection correlate with itself.** |
| D8 | Non-`select` terminals in scoped blocks | Allow whatever the chained form allows. **Implemented.** |
| D9 | Should a script stop at the first failing statement? | Yes outside a transaction. Today it continues, which can run later statements against a state the author did not intend. **Implemented.** |
| D10 | Access to a group's rows | Bare collection operations inside a grouped `select` apply to the group. **Implemented.** |
| D11 | Update or delete with no `where` | Keep allowed: `users.delete()` says what it does. Hosts that want a guard can add one. **Left as recommended.** |
| D12 | Behaviour when a referenced document is deleted | `on delete restrict` by default, with `cascade` and `nullify`. **Implemented.** |
| D13 | Local names in function bodies | `name = expression` lines, as in scoped blocks; retire the unused `let`. **Implemented.** |
| D14 | Plans from the language | Terminal `.explain()`. **Implemented.** |

---

## 24. Implementation order

Ordered so that each phase leaves the language more trustworthy than before,
and so the demo application and benchmark at the end exercise only finished
features.

**Phase 1 — stop silent wrong answers. Done**, and covered by
`tests/oql_language_conformance_test.rs`.

1. Unique `id` per collection (4.3).
2. `select { * }` (section 7).
3. Object-valued `select` on a reference, and `.first().select {..}` returning an object (6.1, 6.2).
4. A collection reached through a collection flattens (6.1).
5. A reference equals its target's id (5.3).
6. Indexing an absent list yields `null` (5.1).
7. A binding to an aggregate gives the same result as the inline expression (section 16).
8. The three must-be-errors cases (6.4).

**Phase 2 — remove the blockers to ordinary data. Done.**

9. Contextual keywords (section 2).
10. Strings accepted wherever a `DateTime` is expected (5.2).
11. Method-call form on paths (5.6).
12. The current value `.` (5.1), which completes scalar lists.
13. Expressions and parameters in `limit` and `offset` (6.2).
14. List concatenation, `length(list)`, negative indexes.

**Phase 3 — complete the collection model. Done.**

15. Aggregates over expressions (6.2).
16. `distinct` (D4).
17. Expression and named keys in `group_by` (section 11).
18. Group rows as a collection (D10).
19. Every operation verified on every collection source, as a generated test matrix.
20. Scoped block terminals (D8).

**Phase 4 — the schema as the single source of truth. Done.**

21. Constraints, defaults and indexes in the schema block (D2).
22. Reverse relationship naming (D6) and delete behaviour (D12).
23. Typed nested objects (D1).
24. Schema evolution and drop (D3).

**Phase 5 — scripts, transactions and tooling. Done.**

25. Transactions on the storage engine's own mechanism, atomic commit failure, function definitions rolled back.
26. Stop a script at the first failure (D9); function locals (D13).
27. Parser errors with source positions and no internal token names.
28. `.explain()` (D14); a nesting limit in the parser.

**Then:** the demo application and benchmark. It should be chosen to lean on
what makes Owl different: deep forward and reverse traversal, nested
projection three or more levels deep, correlated aggregates, time-bucketed
grouping, top-k per parent, and filtered vector search, each compared against
the same workload expressed in SQL.
