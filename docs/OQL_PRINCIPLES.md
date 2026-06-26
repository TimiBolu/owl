# OQL Principles

## Purpose

This document states the core principles of Owl Query Language (OQL).

Its job is to explain what OQL is trying to be before implementation details,
parser details, or feature checklists enter the picture.

A language becomes easier to grow well when its principles are written down
clearly enough that future additions can be judged against them.

This document also records why several recent language additions have made Owl
better rather than merely bigger.

## Core Principles Of OQL

### 1. Least effort

The user should write the minimum necessary to express intent.

OQL should reduce:
- repeated joins
- repeated path plumbing
- repeated array boilerplate
- duplicated relationship fields
- repeated projection wiring

If a relationship or shape can be derived naturally, the user should not have to
spell it twice.

### 2. Least surprise

The language should behave the way a thoughtful user expects before they have to
read a manual.

A user should not need to memorize exceptions just to query ordinary data.

Examples of least surprise in practice:
- read traversal should not fail noisily when a path is absent
- a declared reference should naturally support forward navigation
- that same reference should naturally imply reverse navigation
- arrays should not require a second query language

### 3. It must feel like the right way intuitively

A good OQL query should feel inevitable, not clever.

The user should feel:
- “of course this is how I would ask for this”
- not “I learned a trick”

This is one of the deepest tests for OQL syntax. If a form works but feels
unnatural, it is probably not yet right.

### 4. One mental model

Collections, nested arrays, reverse references, and derived related sets should
feel like the same kind of thing.

That means the same core operations should apply consistently across
collection-valued things:
- `where`
- `select`
- `sort`
- `limit`
- `offset`
- `count`
- `group_by`
- `nearest`

OQL should not fracture into mini-languages.

### 5. Object-graph first

OQL is not trying to be table-first with object syntax pasted on top.

It is centered on:
- persistent objects
- references between them
- nested values
- collection-valued relationships
- graph-like traversal

The language should read as though the model itself is alive and navigable.

### 6. Composability over ceremony

Complex work should be built by composing a small number of clear ideas.

OQL should avoid forcing the user into:
- stage-heavy pipelines for ordinary shaping
- repeated relational reconstruction
- multiple declaration forms for the same concept
- extra control syntax where composition would do

The language should grow by deepening a few good concepts rather than by
accumulating many unrelated ones.

### 7. Projection is first-class

Users often want shaped output, not raw rows.

So projection should feel central to the language, not like a final afterthought.

OQL should make it easy to say:
- what data is wanted
- what shape it should take
- which nested pieces should be kept
- which object views should be returned

### 8. Graceful by default, precise when needed

The common case should be calm.

Reads should be safe and smooth by default. Precision tools should exist, but
should not dominate ordinary query writing.

This means:
- null-safe traversal by default
- explicit precision when users need to distinguish null, missing, and broken
  reference states

The language should not make every user pay the price of edge-case precision in
everyday queries.

### 9. Model once, navigate both ways

If a relationship is declared once, users should not have to duplicate it in
multiple schemas just to query it comfortably.

If:

```oql
schema businesses {
  account: Ref(accounts)
}
```

then OQL should naturally support:
- forward navigation: `business.account`
- reverse navigation: `account.businesses`

without forcing a mirrored schema field such as:

```oql
schema accounts {
  businesses: List(Ref(businesses))
}
```

unless the user explicitly wants a stored field with its own meaning.

### 10. The language should remove data-model friction, not expose it

If the model is good, the query should get simpler.

If the model is awkward, the language should still help the user express the
truth of the model clearly, but OQL should not confuse model awkwardness with a
reason to add bad syntax.

A language grows well when it helps good modeling rather than compensating for
bad modeling with noise.

## What Recent Additions Improved In OQL

The recent additions to Owl did not merely add features. They made the language
more unified, more readable, and more Owl-native.

### 1. Scoped query blocks

OQL now has a scoped form such as:

```oql
stores {
  where { .region == "north" }
  sort { .name.asc }
  select { id, name }
}
```

Why this improves the language:
- it reads top-down naturally
- it supports local reusable names without visual clutter
- it keeps `select` focused on shaping output
- it makes serious queries easier to read without changing the core model

