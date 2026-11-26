# Completeness matrix

What SQL and MongoDB can say, and whether Owl can say it. SQL here means
PostgreSQL; MongoDB means its query and aggregation language.

Each row has one of three answers.

- **yes**: Owl does it, and the OQL on the row is the proof. The test runs it
  against a small shop and checks the answer.
- **different**: Owl reaches the same end another way on purpose. That way is
  run and checked the same.
- **no**: Owl does not do it. Where there is something a user might try, the
  row says so and the test checks it is refused, so a row that starts working
  fails the test until it is moved.

A few rows are about behaviour no single query shows, such as surviving a
crash. Those name the test that covers them.

This file is written by `tests/completeness_matrix_test.rs` from the rows in
`tests/completeness_matrix/rows.rs`, and the test fails if the two differ. To
change it, change a row and run:

```bash
OWL_WRITE_MATRIX=1 cargo test --release --test completeness_matrix_test
```

The shop has five customers, five products and six orders. Customers have a
city, an optional email, a list of tags, an address and an optional reference
to the customer who referred them. Orders refer to a customer and hold a list
of items, each of which refers to a product.

## Summary

| Area | Yes | Different | No |
|---|---:|---:|---:|
| Filtering | 16 | 0 | 0 |
| Shaping and ordering | 23 | 0 | 0 |
| Aggregation | 16 | 0 | 0 |
| Relationships and joins | 13 | 1 | 1 |
| Subqueries and sets | 8 | 0 | 0 |
| Window functions | 9 | 0 | 0 |
| Documents and lists | 19 | 0 | 0 |
| Writing | 17 | 0 | 1 |
| Schema | 12 | 2 | 5 |
| Indexes | 5 | 0 | 4 |
| Transactions | 7 | 0 | 3 |
| Search and specialised data | 2 | 0 | 4 |
| Operations | 3 | 0 | 3 |
| **All** | **150** | **3** | **21** |

## Filtering

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Equality | `WHERE status = 'paid'` | `{ status: "paid" }` | yes | `orders.where { .status == "paid" }.select { id }` |
| Comparison | `WHERE total > 50` | `{ total: { $gt: 50 } }` | yes | `orders.where { .total > 50 }.select { id }` |
| And, or, not | `WHERE a AND (b OR NOT c)` | `$and, $or, $nor, $not` | yes | `orders.where { .total > 20 && (.status == "open" \|\| !(.total < 100)) }.select { id }` |
| In a list | `WHERE city IN ('Lagos', 'Accra')` | `{ city: { $in: [...] } }` | yes | `customers.where { .city in ["Lagos", "Nairobi"] }.select { id }` |
| Not in a list | `WHERE city NOT IN (...)` | `{ city: { $nin: [...] } }` | yes | `customers.where { .city not in ["Lagos", "Nairobi"] }.select { id }` |
| Between | `WHERE total BETWEEN 25 AND 60` | `{ total: { $gte: 25, $lte: 60 } }` | yes | `orders.where { .total between 25 and 60 }.select { id }` |
| Text starts with, ends with, contains | `WHERE name LIKE 'A%'` | `{ name: /^A/ }` | yes | `customers.where { .name like "A%" \|\| .email like "%.org" \|\| .city contains "rob" }.select { id }` |
| Regular expression | `WHERE email ~ '^[a-c].*io$'` | `{ email: { $regex: ... } }` | yes | `customers.where { matches(.email, "^[a-c].*io$") }.select { id }` |
| Ignoring case | `WHERE name ILIKE 'ada'` | `{ name: { $regex: "^ada$", $options: "i" } }` | yes | `customers.where { .name ilike "ada" \|\| lower(.name) == "eve" }.select { id }` |
| Is null | `WHERE email IS NULL` | `{ email: null }` | yes | `customers.where { .email is null }.select { id }` |
| Is not null | `WHERE referrer IS NOT NULL` | `{ referrer: { $ne: null } }` | yes | `customers.where { .referrer is not null }.select { id }` |
| Field is present | Columns always exist | `{ email: { $exists: true } }` | yes | `customers.where { exists(.email) }.select { id }` |
| One field against another | `WHERE price > stock` | `{ $expr: { $gt: ["$price", "$stock"] } }` | yes | `products.where { .price > .stock }.select { id }` |
| Arithmetic in a condition | `WHERE price * stock > 400` | `{ $expr: { $gt: [{ $multiply: [...] }, 400] } }` | yes | `products.where { .price * .stock > 400 }.select { id }` |
| Dates and parts of dates | `WHERE EXTRACT(YEAR FROM joined) = 2025` | `{ $expr: { $eq: [{ $year: "$joined" }, 2025] } }` | yes | `customers.where { .joined.year() == 2025 && .joined < "2025-07-01" }.select { id }` |
| Kind of value | `pg_typeof(x)` | `{ x: { $type: "string" } }` | yes | `customers.where { type_of(.email) == "String" }.count()` |

