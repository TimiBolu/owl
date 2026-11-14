# Owl Query Language

## Purpose

This document describes Owl Query Language (OQL) as a language, not as a parser dump
and not as an implementation note.

Its job is to define the intended surface of Owl clearly enough that:

1. a user can read it and imagine what queries do,
2. an implementer can use it as a language contract,
3. new features can be evaluated against a stable set of design principles.

This document is intentionally closer in spirit to a language reference than a casual
guide. It is not meant to be a baby introduction.

It is also intentionally Owl-native. The goal is not to copy SQL or MongoDB. The goal
is to define Owl well enough that useful ideas from those systems can be recognized,
translated, or rejected on purpose.

## Design Principles

### 1. Least effort

The language should make common work feel direct.

If the data is already modeled as an object graph, the query language should let the
user move through that graph directly instead of reconstructing relationships by hand.

### 2. Least surprise

The same syntax should mean the same kind of thing everywhere.

A user should not have to learn one mental model for top-level collections, another for
nested arrays, another for reverse references, and another for aggregation. Owl should
reuse a small number of concepts consistently.

### 3. Object graph first

Owl is centered on persistent objects, references between them, nested values, and
collection-valued relationships. It should feel native to that shape of data.

### 4. Progressive power

Simple queries should stay simple. More complex queries should be built by composing the
same primitives rather than by switching into a different mini-language.

### 5. Description before implementation detail

This document defines the language the user sees. Storage, planning, and execution may
change beneath it so long as observable semantics remain consistent.

## Core Mental Model

Owl has five primary ideas:

1. `schema`
   defines the shape of a collection.
2. a `collection`
   contains documents of one schema.
3. a `document`
   is an object with fields.
4. a `value`
   may be scalar, object, reference, vector, or collection-valued.
5. a `query`
   starts from a collection-valued thing and transforms it.

The language becomes much easier to understand once one idea is accepted fully:

> Anything that behaves like a collection should support the same collection-level
> operations.

This includes:

- a top-level collection such as `orders`
- a reverse relationship such as `.findings`
- an array-valued field such as `.items`
- the result of another query such as `openOrders`

This is the most important simplification in Owl.

A user should not need to learn a special array language.

## Files, Statements, and Scripts

An OQL file is a sequence of statements.

A statement may be:

- a schema declaration
- a function declaration
- a binding declaration
- a query in chained form
- a query in scoped block form
- a transaction block

A script may contain one statement or many statements.

Example:

```oql
schema users {
  id: String,
  name: String,
  team: Ref(teams)
}

activeUsers = users.where { .active == true }

activeUsers.select { id, name }
```

A script is evaluated top to bottom.

Layout is free. A statement ends at a line break outside all brackets, unless
it visibly continues: the line ends with an operator or `=`, or the next line
begins with something other than a name (most often `.where`, `.select`, and
so on). Blank lines and comments may appear anywhere, including inside a
block, and braces inside strings or comments have no effect.

Later statements may refer to earlier declarations.

## Names and Scope

Names in Owl refer to one of four things:

- a schema or collection name
- a field name
- a function name
- a binding name

A query also introduces scope.

Inside a query:

- `.` means the current row or current value in scope
- `orders` means the collection named `orders`
- `users.id` means the `id` field of the current row of the enclosing `users` scope

This gives Owl a small but expressive scoping model.

### Scope anchors

Collection names may be used as scope anchors inside nested queries.

```oql
companies.select {
  id,
  open_findings: findings.where { .company.id == companies.id && .status == "open" }.count()
}
```

The meaning is:

- `companies.id` refers to the current `companies` row from the outer scope
- `.company.id` refers to the current `findings` row inside the inner scope

This is preferred over synthetic outer-scope keywords because it is explicit and scales
naturally with nested scopes.

## Values

Owl values are divided into five broad categories.

### Scalars

Scalar values include:

- `Null`
- `Boolean`
- `Integer`
- `Float`
- `String`
- `DateTime`

Literal forms:

```oql
42        -7        3.14      -0.5
true      false     null
"plain"   "a \"quoted\" word"   "line one\nline two"
```

A leading `-` makes a negative number wherever a value may appear, and negates
any other expression (`-.balance`).

