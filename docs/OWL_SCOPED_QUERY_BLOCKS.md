# Owl Scoped Query Blocks

## Purpose

This document defines scoped query blocks as the block-form equivalent of Owl's chained
query syntax.

This equivalence is the key design rule.

> In Owl, you either use pipe-style chaining or a scoped block. They mean the same kind
> of thing.

The scoped form exists so users can introduce local names and write serious queries more
readably, without changing mental model.

## Core Rule: Owl Equivalence

These two styles should be understood as equivalent forms of the same language.

### Chained form

```oql
stores
  .where { .region == "north" }
  .sort { .name.asc }
  .select { id, name }
```

### Scoped block form

```oql
stores {
  where { .region == "north" }
  sort { .name.asc }
  select { id, name }
}
```

The scoped block is not a new sublanguage.
It is Owl equivalence.

It simply gives the query a place to define local names before or between the same query
transforms.

## Why This Matters

Without this rule, scoped blocks risk becoming a second query language.
With this rule, the language stays unified.

A user only needs to learn one model:

- start from a collection-valued thing
- optionally filter, sort, limit, offset, group, or project it
- optionally define local names when the query benefits from them

That is all.

## Example Without Local Names

```oql
stores {
  where { .region == "north" }
  sort { .name.asc }
  select { id, name }
}
```

This should mean the same thing as the chained form.

## Example With Local Names

```oql
stores {
  returns = storeReturns2001.where { .store.id == stores.id }

  where { .region == "north" }
  sort { .name.asc }

  select { id, name, return_count: returns.count() }
}
```

This is the same underlying query style, but with a local derived collection added to the
scope.

That is the real value of the block form.

## What May Appear Inside A Scoped Block

A scoped block should allow two categories of entries.

### 1. Local definitions

Examples:

```oql
returns = storeReturns2001.where { .store.id == stores.id }
avg_return = returns.avg(.return_amt)
```

A local name may refer to:

- scalar values
- objects
- collection-valued expressions
- aggregate results

Definitions are evaluated in order and may use earlier local names.

### 2. Ordinary query transforms

A scoped block should allow the same core transforms Owl already supports in chained
form.

Examples:

- `where { ... }`
- `sort { ... }`
- `limit(...)`
- `offset(...)`
- `group_by { ... }`
- `select { ... }`
- `nearest { ... }`

That is what makes the equivalence real.

## Final Expression Rule

The simplest first rule is:

- a scoped block must end with a final result-shaping query transform, usually `select`

This keeps the form teachable.

A stricter first version could define:

- zero or more local definitions
- zero or more query transforms
- exactly one final `select`

That would be enough for most real queries.

## Why Scoped Blocks Are Better Than `let` Inside `select`

A previous design direction was to place local bindings inside `select`.

Example:

```oql
orders.select {
  let books = .items.where { .category == "books" },
  book_count: books.count()
}
```

This works, but the block form is cleaner because:

1. derivation and shaping are separated
2. `select` stays focused on output shape
3. the same transforms can appear in either chained or block form
4. serious queries read top-down in a natural way

## Repeated-Field Example

```oql
orders {
  books = .items.where { .category == "books" }

  select {
    id,
    book_count: books.count(),
    total_book_value: books.sum(.price),
    books: books.select { sku, price }
  }
}
```

This is a strong example of Owl equivalence plus local naming.

The user is still working with collections and collection transforms. There is no second
array language.

## Correlated Analytics Example

```oql
stores {
  returns = storeReturns2001.where { .store.id == stores.id }
  avg_return = returns.avg(.return_amt)

  where { .region == "north" }
  sort { .name.asc }

  select {
    id,
    name,
    anomalous_customers: .customers.where {
      returns.where { .customer.id == customers.id }.sum(.return_amt) > avg_return * 1.2
    }.select { id, name }
  }
}
```

This is much cleaner than repeating the same scoped filters inline.

## Recommended Terminology

The language description should call this:

- `scoped query block`

and should explicitly define it as:

- block-form Owl query equivalence

That phrasing matters because it prevents the language from splitting conceptually.

## Suggested Wording For The Main Language Description

A concise statement for the main spec could be:

> Owl supports two equivalent query forms: chained form and scoped block form.
> Scoped blocks are for the same query operations, but allow local derived names to be
> declared inline with the query's scope.

## Summary

Scoped query blocks should not be thought of as an extra feature layered onto Owl.

They should be thought of as the scoped presentation of the same collection language Owl
already has.

That is what keeps the language elegant.
