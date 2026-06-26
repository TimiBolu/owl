# Owl Null, Missing, And Existence Semantics

## Purpose

This document defines how Owl should behave when data is absent, incomplete, or cannot
be fully traversed.

This is one of the most important semantic chapters in the language because it affects:

- field access
- reference traversal
- filtering
- projection
- aggregation
- updates
- user trust

The design goal is simple:

> Owl should be graceful by default without becoming vague.

That means the language should protect users from nil-pointer-style pain and undefined
access errors, while still giving them precise tools when precision matters.

## Core Principle

### Read traversal is null-safe by default

If an Owl path cannot be fully resolved during read evaluation, the expression evaluates
to `null` rather than raising an access error.

Examples:

```oql
.user.team.org.name
.profile.address.city
.company.owner.email
```

If any link in the path is absent, null, or unresolvable, the full expression should
produce `null`.

This applies to:

- ordinary field access
- nested object traversal
- reference traversal
- chained traversal through mixed object and reference paths

This should be the default behavior. It should not require extra syntax.

## Semantic States

For language purposes, Owl should recognize four distinct situations.

### 1. Present value

The field or path resolves to a concrete non-null value.

Example:

- `.team.name` resolves to `"Payments"`

### 2. Present null

The field exists, but its value is explicitly `null`.

Example:

- `.deleted_at` exists and is `null`

### 3. Missing

A field does not exist at the requested location.

Examples:

- `.profile.avatar` where `avatar` is not present
- `.address.city` where `address` exists but `city` is absent

### 4. Broken reference

A reference value exists, but the referenced document cannot be resolved.

Example:

- `.team` contains a reference id that no longer points to a valid team document

## Default Read Semantics

Although Owl distinguishes these states conceptually, the default read behavior should
collapse some of them for convenience.

### Rule

For ordinary expression evaluation, these all yield `null` at the point of failure:

- present null
- missing field
- broken reference
- traversal through an already-null intermediate value

So:

```oql
.user.team.org.name
```

returns `null` if:

- `.user` is null
- `.user` is missing
- `.user.team` is a broken ref
- `.user.team.org` is missing

This is intentional.

It keeps common queries readable and prevents fragile error behavior from infecting the
whole language.

## Why Owl Should Do This

This behavior satisfies both least effort and least surprise.

### Least effort

Users should not need to wrap ordinary path access in defensive syntax just to avoid
runtime errors.

### Least surprise

A path either yields a value or it yields `null`. It should not sometimes explode and
sometimes quietly work depending on which hop failed.

### Language coherence

This keeps field access, nested object access, and reference traversal under one stable
rule.

## Filtering Semantics

`where` evaluates to a boolean decision per row.

### Rule

If a path inside a predicate cannot be resolved, it evaluates to `null`.
Comparison against `null` then follows ordinary boolean semantics.

That means most ordinary comparisons simply fail to match.

Example:

```oql
users.where { .team.org.name == "Zirro" }
```

If `.team.org.name` cannot be resolved for a given row, that row does not match.

This is exactly the common-sense result most users expect.

### Boolean behavior in filters

Recommended semantic direction:

- `true && true` -> `true`
- any comparison involving unresolved path values behaves as non-matching unless the
  predicate is explicitly testing for absence
- unresolved path values should not crash filter evaluation

This keeps `where` humane by default.

## Projection Semantics

In `select`, unresolved paths should project as `null`.

Example:

```oql
users.select {
  id,
  team_name: .team.name,
  org_name: .team.org.name
}
```

If `team` or `org` cannot be resolved, the projected field should appear as `null`.

This is especially important for UI-facing and report-facing queries, where partial data
is normal.

## Aggregation Semantics

Aggregates must also behave gracefully.

### Count

`count()` counts rows in the current collection.

It should not care whether particular fields are null.

### Field-based aggregates

For aggregates like:

- `sum(.amount)`
- `avg(.score)`
- `min(.created_at)`
- `max(.created_at)`

recommended behavior is:

- unresolved values are treated as absent inputs
- nulls are ignored rather than crashing the aggregate

This matches how users typically expect aggregates to behave over imperfect data.

### Empty input

A complete language contract should later define exact empty-collection results, but the
important point here is that absence should not turn aggregate evaluation into an access
error.

## Sorting Semantics

When sorting by a path that cannot be resolved for some rows:

```oql
users.sort { .team.org.name.asc }
```

those rows should sort predictably rather than erroring.

The exact ordering of null/unresolved values can be specified separately, but it should
be stable and documented.

The core requirement is:

- no access error
- deterministic ordering behavior

## Update Semantics

Reads should be soft by default. Writes should be more careful.

### Principle

A path that is graceful to read is not automatically safe to write.

For updates, Owl should be stricter because silent write failure is much more dangerous
than a soft read.

### Recommended direction

#### Updating an existing resolvable path

Allowed.