Inside a string, a backslash escapes the next character: `\"`, `\\`, `\n`,
`\r`, `\t`, `\0`, and `\u{1F989}` for a Unicode scalar. An unknown escape is
an error, not a guess.

### Objects

Objects are named-field values.

```oql
{ city: "Lagos", country: "NG" }
```

### References

A reference points to a document of another schema.

```oql
team: Ref(teams)
```

A reference may later be traversed as an object.

```oql
users.select {
  id,
  team_name: .team.name
}
```

### Vectors

Vectors are fixed-dimension numeric values used for similarity search.

```oql
embedding: Vector(768)
```

### Collection-valued values

A value may itself behave like a collection.

Examples:

- a top-level collection: `orders`
- an array field: `.items`
- a reverse relationship: `.findings`
- a nested query result

These should all support the same collection operations where sensible.

## Schema Declarations

A schema declaration defines a collection and its field types.

```oql
schema orders {
  id: String,
  customer: Ref(customers),
  items: List(Object),
  amount: Float,
  status: String,
  created_at: DateTime
}
```

### Field types

The language description should recognize at least the following types:

- `String`
- `Integer`
- `Float`
- `Boolean`
- `DateTime`
- `Object`
- `List(T)`
- `Optional(T)`
- `Ref(schema)`
- `Vector(n)`

A schema may be declared in a compact form, but the meaning should remain the same.

### Altering schemas

Owl should support schema evolution, but the language should treat it as a schema
operation, not as ad hoc mutation of arbitrary shape.

A future complete language description should define:

- adding a field
- renaming a field
- removing a field
- changing optionality
- changing reference target
- default and migration behavior

The key principle is that schema change is explicit and reviewable.

## Bindings

A binding gives a name to a value or query.

```oql
openOrders = orders.where { .status == "open" }
```

Bindings are important because they let complex queries read in stages.

This is Owl-native and should be favored.

A binding may refer to:

- a query
- a scalar expression
- an object
- a collection-valued expression

Bindings are resolved in script order.

## Functions

A function defines reusable logic.

```oql
fn is_open_order(o: orders) -> Bool {
  o.status == "open" || o.status == "pending"
}
```

Functions may return:

- scalar values
- object values
- collection-derived scalar results

The language should prefer plain function forms over introducing separate declaration
kinds such as a special `query` keyword unless a compelling need emerges later.

That keeps the language smaller and more uniform.

## Query Shape

A query begins from a collection-valued thing and applies zero or more operations.

Owl supports two equivalent query forms: chained form and scoped block form.

The scoped block form is used when a query benefits from local derived collections or
local summary values before the final result is shaped.

The usual shape is:

```oql
collection
  .where { ... }
  .sort { ... }
  .limit(n)
  .select { ... }
```

Each stage transforms the current collection.

This is close enough to a pipeline to feel composable, but it remains expression-like
instead of becoming a separate pipeline language.

## Collection Operations

The same family of operations should work on any collection-valued thing.

That includes top-level collections, array-valued fields, reverse refs, and bindings.

When a query becomes locally complex, a scoped query block should be preferred over
repeating the same derived collection or summary expression many times. This does not
change the language model; it is the scoped presentation of the same query operations.

### `where`

`where` filters the current collection.

```oql
orders.where { .status == "paid" }
```

Meaning:

- walk each row in `orders`
- keep only rows whose `status` is `paid`

### `select`

`select` shapes the current collection into a new collection of projected rows.

```oql
orders.select {
  id,
  amount,
  customer_name: .customer.name
}
```

Meaning:

- for each row in `orders`
- build a new object with the listed fields

### `sort`

`sort` orders the current collection.

```oql
orders.sort { .created_at.desc, .amount.asc }
```

### `limit`

`limit` keeps the first `n` rows.

```oql
orders.limit(20)
```

### `offset`

`offset` skips the first `n` rows.

```oql
orders.offset(20)
```

### `count`

`count` turns a collection into an integer.

```oql
orders.where { .status == "paid" }.count()
```

### Aggregate functions

The following aggregate forms should be understood as collection-to-scalar operations:

- `count()`
- `sum(expr)`
- `avg(expr)`
- `min(expr)`
- `max(expr)`

### `first`

A useful primitive for least effort is a direct way to take the first row of a
collection.

```oql
orders.sort { .created_at.desc }.first()
```

