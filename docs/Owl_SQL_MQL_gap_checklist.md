# Owl Vs PostgreSQL Vs MongoDB Gap Checklist

This is a mechanical checklist for comparing Owl against PostgreSQL SQL and
MongoDB MQL.

Use this as a feature inventory, not as a marketing comparison.

Legend:

- `Y` = supported
- `P` = partially supported
- `N` = not supported
- `?` = unclear, needs verification

## Inputs

PostgreSQL reference material:

- `docs/reference_grammars/postgres/gram.y`
- `docs/reference_grammars/postgres/scan.l`
- `docs/reference_grammars/postgres/sql-syntax.html`
- `docs/reference_grammars/postgres/sql-language.html`

MongoDB reference material:

- `docs/reference_grammars/mongodb/mql-reference.html`
- `docs/reference_grammars/mongodb/query-operators.html`
- `docs/reference_grammars/mongodb/aggregation-quick-reference.html`
- `docs/reference_grammars/mongodb/aggregation-operators.html`
- `docs/reference_grammars/mongodb/update-operators.html`

Owl implementation surface:

- `src/parser.rs`
- `src/executor.rs`
- `src/storage.rs`
- `src/schema.rs`
- `src/planner/`
- `src/bin/owl-explorer/api/`

## 1. Core Query Shape

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| top-level select/query statement | Y | Y |  |  |
| chaining/pipeline query syntax | P | Y |  |  |
| named reusable query fragments | P | P |  |  |
| function definitions | P | N |  |  |
| query parameters/bindings | Y | Y |  |  |
| comments | Y | Y |  |  |

## 2. DDL / Schema

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| create collection/table | Y | Y |  |  |
| alter schema | Y | P |  |  |
| drop collection/table | Y | Y |  |  |
| typed fields | Y | P |  |  |
| nested object types | P | Y |  |  |
| reference field type | P | P |  |  |
| vector field type | P | P |  |  |
| optional fields | Y | Y |  |  |

## 3. DML / Mutation

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| insert single row/document | Y | Y |  |  |
| batch insert | Y | Y |  |  |
| update with predicate | Y | Y |  |  |
| delete with predicate | Y | Y |  |  |
| upsert | Y | Y |  |  |
| insert returning inserted rows | Y | P |  |  |
| update returning updated rows | Y | P |  |  |
| delete returning deleted rows | Y | P |  |  |

## 4. Filtering

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| equality / inequality | Y | Y |  |  |
| numeric comparisons | Y | Y |  |  |
| boolean logic | Y | Y |  |  |
| null checks | Y | Y |  |  |
| membership / in | Y | Y |  |  |
| between | Y | P |  |  |
| pattern matching | Y | Y |  |  |
| regex | P | Y |  |  |
| exists / field existence | Y | Y |  |  |

## 5. Projection / Shape Control

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| field selection | Y | Y |  |  |
| aliases | Y | P |  |  |
| computed expressions | Y | Y |  |  |
| object construction | P | Y |  |  |
| array construction | Y | Y |  |  |
| nested projection | P | Y |  |  |

## 6. Navigation / Relationships

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| explicit joins | Y | P |  |  |
| reference traversal | P | P |  |  |
| reverse reference traversal | P | N |  |  |
| deep chained reference traversal | P | N |  |  |
| nested property access | Y | Y |  |  |
| graph-native prefilter execution | N | N |  |  |

## 7. Aggregation

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| count/sum/avg/min/max | Y | Y |  |  |
| group by | Y | Y |  |  |
| having | Y | P |  |  |
| sort by aggregate | Y | Y |  |  |
| distinct | Y | Y |  |  |
| bucket/facet style analytics | P | Y |  |  |
| array unwind style stage | N/A | Y |  |  |
| window functions | Y | N |  |  |

## 8. Ordering / Pagination

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| order by / sort | Y | Y |  |  |
| limit | Y | Y |  |  |
| offset / skip | Y | Y |  |  |
| cursor pagination | Y | Y |  |  |

## 9. Indexing

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| scalar secondary indexes | Y | Y |  |  |
| composite indexes | Y | Y |  |  |
| unique indexes | Y | Y |  |  |
| nested-path indexes | P | Y |  |  |
| ref-path / reverse-edge indexes | N | N |  |  |
| vector indexes | P | P |  |  |
| index lifecycle/status APIs | P | P |  |  |

## 10. Transactions / Correctness

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| begin/commit/rollback | Y | Y |  |  |
| multi-operation atomicity | Y | Y |  |  |
| rollback on failure | Y | Y |  |  |
| isolation levels | Y | P |  |  |
| durability guarantees | Y | Y |  |  |

## 11. Update Operators / Mutation Semantics

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| partial field update | Y | Y |  |  |
| nested field update | P | Y |  |  |
| array element update | P | Y |  |  |
| increment/decrement operators | Y | Y |  |  |
| set/unset semantics | P | Y |  |  |

## 12. Search / AI / Specialized

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| vector nearest query syntax | P | P |  |  |
| ANN index support | P | P |  |  |
| full text search | Y | Y |  |  |
| geospatial query support | Y | Y |  |  |
| hybrid graph + vector query | N | N |  |  |

## 13. Operational / Explainability

| Feature | PostgreSQL | MongoDB | Owl | Notes |
|---|---|---|---|---|
| explain plan | Y | Y |  |  |
| index usage visibility | Y | Y |  |  |
| graph-native prefilter visibility | N | N |  |  |
| backup / restore commands | Y | Y |  |  |

## Suggested next pass

For each row, fill in Owl with:

- syntax support
- executor support
- storage/index support
- known bugs
- tests present or absent

That gives us a grounded roadmap instead of vague “missing SQL features” lists.

