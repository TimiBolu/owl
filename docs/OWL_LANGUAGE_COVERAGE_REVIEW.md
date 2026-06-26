# Owl Language Coverage Review

## Purpose

This document compares two internal design artifacts:

- `/Users/timiadesina/Documents/owl/docs/OWL_LANGUAGE_DESCRIPTION.md`
- `/Users/timiadesina/Documents/owl/docs/REFERENCE_WORKLOAD_SHAPES.md`

Its purpose is to answer:

> which serious workload shapes already have a good language story in Owl, and which
> still require language design decisions?

This is not an implementation review. It is a language-coverage review.

## Reading Guide

Each workload shape is placed into one of three buckets:

- `Strongly covered`
- `Partially covered`
- `Not yet properly covered`

The judgment here is about the language description, not whether the engine currently
executes everything.

## 1. Strongly Covered By The Current Owl Description

These are the areas where Owl now has a clear and coherent language story.

## A. Collection composition

Workload shapes covered:

- filter -> summarize -> reshape
- staged summarization via bindings
- entity -> related collection -> summary
- entity -> related collection -> filtered nested output

Why coverage is strong:

- Owl now explicitly treats collection-valued things as one family
- bindings are first-class
- nested collection projection is explicit
- collection operations are small, stable, and composable

This is one of the strongest parts of the current description.

## B. Nested repeated-field analysis

Workload shapes covered:

- repeated-field analysis
- projection-heavy shaping over nested components
- array-backed summary patterns like counts and filtered nested output

Why coverage is strong:

- arrays are intentionally described as collections
- the document rejects a separate array mini-language
- examples already show `.items.where { ... }.select { ... }` and `.items.count()`

This is a major design win because it answers a serious workload need without growing a
second language.

## C. Forward traversal

Workload shapes covered:

- relationship traversal
- entity -> related entity projection
- deep chained access through refs and nested values

Why coverage is strong:

- refs are first-class values
- field access is uniform
- forward traversal reads directly and naturally

## D. Reverse traversal

Workload shapes covered:

- entity -> reverse-related collection -> summary
- entity -> reverse-related collection -> filtered nested output

Why coverage is strong:

- reverse relationships are named in the mental model
- reverse traversal is explicitly described as collection-valued
- examples like `.findings.where { ... }.count()` are already natural in the spec

This is one of Owl's most important differentiators.

## E. Correlated nested queries

Workload shapes covered:

- correlated nested query
- local-scope filtering against outer entity context
- scoped structured + vector retrieval

Why coverage is strong:

- scope anchors are explained
- nested query expressions are already part of the described language
- this matches a recurring serious workload shape rather than an edge case

## F. Projection-heavy output shaping

Workload shapes covered:

- projection-heavy output shaping
- nested projection
- aliasing and computed fields
- shaping nested related output

Why coverage is strong:

- `select` has a central role in the Owl language description
- nested projection is already treated as normal
- this aligns well with both app queries and document workloads

## G. Transaction blocks

Workload shapes covered:

- multi-statement mutation units
- application-friendly atomic operations

Why coverage is strong:

- `transaction { ... }` is clearly positioned as the primary language form
- it is readable, intuitive, and Owl-native

This is one of the cleanest parts of the spec.

## H. Hybrid structured + semantic retrieval

Workload shapes covered:

- vector nearest within structured scope
- shaped projection of vector results
- graph/ref-scoped vector retrieval as a language direction

Why coverage is strong:

- vector fields are part of the type system
- `nearest` is described as a collection operator
- the language description already treats vector querying as integrated rather than
  bolt-on

## 2. Partially Covered By The Current Owl Description

These are areas where Owl has the beginning of a good story, but the description still
needs sharper decisions.

## A. Existence and anti-existence logic

Workload shapes covered only partially:

- anti-join style reasoning
- at-least-one / no-related-rows queries
- nested collection presence checks

What is good already:

- the document recognizes `exists(...)`
- it shows collection-based presence checks using `.count() > 0`

What is now settled at the language-design level:

- `exists(path)` should check structural path existence
- `collection.exists()` should check cardinality of a collection-valued thing
- ordinary reads remain graceful, while explicit predicates provide precision

What still remains for later tightening:

- exact boolean interaction rules in complex predicates
- final wording and examples in the main reference text

## B. Null vs missing semantics

Workload shapes covered only partially:

- field absence checks
- null-aware filtering
- broken traversal behavior

What is now settled at the language-design level:

- read traversal should be null-safe by default
- null, missing, and broken-reference states are distinct concepts
- ordinary reads collapse unresolved states to `null`
- explicit predicates should exist for exactness

What still remains for later tightening:

- exact boolean truth rules for null-like values in complex predicates
- final aggregate edge-case wording
- final update-path creation rules for missing object intermediates

