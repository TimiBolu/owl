# Reference Grammars And Language Surfaces

This folder vendors the external reference material we will use for mechanical
comparison against Owl.

## PostgreSQL

PostgreSQL exposes a real parser grammar in source form.

Files:

- `postgres/gram.y`
- `postgres/scan.l`
- `postgres/sql-language.html`
- `postgres/sql-syntax.html`
- `postgres/parser-stage.html`

Interpretation:

- `gram.y` is the closest thing to the authoritative SQL grammar for PostgreSQL.
- `scan.l` is the lexical scanner and matters for keyword/token behavior.
- the HTML docs are the official syntax/reference surface for user-facing SQL.

## MongoDB

MongoDB does not publish one single BNF-style public grammar equivalent to
PostgreSQL's `gram.y`.

Instead, the authoritative MQL surface is spread across reference sections.

Files:

- `mongodb/mql-reference.html`
- `mongodb/query-operators.html`
- `mongodb/aggregation-quick-reference.html`
- `mongodb/aggregation-operators.html`
- `mongodb/update-operators.html`

Interpretation:

- use these as the official MongoDB language surface
- compare Owl against MQL by feature inventory, not by parser-file diff alone

## Intended use

These files are vendored so Owl feature-gap analysis can be:

1. repeatable
2. local-first
3. grounded in official source material

The recommended analysis flow is:

1. classify each feature by category
2. mark whether Owl supports syntax
3. mark whether Owl supports execution semantics
4. mark whether support is partial, unstable, or missing