This avoids forcing users to simulate common one-row patterns indirectly.

### `nearest`

`nearest` performs vector similarity search on a collection.

```oql
kb_chunks
  .where { .status == "published" }
  .nearest { field: .embedding, query: [0.1, 0.2, 0.3], metric: "cosine", k: 10 }
  .select { id, title: .article.title, score: ._score }
```

This should feel like another collection operator, not like a separate subsystem.

## Array Fields And Collection-Level Semantics

This is a central part of the Owl language design.

If a field holds multiple values, the language should let the user treat that field as a
collection.

That means an array field should support the same query operations users already know.

Example schema:

```oql
schema orders {
  id: String,
  items: List(Object)
}
```

Then:

```oql
orders.select {
  id,
  expensive_items: .items.where { .price > 100 }.select { sku, price },
  book_count: .items.where { .category == "books" }.count()
}
```

This is preferred over introducing an extra array mini-language such as `any(...)`,
`map(...)`, `expand(...)`, or a facet-like operator family unless a clear gap remains.

The reason is simple:

- a user already learned `where`, `select`, `sort`, `limit`, `count`
- Owl should let them reuse those words on any collection-valued thing

### Presence checks on nested collections

Because arrays are treated as collections, existence checks should read naturally too.

Preferred form:

```oql
orders.where { .items.where { .category == "books" }.count() > 0 }
```

A shorter existence form may later be added, but it should remain conceptually the same:

```oql
orders.where { .items.where { .category == "books" }.exists() }
```

The important point is that the user is still operating on a collection, not switching to
an unrelated array sublanguage.

## Field Access

Field access uses dot notation.

```oql
.profile.city
.customer.org.name
```

Field access should work uniformly across:

- current row fields
- nested object fields
- reference traversal

### Array indexing

Positional access may still exist for cases where order matters.

```oql
.items[0]
```

But indexed access should be treated as the specialized form, not as the primary way to
work with repeated data.

The primary way should be collection semantics.

## Filtering

A `where` block evaluates a boolean expression against each row of the current
collection.

### Core comparisons

```oql
.status == "paid"
.amount > 100
.active == true
.deleted_at == null
```

A comparison never fails; a predicate that cannot be decided for a row simply
does not match it. The rules are the same everywhere a comparison appears and
for every way the engine may execute the query:

- Integers and floats compare by numeric value: `10 == 10.0`.
- `null == null` is true, and `x != null` is true for any present `x`. Every
  ordered comparison (`<`, `<=`, `>`, `>=`) with `null` on either side is
  false, so `.age > 25` needs no `.age != null` guard.
- A `DateTime` compares with a string holding an RFC 3339 timestamp by
  instant, not by text.
- Values of otherwise different types are unequal and unordered.

A comparison is an ordinary boolean expression. It can be projected or passed
to a function, not only used in `where`:

```oql
people.select { id, adult: .age >= 18 }
```

Parentheses group either a condition or an operand:

```oql
(.status == "paid" || .status == "open") && .amount > 100
(.width + .height) > 5
```

### Boolean logic

```oql
.status == "paid" && .amount > 100
.country == "NG" || .country == "GH"
!(.active == true)
```

`&&` and `||` evaluate left to right and stop as soon as the result is known.
In a value position, `a || b` yields the first operand that is present and
truthy, which makes it a fallback:

```oql
users.select { id, shown: .nickname || .name }
```

### Membership

A list membership form should exist because it is both common and unsurprising.

```oql
.country in ["NG", "GH", "KE"]
```

### Null-safe traversal and existence

Read traversal in Owl is null-safe by default.

If a path cannot be fully resolved during ordinary read evaluation, it produces `null`
rather than an access error.

That means ordinary path access remains graceful across:

- present null values
- missing fields
- broken references
- failed traversal through an already-null intermediate value

For precision, Owl should distinguish these states explicitly when needed.

Recommended surface:

```oql
is_null(.deleted_at)
is_missing(.profile.avatar)
is_broken_ref(.team)
exists(.profile.avatar)
.items.where { .category == "books" }.exists()
```

The intended distinction is:

- `exists(path)` checks structural path existence
- `collection.exists()` checks whether a collection-valued thing has at least one row

This lets Owl stay graceful by default without losing semantic precision

### Pattern matching