## C. Top-k within scope

Workload shapes covered only partially:

- latest N related rows
- top-N nested items
- top-N semantic results per enclosing scope

What is good already:

- `sort`, `limit`, `first`, and correlation are all present

What is still unresolved:

- there is no single dedicated narrative for “top-k within subgroup”
- ranking semantics beyond basic sorting and limiting are not described yet

This is serviceable already, but not yet fully articulated.

## D. Time-windowed reporting

Workload shapes covered only partially:

- date filtering
- recency sorting
- time-oriented grouped reporting

What is good already:

- `DateTime` exists in the type system
- sorting and filtering are present
- duration-like expression style is hinted at

What is still unresolved:

- clearer date literal conventions
- bucket-style time grouping story
- period comparison idioms

The language description recognizes time as important, but does not yet fully teach it.

## E. Nested and partial updates

Workload shapes covered only partially:

- nested field updates
- repeated-field transformation during update
- ergonomic partial mutation

What is good already:

- the document has a clear direction away from Mongo-style operator sprawl
- nested dot-path update and projection-style repeated-field update are described

What is still unresolved:

- the final syntax contract
- how much update sugar is acceptable
- how list-element mutation should be described in the smallest core

This is promising, but still unsettled.

## F. Distinct / dedup semantics

Workload shapes covered only partially:

- unique grouped values
- deduplicated projection patterns

What is good already:

- the document recognizes this as an open question

What is still unresolved:

- whether distinct is first-class
- whether grouping is the preferred way to express dedup

## 3. Not Yet Properly Covered By The Current Owl Description

These are serious workload shapes that the current Owl description acknowledges only
weakly or not at all as language forms.

## A. Partition-aware ranking

Workload shapes not yet properly covered:

- row number within subgroup
- nth item per parent group
- percentile or dense-rank style reasoning

Current state:

- the need is recognized in the workload review
- the Owl language description does not yet provide a settled language form

This should influence long-term capability, but it is not yet a language decision.

## B. Multi-dimensional rollups and hierarchy analytics

Workload shapes not yet properly covered:

- rollup
- cube-style aggregation
- one-query multi-level summaries

Current state:

- the need is acknowledged
- the spec intentionally avoids prematurely copying SQL constructs such as `ROLLUP`

That is reasonable, but it means the coverage is currently not strong.

## C. Advanced bucketing families

Workload shapes not yet properly covered:

- histogram-like grouping
- fixed-range bucketing
- category bucket summarization

Current state:

- the workload review names bucketing as a real need
- the language description does not yet offer an Owl-native answer

## D. Very large warehouse-style analytical forms

Workload shapes not yet properly covered:

- report-oriented subquery towers
- cross-dimensional warehouse analysis
- highly parameterized benchmark-style analytical queries

Current state:

- the language has pieces that may express parts of these workloads
- the Owl description does not yet present a dedicated analytical chapter or strategy

This is acceptable for now, but should remain explicit.

## E. Output-shaping exclusion semantics

Workload shapes not yet properly covered:

- inclusion vs exclusion projection control
- projection variants common in document systems

Current state:

- Owl's `select` is strong for constructive shaping
- it does not yet say much about omission-driven projection styles

This is less urgent than the other gaps, but worth noting.

## 4. Main Conclusions

## A. Owl already has a powerful center of gravity

The strongest part of the current description is not any single keyword.
It is the unified model of:

- collection-valued things,
- nested shaping,
- traversal,
- bindings,
- transactions.

That is a very solid center for the language.

## B. The arrays-as-collections decision is heavily validated

The external workload review strongly supports the decision to avoid a separate array
query language.

Repeated-field analysis is a real workload, not a toy edge case.
Treating repeated fields as collections gives Owl a clean answer.

## C. The next most important language work is semantic tightening, not feature sprawl

The biggest gaps are not “missing 50 keywords”.
They are:

- null vs missing semantics
- existence / anti-existence semantics
- nested update contract
- distinct / dedup contract
- top-k and ranking boundary

These deserve careful language work because they influence many other features at once.

## D. Owl should resist analytical syntax inflation in the core language

Rollups, ranking, and advanced bucketing are real needs.
But the current review suggests they should shape Owl's long-term capability more than
its smallest core syntax right now.

That is consistent with least effort and least surprise.

## 5. Recommended Next Step

The best next design pass is not “add more syntax”.
It is to settle the ambiguous semantics in the existing model.

Recommended order:

1. null vs missing vs broken traversal
2. existence and anti-existence forms
3. nested and repeated-field update semantics
4. distinct / dedup expression
5. top-k and ranking boundary

Once those are settled, Owl's language description will be much harder to accidentally
fragment.
