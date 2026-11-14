# Improved OQL Syntax Guide

This guide documents the currently implemented improved syntax primitives.

## 1) Query Bindings

You can define reusable query pipelines at file scope:

```oql
openFindings = findings
    .where { .workflow.status == "OPEN" || .workflow.status == "ONGOING" }
```

Then reuse them as query roots:

```oql
openFindings
    .select { id, severity }
    .limit(20)
```

Notes:
- Binding names must be valid identifiers (e.g. `openFindings`).
- Bindings can chain from other bindings.
- Cyclic bindings are rejected.

## 2) Query Expressions Inside `select`

You can embed query chains as expressions:

```oql
users.select {
    id,
    open_count: findings.where { .status == "open" }.count()
}
```

Current behavior:
- Terminal `.count()` returns an `Integer`.
- Non-count query expressions return a list-like value (array of object rows).

## 3) Correlation via Collection Name Scope Anchors

For correlated nested query expressions, use the enclosing collection name as scope anchor:

```oql
companies.select {
    id,
    finding_count: findings.where { .company_id == companies.id }.count()
}
```

Semantics:
- `collection.field` means “field from the current row of the enclosing `collection` scope”.
- `collection.<path>` is resolved before the nested query executes.
- This keeps scoping explicit and avoids ambiguity between inner and outer fields.

Current restrictions:
- `collection.<path>` must resolve to a scalar literal (`String`, `Integer`, `Float`, `Boolean`, `DateTime`, `Reference`).
- Null or non-scalar scoped values in correlated predicates currently return an execution error.
- Legacy `outer.<path>` is not supported.

## 4) Method-Style Calls on Expressions

Method-style chaining on expressions is supported:

```oql
companies.select {
    days_in_assessment: (.timeline.assessment.entered_at || .created_at).days_until(now())
}
```

This is parsed as a function call where the left expression becomes the first argument.

## 5) Boolean Function Bodies

Functions returning `Bool`/`Boolean` accept condition-style bodies:

```oql
fn is_active_company(c: companies) -> Bool {
    c.status == "ASSESSMENT_REQUIRED" || c.status == "RE_ASSESSMENT_REQUIRED"
}
```

This avoids forcing ternary wrappers for simple predicates.

## 6) Recommended Style

- Prefer function-first composition for reusable predicate logic.
- Prefer bindings for reusable pipelines.
- Use collection-name scope anchors (for example `companies.id`) for correlated subquery semantics.
- Avoid implicit correlation assumptions.

## 7) Minimal End-to-End Example

```oql
schema users {
    id: String,
    name: String
}

schema findings {
    id: String,
    owner_id: String,
    status: String
}

openFindings = findings.where { .status == "open" }

users.select {
    id,
    name,
    open_count: openFindings.where { .owner_id == users.id }.count()
}
```
