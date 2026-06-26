# Reference Workload Shapes

## Purpose

This document extracts recurring query shapes from serious external SQL and MongoDB
reference corpora.

It is intentionally not a syntax comparison.

Its job is to answer a narrower and more useful question:

> what kinds of work do real query languages have to express once the workload is no
> longer toy-sized?

This gives Owl a clean design input.

We can then decide separately:

- which workload shapes should be first-class in Owl,
- which should be expressible without new core syntax,
- which should not determine Owl's core design.

## Source Material

Local sources reviewed:

### SQL

- `/Users/timiadesina/Documents/owl/docs/reference_queries/sql/TPC-DS_v4.0.0.pdf`
- `/Users/timiadesina/Documents/owl/docs/reference_queries/sql/dsb/`
- `/Users/timiadesina/Documents/owl/docs/reference_queries/sql/join-order-benchmark/`
- `/Users/timiadesina/Documents/owl/docs/reference_queries/sql/jaffle-shop/`

### MongoDB

- `/Users/timiadesina/Documents/owl/docs/reference_queries/mongodb/aggregation-pipeline-core.html`
- `/Users/timiadesina/Documents/owl/docs/reference_queries/mongodb/aggregation-zip-code-data-set.html`
- `/Users/timiadesina/Documents/owl/docs/reference_queries/mongodb/aggregation-unpack-arrays.html`

## A First Important Distinction

The materials fall into two different families.

### 1. Workloads that are complex because they summarize large populations

These are analytical and reporting shapes.

Typical pressure points:

- grouping
- date filtering
- rollups
- ranking
- top-k
- subquery thresholds
- distribution analysis

### 2. Workloads that are complex because they navigate or reshape rich structure

These are navigational, document, or graph-adjacent shapes.

Typical pressure points:

- nested fields
- repeated fields
- relationship traversal
- reverse traversal
- object shaping
- correlated nested filtering

Owl should not respond to both families with the same language moves.

## Recurrent SQL Workload Shapes

## 1. Staged summarization

Very common in `jaffle-shop` and analytical SQL generally.

Pattern:

1. derive one or more intermediate summaries,
2. join them back to a base entity,
3. project enriched output.

Example shape:

- summarize orders per customer,
- then join that summary back to customers,
- then classify customers as new or returning.

This is not just a query trick. It is a recurring way people think.

### What users are trying to do

- compute rollups at one level,
- attach them to another level,
- derive business classifications from those rollups.

### Why it matters for Owl

Owl should have a clean way to express:

- reusable intermediate bindings,
- nested summaries,
- projecting derived metrics back into entity-shaped results.

Bindings already look like the right Owl-native answer here.

## 2. Aggregate summary joined back to entity output

A closely related but distinct pattern.

Example from `jaffle-shop`:

- summarize order items per order,
- join those aggregate results back to orders,
- derive booleans like `is_food_order` and `is_drink_order`.

### What users are trying to do

- enrich each main row with rolled-up facts computed over related rows.

### Why it matters for Owl

This suggests Owl should be strong at expressions like:

- `.items.count()`
- `.findings.where { ... }.count()`
- derived booleans from nested collection summaries

This is one of the strongest arguments for collection-valued semantics over special array
syntax.

## 3. Ranking within partitions

Seen clearly in analytical SQL patterns such as `row_number() over (partition by ...)`.

### What users are trying to do

- order rows within a subgroup,
- identify first, latest, highest, lowest, nth,
- derive sequence position or cohort position.

### Why it matters for Owl

This is a real workload shape, even if Owl chooses not to copy SQL window syntax.

Owl still needs a language story for:

- first within group,
- latest related row,
- ordering within a collection-valued field or reverse relationship,
- maybe later partition-aware ranking.

This should influence Owl's description, even if not yet its full core syntax.

## 4. Correlated threshold queries

Seen in DSB and TPC-style queries.

Pattern:

- compute a grouped value,
- compare one entity's value against a summary computed over related peers,
- keep entities above or below the threshold.

Example shape:

- customers whose returns are more than 1.2x the store average.

### What users are trying to do

- compare a row or group to its peer population,
- identify outliers, high performers, anomalies, or exceptions.

### Why it matters for Owl

Owl needs a clean story for:

- grouped bindings,
- correlated nested aggregates,
- comparing local entity context to peer-group summaries.

This is more important than copying SQL's exact subquery surface.

## 5. Multi-dimensional rollups