Useful pattern matching should exist, but the language should avoid becoming regex-first.

Simple forms should be favored:

```oql
.name.starts_with("Tim")
.email.contains("@example.com")
```

A regex form can exist without becoming the normal path.

## Projection

`select` constructs the output shape.

### Plain field projection

```oql
users.select { id, name }
```

### Aliases

```oql
users.select {
  id,
  display_name: .profile.name
}
```

### Computed fields

```oql
orders.select {
  id,
  vat: .amount * 0.075,
  total: .amount * 1.075
}
```

### Nested projection

```oql
users.select {
  id,
  team: {
    id: .team.id,
    name: .team.name
  }
}
```

### Nested collection projection

```oql
orders.select {
  id,
  books: .items.where { .category == "books" }.select { sku, price }
}
```

This is one of Owl's strongest forms because it allows deeply expressive shaping without
changing mental model.

## Relationships

Owl should prefer relationship navigation over join ceremony.

### Forward traversal

```oql
users.select {
  id,
  team_name: .team.name,
  org_name: .team.org.name
}
```

### Reverse traversal

If a relationship is known, the reverse side should behave as a collection.

```oql
companies.select {
  id,
  open_findings: .findings.where { .status == "open" }.count()
}
```

This is much more Owl-native than forcing the user to restate foreign-key style matching
in every nested query.

### Deep traversal

Because references and nested objects are both navigable values, deep traversal should be
normal.

```oql
findings.where { .company.org.tier == "enterprise" }
```

## Grouping And Aggregation

Owl should support grouping, but the surface should remain small and direct.

### Grouping

```oql
orders
  .group_by { .status }
  .select {
    status,
    total_orders: count(),
    total_amount: sum(.amount)
  }
```

Meaning:

- partition the current collection by the grouping keys
- for each group, create one output row

### Grouping by multiple keys

```oql
orders
  .group_by { .customer.id, .status }
  .select {
    customer_id,
    status,
    total: count()
}
```

### Filtering groups with `having`

`having` filters materialized groups and may use the same aggregate expressions
available in grouped projection.

```oql
orders
  .group_by { .status }
  .having { count() >= 10 && sum(.amount) > 1000 }
  .select {
    status,
    total_orders: count(),
    total_amount: sum(.amount)
  }
```

Aggregate calls may also participate in larger expressions:

```oql
.having { sum(.amount) / count(.amount) >= 100 }
```

The same composability applies without grouping:

```oql
orders.select {
  average_amount: sum(.amount) / count(.amount),
  healthy: if avg(.amount) >= 100 then true else false
}
```

An aggregate call does not become a restricted projection mode. It remains an
ordinary scalar expression derived from the current collection.

`count()` counts group rows. `count(field)` counts resolved, non-null field
values. `sum`, `avg`, `min`, and `max` ignore null or unresolved inputs;
`avg`, `min`, and `max` return `null` when no usable input exists.

### Aggregation philosophy

Owl should support the aggregates people naturally reach for.

What it should avoid is inventing large alternative analytics sublanguages too early.

For that reason, this document does not recommend a `facet`-style construct as part of
Owl's core language.

If multiple grouped views are needed, bindings already provide a smaller and cleaner
mechanism.

```oql
ordersByStatus = orders.group_by { .status }.select { status, total: count() }
ordersByCountry = orders.group_by { .country }.select { country, total: count() }
```

That keeps Owl closer to a single compositional language.

## Sorting And Pagination

Sorting and pagination should be simple and unsurprising.

```oql
orders
  .where { .status == "paid" }
  .sort { .created_at.desc }
  .limit(25)
  .offset(50)
```

Cursor-oriented pagination may exist operationally, but the language should keep the
basic shape simple.

## Mutation

Mutation should remain declarative and close to the document shape.

### Insert

```oql
users.insert {
  id: "u1",
  name: "Ada",
  age: 29
}
```

### Batch insert

```oql
users.insert [
  { id: "u1", name: "Ada" },
  { id: "u2", name: "Grace" }
]
```

### Update

```oql
users.where { .id == "u1" }.update {
  name: "Ada Lovelace",
  age: 30
}
```

### Delete

```oql
users.where { .inactive == true }.delete()
```

### Upsert

Upsert is a first-class collection mutation and reads in Owl's own style:

```oql
users.upsert on id {
  id: "u1",
  name: "Ada",
  city: "Lagos"
}
```

This says exactly what matters:

- the target collection
- the key that determines identity
- the shape to insert or update

The named key must occur exactly once in the document block. If no document has
that key value, Owl inserts the document. If exactly one matches, Owl updates that
document while preserving fields omitted from the block. More than one match is a
data-integrity error; Owl must not choose one arbitrarily.

## Partial And Nested Updates

Owl should support updating only part of a document without forcing a second update
language full of symbolic operators.

Preferred direction:

```oql
users.where { .id == "u1" }.update {
  profile.city: "Lagos",
  profile.country: "NG"
}
```

For object paths, Owl creates a missing intermediate only when the schema declares
the root as `Object` (possibly optional). It does not replace an explicit `null` or
scalar with an object, and it never synthesizes or mutates through a reference.

For repeated fields, Owl should again try to reuse collection semantics rather than jump
immediately to a special operator language.

A mature form might look like:

```oql
orders.where { .id == "o1" }.update {
  items: .items.select {
    sku,
    price,
    quantity: if sku == "bk-1" then quantity + 1 else quantity
  }
}
```

This keeps the user inside the same projection model instead of asking them to learn a
completely separate array update vocabulary.

### Arithmetic update sugar

Small update conveniences may exist where they reduce effort without introducing a whole
new mutation language.

```oql
wallets.where { .id == "w1" }.update { balance += 100 }
```

This is a thin shorthand for `balance: .balance + 100`. The same lowering applies
to `-=`, `*=`, and `/=`.

## Transactions

Transactions are one of the clearest examples of Owl at its best.

A transaction block is easy to read, easy to imagine, and easy to explain.

```oql
transaction {
  wallets.where { .id == "w1" }.update { balance: 900 }
  wallets.where { .id == "w2" }.update { balance: 600 }
}
```

Its meaning is straightforward:

- execute the enclosed statements as one unit
- if one fails, the whole block fails
- no partial write should be observed as committed

This is cleaner than forcing the user into command-style begin/commit scaffolding for
most application-level work.

Explicit transaction control may still exist operationally, but the block form should be
considered the primary language form.

## Query Parameters

A query may refer to values supplied by its caller with `$name`:

```oql
events
  .where { .level == $level && .occurred_at >= $from }
  .limit(100)
```

The query text is fixed and the values arrive separately as data, so a value
can never alter the structure of the query. A host binds every parameter
before execution (`bind_parameters` in the Rust API); executing a query with an
unbound parameter is an error. Inside a script, a value binding satisfies the
parameter of the same name:

```oql
from = "2026-01-01T00:00:00Z"
events.where { .occurred_at >= $from }
```

## Functions, Conditionals, And Expressions

Owl expressions should remain small and readable.

A function's parameters are names in scope throughout its body, including in
nested calls, conditions, and nested queries. A parameter typed with a schema
name receives a whole row: pass the current row with `*`, and read it with
ordinary paths, which traverse references as they do anywhere else.

```oql
fn is_core_staff(m: members, tag: String) -> Bool {
  m.status == "active" && m.tags contains tag && m.team.name == "Core"
}

members.where { is_core_staff(*, "staff") }
```

Integer arithmetic is checked: a result outside the 64-bit range is an error,
never a wrapped value.

### Function call

```oql
normalize_name(.name)
```

### Conditional expression

```oql
if .amount >= 500 then "gold" else "standard"
```

### Method-style expression call

Method-style calls may exist when they improve readability without changing meaning.

```oql
.created_at.days_until(now())
```

These should behave as syntax sugar for function calls, not as a separate method system.

## Vector Querying

Vector querying should appear as a natural extension of collection querying.

### Vector field declaration

```oql
schema kb_chunks {
  id: String,
  text: String,
  embedding: Vector(768)
}
```

### Similarity search

```oql
kb_chunks
  .where { .status == "published" }
  .nearest { field: .embedding, query: [0.1, 0.2, 0.3], metric: "cosine", k: 5 }
  .select {
    id,
    text,
    score: ._score
  }
```

This keeps vector search inside the same overall query language rather than introducing a
new specialized surface.

## What Owl Should Intentionally Avoid In The Core Language

This section matters because a serious language description must define both what belongs
and what does not belong.

