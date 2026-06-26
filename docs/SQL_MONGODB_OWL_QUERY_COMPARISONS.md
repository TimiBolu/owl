# SQL, MongoDB, And Owl Query Comparisons

## Purpose

This document compares serious query shapes across SQL, MongoDB, and Owl.

It is not meant to prove that Owl should copy either of them. It is meant to do three
things:

1. show what real workloads look like in mature systems,
2. show what an Owl-native expression of the same workload could look like,
3. identify where Owl is already stronger, and where SQL still has a clearer mature
   language that Owl should answer with simpler, cleaner design.

This document is language-focused. It is about clarity of expression and fitness of the
surface language, not execution benchmarks.

## Design Rule For These Comparisons

The right question is not:

> how do we translate this syntax into Owl?

The right question is:

> what is the user trying to express, and what is the smallest Owl-native way to say it?

That matters especially in places where SQL is mature but verbose, or MongoDB is capable
but stage-heavy.

A second rule also matters now:

> in Owl, chained query syntax and scoped block syntax are equivalent forms of the same
> language.

That means the task is not to invent a second query mode. It is to decide when scoped
presentation improves readability.

## 1. Entity Enriched By Related Summary

### What the query is doing

- start from one entity type,
- summarize related rows,
- attach those summaries back to each entity,
- project the enriched result.

### SQL

```sql
with customer_orders_summary as (
  select
    orders.customer_id,
    count(distinct orders.order_id) as count_lifetime_orders,
    min(orders.ordered_at) as first_ordered_at,
    max(orders.ordered_at) as last_ordered_at,
    sum(orders.order_total) as lifetime_spend
  from orders
  group by 1
)
select
  customers.*,
  customer_orders_summary.count_lifetime_orders,
  customer_orders_summary.first_ordered_at,
  customer_orders_summary.last_ordered_at,
  customer_orders_summary.lifetime_spend
from customers
left join customer_orders_summary
  on customers.customer_id = customer_orders_summary.customer_id;
```

### MongoDB

```javascript
db.orders.aggregate([
  {
    $group: {
      _id: "$customer_id",
      count_lifetime_orders: { $sum: 1 },
      first_ordered_at: { $min: "$ordered_at" },
      last_ordered_at: { $max: "$ordered_at" },
      lifetime_spend: { $sum: "$order_total" }
    }
  },
  {
    $lookup: {
      from: "customers",
      localField: "_id",
      foreignField: "customer_id",
      as: "customer"
    }
  }
])
```

### Owl

If the data is modeled Owl-first, the user should not need to reconstruct the
relationship.

```oql
customers.select {
  id,
  name,
  count_lifetime_orders: .orders.count(),
  first_ordered_at: .orders.sort { .ordered_at.asc }.first().ordered_at,
  last_ordered_at: .orders.sort { .ordered_at.desc }.first().ordered_at,
  lifetime_spend: .orders.sum(.order_total)
}
```

### Assessment

- SQL is mature and explicit, but it is carrying relationship reconstruction and join
  scaffolding.
- MongoDB is capable, but the pipeline is less direct about the entity being returned.
- Owl is cleaner here if refs and reverse refs are modeled well.

### Owl language takeaway

This is already one of Owl's strengths. The language should lean into it.

## 2. Repeated-Field Analysis

This is one of the places where MongoDB is very strong operationally, but Owl can become
much clearer syntactically.

### What the query is doing

- inspect repeated components inside a parent record,
- filter those components,
- summarize them,
- optionally project them back out.

### SQL

If repeated values are normalized out, SQL might look like:

```sql
select
  order_id,
  count(*) as book_count,
  sum(price) as total_book_value
from order_items
where category = 'books'
group by order_id;
```

### MongoDB

```javascript
db.orders.aggregate([
  { $unwind: "$items" },
  { $match: { "items.category": "books" } },
  {
    $group: {
      _id: "$_id",
      book_count: { $sum: 1 },
      total_book_value: { $sum: "$items.price" }
    }
  }
])
```

### Owl, first draft

```oql
orders.select {
  id,
  book_count: .items.where { .category == "books" }.count(),
  total_book_value: .items.where { .category == "books" }.sum(.price),
  books: .items.where { .category == "books" }.select { sku, price }
}
```

### Problem in the first Owl draft

This is already cleaner than SQL or MongoDB, but it still repeats the same filter three
separate times.

That is unnecessary effort.

### Better Owl direction

A better Owl style is to use the scoped-equivalent form and name the filtered
subcollection once.

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

### Assessment

- SQL is clear only because the repeated values have already been forced into a separate
  table.
- MongoDB is capable, but requires a repeated-field stage model.
- Owl is already conceptually better here, but it still needs a graceful reusable local
  subcollection form.

### Owl language takeaway

Owl should support scoped query blocks so repeated derived subcollections can be named
once and reused naturally.

## 3. Correlated Threshold / Anomaly Queries

This is one of the places where SQL still has a clearer mature language today.

### What the query is doing

- compute a population summary,
- compare each entity against its local peer group,
- keep only those that are meaningfully above or below the peer baseline.