Seen in DSB/TPC-style queries with `rollup` and grouped geographic or product hierarchies.

### What users are trying to do

- summarize data at multiple levels at once,
- such as:
  - item
  - county
  - state
  - country

### Why it matters for Owl

This is a hard analytical shape.

It does not mean Owl must copy `ROLLUP` syntax directly.
But it does mean the language description should at least acknowledge hierarchical
aggregation as a serious need.

This belongs in Owl's long-term analytical story.

## 6. Join-heavy existence filtering

Seen strongly in the Join Order Benchmark.

Pattern:

- many joins,
- many equality predicates,
- one or two very selective filters,
- final output is often small or aggregated.

### What users are trying to do

- find entities that satisfy a precise intersection of relationships and predicates.

### Why it matters for Owl

This is where Owl should ask a design question, not just an implementation question:

- should the user be writing join ceremony,
- or should the language let them traverse and filter across related collections more
  directly?

In other words, this is a pressure point that argues for Owl-native relationship syntax,
not necessarily for SQL-style join syntax.

## 7. Anti-join and existence logic

Very common in serious SQL even when not obvious at first glance.

Pattern:

- keep rows that do or do not have related rows matching some condition.

Examples:

- customers with no recent refunds,
- orders with at least one chargeback,
- products with no inventory in a region.

### What users are trying to do

- express existence and absence cleanly.

### Why it matters for Owl

Owl needs first-class and readable existence semantics.

This becomes even more important if array-valued fields and reverse refs are both meant
to behave like collections.

## 8. Time-windowed reporting

Common in TPC-DS and real analytics models.

Pattern:

- filter by date ranges,
- group by time buckets,
- compare periods,
- order by recency.

### What users are trying to do

- build reports over time.

### Why it matters for Owl

The language should treat time as a first-class everyday concern, not a niche one.

That affects:

- date literal clarity,
- bucket-friendly grouping,
- sorting by recency,
- expressions like latest, earliest, age, duration.

## Recurrent MongoDB Workload Shapes

## 9. Stage-by-stage document reshaping

MongoDB aggregation examples are less about relational joining and more about gradual
reshaping.

Pattern:

1. filter documents,
2. unpack or reshape nested structure,
3. regroup,
4. project output shape.

### What users are trying to do

- transform nested documents into the shape needed for reporting or UI output.

### Why it matters for Owl

The need is real, but Owl does not have to adopt pipeline-stage syntax literally.

The deeper requirement is:

- nested values must be easy to shape,
- repeated values must be easy to treat as collections,
- regrouping after reshaping must remain understandable.

## 10. Repeated-field analysis

The `unpack arrays` style examples matter a lot.

Pattern:

- a field contains repeated values,
- the query needs to inspect, filter, summarize, or regroup those values.

### What users are trying to do

- answer questions about components inside a document, not just about the document as a
  whole.

### Why it matters for Owl

This is exactly where Owl should lean on collection-level semantics for array-valued
fields.

Instead of treating arrays as a special alien type, Owl can let the user work with them
as nested collections.

That may be one of Owl's cleanest advantages if designed consistently.

## 11. Projection-heavy output shaping

MongoDB examples emphasize that many real workloads are not just about filtering. They
are about returning the right output shape.

### What users are trying to do

- rename fields,
- nest output,
- exclude or include selected fields,
- compute small derived values,
- restructure data for clients or reports.

### Why it matters for Owl

Projection is not a secondary feature. It is a core language responsibility.

Owl's `select` block should be treated as one of the most important parts of the
language, not as an afterthought.

## 12. Multi-step grouping over reshaped data

MongoDB often makes users:

- unpack,
- regroup,
- project,
- sort.

### What users are trying to do

- move between entity-level and component-level views of the same data.

### Why it matters for Owl

Owl should support this behavior, but ideally through one small model:

- a collection can produce another collection,
- nested collections can be filtered and projected,
- grouped results can be shaped again.

This reinforces the decision to avoid separate sublanguages where possible.

## Cross-Cutting Workload Shapes

The most important shapes appear in both SQL and MongoDB worlds, even though the syntax
looks different.

## 13. Filter -> summarize -> reshape

This may be the most common serious query shape overall.

Examples:

- filter orders for a time window,
- group by customer or region,
- return shaped summary rows.

### Why it matters for Owl

This should feel natural and boring in Owl. It is too common to feel advanced.