```oql
users.where { .id == "u1" }.update {
  profile.city: "Lagos"
}
```

#### Updating through a missing intermediate object

This should be defined explicitly.

There are two possible directions:

1. auto-create missing intermediate objects
2. fail the update unless the path can already be resolved structurally

My recommendation for Owl is:

- allow creation of missing intermediate objects only when that behavior is obvious and
  unambiguous for object paths
- do not silently synthesize through references

So:

- `profile.city: "Lagos"` may create `profile` if `profile` is an object slot
- `team.org.name: "X"` must not auto-create through a missing or broken reference chain

#### Updating through a broken reference

Should fail clearly.

A broken reference is not just “missing structure”. It is a relationship problem.
Silently treating it as writable structure would be surprising.

### Why writes must be stricter

Because users forgive a missing read value much more readily than a mutation that seemed
to succeed but changed nothing or changed the wrong thing.

## Existence Semantics

To preserve graceful reads without losing precision, Owl should support explicit
existence predicates.

There are two related but different needs.

### 1. Path existence

Question:

> does this path resolve to something present in the structure?

Candidate form:

```oql
exists(.profile.avatar)
```

This should answer whether the path exists structurally, not merely whether ordinary read
traversal would return a non-null value.

### 2. Collection existence

Question:

> does this collection-valued thing contain at least one row?

Candidate form:

```oql
.items.where { .category == "books" }.exists()
```

This is conceptually different from field existence.

One checks structure.
The other checks cardinality.

## Recommended Distinction

Owl should keep both forms, but define them clearly.

- `exists(path)` checks structural/path existence
- `collection.exists()` checks whether the collection has at least one row

This is a good place to preserve precision without forcing defensive syntax into every
ordinary query.

## Explicit Precision Predicates

Because ordinary reads collapse several failure states into `null`, Owl should also offer
precise predicates for users who need them.

Recommended direction:

- `is_null(path)`
- `is_missing(path)`
- `is_broken_ref(path)`

These should not be required for ordinary querying.
They exist so advanced users can ask exact questions when exactness matters.

## Recommended Meanings

### `is_null(path)`

True when the path resolves to an explicitly present null value.

### `is_missing(path)`

True when the path does not exist structurally.

### `is_broken_ref(path)`

True when the path reaches a reference value that cannot be resolved to a valid target.

This is especially important for maintenance, validation, and data-quality workflows.

## Broken Reference Semantics

Broken refs deserve special attention because Owl is graph-oriented.

### Default read behavior

Broken refs should still read as `null` in ordinary traversal.

That preserves graceful querying.

### Precision behavior

Users must still be able to detect broken refs explicitly when needed.

That is why `is_broken_ref(...)` matters.

### Design benefit

This gives Owl two things at once:

- pleasant everyday traversal
- explicit graph-integrity visibility

## Recommended Truth Table

The following is the intended semantic direction.

| Situation | Ordinary read of path | `exists(path)` | `is_null(path)` | `is_missing(path)` | `is_broken_ref(path)` |
|---|---|---:|---:|---:|---:|
| present non-null value | value | true | false | false | false |
| present null value | null | true | true | false | false |
| missing field | null | false | false | true | false |
| broken reference | null | false | false | false | true |

This table is extremely useful because it makes the language humane without making it
mushy.

## Why This Is Better Than Extra Syntax

Many languages solve null-safety with extra punctuation or mandatory defensive chaining.

Owl does not need to do that.

If read traversal is safe by default, users can write:

```oql
.team.org.name
```

instead of learning an entirely separate safe-navigation surface.

That is consistent with least effort and least surprise.

## Impact On Other Parts Of The Language

### Arrays as collections

This semantic model strengthens the arrays-as-collections decision.

Users can query nested repeated data naturally while still being protected from broken
paths.

### Reverse traversal

Reverse traversal should also remain graceful.
If no reverse-related rows exist, the result is simply an empty collection, not an error.

### Correlated nested queries

A correlated nested query should not fail just because the correlation path is absent on
some rows. Those rows should simply produce null or empty results according to context.

### Transactions and mutation

Transaction blocks become easier to reason about when reads are graceful and writes are
strict.

That is a good separation of concerns.

## Recommended Changes To The Main Owl Language Description

The main Owl language description should be updated in these ways:

1. add a formal statement that read traversal is null-safe by default
2. define the four semantic states:
   - present value
   - present null
   - missing
   - broken reference
3. distinguish `exists(path)` from `collection.exists()`
4. specify aggregate behavior over null/unresolved values
5. specify stricter write behavior for unresolved update paths

## Summary

The right Owl behavior is:

- graceful reads
- precise predicates
- strict writes

In one sentence:

> Owl should make it easy to ask for data without fear, and easy to ask for exactness
> when exactness matters.

That is a strong semantic foundation for a language built on nested values,
relationships, and collection composition.