### SQL

```sql
with customer_total_return as (
  select
    sr_customer_sk as ctr_customer_sk,
    sr_store_sk as ctr_store_sk,
    sum(sr_return_amt) as ctr_total_return
  from store_returns, date_dim
  where sr_returned_date_sk = d_date_sk
    and d_year = 2001
  group by sr_customer_sk, sr_store_sk
)
select c_customer_id
from customer_total_return ctr1, store, customer
where ctr1.ctr_total_return > (
  select avg(ctr_total_return) * 1.2
  from customer_total_return ctr2
  where ctr1.ctr_store_sk = ctr2.ctr_store_sk
)
and ctr1.ctr_customer_sk = c_customer_sk;
```

### Owl, current plausible direction

```oql
storeReturns2001 = store_returns.where { .returned_at.year() == 2001 }

stores.select {
  id,
  anomalous_customers: .customers.where {
    storeReturns2001.where { .customer.id == customers.id && .store.id == stores.id }.sum(.return_amt) >
    (
      storeReturns2001.where { .store.id == stores.id }.avg(.return_amt) * 1.2
    )
  }.select {
    id,
    name
  }
}
```

### What is good in Owl already

- `storeReturns2001` as a named binding is good
- the enclosing-entity shape is good
- scope anchors make the logic possible

### What is not good enough yet

This is still too heavy.

The repeated correlated filters make the query feel more mechanical than Owl-like.

The user should not need to restate:

- `.store.id == stores.id`
- `.customer.id == customers.id`

at that density if the query is already operating inside store and customer scope.

### Better Owl direction

The language needs a scoped-equivalent form that lets a collection define local derived
collections and local summary values before the final result is shaped.

```oql
storeReturns2001 = store_returns.where { .returned_at.year() == 2001 }

stores {
  returns = storeReturns2001.where { .store.id == stores.id }
  high_value_returns = returns.where { .return_amt > 500 }
  avg_return = returns.avg(.return_amt)
  avg_high_value = high_value_returns.avg(.return_amt)
  return_count = returns.count()

  where { .region == "north" }
  sort { .name.asc }

  select {
    id,
    name,
    region,
    anomalous_customers: .customers.where {
      returns.where { .customer.id == customers.id }.sum(.return_amt) > avg_return * 1.2
    }.select { id, name, email },
    high_value_anomalous_customers: .customers.where {
      high_value_returns.where { .customer.id == customers.id }.sum(.return_amt) > avg_high_value * 1.5
    }.select { id, name, lifetime_value },
    return_summary: {
      total: return_count,
      avg: avg_return,
      avg_high_value: avg_high_value
    }
  }
}
```

This is much more readable because:

- the store-scoped subsets are named once
- the summary metrics are named once
- normal query transforms like `where` and `sort` still appear naturally inside the same
  scope
- the final `select` reads in business terms

### What SQL is still doing better today

SQL is verbose, but it has a mature language for:

- peer-group comparison,
- aggregated subqueries,
- correlated thresholds,
- expressing “compare this entity to the average of its local population”.

Owl needs a cleaner way to express the same intent without inheriting SQL's visual
weight.

### Owl language takeaway

To close this gap with SQL while staying simple, Owl should define scoped query blocks as
true equivalents of chained queries, with the extra ability to declare local names.

This is one of the most important syntax-design gaps left.

## 4. Join-Heavy Existence Filtering

### What the query is doing

- enforce multiple relationship constraints,
- apply selective filters,
- return entities satisfying a relational intersection.

### SQL

```sql
SELECT MIN(t.title) AS movie_title
FROM company_name AS cn,
     keyword AS k,
     movie_companies AS mc,
     movie_keyword AS mk,
     title AS t
WHERE cn.country_code ='[de]'
  AND k.keyword ='character-name-in-title'
  AND cn.id = mc.company_id
  AND mc.movie_id = t.id
  AND t.id = mk.movie_id
  AND mk.keyword_id = k.id
  AND mc.movie_id = mk.movie_id;
```

### MongoDB

Usually awkward unless heavily denormalized or lookup-heavy.

### Owl

If modeled as graph-shaped data:

```oql
companies.where { .country_code == "[de]" }.select {
  id,
  movies: .movies.where {
    .keywords.where { .keyword == "character-name-in-title" }.exists()
  }.select {
    id,
    title
  }
}
```

### Assessment

- SQL is mature, but relationship reconstruction dominates the surface.
- MongoDB is usually not elegant here.
- Owl is conceptually much cleaner if refs and reverse refs are first-class.

### Owl language takeaway

This is a place where Owl should not imitate SQL. The workload is real; the syntax model
should remain traversal-first.

## 5. Time-Windowed Reporting

This is a place where SQL currently has a clearer mature language than Owl.

### What the query is doing

- constrain data to a time window,
- group or summarize by period,
- compare or rank outcomes over time.

### SQL

```sql
select
  date_trunc('month', ordered_at) as month,
  customer_id,
  sum(order_total) as revenue
from orders
where ordered_at >= current_date - interval '12 months'
group by 1, 2
order by month asc, revenue desc;
```