## 14. Entity -> related collection -> summary

This shape appears everywhere once data has relationships or repeated values.

Examples:

- customer -> orders -> total spend,
- order -> items -> item count,
- company -> findings -> open critical count.

### Why it matters for Owl

This is one of Owl's best native opportunities.

It fits perfectly with:

- refs,
- reverse refs,
- arrays as collections,
- nested `select` and `count`.

## 15. Entity -> related collection -> filtered nested output

Examples:

- order -> expensive items,
- company -> high-risk findings,
- user -> recent notifications.

### Why it matters for Owl

This supports a very application-facing style of query that SQL often makes verbose and
Mongo often solves through document-shaping pipelines.

Owl can make it feel direct.

## 16. Correlated nested query

Examples:

- for each company, count findings related to that company,
- for each org, return the top similar findings inside that org.

### Why it matters for Owl

This is the workload shape behind scope anchors and nested query expressions.

It is not a niche feature. It is a recurring serious pattern.

## 17. Top-k within scope

Examples:

- latest 5 tasks for each project,
- top 3 orders for each customer,
- top 10 nearest vectors inside one organization.

### Why it matters for Owl

This combines:

- local scope,
- ordering,
- limiting,
- sometimes vector search.

It is exactly the kind of shape Owl should express elegantly.

## 18. Hybrid structured + semantic retrieval

This is newer than the other workload families, but it matters for Owl specifically.

Examples:

- documents in team X, nearest to query vector Y,
- findings in org Z, filtered by status and ranked by embedding similarity.

### Why it matters for Owl

This is a workload where Owl can become more than “SQL plus vectors” or “documents plus
vectors”.

It combines:

- structured filtering,
- graph or ref scoping,
- vector nearest search,
- shaped projection.

This should be treated as a first-class long-term workload shape.

## What These Workloads Suggest For Owl

These workload shapes point toward a few stable language needs.

## A. Owl must be excellent at collection composition

Because so many serious workloads can be restated as:

- start with a collection,
- filter it,
- summarize it,
- shape it,
- reuse it.

Bindings and nested collection operations are central, not ornamental.

## B. Arrays and reverse relationships should reuse collection semantics

The MongoDB materials reinforce this strongly.

Users want to ask serious questions about repeated nested data.

Owl should meet that need by extending the same collection operations into nested
collection-valued fields rather than inventing a separate language.

## C. Projection is as important as filtering

Real users often care just as much about output shape as selection criteria.

That means `select` deserves equal weight in the language description.

## D. Correlation matters

Many serious workloads depend on expressing something relative to an enclosing entity or
peer group.

That means Owl needs clear, readable correlation semantics.

## E. Analytics pressure is real, but should not force syntax imitation

TPC-DS and DSB show real analytical pressure.

Owl should respect that pressure, but respond in Owl-native ways where possible.

For example:

- bindings instead of large subquery scaffolding,
- collection-valued nested summaries instead of ceremony-heavy joins,
- concise transaction blocks instead of command-oriented control flow,
- direct nested collection semantics instead of array-specific sublanguages.

## F. Some workload shapes should inform long-term capability without distorting the core language

Examples:

- rollups,
- advanced ranking,
- bucketing,
- very large analytical reporting forms.

These are real needs, but not every serious need belongs in Owl's smallest core.

## Proposed Classification For The Next Step

We should use these workload shapes in the next Owl design pass under three headings.

### 1. Should be first-class in Owl

Likely includes:

- collection filtering and shaping
- nested collection semantics
- forward and reverse traversal
- correlated nested queries
- grouped summaries
- transaction blocks
- vector nearest within structured scope

### 2. Should be expressible in Owl without growing the core too much

Likely includes:

- existence and anti-existence patterns
- top-k within scope
- staged summarization via bindings
- nested repeated-field analysis
- moderate analytical reporting

### 3. Should influence long-term capability, but not necessarily core syntax now

Likely includes:

- rollup/cube-style hierarchy analytics
- full partition-ranking semantics
- advanced bucketing families
- very large warehouse-style analytical constructs

## Summary

The serious external workloads do not mainly teach us which keywords Owl is missing.

They teach us which kinds of thinking real users return to over and over:

- summarize related things,
- compare an entity to its peers,
- reshape nested data,
- analyze repeated values,
- traverse relationships,
- rank within a local scope,
- combine structured filters with semantic retrieval.

That is the right raw material for the next refinement of Owl's language description.