This was one of the strongest improvements to OQL.

### 2. Owl equivalence between chained form and scoped-block form

OQL now treats these as equivalent:

```oql
stores.where { .region == "north" }.sort { .name.asc }.select { id, name }
```

and

```oql
stores {
  where { .region == "north" }
  sort { .name.asc }
  select { id, name }
}
```

Why this matters:
- OQL did not split into a simple mode and an advanced mode
- the language stayed unified
- scoped blocks are presentation, not a different query model

That preserves clarity and keeps the language learnable.

### 3. Local scoped bindings

Bindings inside scoped blocks make difficult queries much calmer.

Example:

```oql
users {
  business = .account.businesses.first()

  select {
    firstname,
    lastname,
    business: business.select {
      name,
      vertical,
      ubxdid
    }
  }
}
```

Why this improves the language:
- a derived value is named once and reused naturally
- repeated path noise disappears
- correlated queries become readable without borrowing SQL surface syntax

This is a major step toward mature serious-query ergonomics.

### 4. Arrays and repeated fields treated as collections

Instead of inventing a separate array-query language, OQL now leans into the
idea that repeated fields are simply collection-valued things.

That means patterns like:

```oql
.items.where { .category == "books" }
.items.select { sku, price }
.items.count()
```

Why this improves the language:
- there is no second mental model for arrays
- the same query operators work across more places
- repeated-field queries become more intuitive
- users learn fewer concepts

This is one of the cleanest and most important OQL decisions.

### 5. Null-safe traversal by default

Reads now lean toward graceful failure by returning `null` instead of exploding
when a path cannot be fully traversed.

Why this improves the language:
- common reads feel humane
- graph traversal becomes safer
- users write fewer defensive guards
- the language feels less hostile without becoming vague

This is a major quality-of-life improvement.

### 6. Explicit existence and precision semantics

Alongside graceful defaults, OQL has become clearer about the need for explicit
predicates such as:
- `exists(...)`
- `is_null(...)`
- `is_missing(...)`
- `is_broken_ref(...)`

Why this improves the language:
- ordinary reads stay smooth
- advanced reasoning remains possible
- precision does not pollute the common case

This is a good example of balanced language design.

### 7. Object-valued `select`

A bound object value can now be projected directly:

```oql
business.select {
  name,
  vertical,
  ubxdid
}
```

instead of forcing a more manual shape like:

```oql
{
  name: business.name,
  vertical: business.vertical,
  ubxdid: business.ubxdid
}
```

Why this improves the language:
- less repetition
- more natural object shaping
- `select` remains the central projection operation
- collection-valued and object-valued projection become more unified

This is a very strong OQL addition.

### 8. Implicit reverse traversal

If a schema field declares a reference:

```oql
schema businesses {
  account: Ref(accounts)
}
```

then OQL can naturally support reverse traversal from an account:

```oql
.account.businesses
```

Why this improves the language:
- users do not need mirrored reverse-list schema fields
- relationships are modeled once, not twice
- graph traversal becomes more natural
- schema design stays cleaner

This is one of the most Owl-native improvements to the language.

### 9. Clearer separation between model issues and language issues

Recent work also improved the way Owl is reasoned about.

We became more careful not to treat every heavy query as proof that syntax is
wrong. Sometimes the true issue is that the data model is not yet Owl-native.

Why this improves the language:
- it protects OQL from unnecessary syntax growth
- it encourages better modeling instead of compensatory noise
- it makes the language more disciplined over time

This is a quieter improvement, but an important one.

## What These Additions Achieved Together

Taken together, the recent additions made OQL:
- more unified
- more intuitive
- more graph-native
- less repetitive
- less hostile on ordinary reads
- more capable for serious projection-heavy and correlated queries

Most importantly, they pushed OQL closer to being the natural expression of
Owl's data model rather than a borrowed syntax sitting on top of it.

## Final View

OQL is strongest when it does not try to impress by adding many features, but by
making a few good ideas work everywhere they should.

The recent additions have been good because they did not merely increase
capability. They reduced friction while preserving and often improving power.

That is the right direction for the language:
- fewer concepts
- better concepts
- stronger defaults
- cleaner serious queries
- one coherent mental model

That is what should continue to guide Owl going forward.