### Owl, current likely direction

```oql
orders
  .where { .ordered_at >= now().minus_months(12) }
  .group_by { .ordered_at.month(), .customer.id }
  .select {
    month,
    customer_id,
    revenue: sum(.order_total)
  }
  .sort { month.asc, revenue.desc }
```

### What is good already

- readable flow
- grouping and sorting are familiar
- no SQL clause choreography

### What is missing

Owl still needs a clear time story:

- date literals
- date truncation idioms
- period bucketing idioms
- readable calendar extraction functions

### Owl language takeaway

Owl needs a dedicated time-expression style chapter, with strong emphasis on a very
small number of readable primitives.

## 6. Ranking Within Partition

This is one of the clearest places where SQL is currently more mature as a language.

### What the query is doing

- rank rows within a local subgroup,
- derive first/latest/top-N position relative to siblings.

### SQL

```sql
select
  customer_id,
  ordered_at,
  order_total,
  row_number() over (
    partition by customer_id
    order by ordered_at asc
  ) as customer_order_number
from orders;
```

### Owl, current likely direction

Owl can express some of the need indirectly:

```oql
customers.select {
  id,
  first_order: .orders.sort { .ordered_at.asc }.first()
}
```

But it does not yet have a compact first-class story for ordinal position within a
partition.

### Assessment

- SQL is clearly more mature here.
- Owl should not copy window syntax lightly.
- But Owl still needs a language answer for the workload shape.

### Owl language takeaway

This should probably stay out of the smallest core for now, but the language spec should
explicitly reserve a future space for:

- `rank`
- `row_number`
- position-within-scope forms

## 7. Structured + Vector Retrieval

This is where Owl can become cleaner than either SQL or MongoDB.

### What the query is doing

- restrict a semantic search to a structured scope,
- rank by vector similarity,
- return shaped results with related fields.

### SQL / Postgres with extensions

Possible, but usually feels like SQL plus an added vector subsystem.

### MongoDB / Atlas style

Possible, but usually split across search-specific stages and document pipeline syntax.

### Owl

```oql
orgs.select {
  id,
  similar_findings: findings
    .where { .company.org.id == orgs.id && .status == "open" }
    .nearest { field: .embedding, query: [0.2, 0.4, 0.9], metric: "cosine", k: 10 }
    .select {
      id,
      title,
      company: .company.name,
      score: ._score
    }
}
```

### Assessment

- Owl has a very coherent language story here already
- this should remain one of Owl's flagship workload families

## 8. Main Conclusions

### A. Where Owl already looks better

Owl already has a more natural surface for:

- entity -> related collection -> summary
- repeated-field analysis
- reverse-traversal existence queries
- graph-scoped vector retrieval
- projection-heavy nested shaping

These are not minor wins. They are real language-shape advantages.

### B. Where SQL still has a clearer mature language

SQL is still clearer today for:

- correlated threshold / peer comparison queries
- time-windowed reporting conventions
- partition-aware ranking
- multi-level analytical rollups

The answer is not to copy SQL clauses. The answer is to design Owl-native
simplifications with stronger scoped derivation.

### C. The most important language-design opportunity

The single most important simplification opportunity revealed by these comparisons is:

> Owl needs a graceful scoped query-block form that lets a collection define local
> derived collections and local summary values before the final `select`.

That would improve:

- repeated-field analysis
- correlated anomaly queries
- time-windowed summaries
- local top-k expressions
- nested reporting logic

without inflating the language much.

## 9. Suggestions For Owl Syntax Design

### Suggestion 1. Add scoped query blocks as true query equivalents

This is the strongest immediate candidate.

Example target direction:

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

This reduces repetition without introducing a second language. It also keeps `select`
focused on shaping instead of turning it into a mini block language.

### Suggestion 2. Let scoped blocks carry ordinary query transforms too

For correlated analytics:

```oql
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

This is far more Owl-like than repeating the same scoped filter plumbing, and it makes
block form a true equivalent of chained form.

### Suggestion 3. Keep arrays under collection semantics

Do not solve repeated-field queries by inventing a separate family such as:

- `any(...)`
- `expand(...)`
- stage-driven repeated-field syntax

The collection model is cleaner.

### Suggestion 4. Add a small, readable time-expression chapter

Owl does not need large date syntax. It needs a small set of readable time operations
that users can remember.

### Suggestion 5. Reserve ranking for later, but name the gap clearly

Owl should not bloat its core just to imitate SQL window syntax. But the language docs
should clearly acknowledge ranking as a real workload shape that will need an Owl-native
answer later.

## Summary

The goal is not for Owl to look more like SQL or MongoDB. The goal is for Owl to express
the same serious workloads more directly.

The cleanest way forward is:

- keep the one-model collection design,
- add scoped query blocks as equivalents of chained queries,
- use them for local derived collections and summary values,
- strengthen time semantics,
- keep ranking and advanced analytics in the long-term lane unless a small elegant form
  emerges.