## Shaping and ordering

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Chosen fields | `SELECT id, name` | `{ id: 1, name: 1 }` | yes | `customers.where { .id == "c1" }.select { id, name }` |
| Every field | `SELECT *` | `find()` | yes | `products.where { .id == "p1" }` |
| Every field but some | List the rest | `{ stock: 0 }` | yes | `products.where { .id == "p1" }.select { *, -stock, -category }` |
| Renamed fields | `SELECT name AS customer` | `{ customer: "$name" }` | yes | `customers.where { .id == "c1" }.select { customer: .name }` |
| Computed fields | `SELECT price * stock AS worth` | `{ $multiply: ["$price", "$stock"] }` | yes | `products.where { .id == "p2" }.select { id, worth: .price * .stock }` |
| Distinct rows | `SELECT DISTINCT city` | `distinct("city")` | yes | `customers.select { city }.distinct().sort { .city }` |
| Conditional value | `CASE WHEN ... THEN ... ELSE ... END` | `$cond, $switch` | yes | `products.sort { .id }.limit(3).select { id, level: if .stock == 0 then "out" else if .stock < 50 then "low" else "fine" }` |
| Matching on a value | `CASE status WHEN 'paid' THEN ... END` | `$switch` | yes | `orders.sort { .id }.limit(3).select { id, done: match .status { "paid" -> true, _ -> false } }` |
| First value that is present | `COALESCE(email, 'none')` | `$ifNull` | yes | `customers.where { .id in ["c1", "c2"] }.sort { .id }.select { id, email: .email or "none" }` |
| Conversion between types | `CAST(stock AS text)` | `$toString, $toInt, $toDouble` | yes | `products.where { .id == "p1" }.select { text: to_string(.stock), whole: to_int(.price), real: to_float(.stock), number: to_int("42") }` |
| Text functions | `upper, lower, length, substring, replace, trim, \|\|` | `$toUpper, $strLenCP, $substrCP, $replaceAll, $trim, $concat` | yes | `customers.where { .id == "c1" }.select { up: upper(.name), size: length(.email), part: substring(.email, 0, 3), swapped: replace(.email, "x.io", "owl.dev"), joined: concat(.name, " of ", .city), pieces: split(.email, "@") }` |
| Number functions | `abs, round, floor, ceil, sqrt, power` | `$abs, $round, $floor, $ceil, $sqrt, $pow` | yes | `products.where { .id == "p2" }.select { rounded: round(.price), down: floor(.price), up: ceil(.price), root: sqrt(.stock + 9), squared: pow(.price, 2), away: abs(.stock - 100) }` |
| Date arithmetic | `joined + INTERVAL '30 days', a - b` | `$dateAdd, $dateDiff` | yes | `customers.where { .id == "c3" }.select { later: (.joined + days(30)).format("%Y-%m-%d"), waited: diff_days(.referrer.joined, .joined) }` |
| Date formatting | `to_char(joined, 'YYYY-MM')` | `$dateToString` | yes | `customers.where { .id == "c1" }.select { month: .joined.format("%Y-%m") }` |
| Nested output | `json_build_object(...)` | `{ who: { name: "$name" } }` | yes | `customers.where { .id == "c1" }.select { id, who: { name: .name, place: { city: .city, zip: .address.zip } } }` |
| List in the output | `ARRAY[a, b]` | `["$a", "$b"]` | yes | `products.where { .id == "p1" }.select { id, figures: [.price, .stock] }` |
| Order by several keys | `ORDER BY city ASC, joined DESC` | `sort({ city: 1, joined: -1 })` | yes | `customers.sort { .city, .joined.desc }.select { id }` |
| Order by an expression | `ORDER BY price * stock DESC` | `$addFields then $sort` | yes | `products.sort { (.price * .stock).desc, .id }.select { id }` |
| Where absent values sort | `ORDER BY email NULLS LAST` | Absent sorts first | yes | `customers.sort { .email.nulls_last }.select { id }`<br>An absent value sorts below every other unless the key says `.nulls_last`. |
| Limit and offset | `LIMIT 2 OFFSET 1` | `skip(1).limit(2)` | yes | `products.sort { .price.desc }.offset(1).limit(2).select { id }` |
| Paging from the last row seen | `WHERE (price, id) > (12.5, 'p2') ORDER BY price, id LIMIT 2` | `find({ price: { $gt: 12.5 } }).sort().limit(2)` | yes | `products.where { (.price, .id) > (12.5, "p2") }.sort { .price, .id }.limit(2).select { id }` |
| Parameters | `WHERE label = $1` | Values are passed as data | yes | Covered by `parameters_carry_values_into_a_fixed_query` in `tests/oql_language_conformance_test.rs`.<br>`$name` in the query, bound by name before it runs. |
| Random sample | `ORDER BY random() LIMIT 2` | `{ $sample: { size: 2 } }` | yes | `products.sample(2).count()` |