### 1. A separate array mini-language

Owl should not rush toward a design where arrays are queried through completely different
constructs from collections.

That would violate least effort and least surprise.

### 2. A large operator vocabulary for basic mutation

Mongo-style operator families are powerful, but copying them directly would make Owl feel
heavier and less coherent.

The first choice should be shaped updates and small amounts of carefully chosen sugar.

### 3. Full SQL join ceremony as the dominant relationship model

Owl may internally plan joins, but the user-facing language should prefer navigation,
reverse navigation, and collection composition.

### 4. Early adoption of large analytics sublanguages

The language should support aggregation, but should avoid growing a second world of
analytics-specific syntax before the core query model is complete.

That is why this document does not recommend making `facet` a core language primitive.

## Error, Null, And Existence Behavior

Owl now adopts the following semantic direction:

- read traversal is null-safe by default
- unresolved read paths yield `null` rather than access errors
- null, missing, and broken-reference states are conceptually distinct even though
  ordinary reads collapse them to `null`
- explicit predicates should exist for precision:
  - `is_null(path)`
  - `is_missing(path)`
  - `is_broken_ref(path)`
  - `exists(path)`
  - `collection.exists()`
- collection operations on empty collections remain well-defined
- writes should be stricter than reads: unresolved update paths must not silently succeed

The detailed semantics for this behavior are defined in:

- `/Users/timiadesina/Documents/owl/docs/OWL_NULL_AND_EXISTENCE_SEMANTICS.md`

## A Readable Grammar Skeleton

The following is not meant to be parser-ready BNF. It is a compact statement of the
shapes the language intends to support.

```text
script
  := statement*

statement
  := schema_decl
   | function_decl
   | binding_decl
   | query_expr
   | transaction_block

schema_decl
  := "schema" IDENT "{" field_decl ("," field_decl)* "}"

field_decl
  := IDENT ":" type_expr

type_expr
  := "String"
   | "Integer"
   | "Float"
   | "Boolean"
   | "DateTime"
   | "Object"
   | "List" "(" type_expr ")"
   | "Optional" "(" type_expr ")"
   | "Ref" "(" IDENT ")"
   | "Vector" "(" INTEGER ")"

function_decl
  := "fn" IDENT "(" param_list? ")" "->" type_expr block_expr

binding_decl
  := IDENT "=" expr

transaction_block
  := "transaction" "{" statement* "}"

query_expr
  := expr ("." query_op)*

query_op
  := "where" block_expr
   | "select" block_expr
   | "sort" block_expr
   | "group_by" block_expr
   | "limit" "(" expr ")"
   | "offset" "(" expr ")"
   | "count" "(" ")"
   | "first" "(" ")"
   | "nearest" block_expr
   | "insert" object_or_array
   | "upsert" "on" IDENT object_expr
   | "update" block_expr
   | "delete" "(" ")"

expr
  := literal
   | IDENT
   | "." path
   | IDENT "." path
   | object_expr
   | array_expr
   | function_call
   | conditional_expr
   | binary_expr
   | parenthesized_expr
```

This grammar skeleton is deliberately small.

That is a feature, not a limitation.

It reflects the claim that Owl should derive power mainly from:

- a consistent value model
- a consistent collection model
- composable blocks
- explicit traversal

not from endless special-purpose forms.

## Canonical Examples

### 1. Basic filtering

```oql
orders.where { .status == "paid" }.select { id, amount }
```

### 2. Reference traversal

```oql
users.select { id, team: .team.name, org: .team.org.name }
```

### 3. Reverse traversal

```oql
companies.select {
  id,
  open_findings: .findings.where { .status == "open" }.count()
}
```

### 4. Arrays as collections

```oql
orders.select {
  id,
  book_items: .items.where { .category == "books" }.select { sku, price },
  book_count: .items.where { .category == "books" }.count()
}
```

### 5. Correlated nested query

```oql
companies.select {
  id,
  paid_total: orders.where { .company.id == companies.id && .status == "paid" }
    .select { amount }
}
```

### 6. Transaction block

```oql
transaction {
  wallets.where { .id == "w1" }.update { balance -= 100 }
  wallets.where { .id == "w2" }.update { balance += 100 }
}
```

### 7. Vector query

