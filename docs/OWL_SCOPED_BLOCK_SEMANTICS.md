# Owl Scoped Block Semantics

## Purpose

This document defines the meaning of Owl's scoped query-block form.

It exists to make one central language rule precise:

> Chained query form and scoped block form are equivalent forms of the same language.

The block form does not introduce a second evaluation model. It is a different
presentation of the same collection-query semantics, with one major addition:
block-local names.

This document is intentionally language-facing. It is meant to be stable enough to
guide parser, planner, and executor work later, while still being readable as a
reference chapter.

## Core Rule

A scoped block is a query over a collection-valued root.

Inside the block, two kinds of entries may appear:

1. local definitions
2. ordinary query transforms

The block is read top to bottom. Local definitions introduce names for reuse.
Query transforms operate on the current root collection state exactly as they would
in chained form.

## Basic Shape

```oql
stores {
  returns = storeReturns2001.where { .store.id == stores.id }

  where { .region == "north" }
  sort { .name.asc }

  select { id, name, return_count: returns.count() }
}
```

This should be read as:

- start from the collection `stores`
- for each current store row, evaluate local definitions in scope
- apply root-level query transforms in source order
- produce the final shaped result

## Equivalence With Chained Form

These two forms mean the same kind of thing:

```oql
stores
  .where { .region == "north" }
  .sort { .name.asc }
  .select { id, name }
```

```oql
stores {
  where { .region == "north" }
  sort { .name.asc }
  select { id, name }
}
```

The scoped form is not "more powerful" because it is a block.
Its extra value is that it can name local derived facts without forcing repetition.

## Scope Model

A scoped block has three relevant scope layers.

### 1. Root collection scope

The collection introducing the block is the root scope.

In:

```oql
stores { ... }
```

- `stores` is the root collection name
- `.` refers to the current root row while evaluating root-row expressions

### 2. Local block scope

Names defined inside the block are visible from the point of definition onward.

Example:

```oql
stores {
  returns = ...
  avg_return = returns.avg(.return_amt)
  ...
}
```

Here:

- `returns` is visible to `avg_return`
- both are visible to later transforms and the final `select`
- neither exists before its own definition line

### 3. Nested query scope

Inside nested query expressions, `.` refers to the nested query's current row.
The enclosing root collection name remains available as a scope anchor.
Local block names also remain visible.

Example:

```oql
stores {
  returns = storeReturns2001.where { .store.id == stores.id }

  select {
    id,
    anomalous_customers: .customers.where {
      returns.where { .customer.id == customers.id }.sum(.return_amt) > 1000
    }
  }
}
```

Inside the inner `where`:

- `.` refers to the current customer in `.customers.where { ... }`
- `customers.id` refers to that same current customer row by scope anchor
- `stores.id` refers to the enclosing root store row
- `returns` refers to the local block binding already defined for that store row

This is the intended scoping model.

## Evaluation Order

Evaluation order must be explicit and predictable.

### Rule 1. Block entries are processed top to bottom

A scoped block is evaluated in source order.

### Rule 2. Local definitions are evaluated against the current root row at the point they appear

Example:

```oql
stores {
  returns = storeReturns2001.where { .store.id == stores.id }
  where { .region == "north" }
  select { id, count: returns.count() }
}
```

Meaning:

- for each current store row, `returns` is derived first
- then the root row is tested by the block `where`
- if that row survives, the final `select` may use `returns`

### Rule 3. Later definitions may use earlier definitions, never the reverse

This is invalid in principle:

```oql
stores {
  avg_return = returns.avg(.return_amt)
  returns = ...
  select { ... }
}
```

because `returns` does not exist yet.

### Rule 4. Query transforms apply to the current root collection state

A block `where`, `sort`, `limit`, `offset`, `group_by`, or `nearest` transforms the
current root collection just as the chained version would.

That means:

```oql
stores {
  where { .region == "north" }
  sort { .name.asc }
  select { id, name }
}
```

is equivalent to:

```oql
stores
  .where { .region == "north" }
  .sort { .name.asc }
  .select { id, name }
```

This equivalence is central.

## Name Visibility

### Rule 1. Local names are block-local

A name defined in a scoped block is visible only within that block.
It does not leak outside the block.

### Rule 2. Local names are visible only after definition

A local name may be referenced only by later entries in the block and by nested
expressions inside those later entries.