## Aggregation

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Count | `SELECT count(*)` | `countDocuments()` | yes | `orders.count()` |
| Sum, average, least, greatest | `sum, avg, min, max` | `$sum, $avg, $min, $max` | yes | `orders.select { sum: sum(.total), avg: avg(.total), least: min(.total), most: max(.total) }` |
| Group by | `GROUP BY status` | `{ $group: { _id: "$status" } }` | yes | `orders.group_by { .status }.select { status, n: count(), total: sum(.total) }.sort { .status }` |
| Group by several keys | `GROUP BY city, status` | `{ _id: { city: ..., status: ... } }` | yes | `orders.group_by { .customer.city, .status }.select { city, status, n: count() }.sort { .city, .status }` |
| Group by an expression | `GROUP BY date_trunc('month', placed)` | `{ _id: { $dateTrunc: ... } }` | yes | `orders.group_by { month: .placed.format("%Y-%m") }.select { month, n: count() }.sort { .month }` |
| Filter on a group | `HAVING count(*) > 1` | `$match after $group` | yes | `orders.group_by { .customer }.having { count() > 1 }.select { customer: .customer.id, n: count() }.sort { .customer }` |
| Count of distinct values | `count(DISTINCT customer)` | `$addToSet then $size` | yes | `orders.customer.distinct().count()` |
| Aggregate over some of the rows | `count(*) FILTER (WHERE status = 'paid')` | `{ $sum: { $cond: [...] } }` | yes | `orders.group_by { .customer }.select { customer: .customer.id, paid: count(where .status == "paid"), paid_total: sum(.total where .status == "paid") }.sort { .customer }` |
| Median and percentiles | `percentile_cont(0.5) WITHIN GROUP (ORDER BY total)` | `$median, $percentile` | yes | `orders.select { median: median(.total), p90: percentile(.total, 0.9) }` |
| Spread | `stddev_samp, var_pop` | `$stdDevSamp, $stdDevPop` | yes | `products.group_by { .category }.select { category, sample: variance(.stock), whole: .stock.variance_pop(), sd: .stock.stddev_pop() }.sort { .category }` |
| Values of a group as a list | `array_agg(total)` | `{ $push: "$total" }` | yes | `orders.group_by { .status }.select { status, totals: .total }.sort { .status }` |
| Distinct values of a group as a list | `array_agg(DISTINCT city)` | `{ $addToSet: "$city" }` | yes | `orders.group_by { .status }.select { status, cities: .customer.city.distinct() }.sort { .status }` |
| First or last row of each group | `DISTINCT ON (customer) ... ORDER BY placed DESC` | `{ $first: ... } after $sort` | yes | `orders.sort { .placed.desc }.group_by { .customer }.select { customer: .customer.id, latest: first().id }.sort { .customer }` |
| Ranges as groups | `width_bucket(total, ...) or CASE` | `$bucket` | yes | `orders.group_by { size: if .total < 30 then "small" else if .total < 100 then "medium" else "large" }.select { size, n: count() }.sort { .size }` |
| Several summaries in one answer | One query each, or UNION ALL | `$facet` | yes | `customers.limit(1).select { by_status: orders.group_by { .status }.select { status, n: count() }.sort { .status }, dearest: orders.sort { .total.desc }.limit(2).select { id } }` |
| Subtotals and grand totals | `GROUP BY ROLLUP (city, status), CUBE` | Several $group stages in $facet | yes | `orders.group_by { rollup(.customer.city, .status) }.select { city, status, n: count() }.sort { .city.nulls_last, .status.nulls_last }`<br>`rollup` gives a subtotal for each leading run of the keys and a grand total; `cube` gives one for every combination. A key a subtotal leaves out is null in its row. |

## Relationships and joins

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Row and the row it refers to | `JOIN customers c ON c.id = o.customer` | `$lookup then $unwind` | yes | `orders.where { .id == "o3" }.select { id, customer: .customer.name, city: .customer.city }`<br>A reference is followed with a dot. No join is written. |
| Filter through a reference | `JOIN ... WHERE c.city = 'Lagos'` | `$lookup then $match` | yes | `orders.where { .customer.city == "Lagos" }.select { id }` |
| Optional reference | `LEFT JOIN customers r ON r.id = c.referrer` | `$lookup, preserveNullAndEmptyArrays` | yes | `customers.sort { .id }.limit(2).select { id, referrer: .referrer.name }` |
| Rows that refer to a row | `LEFT JOIN orders o ON o.customer = c.id GROUP BY c.id` | `$lookup then $size` | yes | `customers.sort { .id }.select { id, orders: .orders.count(), spent: .orders.sum(.total) }`<br>Every reference can be followed backwards under the name of the collection that holds it. |
| Related rows nested in the answer | `json_agg(o.*) in a lateral join` | `$lookup` | yes | `customers.where { .id == "c3" }.select { id, orders: .orders.sort { .id }.select { id, status } }` |
| Several steps | `three JOINs` | `three $lookup stages` | yes | `customers.where { .id == "c4" }.select { id, through: .referrer.referrer.name, bought: .orders.items.product.name }` |
| Row related to its own collection | `JOIN customers r ON r.id = c.referrer` | `$lookup from the same collection` | yes | `customers.where { .referred.exists() }.sort { .id }.select { id, referred: .referred.count() }` |
| Many to many | `JOIN through a link table` | `$lookup through a list of ids` | yes | `products.sort { .id }.select { id, buyers: orders.where { products.id in .items.product.id }.customer.distinct().count() }` |
| Rows with a related row | `WHERE EXISTS (SELECT 1 FROM orders ...)` | `$lookup then $match` | yes | `customers.where { .orders.any { .status == "paid" } }.sort { .id }.select { id }` |
| Rows with no related row | `WHERE NOT EXISTS (...)` | `$lookup then $match on empty` | yes | `customers.where { !.orders.exists() }.select { id }` |
| Join on any condition | `JOIN customers b ON b.city = a.city AND b.id <> a.id` | `$lookup with a pipeline` | yes | `customers.sort { .id }.select { id, neighbours: customers.where { .city == customers.city && .id != customers.id }.select { id } }` |
| Every row with every row | `CROSS JOIN` | `$lookup with an empty pipeline` | different | `customers.where { .city == "Accra" }.sort { .id }.select { id, with: products.where { .price > 20 }.sort { .id }.select { id } }`<br>The second side comes nested under each row of the first, not as one flat row per pair. |
| Unmatched rows from both sides | `FULL OUTER JOIN` | Two $lookup stages and $unionWith | no | No single form. Each side is a query, and `union` puts them together. |
| Hierarchy of any depth | `WITH RECURSIVE` | `$graphLookup` | yes | `customers.where { .id == "c4" }.walk(.referrer).sort { ._depth }.select { id, depth: ._depth }` |
| Everything below a row | `WITH RECURSIVE, downwards` | `$graphLookup` | yes | `customers.where { .id == "c1" }.select { id, below: .walk(.referred).count(), deepest: .walk(.referred).max(._depth) }` |

