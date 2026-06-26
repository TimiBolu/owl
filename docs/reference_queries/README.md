# Reference Query Corpora

This folder collects serious external query corpora and reference examples that are
useful for evolving Owl's language description.

## Why these are here

We need more than toy queries. We need examples of:

- analytical SQL with real complexity
- production-style joins, grouping, windows, and time analysis
- document-oriented aggregation pipelines that do substantial shaping work
- benchmark-grade workloads that expose the pressure points of a language

These materials are not Owl examples. They are design pressure.

## SQL sources

### 1. TPC-DS specification

File:
- `sql/TPC-DS_v4.0.0.pdf`

Use it for:
- classical warehouse analytics
- heavy grouping and filtering
- subqueries and reporting patterns
- understanding what mature analytical query surfaces must eventually explain well

### 2. DSB (Decision Support Benchmark)

Source:
- `sql/dsb/`

Use it for:
- modernized TPC-DS style analytical workloads
- query templates split into single-block and multi-block forms
- pressure-testing Owl's description of grouping, correlation, and complex filtering

### 3. jaffle-shop

Source:
- `sql/jaffle-shop/`

Use it for:
- realistic analytics engineering patterns
- models and transformations people actually maintain
- seeing how business logic grows around SQL in real projects

This is less extreme than TPC-DS or DSB, but often more relatable.

### 4. Join Order Benchmark (JOB)

Source:
- `sql/join-order-benchmark/`

Use it for:
- real, join-heavy SQL over IMDb-style data
- correlated predicates and multi-join pressure
- studying where mature relational systems earn their keep

This is especially useful when asking where Owl should stay graph-native and where it
must eventually match relational execution discipline.

## MongoDB sources

### 1. Aggregation pipeline core

File:
- `mongodb/aggregation-pipeline-core.html`

Use it for:
- the main mental model MongoDB uses for aggregation
- understanding stage-oriented document transformations

### 2. Zip code aggregation example

File:
- `mongodb/aggregation-zip-code-data-set.html`

Use it for:
- grouping and reshaping examples that are more realistic than toy CRUD

### 3. Unpack arrays example

File:
- `mongodb/aggregation-unpack-arrays.html`

Use it for:
- serious repeated-field and document reshaping behavior
- understanding what users expect to do with nested repeated data

## Recommended use in Owl design

These materials are most useful when asking:

1. what real users actually need to express,
2. where Owl should match capability,
3. where Owl should deliberately differ in syntax,
4. how to keep Owl small while still covering serious workloads.

The key rule is:

> do not copy syntax mechanically; extract the user need and restate it in Owl's own
> mental model.