### Rule 3. Nested queries may capture local names from the enclosing block

This is allowed and desirable.

It is one of the main reasons the scoped form exists.

## Shadowing

Shadowing should be simple and conservative.

### Recommended first rule

Local block names must not shadow:

- the root collection name
- another local block name already defined in the same block

This means the following should be rejected:

```oql
stores {
  stores = ...
  select { id }
}
```

and:

```oql
stores {
  returns = ...
  returns = ...
  select { id }
}
```

This is a good first rule because it prevents needless ambiguity.

## Final Expression Rule

### Recommended first rule

A scoped block must end in exactly one final `select`.

This gives the form a clean and teachable contract:

- define local facts
- optionally filter, sort, limit, offset, group, or rank
- shape the result

### Why this is good

It preserves the block as a result-producing query form instead of letting it become
an open-ended control structure.

### Future relaxation

A future Owl may allow other terminal forms such as:

- `count()`
- `delete()`
- `update { ... }`

But the first stable version should keep the end condition simple.

## Legal Transforms Inside A Scoped Block

To preserve equivalence, a scoped block should allow the same root-level query
transforms as chained form, subject to sensible ordering.

### Recommended supported transforms

- `where { ... }`
- `sort { ... }`
- `limit(...)`
- `offset(...)`
- `group_by { ... }`
- `nearest { ... }`
- final `select { ... }`

### Recommended first ordering discipline

A good first discipline is:

- local definitions may appear anywhere before the final `select`
- ordinary transforms may appear in any order that would make sense in chained form
- `select` must be last

This keeps equivalence intuitive.

## Relationship To Collection Semantics

Scoped blocks do not change Owl's collection model.

If a value behaves like a collection, then the same collection operations still apply.
That means the root of a scoped block may be:

- a top-level collection such as `stores`
- a reverse relationship such as `.orders`
- an array field such as `.items`
- a previously bound query result such as `openStores`

Examples:

```oql
orders {
  books = .items.where { .category == "books" }
  select { id, book_count: books.count() }
}
```

```oql
companies {
  select {
    id,
    open_findings: .findings.where { .status == "open" }.count()
  }
}
```

The block form preserves the same collection-first mental model.

## Relationship To Null-Safe Traversal

Scoped blocks do not change Owl's null-safe traversal semantics.

That means:

- path reads inside local definitions remain null-safe
- path reads inside block `where` remain null-safe
- path reads inside final `select` remain null-safe

If a path cannot be resolved during a read, it yields `null` rather than raising an
access error.

This matters especially for local bindings such as:

```oql
stores {
  manager_name = .manager.profile.display_name
  select { id, manager_name }
}
```

If `.manager.profile.display_name` cannot be fully traversed, `manager_name` evaluates
to `null` for that row.

## Distinction From Transaction Blocks

A scoped query block is not a transaction block.

A query block:

- introduces local names
- applies collection transforms
- produces a query result

A transaction block:

- groups mutation statements
- defines atomic execution boundaries
- is not merely a scoped query presentation

The visual resemblance should not obscure the semantic difference.

## Equivalence Examples

### Example 1. Simple filter and projection

Chained form:

```oql
stores
  .where { .region == "north" }
  .sort { .name.asc }
  .select { id, name }
```

Scoped form:

```oql
stores {
  where { .region == "north" }
  sort { .name.asc }
  select { id, name }
}
```

Meaning: identical.

### Example 2. Local derived collection

Chained form is awkward because the derived collection would need to be repeated.

Scoped form:

```oql
orders {
  books = .items.where { .category == "books" }

  select {
    id,
    book_count: books.count(),
    total_book_value: books.sum(.price)
  }
}
```

Meaning:

- evaluate `books` for each order row
- use it repeatedly inside the final shaped result

This is equivalent in language model, but more readable in scoped presentation.

### Example 3. Scoped transform plus local summary

```oql
storeReturns2001 = store_returns.where { .returned_at.year() == 2001 }

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

Meaning:

- derive local store-scoped facts first
- filter and order the outer store collection
- shape the final result with those local facts still in scope

## Summary

Scoped blocks should be understood as Owl's readable scoped form of ordinary querying,
not as a different language mode.

Their purpose is to let serious queries stay calm and local:

- define reusable local facts once
- keep ordinary collection transforms in the same query surface
- finish with a shaped result

That is what makes them worth having.