## Subqueries and sets

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| One value from another query | `WHERE total > (SELECT avg(total) FROM orders)` | Two queries, or $lookup | yes | `orders.where { .total > orders.avg(.total) }.select { id }` |
| Query that depends on the row | `WHERE total = (SELECT max(total) ... WHERE customer = o.customer)` | `$lookup with let` | yes | `orders.where { .total == .customer.orders.max(.total) }.sort { .id }.select { id }` |
| In the rows of another query | `WHERE id IN (SELECT customer FROM orders WHERE total > 50)` | Two queries, or $lookup | yes | `customers.where { .id in orders.where { .total > 50 }.customer.id }.select { id }` |
| Named query used later | `WITH paid AS (...) SELECT ... FROM paid` | Variables in application code | yes | Covered by `a_binding_means_the_expression_it_names` in `tests/oql_language_conformance_test.rs`.<br>`paid = orders.where { .status == "paid" }` on a line of its own names the rows for the statements after it. |
| Rows of either query | `UNION` | `$unionWith then $group` | yes | `customers.where { .city == "Nairobi" }.union(customers.where { "vip" in .tags }).sort { .id }.select { id }` |
| Rows of both, repeats kept | `UNION ALL` | `$unionWith` | yes | Customers.where { .city == "Lagos" } + customers.where { "vip" in .tags }).count( |
| Rows in both queries | `INTERSECT` | No stage | yes | `customers.where { .city == "Accra" }.intersect(customers.where { .email != null }).select { id }` |
| Rows in one query and not the other | `EXCEPT` | No stage | yes | `customers.where { .city == "Accra" }.except(customers.where { .email != null }).select { id }` |

## Window functions

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Position in a partition | `row_number() OVER (PARTITION BY customer ORDER BY placed)` | `$setWindowFields, $documentNumber` | yes | `orders.sort { .customer.id, .placed }.partition_by { .customer }.select { id, n: row_number() }` |
| Rank, with and without gaps | `rank(), dense_rank()` | `$rank, $denseRank` | yes | `products.sort { .category, .price.desc }.partition_by { .category }.select { id, rank: rank(.price), dense: dense_rank(.price) }` |
| Row before and row after | `lag(total), lead(total)` | `$shift` | yes | `orders.sort { .placed }.select { id, before: prev(.total), after: next(.total) }.limit(3)` |
| Running total | `sum(total) OVER (ORDER BY placed)` | `$sum with a window of documents` | yes | `orders.sort { .placed }.select { id, running: so_far().total.sum() }` |
| Moving window | `avg(total) OVER (ROWS 1 PRECEDING)` | `$avg with window: { documents: [-1, 0] }` | yes | `orders.sort { .placed }.select { id, recent: so_far().last(2).total.avg() }` |
| Share of a partition | `total / sum(total) OVER (PARTITION BY customer)` | `$setWindowFields without a window` | yes | `orders.sort { .id }.partition_by { .customer }.select { id, of: sum(.total), size: count() }.limit(3)` |
| First and last of a partition | `first_value(id), last_value(id)` | `$first, $last in $setWindowFields` | yes | `orders.sort { .customer.id, .placed }.partition_by { .customer }.select { id, opened_with: first().id, latest: last(1).select { id } }.limit(3)` |
| Top rows of each partition | `WHERE row_number() ... <= 1 in a subquery` | `$topN` | yes | `orders.sort { .customer.id, .total.desc }.partition_by { .customer }.select { id, n: row_number() }.where { .n == 1 }.select { id }` |
| Equal-sized buckets and relative rank | `ntile(4), percent_rank(), cume_dist()` | No operator | yes | `orders.sort { .total }.select { id, quarter: ntile(4), rank: percent_rank(.total), within: round(cume_dist(.total), 2) }` |

## Documents and lists

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Field inside an object | `address->>'zip'` | `"address.zip"` | yes | `customers.where { .address.zip == "00233" }.sort { .id }.select { id, street: .address.street }` |
| List holds a value | `'vip' = ANY(tags)` | `{ tags: "vip" }` | yes | `customers.where { "vip" in .tags }.sort { .id }.select { id }` |
| List holds all of some values | `tags @> ARRAY['vip', 'early']` | `{ tags: { $all: [...] } }` | yes | `customers.where { .tags contains_all ["vip", "early"] }.select { id }` |
| Length of a list | `cardinality(tags)` | `$size` | yes | `customers.where { .tags.count() == 0 }.sort { .id }.select { id }` |
| Element by position | `tags[1]` | `$arrayElemAt, $first, $last` | yes | `customers.where { .id == "c1" }.select { first: .tags[0], last: .tags[-1], none: .tags[9] }` |
| One element meets several conditions | `EXISTS (SELECT 1 FROM unnest(items) ...)` | `$elemMatch` | yes | `orders.where { .items.any { .qty > 5 && .price < 5 } }.sort { .id }.select { id }` |
| Every element meets a condition | `NOT EXISTS (... WHERE NOT ...)` | `{ $not: { $elemMatch: ... } }` | yes | `orders.where { .items.all { .qty >= 2 } }.sort { .id }.select { id }` |
| One row per element | `CROSS JOIN LATERAL unnest(items)` | `$unwind` | yes | `orders.items.sort { orders.id, .qty.desc }.limit(3).select { order: orders.id, product: .product.name, qty }` |
| Group over elements | `unnest then GROUP BY` | `$unwind then $group` | yes | `orders.items.group_by { product: .product.id }.select { product, sold: sum(.qty), revenue: sum(.qty * .price) }.sort { .product }` |
| One field of the elements that match | Subquery over unnest | `$filter then $map` | yes | `customers.where { .id == "c1" }.select { big: .orders.where { .total > 100 }.total, dear: .orders.items.where { .price > 10 }.product.name }` |
| Some of a list in the answer | Subquery over unnest | `$filter` | yes | `orders.where { .id == "o4" }.select { id, small: .items.where { .price < 10 }.select { qty, price } }` |
| Each element reshaped | Subquery over unnest | `$map` | yes | `orders.where { .id == "o1" }.select { id, lines: .items.select { name: .product.name, cost: .qty * .price } }` |
| List reduced to a value | Subquery over unnest | `$reduce, $sum` | yes | `orders.sort { .id }.limit(2).select { id, units: .items.sum(.qty), value: .items.sum(.qty * .price) }` |
| Part of a list | `tags[1:1]` | `$slice` | yes | `orders.where { .id == "o1" }.select { id, first: .items[:1].select { qty }, rest: .items[1:].select { qty } }` |
| List sorted | Subquery with ORDER BY | `$sortArray` | yes | `orders.where { .id == "o4" }.select { id, by_qty: .items.sort { .qty.desc }.select { qty } }` |
| Lists joined | `a \|\| b` | `$concatArrays` | yes | `customers.where { .id == "c1" }.select { all: .tags + ["checked"], size: (.tags + ["checked"]).count() }` |
| Row with fields added | `to_jsonb(c) \|\| '{...}'` | `$addFields, $mergeObjects` | yes | `products.where { .id == "p5" }.select { *, worth: .price * .stock }` |
| Collection with no declared shape | A jsonb column | The default | yes | `notes.insert [ { id: "n1", text: "hello", stars: 3 }, { id: "n2", about: { any: [1, "two"] } } ].select { id, stars, inner: .about.any[1] }` |
| Names of an object's fields as data | `jsonb_each, jsonb_object_keys` | `$objectToArray` | yes | `customers.where { .id == "c1" }.select { names: keys(.address) }` |

## Writing

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Insert a row | `INSERT INTO products VALUES (...)` | `insertOne()` | yes | `products.insert { id: "p6", name: "Clip", price: 0.5, stock: 900, category: "stationery" }.select { name, stock }`<br>A write gives back the rows it wrote, so what follows it reads them. |
| Insert many rows | `INSERT ... VALUES (...), (...)` | `insertMany()` | yes | `products.insert [ { id: "p6", name: "Clip", price: 0.5, stock: 900, category: "stationery" }, { id: "p7", name: "Rug", price: 80.0, stock: 3, category: "home" } ].select { id, name }` |
| Update the rows a condition picks | `UPDATE ... SET ... WHERE ...` | `updateMany()` | yes | `orders.where { .status == "open" }.update { status: "paid" }.select { id, status }` |
| Update from the row's own values | `SET stock = stock - 1, price = price * 1.1` | `$inc, $mul` | yes | `products.where { .id == "p2" }.update { stock -= 1, price: .price * 2 }.select { stock, price }` |
| Update from related rows | `UPDATE ... SET total = (SELECT sum(...) ...)` | Read, then write | yes | `orders.where { .id == "o1" }.update { total: .items.sum(.qty * .price) + .customer.orders.count() }.select { total }` |
| Update a field inside an object | `jsonb_set(address, '{zip}', ...)` | `{ $set: { "address.zip": ... } }` | yes | `customers.where { .id == "c1" }.update { address.zip: "100002" }.select { address }` |
| Clear a field | `SET email = NULL` | `{ $set: { email: null } }` | yes | `customers.where { .id == "c1" }.update { email: null }.select { id, email }` |
| Take a field off a row | Columns always exist | `$unset` | yes | `customers.where { .id == "c1" }.update { -email }.select { id, has_email: exists(.email) }`<br>`-email` takes the field off the row. `email: null` keeps it, holding null. |
| Add to a list | `SET tags = tags \|\| 'x'` | `$push` | yes | `customers.where { .id == "c2" }.update { tags += "back" }.select { tags }` |
| Add to a list once | CASE WHEN ...  | `$addToSet` | yes | `customers.where { .id == "c1" }.update { tags: (.tags + ["vip", "gold"]).distinct() }.select { tags }` |
| Take out of a list | `array_remove(tags, 'vip')` | `$pull` | yes | `customers.where { .id == "c1" }.update { tags -= "vip" }.select { tags }` |
| Change the elements of a list that match | Rebuild the array | `"items.$[e].qty" with arrayFilters` | yes | `orders.where { .id == "o1" }.update { items.where { .price < 5 }.qty += 1 }.select { qty: .items.qty }` |
| Delete the rows a condition picks | `DELETE FROM ... WHERE ...` | `deleteMany()` | yes | `orders.where { .status == "cancelled" }.delete().select { id }` |
| Delete every row | `TRUNCATE, DELETE FROM` | `deleteMany({})` | yes | `orders.delete().count()`<br>The rows deleted are given back, here counted. |
| Insert or replace | `INSERT ... ON CONFLICT (id) DO UPDATE` | `replaceOne(..., { upsert: true })` | yes | `products.upsert on id { id: "p1", name: "Pencil", price: 1.0, stock: 10, category: "stationery" }`<br>`products.upsert on id { id: "p9", name: "Tape", price: 3.0, stock: 10, category: "stationery" }` |
| Rows a write changed | `UPDATE ... RETURNING *` | `findOneAndUpdate()` | yes | `products.where { .category == "home" }.update { stock += 1 }.select { id, stock }` |
| Insert the rows of a query | `INSERT INTO ... SELECT ...` | `$out, $merge` | yes | `archive.insert(orders.where { .status == "cancelled" }).select { id, status }` |
| One statement that inserts, updates and deletes by a join | `MERGE` | `$merge with whenMatched` | no | `upsert` covers insert-or-replace. Anything more is separate statements in a `transaction` block. |

## Schema

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Typed fields | `CREATE TABLE t (n integer, ...)` | `$jsonSchema validator` | yes | `schema gauges { id: String, n: Integer, ratio: Float, live: Boolean, at: DateTime, names: List(String) }`<br>`gauges.insert { id: "g1", n: 7, ratio: 0.5, live: true, at: "2026-01-01", names: ["a"] }.select { n: type_of(.n), ratio: type_of(.ratio), live: type_of(.live), at: type_of(.at), names: type_of(.names) }`<br>Refused: `schema gauges { id: String, n: Integer }`<br>`gauges.insert { id: "g1", n: "seven" }` |
| Optional and required fields | `NOT NULL` | `required: [...]` | yes | `customers.insert { id: "c9", name: "Zed", city: "Lagos", joined: "2026-01-01", tags: [], address: { street: "9", zip: "9" } }.select { id, email }`<br>`email: String?` may be left out. `city: String` may not.<br>Refused: `customers.insert { id: "c9", name: "Zed", joined: "2026-01-01", tags: [], address: { street: "9", zip: "9" } }` |
| Defaults | `DEFAULT 'free'` | Application code | yes | `schema plans { id: String, tier: String = "free", label: String = upper(.id), made: DateTime = now() }`<br>`plans.insert { id: "a" }.select { tier, label }` |
| Unique fields | `UNIQUE (email)` | `createIndex(..., { unique: true })` | yes | `schema logins { id: String, email: String unique, unique(id, email) }`<br>`logins.insert { id: "l1", email: "a@x.io" }`<br>Refused: `schema logins { id: String, email: String unique }`<br>`logins.insert [ { id: "l1", email: "a@x.io" }, { id: "l2", email: "a@x.io" } ]` |
| Conditions every row must meet | `CHECK (age >= 0)` | `$jsonSchema, $expr validator` | yes | `schema ages { id: String, age: Integer, check(.age >= 0 && .age < 150) }`<br>`ages.insert { id: "a1", age: 36 }`<br>Refused: `schema ages { id: String, age: Integer, check(.age >= 0 && .age < 150) }`<br>`ages.insert { id: "a1", age: -1 }` |
| One of a fixed set of values | `CREATE TYPE ... AS ENUM` | `enum: [...]` | different | `schema tickets { id: String, state: String, check(.state in ["open", "closed"]) }`<br>`tickets.insert { id: "t1", state: "open" }`<br>A `check` on the field. There is no separate enum type. |
| References that must resolve | `FOREIGN KEY` | Not enforced | yes | `orders.where { .id == "o1" }.select { id, customer: .customer.id }`<br>Refused: `orders.insert { id: "o9", customer: "nobody", status: "open", placed: "2026-01-01", total: 1.0, items: [] }` |
| What deleting a referenced row does | `ON DELETE CASCADE, SET NULL, RESTRICT` | Application code | yes | `schema folders { id: String }`<br>`schema files { id: String, folder: Ref(folders) on delete cascade, owner: Optional(Ref(customers)) on delete nullify }`<br>`folders.insert { id: "f1" }`<br>`files.insert { id: "x", folder: "f1" }`<br>`folders.delete()` |
| Identity of a row | `PRIMARY KEY (id)` | `_id` | yes | `customers.where { .id == "c1" }.select { id }`<br>Every row has an `id`, unique in its collection and never changed. |
| Identity made of several fields | `PRIMARY KEY (tenant, id)` | `_id: { tenant: ..., id: ... }` | no | The identity is the one `id` field. Several fields together can be declared `unique`. |
| Identity chosen by the database | `SERIAL, GENERATED AS IDENTITY` | `ObjectId` | yes | `notes.insert { text: "no id given" }.select { has_id: .id != null }` |
| Add, rename, retype and drop fields | `ALTER TABLE ... ADD, RENAME, ALTER TYPE, DROP COLUMN` | `$set, $rename, $unset over every document` | yes | `schema products { id: String, title: String from name, price: Float, stock: Float, category: removed, rating: Integer = 5, note: String? }` |
| Drop a collection | `DROP TABLE` | `drop()` | yes | `schema scraps { id: String }`<br>`scraps.insert { id: "s" }`<br>`drop scraps` |
| Stored functions | `CREATE FUNCTION` | Application code | yes | `fn with_tax(amount: Float, rate: Float = 0.5) { amount * (1 + rate) }`<br>`fn paid(rows) { rows.where { .status == "paid" } }` |
| Saved query | `CREATE VIEW` | `createView()` | different | `fn paid(rows) { rows.where { .status == "paid" } }`<br>A function of rows, used as a step in any query. It is not a named collection. |
| Stored result kept up to date | `CREATE MATERIALIZED VIEW` | `$merge on a schedule, on-demand views` | no | Nothing stores a query's result and refreshes it. |
| Value computed from other fields on every write | `GENERATED ALWAYS AS (...) STORED` | Application code | no | A default may be computed, but only when the row is first written. |
| Code that runs when rows change | `CREATE TRIGGER` | `change streams, Atlas triggers` | no | No triggers. |
| One collection split by a key | `PARTITION BY RANGE (...)` | `sharding, time series collections` | no | A collection is not partitioned. |

## Indexes

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Index on a field | `CREATE INDEX ON jobs (state)` | `createIndex({ state: 1 })` | yes | `schema jobs { id: String, state: String, index(.state) }`<br>`jobs.insert [ { id: "j1", state: "open" }, { id: "j2", state: "done" } ]`<br>`jobs.where { .state == "open" }.explain()` |
| Index on several fields | `CREATE INDEX ON jobs (state, n)` | `createIndex({ state: 1, n: 1 })` | yes | `schema jobs { id: String, state: String, n: Integer, index(.state, .n) }`<br>`jobs.insert [ { id: "j1", state: "open", n: 1 }, { id: "j2", state: "open", n: 2 } ]`<br>`jobs.where { .state == "open" && .n == 2 }.explain()` |
| Index on a field inside an object | `CREATE INDEX ON places ((address->>'zip'))` | `createIndex({ "address.zip": 1 })` | no | `schema places { id: String, address: { zip: String }, index(.address.zip) }`<br>`places.insert { id: "a", address: { zip: "1" } }`<br>`places.where { .address.zip == "1" }.explain()`<br>The declaration is accepted, but a filter on the field still reads every row. |
| Index that keeps values unique | `CREATE UNIQUE INDEX` | `createIndex(..., { unique: true })` | yes | `schema logins { id: String, email: String unique }`<br>`logins.insert { id: "l1", email: "a@x.io" }`<br>`logins.where { .email == "a@x.io" }.explain()` |
| Index for following references backwards | `CREATE INDEX ON orders (customer)` | `createIndex({ customer: 1 })` | yes | `customers.where { .id == "c1" }.select { n: .orders.count() }`<br>Kept for every reference without being declared. |
| How a query will run | `EXPLAIN` | `explain()` | yes | `orders.where { .status == "paid" }.explain()` |
| Index on the elements of a list | `CREATE INDEX ... USING gin (tags)` | `multikey index` | no | A list cannot be an index key, so `"vip" in .tags` reads every row.<br>Refused: `schema posts { id: String, tags: List(String), index(.tags) }`<br>`posts.insert { id: "a", tags: ["x"] }` |
| Index on an expression | `CREATE INDEX ON t (lower(name))` | Store the value in a field | no | An index is on fields as stored.<br>Refused: `schema names { id: String, name: String, index(lower(.name)) }` |
| Index over some of the rows | `CREATE INDEX ... WHERE state = 'open'` | `partialFilterExpression` | no | An index covers every row of its collection.<br>Refused: `schema jobs { id: String, state: String, index(.state, where: .state == "open") }` |

## Transactions

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Several writes kept or dropped together | `BEGIN; ...; COMMIT;` | `session.withTransaction()` | yes | `transaction {`<br>`products.where { .id == "p1" }.update { stock -= 10 }`<br>`products.where { .id == "p2" }.update { stock += 10 }`<br>`products.where { .id in ["p1", "p2"] }.sort { .id }.select { stock }`<br>`}`<br>The block answers with the rows of its last statement. |
| A failure undoes the whole block | `ROLLBACK on error` | `abortTransaction()` | yes | Covered by `a_transaction_block_is_all_or_nothing` in `tests/oql_language_conformance_test.rs`. |
| A transaction held open across statements | `BEGIN ... COMMIT over one connection` | `startSession(), startTransaction()` | yes | Covered by `a_transaction_sees_its_own_writes_and_nobody_else_does` in `tests/concurrent_transactions_test.rs`. |
| Explicit rollback | `ROLLBACK` | `abortTransaction()` | yes | Covered by `rolling_back_leaves_nothing_and_the_transaction_is_over` in `tests/concurrent_transactions_test.rs`. |
| Two writers of one row cannot both win | `serialization failure, row locks` | `WriteConflict` | yes | Covered by `the_second_to_commit_a_change_to_the_same_document_is_refused` in `tests/concurrent_transactions_test.rs`. |
| Read-then-write in one statement is atomic | `UPDATE ... SET n = n + 1` | `$inc` | yes | Covered by `concurrent_increments_are_all_counted` in `tests/concurrent_transactions_test.rs`. |
| A commit that was acknowledged survives a crash | `durability` | `writeConcern: { j: true }` | yes | Covered by `acknowledged_writes_survive_a_crash_at_any_point` in `tests/crash_simulation_test.rs`. |
| Partial rollback inside a transaction | `SAVEPOINT, ROLLBACK TO` | None | no | A transaction is kept whole or dropped whole. |
| Choice of isolation level | `SET TRANSACTION ISOLATION LEVEL ...` | `readConcern, writeConcern` | no | One level: a transaction reads the database as of its start and is refused at commit if a row it changed was changed by another. |
| Locking rows for a later write | `SELECT ... FOR UPDATE` | None | no | No row locks are taken by reads. A conflicting commit is refused and retried instead. |

## Search and specialised data

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Nearest vectors | `ORDER BY embedding <-> $1 (pgvector)` | `$vectorSearch (Atlas)` | yes | `schema docs { id: String, embedding: Vector(3), index(.embedding, metric: "cosine") }`<br>`docs.insert [ { id: "d1", embedding: [1.0, 0.0, 0.0] }, { id: "d2", embedding: [0.0, 1.0, 0.0] } ]` |
| Full-text search | `to_tsvector, @@, ts_rank` | `$text, $search (Atlas)` | no | No word index, stemming or ranking. Text is matched with `contains` and `matches`, which read the rows.<br>Refused: `customers.search("ada")` |
| Places and distances | `PostGIS` | `2dsphere, $near, $geoWithin` | no | No geographic types or indexes.<br>Refused: `customers.where { near(.address, [3.4, 6.5], 1000) }` |
| Time series in buckets | `date_trunc, date_bin` | `$dateTrunc, time series collections` | yes | `orders.group_by { week: .placed.format("%G-W%V") }.select { week, n: count(), total: sum(.total) }.sort { .week }.limit(3)` |
| Rows that expire | A scheduled DELETE | `TTL index` | no | Nothing deletes rows by age. It is a `delete()` run on a schedule. |
| Being told when rows change | `LISTEN, NOTIFY, logical replication` | `change streams` | no | There is change-stream code in the repository, but no test drives it, so it is not counted. |

## Operations

| Feature | SQL | MongoDB | Owl | In Owl |
|---|---|---|---|---|
| Backup and restore | `pg_dump, pg_basebackup` | `mongodump, mongorestore` | yes | Covered by `a_restored_backup_answers_as_the_original_did` in `tests/backup_restore_test.rs`. |
| Backup while writes continue | `pg_basebackup` | `mongodump --oplog` | yes | Covered by `a_backup_taken_during_writes_is_one_moment_of_the_database` in `tests/backup_restore_test.rs`. |
| Readers never see half of a write | `MVCC` | `WiredTiger snapshots` | yes | Covered by `updates_to_many_rows_at_once_do_not_tear` in `tests/concurrent_transactions_test.rs`. |
| Users, roles and permissions | `CREATE ROLE, GRANT, row-level security` | `createUser(), roles` | no | Whoever can open the database can read and write all of it. |
| Copies of the database on other machines | `streaming replication` | `replica sets` | no | One process owns a database. There is replication code in the repository, but no test drives it, so it is not counted. |
| One database spread over machines | Extensions such as Citus | `sharding` | no | A database lives on one machine. |

## Defects

Things the engine gets wrong that writing a row turned up. A row uses a form
that works; the defect is listed here and pinned by the test with the wrong
answer it gives, so fixing it fails the test until it is taken off this list.

None at present.