```oql
kb_chunks
  .where { .status == "published" }
  .nearest { field: .embedding, query: [0.2, 0.4, 0.9], metric: "cosine", k: 10 }
  .select { id, title: .article.title, score: ._score }
```

## Open Questions That Must Be Resolved Deliberately

A serious language description should leave room for unresolved questions, but it should
name them clearly.

1. How exactly should missing vs null be represented and tested?
2. What is the final surface for `exists` on fields and collections?
3. What is the final syntax for schema alteration and migration?
4. What update sugar is acceptable before the language starts to fragment?
5. How should list element mutation behave while preserving Owl's one-model principle?
6. How should distinct/dedup be expressed without bloating the core language?
7. What forms of pattern matching belong in the core language?
8. How far should method-style syntax go before it becomes surprising?

These questions should be resolved by preserving the central idea of the language rather
than by copying precedents mechanically.

## Summary

Owl should be understood as a language of:

- collections
- values
- traversal
- shaping
- composition
- transactions

Its most important choice is that array-valued fields, reverse relationships, and
ordinary collections should feel like variations of the same thing.

That is the simplest path to a language that is powerful, learnable, and distinctly Owl.

## Lessons Taken From PostgreSQL And MongoDB

This document was shaped by reviewing the vendored PostgreSQL and MongoDB reference
materials in:

- `/Users/timiadesina/Documents/owl/docs/reference_grammars/postgres/`
- `/Users/timiadesina/Documents/owl/docs/reference_grammars/mongodb/`

The goal was not to copy their syntax. The goal was to observe how mature languages make
large surfaces understandable.

### From PostgreSQL

PostgreSQL is useful as a reference because its language is described through a small
number of enduring ideas:

- value expressions
- field selection
- table and query expressions
- grouping and ordering
- arrays as first-class values

The main lesson Owl should take from PostgreSQL is this:

> a language becomes teachable when it is described in terms of a few stable semantic
> categories rather than a giant list of commands.

That is why this document is organized around:

- values
- collections
- traversal
- shaping
- mutation
- transactions

rather than around a long alphabetical keyword inventory.

### From MongoDB

MongoDB is useful as a reference because it treats nested data, projections, update
behavior, and aggregation as serious first-class concerns.

The strongest lesson Owl should take from MongoDB is not the operator syntax itself.
It is this:

> users want to work with shaped, nested data directly, without pretending everything is
> flat.

That is why Owl should preserve:

- dot-path access
- nested projection
- collection-valued nested fields
- direct manipulation of document-shaped data

without importing MongoDB's heavy operator vocabulary wholesale.

### What Owl Should Not Copy Literally

The review also made some rejections clearer.

Owl should not let feature parity pressure push it into:

- SQL-style join ceremony as the primary way to traverse relationships
- Mongo-style operator sprawl for ordinary updates
- special-purpose array query sublanguages when collection semantics already work
- early adoption of large pipeline-specific analytics forms such as facet-driven cores

The right question for Owl is not:

> does PostgreSQL or MongoDB have a feature with this name?

The right question is:

> can this capability be expressed through Owl's existing mental model without making the
> language harder to learn?

If the answer is yes, the feature probably belongs.
If the answer is no, Owl should be slow and deliberate.


## Scoped Query Blocks

Owl supports two equivalent query forms:

- chained form
- scoped block form

A scoped query block is the block-form equivalent of a chained Owl query. It lets a
collection define local names before producing a final shaped result.

Example:

```oql
stores {
  returns = storeReturns2001.where { .store.id == stores.id }
  avg_return = returns.avg(.return_amt)

  where { .region == "north" }
  sort { .name.asc }

  select {
    id,
    anomalous_customers: .customers.where {
      returns.where { .customer.id == customers.id }.sum(.return_amt) > avg_return * 1.2
    }.select { id, name }
  }
}
```

The intended reading is simple:

- enter collection scope,
- define local facts,
- apply ordinary query transforms,
- produce the final shape.

Scoped blocks preserve the same query semantics as chained form. They are preferred when
a query would otherwise repeat the same derived collection or summary expression many
times.

The precise rules for scope, ordering, legality of transforms, and final-expression
shape are defined in `/Users/timiadesina/Documents/owl/docs/OWL_SCOPED_BLOCK_SEMANTICS.md`.
