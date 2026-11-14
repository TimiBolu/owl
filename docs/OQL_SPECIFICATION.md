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
| Block comment | `/* ... */` | Out. One comment form is enough. |
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
| Bare name as a field | `select { id, name }`, `where { status == "x" }` | Works. Resolution order: function parameter, query parameter, script binding, block local, scope anchor, then field of the current row. |

### 5.2 Timestamps

| Feature | Status |
|---|---|
| Compare a `DateTime` with an RFC 3339 string, by instant | Works |
| `now()`, `today()`, `days_ago(n)`, `hours_ago(n)`, `minutes_ago(n)` | Works |
| `year month day hour minute second weekday`, `format_date`, `parse_date`, `add_days`, `add_hours`, `diff_days` | Works on `DateTime` values |
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
| A reference equals the id of its target | Works for `==`, `!=` and `in`: `orders.where { .user == "u1" }`. On the general executor only; the physical pipeline declines such predicates. |
| `&&`, `\|\|`, `!` with short-circuit | Works |
| `a \|\| b` yields the first present, truthy operand | Works |
| Membership `x in [..]` | Works |
| `list contains x`, `text contains part` | Works |
| `starts_with`, `ends_with` infix | Works |
| Range `x between a and b` | Out. `x >= a && x <= b` is already direct. |
| Parentheses for conditions and for operands | Works |
| A comparison is an ordinary boolean expression | Works |

### 5.4 Conditionals

| Feature | Status |
|---|---|
| `if c then a else b` | Works |
| `match x { "a" -> 1, _ -> 0 }` | Works |
| Match guards | Works: `match .status { "paid" if .amount > 200 -> "big", _ -> "other" }` |
| `coalesce(a, b, ...)`, `if_null(a, b)` | Works |

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
- **Time:** see 5.2
- **Utility:** `coalesce if_null to_string to_int to_float type_of`
- **Existence:** `exists is_null is_missing is_broken_ref`

Gaps found:

| Gap | Status |
|---|---|
| `length(list)` | Works. `length` counts the characters of text and the elements of a list. |
| Regular expression match | Proposed: `matches(text, pattern)`. One function, no operator. |
| `to_bool`, `to_datetime`, `days_until(from, to)`, `days_since(as_of, earlier)` | Works |
| List helpers beyond collection operations | Out. `where`, `select`, `sort`, `count` and the rest already apply to lists. |

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
| Order by an expression | `.sort { (.price * .qty).desc }` | Works |
| Limit, offset | `.limit(n)`, `.offset(n)` | Works with any expression: a literal, arithmetic, a `$parameter`, or a value computed from another collection. A negative or non-numeric count is an error. |
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

Still open: a misspelt *scalar* path such as `.nmae` on a schema'd collection
reads as `null` when it is not used as a collection. It should be the same
error.

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
| Block locals, visible from their definition onward, including inside nested queries | Works |
| A local may be a collection, an aggregate, or any value | Works |
| No shadowing of the root name or of an earlier local | Works |
| `group_by`, `limit`, `nearest` inside a block | Works |
| Object-valued select on a local bound to a reference | Works |
| A block ends in exactly one result entry | Works. The entry is a `select` or any terminal the chained form allows: `count()`, `first()`, `exists()`, an aggregate, `update {..}`, `delete()`. Nothing may follow it, and a block with no result entry is rejected. |
| A block rooted in a binding, an array field or a reverse relationship | *(not probed)* |

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
| Name a group key | `.group_by { month: format_date(.placed, "%Y-%m"), big: .amount > 100 }` | Works. Any other expression must be named. Two keys with one name are an error. |
| Count distinct | `.select { status }.distinct().count()` | Works |
| The rows of a group as a collection | `top: sort { .amount.desc }.limit(3).select { id }`, `big: where { .amount > 100 }.count()` | Works (6.2) |
| Grouping an empty collection gives no rows | | Works |
| `1` and `1.0` are one group | | Works |

Time-bucketed reporting, the main reporting gap named in the coverage review,
needed no new feature: `orders.group_by { month: format_date(.placed, "%Y-%m") }`.

Ranking within a partition, rollups, cubes and histogram families stay out of
the core (section 22). Top-k per parent is already expressible (section 8).

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

---

## 15. Functions

| Feature | Status |
|---|---|
| `fn name(p: Type, ...) -> Type { expression }` | Works |
| Return type is optional | Works |
| Parameters are in scope throughout the body | Works |
| Row parameters (`m: members`, passed with `*`) keep full path semantics | Works |
| Functions call functions; a row parameter can be passed on | Works |
| A function body may contain a nested query over other collections | Works |
| Arguments are type-checked; an `Integer` widens to `Float` | Works |
| Recursion is bounded with a clear error | Works |
| Local names inside a body | Works: `name = expression` lines before the value, each able to use the ones before it. A local may not hide a parameter. |
| A function that returns a collection | *(not probed)* |
| Redefining a function | Works: the later definition replaces the earlier. A built-in cannot be redefined. |

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

Performance note for the benchmark phase: an uncorrelated nested query,
bound or inline, is currently re-evaluated for every row it is compared
against. It should be evaluated once.

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
Unexpected token: Expected '}', found 'between' at line 1, column 20:
  users.where { .age between 1 }
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

`explain()` must be the last operation.

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
| Aggregation | count, sum, avg, min, max, group by, having, sort by aggregate, distinct, expression keys, aggregates over expressions, conditional aggregates, top-k per group | | window functions (out) |
| Ordering | sort, limit, offset, parameterised paging | | |
| Transactions | atomic blocks, rollback, read-your-writes | | isolation levels (out) |
| Update semantics | partial, nested, increment, list append and removal | | |
| Specialised | vector nearest, graph plus vector | | full text, geospatial (out) |
| Explain | `.explain()` | | |

---

## 22. Deliberately out of the core

Stated so that their absence is a decision, not an oversight.

- Join syntax. Relationships are navigated.
- A separate array language (`any`, `map`, `unwind`, `$elemMatch`).
- Update operator families (`$set`, `$push`, `$inc`). Shaped updates and `+=` cover them.
- Window functions, `ROLLUP`, `CUBE`, facets, histogram families.
- Set operations (`union`, `intersect`). Bindings and `in` cover the common cases.
- Regex as an operator. One function at most.
- `between`, block comments, nested transactions, isolation-level syntax.
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
