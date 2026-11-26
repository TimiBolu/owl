// The rows of the completeness matrix. Included by `completeness_matrix_test.rs`.

const INTRODUCTION: &str = r#"# Completeness matrix

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
"#;

const DEFECTS: &str = r#"
## Defects

Things the engine gets wrong that writing a row turned up. A row uses a form
that works; the defect is listed here and pinned by the test with the wrong
answer it gives, so fixing it fails the test until it is taken off this list.
"#;

fn matrix() -> Vec<(&'static str, Vec<Row>)> {
    vec![
        ("Filtering", filtering()),
        ("Shaping and ordering", shaping()),
        ("Aggregation", aggregation()),
        ("Relationships and joins", relationships()),
        ("Subqueries and sets", subqueries()),
        ("Window functions", windows()),
        ("Documents and lists", documents()),
        ("Writing", writing()),
        ("Schema", schema()),
        ("Indexes", indexes()),
        ("Transactions", transactions()),
        ("Search and specialised data", search()),
        ("Operations", operations()),
    ]
}

fn filtering() -> Vec<Row> {
    vec![
        yes("Equality", "WHERE status = 'paid'", r#"{ status: "paid" }"#,
            r#"orders.where { .status == "paid" }.select { id }"#, r#"{"id":"o1"} {"id":"o2"} {"id":"o4"} {"id":"o6"}"#),
        yes("Comparison", "WHERE total > 50", "{ total: { $gt: 50 } }",
            "orders.where { .total > 50 }.select { id }", r#"{"id":"o2"} {"id":"o3"}"#),
        yes("And, or, not", "WHERE a AND (b OR NOT c)", "$and, $or, $nor, $not",
            r#"orders.where { .total > 20 && (.status == "open" || !(.total < 100)) }.select { id }"#, r#"{"id":"o2"} {"id":"o3"}"#),
        yes("In a list", "WHERE city IN ('Lagos', 'Accra')", "{ city: { $in: [...] } }",
            r#"customers.where { .city in ["Lagos", "Nairobi"] }.select { id }"#, r#"{"id":"c1"} {"id":"c3"} {"id":"c4"}"#),
        yes("Not in a list", "WHERE city NOT IN (...)", "{ city: { $nin: [...] } }",
            r#"customers.where { .city not in ["Lagos", "Nairobi"] }.select { id }"#, r#"{"id":"c2"} {"id":"c5"}"#),
        yes("Between", "WHERE total BETWEEN 25 AND 60", "{ total: { $gte: 25, $lte: 60 } }",
            "orders.where { .total between 25 and 60 }.select { id }", r#"{"id":"o1"} {"id":"o3"} {"id":"o4"} {"id":"o6"}"#),
        yes("Text starts with, ends with, contains", "WHERE name LIKE 'A%'", "{ name: /^A/ }",
            r#"customers.where { .name like "A%" || .email like "%.org" || .city contains "rob" }.select { id }"#, r#"{"id":"c1"} {"id":"c3"} {"id":"c4"} {"id":"c5"}"#),
        yes("Regular expression", "WHERE email ~ '^[a-c].*io$'", "{ email: { $regex: ... } }",
            r#"customers.where { matches(.email, "^[a-c].*io$") }.select { id }"#, r#"{"id":"c1"}"#),
        yes("Ignoring case", "WHERE name ILIKE 'ada'", r#"{ name: { $regex: "^ada$", $options: "i" } }"#,
            r#"customers.where { .name ilike "ada" || lower(.name) == "eve" }.select { id }"#, r#"{"id":"c1"} {"id":"c5"}"#),
        yes("Is null", "WHERE email IS NULL", "{ email: null }",
            "customers.where { .email is null }.select { id }", r#"{"id":"c2"}"#),
        yes("Is not null", "WHERE referrer IS NOT NULL", "{ referrer: { $ne: null } }",
            "customers.where { .referrer is not null }.select { id }", r#"{"id":"c2"} {"id":"c3"} {"id":"c4"}"#),
        yes("Field is present", "(columns always exist)", "{ email: { $exists: true } }",
            "customers.where { exists(.email) }.select { id }", r#"{"id":"c1"} {"id":"c3"} {"id":"c4"} {"id":"c5"}"#),
        yes("One field against another", "WHERE price > stock", "{ $expr: { $gt: [\"$price\", \"$stock\"] } }",
            "products.where { .price > .stock }.select { id }", r#"{"id":"p3"} {"id":"p4"}"#),
        yes("Arithmetic in a condition", "WHERE price * stock > 400", "{ $expr: { $gt: [{ $multiply: [...] }, 400] } }",
            "products.where { .price * .stock > 400 }.select { id }", r#"{"id":"p2"} {"id":"p4"}"#),
        yes("Dates and parts of dates", "WHERE EXTRACT(YEAR FROM joined) = 2025", "{ $expr: { $eq: [{ $year: \"$joined\" }, 2025] } }",
            r#"customers.where { .joined.year() == 2025 && .joined < "2025-07-01" }.select { id }"#, r#"{"id":"c3"}"#),
        yes("Kind of value", "pg_typeof(x)", "{ x: { $type: \"string\" } }",
            r#"customers.where { type_of(.email) == "String" }.count()"#, r#"{"count":4}"#),
    ]
}

fn shaping() -> Vec<Row> {
    vec![
        yes("Chosen fields", "SELECT id, name", "{ id: 1, name: 1 }",
            r#"customers.where { .id == "c1" }.select { id, name }"#, r#"{"id":"c1","name":"Ada"}"#),
        yes("Every field", "SELECT *", "find()",
            r#"products.where { .id == "p1" }"#, r#"{"category":"stationery","id":"p1","name":"Pen","price":2.0,"stock":100}"#),
        yes("Every field but some", "(list the rest)", "{ stock: 0 }",
            r#"products.where { .id == "p1" }.select { *, -stock, -category }"#, r#"{"id":"p1","name":"Pen","price":2.0}"#),
        yes("Renamed fields", "SELECT name AS customer", "{ customer: \"$name\" }",
            r#"customers.where { .id == "c1" }.select { customer: .name }"#, r#"{"customer":"Ada"}"#),
        yes("Computed fields", "SELECT price * stock AS worth", "{ $multiply: [\"$price\", \"$stock\"] }",
            r#"products.where { .id == "p2" }.select { id, worth: .price * .stock }"#, r#"{"id":"p2","worth":500.0}"#),
        yes("Distinct rows", "SELECT DISTINCT city", "distinct(\"city\")",
            "customers.select { city }.distinct().sort { .city }", r#"{"city":"Accra"} {"city":"Lagos"} {"city":"Nairobi"}"#),
        yes("Conditional value", "CASE WHEN ... THEN ... ELSE ... END", "$cond, $switch",
            r#"products.sort { .id }.limit(3).select { id, level: if .stock == 0 then "out" else if .stock < 50 then "low" else "fine" }"#, r#"{"id":"p1","level":"fine"} {"id":"p2","level":"low"} {"id":"p3","level":"out"}"#),
        yes("Matching on a value", "CASE status WHEN 'paid' THEN ... END", "$switch",
            r#"orders.sort { .id }.limit(3).select { id, done: match .status { "paid" -> true, _ -> false } }"#, r#"{"done":true,"id":"o1"} {"done":true,"id":"o2"} {"done":false,"id":"o3"}"#),
        yes("First value that is present", "COALESCE(email, 'none')", "$ifNull",
            r#"customers.where { .id in ["c1", "c2"] }.sort { .id }.select { id, email: .email or "none" }"#, r#"{"email":"ada@x.io","id":"c1"} {"email":"none","id":"c2"}"#),
        yes("Conversion between types", "CAST(stock AS text)", "$toString, $toInt, $toDouble",
            r#"products.where { .id == "p1" }.select { text: to_string(.stock), whole: to_int(.price), real: to_float(.stock), number: to_int("42") }"#, r#"{"number":42,"real":100.0,"text":"100","whole":2}"#),
        yes("Text functions", "upper, lower, length, substring, replace, trim, ||", "$toUpper, $strLenCP, $substrCP, $replaceAll, $trim, $concat",
            r#"customers.where { .id == "c1" }.select { up: upper(.name), size: length(.email), part: substring(.email, 0, 3), swapped: replace(.email, "x.io", "owl.dev"), joined: concat(.name, " of ", .city), pieces: split(.email, "@") }"#, r#"{"joined":"Ada of Lagos","part":"ada","pieces":["ada","x.io"],"size":8,"swapped":"ada@owl.dev","up":"ADA"}"#),
        yes("Number functions", "abs, round, floor, ceil, sqrt, power", "$abs, $round, $floor, $ceil, $sqrt, $pow",
            r#"products.where { .id == "p2" }.select { rounded: round(.price), down: floor(.price), up: ceil(.price), root: sqrt(.stock + 9), squared: pow(.price, 2), away: abs(.stock - 100) }"#, r#"{"away":60,"down":12,"root":7.0,"rounded":13,"squared":156.25,"up":13}"#),
        yes("Date arithmetic", "joined + INTERVAL '30 days', a - b", "$dateAdd, $dateDiff",
            r#"customers.where { .id == "c3" }.select { later: (.joined + days(30)).format("%Y-%m-%d"), waited: diff_days(.referrer.joined, .joined) }"#, r#"{"later":"2025-07-20","waited":527}"#),
        yes("Date formatting", "to_char(joined, 'YYYY-MM')", "$dateToString",
            r#"customers.where { .id == "c1" }.select { month: .joined.format("%Y-%m") }"#, r#"{"month":"2024-01"}"#),
        yes("Nested output", "json_build_object(...)", "{ who: { name: \"$name\" } }",
            r#"customers.where { .id == "c1" }.select { id, who: { name: .name, place: { city: .city, zip: .address.zip } } }"#, r#"{"id":"c1","who":{"name":"Ada","place":{"city":"Lagos","zip":"100001"}}}"#),
        yes("List in the output", "ARRAY[a, b]", "[\"$a\", \"$b\"]",
            r#"products.where { .id == "p1" }.select { id, figures: [.price, .stock] }"#, r#"{"figures":[2.0,100],"id":"p1"}"#),
        yes("Order by several keys", "ORDER BY city ASC, joined DESC", "sort({ city: 1, joined: -1 })",
            "customers.sort { .city, .joined.desc }.select { id }", r#"{"id":"c5"} {"id":"c2"} {"id":"c3"} {"id":"c1"} {"id":"c4"}"#),
        yes("Order by an expression", "ORDER BY price * stock DESC", "$addFields then $sort",
            "products.sort { (.price * .stock).desc, .id }.select { id }", r#"{"id":"p4"} {"id":"p2"} {"id":"p1"} {"id":"p5"} {"id":"p3"}"#),
        yes("Where absent values sort", "ORDER BY email NULLS LAST", "(absent sorts first)",
            "customers.sort { .email.nulls_last }.select { id }", r#"{"id":"c1"} {"id":"c3"} {"id":"c4"} {"id":"c5"} {"id":"c2"}"#)
            .note("An absent value sorts below every other unless the key says `.nulls_last`."),
        yes("Limit and offset", "LIMIT 2 OFFSET 1", "skip(1).limit(2)",
            "products.sort { .price.desc }.offset(1).limit(2).select { id }", r#"{"id":"p3"} {"id":"p2"}"#),
        yes("Paging from the last row seen", "WHERE (price, id) > (12.5, 'p2') ORDER BY price, id LIMIT 2", "find({ price: { $gt: 12.5 } }).sort().limit(2)",
            r#"products.where { (.price, .id) > (12.5, "p2") }.sort { .price, .id }.limit(2).select { id }"#, r#"{"id":"p3"} {"id":"p4"}"#),
        tested("Parameters", "WHERE label = $1", "(values are passed as data)",
            "tests/oql_language_conformance_test.rs", "parameters_carry_values_into_a_fixed_query")
            .note("`$name` in the query, bound by name before it runs."),
        yes("Random sample", "ORDER BY random() LIMIT 2", "{ $sample: { size: 2 } }",
            "products.sample(2).count()", r#"{"count":2}"#),
    ]
}

fn aggregation() -> Vec<Row> {
    vec![
        yes("Count", "SELECT count(*)", "countDocuments()",
            "orders.count()", r#"{"count":6}"#),
        yes("Sum, average, least, greatest", "sum, avg, min, max", "$sum, $avg, $min, $max",
            "orders.select { sum: sum(.total), avg: avg(.total), least: min(.total), most: max(.total) }", r#"{"avg":52.666666666666664,"least":12.5,"most":150.0,"sum":316.0}"#),
        yes("Group by", "GROUP BY status", "{ $group: { _id: \"$status\" } }",
            "orders.group_by { .status }.select { status, n: count(), total: sum(.total) }.sort { .status }", r#"{"n":1,"status":"cancelled","total":12.5} {"n":1,"status":"open","total":60.0} {"n":4,"status":"paid","total":243.5}"#),
        yes("Group by several keys", "GROUP BY city, status", "{ _id: { city: ..., status: ... } }",
            "orders.group_by { .customer.city, .status }.select { city, status, n: count() }.sort { .city, .status }", r#"{"city":"Accra","n":1,"status":"open"} {"city":"Lagos","n":1,"status":"cancelled"} {"city":"Lagos","n":3,"status":"paid"} {"city":"Nairobi","n":1,"status":"paid"}"#),
        yes("Group by an expression", "GROUP BY date_trunc('month', placed)", "{ _id: { $dateTrunc: ... } }",
            r#"orders.group_by { month: .placed.format("%Y-%m") }.select { month, n: count() }.sort { .month }"#, r#"{"month":"2026-01","n":1} {"month":"2026-02","n":2} {"month":"2026-03","n":3}"#),
        yes("Filter on a group", "HAVING count(*) > 1", "$match after $group",
            "orders.group_by { .customer }.having { count() > 1 }.select { customer: .customer.id, n: count() }.sort { .customer }", r#"{"customer":"c1","n":2} {"customer":"c3","n":2}"#),
        yes("Count of distinct values", "count(DISTINCT customer)", "$addToSet then $size",
            "orders.customer.distinct().count()", r#"{"count":4}"#),
        yes("Aggregate over some of the rows", "count(*) FILTER (WHERE status = 'paid')", "{ $sum: { $cond: [...] } }",
            r#"orders.group_by { .customer }.select { customer: .customer.id, paid: count(where .status == "paid"), paid_total: sum(.total where .status == "paid") }.sort { .customer }"#, r#"{"customer":"c1","paid":2,"paid_total":176.5} {"customer":"c2","paid":0,"paid_total":0} {"customer":"c3","paid":1,"paid_total":42.0} {"customer":"c4","paid":1,"paid_total":25.0}"#),
        yes("Median and percentiles", "percentile_cont(0.5) WITHIN GROUP (ORDER BY total)", "$median, $percentile",
            "orders.select { median: median(.total), p90: percentile(.total, 0.9) }", r#"{"median":34.25,"p90":105.0}"#),
        yes("Spread", "stddev_samp, var_pop", "$stdDevSamp, $stdDevPop",
            "products.group_by { .category }.select { category, sample: variance(.stock), whole: .stock.variance_pop(), sd: .stock.stddev_pop() }.sort { .category }", r#"{"category":"home","sample":12.5,"sd":2.5,"whole":6.25} {"category":"kitchen","sample":null,"sd":0.0,"whole":0.0} {"category":"stationery","sample":1800.0,"sd":30.0,"whole":900.0}"#),
        yes("Values of a group as a list", "array_agg(total)", "{ $push: \"$total\" }",
            "orders.group_by { .status }.select { status, totals: .total }.sort { .status }", r#"{"status":"cancelled","totals":[12.5]} {"status":"open","totals":[60.0]} {"status":"paid","totals":[26.5,150.0,42.0,25.0]}"#),
        yes("Distinct values of a group as a list", "array_agg(DISTINCT city)", "{ $addToSet: \"$city\" }",
            "orders.group_by { .status }.select { status, cities: .customer.city.distinct() }.sort { .status }", r#"{"cities":["Lagos"],"status":"cancelled"} {"cities":["Accra"],"status":"open"} {"cities":["Lagos","Nairobi"],"status":"paid"}"#),
        yes("First or last row of each group", "DISTINCT ON (customer) ... ORDER BY placed DESC", "{ $first: ... } after $sort",
            "orders.sort { .placed.desc }.group_by { .customer }.select { customer: .customer.id, latest: first().id }.sort { .customer }", r#"{"customer":"c1","latest":"o2"} {"customer":"c2","latest":"o3"} {"customer":"c3","latest":"o5"} {"customer":"c4","latest":"o6"}"#),
        yes("Ranges as groups", "width_bucket(total, ...) or CASE", "$bucket",
            r#"orders.group_by { size: if .total < 30 then "small" else if .total < 100 then "medium" else "large" }.select { size, n: count() }.sort { .size }"#, r#"{"n":1,"size":"large"} {"n":2,"size":"medium"} {"n":3,"size":"small"}"#),
        yes("Several summaries in one answer", "(one query each, or UNION ALL)", "$facet",
            r#"customers.limit(1).select { by_status: orders.group_by { .status }.select { status, n: count() }.sort { .status }, dearest: orders.sort { .total.desc }.limit(2).select { id } }"#, r#"{"by_status":[{"n":1,"status":"cancelled"},{"n":1,"status":"open"},{"n":4,"status":"paid"}],"dearest":[{"id":"o2"},{"id":"o3"}]}"#),
        yes("Subtotals and grand totals", "GROUP BY ROLLUP (city, status), CUBE", "(several $group stages in $facet)",
            "orders.group_by { rollup(.customer.city, .status) }.select { city, status, n: count() }.sort { .city.nulls_last, .status.nulls_last }", r#"{"city":"Accra","n":1,"status":"open"} {"city":"Accra","n":1,"status":null} {"city":"Lagos","n":1,"status":"cancelled"} {"city":"Lagos","n":3,"status":"paid"} {"city":"Lagos","n":4,"status":null} {"city":"Nairobi","n":1,"status":"paid"} {"city":"Nairobi","n":1,"status":null} {"city":null,"n":6,"status":null}"#)
            .note("`rollup` gives a subtotal for each leading run of the keys and a grand total; `cube` gives one for every combination. A key a subtotal leaves out is null in its row."),
    ]
}

fn relationships() -> Vec<Row> {
    vec![
        yes("Row and the row it refers to", "JOIN customers c ON c.id = o.customer", "$lookup then $unwind",
            r#"orders.where { .id == "o3" }.select { id, customer: .customer.name, city: .customer.city }"#, r#"{"city":"Accra","customer":"Bob","id":"o3"}"#)
            .note("A reference is followed with a dot. No join is written."),
        yes("Filter through a reference", "JOIN ... WHERE c.city = 'Lagos'", "$lookup then $match",
            r#"orders.where { .customer.city == "Lagos" }.select { id }"#, r#"{"id":"o1"} {"id":"o2"} {"id":"o4"} {"id":"o5"}"#),
        yes("Optional reference", "LEFT JOIN customers r ON r.id = c.referrer", "$lookup, preserveNullAndEmptyArrays",
            "customers.sort { .id }.limit(2).select { id, referrer: .referrer.name }", r#"{"id":"c1","referrer":null} {"id":"c2","referrer":"Ada"}"#),
        yes("Rows that refer to a row", "LEFT JOIN orders o ON o.customer = c.id GROUP BY c.id", "$lookup then $size",
            "customers.sort { .id }.select { id, orders: .orders.count(), spent: .orders.sum(.total) }", r#"{"id":"c1","orders":2,"spent":176.5} {"id":"c2","orders":1,"spent":60.0} {"id":"c3","orders":2,"spent":54.5} {"id":"c4","orders":1,"spent":25.0} {"id":"c5","orders":0,"spent":0}"#)
            .note("Every reference can be followed backwards under the name of the collection that holds it."),
        yes("Related rows nested in the answer", "json_agg(o.*) in a lateral join", "$lookup",
            r#"customers.where { .id == "c3" }.select { id, orders: .orders.sort { .id }.select { id, status } }"#, r#"{"id":"c3","orders":[{"id":"o4","status":"paid"},{"id":"o5","status":"cancelled"}]}"#),
        yes("Several steps", "three JOINs", "three $lookup stages",
            r#"customers.where { .id == "c4" }.select { id, through: .referrer.referrer.name, bought: .orders.items.product.name }"#, r#"{"bought":["Book"],"id":"c4","through":"Ada"}"#),
        yes("Row related to its own collection", "JOIN customers r ON r.id = c.referrer", "$lookup from the same collection",
            r#"customers.where { .referred.exists() }.sort { .id }.select { id, referred: .referred.count() }"#, r#"{"id":"c1","referred":2} {"id":"c3","referred":1}"#),
        yes("Many to many", "JOIN through a link table", "$lookup through a list of ids",
            r#"products.sort { .id }.select { id, buyers: orders.where { products.id in .items.product.id }.customer.distinct().count() }"#, r#"{"buyers":2,"id":"p1"} {"buyers":3,"id":"p2"} {"buyers":2,"id":"p3"} {"buyers":1,"id":"p4"} {"buyers":0,"id":"p5"}"#),
        yes("Rows with a related row", "WHERE EXISTS (SELECT 1 FROM orders ...)", "$lookup then $match",
            r#"customers.where { .orders.any { .status == "paid" } }.sort { .id }.select { id }"#, r#"{"id":"c1"} {"id":"c3"} {"id":"c4"}"#),
        yes("Rows with no related row", "WHERE NOT EXISTS (...)", "$lookup then $match on empty",
            "customers.where { !.orders.exists() }.select { id }", r#"{"id":"c5"}"#),
        yes("Join on any condition", "JOIN customers b ON b.city = a.city AND b.id <> a.id", "$lookup with a pipeline",
            "customers.sort { .id }.select { id, neighbours: customers.where { .city == customers.city && .id != customers.id }.select { id } }", r#"{"id":"c1","neighbours":[{"id":"c3"}]} {"id":"c2","neighbours":[{"id":"c5"}]} {"id":"c3","neighbours":[{"id":"c1"}]} {"id":"c4","neighbours":[]} {"id":"c5","neighbours":[{"id":"c2"}]}"#),
        yes("Every row with every row", "CROSS JOIN", "$lookup with an empty pipeline",
            r#"customers.where { .city == "Accra" }.sort { .id }.select { id, with: products.where { .price > 20 }.sort { .id }.select { id } }"#, r#"{"id":"c2","with":[{"id":"p3"},{"id":"p4"}]} {"id":"c5","with":[{"id":"p3"},{"id":"p4"}]}"#)
            .differently()
            .note("The second side comes nested under each row of the first, not as one flat row per pair."),
        no("Unmatched rows from both sides", "FULL OUTER JOIN", "(two $lookup stages and $unionWith)",
            "No single form. Each side is a query, and `union` puts them together."),
        yes("Hierarchy of any depth", "WITH RECURSIVE", "$graphLookup",
            r#"customers.where { .id == "c4" }.walk(.referrer).sort { ._depth }.select { id, depth: ._depth }"#, r#"{"depth":1,"id":"c3"} {"depth":2,"id":"c1"}"#),
        yes("Everything below a row", "WITH RECURSIVE, downwards", "$graphLookup",
            r#"customers.where { .id == "c1" }.select { id, below: .walk(.referred).count(), deepest: .walk(.referred).max(._depth) }"#, r#"{"below":3,"deepest":2,"id":"c1"}"#),
    ]
}

fn subqueries() -> Vec<Row> {
    vec![
        yes("One value from another query", "WHERE total > (SELECT avg(total) FROM orders)", "(two queries, or $lookup)",
            "orders.where { .total > orders.avg(.total) }.select { id }", r#"{"id":"o2"} {"id":"o3"}"#),
        yes("Query that depends on the row", "WHERE total = (SELECT max(total) ... WHERE customer = o.customer)", "$lookup with let",
            "orders.where { .total == .customer.orders.max(.total) }.sort { .id }.select { id }", r#"{"id":"o2"} {"id":"o3"} {"id":"o4"} {"id":"o6"}"#),
        yes("In the rows of another query", "WHERE id IN (SELECT customer FROM orders WHERE total > 50)", "(two queries, or $lookup)",
            "customers.where { .id in orders.where { .total > 50 }.customer.id }.select { id }", r#"{"id":"c1"} {"id":"c2"}"#),
        tested("Named query used later", "WITH paid AS (...) SELECT ... FROM paid", "(variables in application code)",
            "tests/oql_language_conformance_test.rs", "a_binding_means_the_expression_it_names")
            .note("`paid = orders.where { .status == \"paid\" }` on a line of its own names the rows for the statements after it."),
        yes("Rows of either query", "UNION", "$unionWith then $group",
            r#"customers.where { .city == "Nairobi" }.union(customers.where { "vip" in .tags }).sort { .id }.select { id }"#, r#"{"id":"c1"} {"id":"c3"} {"id":"c4"}"#),
        yes("Rows of both, repeats kept", "UNION ALL", "$unionWith",
            r#"(customers.where { .city == "Lagos" } + customers.where { "vip" in .tags }).count()"#, r#"{"count":4}"#),
        yes("Rows in both queries", "INTERSECT", "(no stage)",
            r#"customers.where { .city == "Accra" }.intersect(customers.where { .email != null }).select { id }"#, r#"{"id":"c5"}"#),
        yes("Rows in one query and not the other", "EXCEPT", "(no stage)",
            r#"customers.where { .city == "Accra" }.except(customers.where { .email != null }).select { id }"#, r#"{"id":"c2"}"#),
    ]
}

fn windows() -> Vec<Row> {
    vec![
        yes("Position in a partition", "row_number() OVER (PARTITION BY customer ORDER BY placed)", "$setWindowFields, $documentNumber",
            "orders.sort { .customer.id, .placed }.partition_by { .customer }.select { id, n: row_number() }", r#"{"id":"o1","n":1} {"id":"o2","n":2} {"id":"o3","n":1} {"id":"o4","n":1} {"id":"o5","n":2} {"id":"o6","n":1}"#),
        yes("Rank, with and without gaps", "rank(), dense_rank()", "$rank, $denseRank",
            "products.sort { .category, .price.desc }.partition_by { .category }.select { id, rank: rank(.price), dense: dense_rank(.price) }", r#"{"dense":1,"id":"p4","rank":1} {"dense":2,"id":"p3","rank":2} {"dense":1,"id":"p5","rank":1} {"dense":1,"id":"p2","rank":1} {"dense":2,"id":"p1","rank":2}"#),
        yes("Row before and row after", "lag(total), lead(total)", "$shift",
            "orders.sort { .placed }.select { id, before: prev(.total), after: next(.total) }.limit(3)", r#"{"after":150.0,"before":null,"id":"o1"} {"after":60.0,"before":26.5,"id":"o2"} {"after":42.0,"before":150.0,"id":"o3"}"#),
        yes("Running total", "sum(total) OVER (ORDER BY placed)", "$sum with a window of documents",
            "orders.sort { .placed }.select { id, running: so_far().total.sum() }", r#"{"id":"o1","running":26.5} {"id":"o2","running":176.5} {"id":"o3","running":236.5} {"id":"o4","running":278.5} {"id":"o5","running":291.0} {"id":"o6","running":316.0}"#),
        yes("Moving window", "avg(total) OVER (ROWS 1 PRECEDING)", "$avg with window: { documents: [-1, 0] }",
            "orders.sort { .placed }.select { id, recent: so_far().last(2).total.avg() }", r#"{"id":"o1","recent":26.5} {"id":"o2","recent":88.25} {"id":"o3","recent":105.0} {"id":"o4","recent":51.0} {"id":"o5","recent":27.25} {"id":"o6","recent":18.75}"#),
        yes("Share of a partition", "total / sum(total) OVER (PARTITION BY customer)", "$setWindowFields without a window",
            "orders.sort { .id }.partition_by { .customer }.select { id, of: sum(.total), size: count() }.limit(3)", r#"{"id":"o1","of":176.5,"size":2} {"id":"o2","of":176.5,"size":2} {"id":"o3","of":60.0,"size":1}"#),
        yes("First and last of a partition", "first_value(id), last_value(id)", "$first, $last in $setWindowFields",
            "orders.sort { .customer.id, .placed }.partition_by { .customer }.select { id, opened_with: first().id, latest: last(1).select { id } }.limit(3)", r#"{"id":"o1","latest":[{"id":"o2"}],"opened_with":"o1"} {"id":"o2","latest":[{"id":"o2"}],"opened_with":"o1"} {"id":"o3","latest":[{"id":"o3"}],"opened_with":"o3"}"#),
        yes("Top rows of each partition", "WHERE row_number() ... <= 1 in a subquery", "$topN",
            "orders.sort { .customer.id, .total.desc }.partition_by { .customer }.select { id, n: row_number() }.where { .n == 1 }.select { id }", r#"{"id":"o2"} {"id":"o3"} {"id":"o4"} {"id":"o6"}"#),
        yes("Equal-sized buckets and relative rank", "ntile(4), percent_rank(), cume_dist()", "(no operator)",
            "orders.sort { .total }.select { id, quarter: ntile(4), rank: percent_rank(.total), within: round(cume_dist(.total), 2) }", r#"{"id":"o5","quarter":1,"rank":0.0,"within":0.17} {"id":"o6","quarter":1,"rank":0.2,"within":0.33} {"id":"o1","quarter":2,"rank":0.4,"within":0.5} {"id":"o4","quarter":2,"rank":0.6,"within":0.67} {"id":"o3","quarter":3,"rank":0.8,"within":0.83} {"id":"o2","quarter":4,"rank":1.0,"within":1.0}"#),
    ]
}

fn documents() -> Vec<Row> {
    vec![
        yes("Field inside an object", "address->>'zip'", "\"address.zip\"",
            r#"customers.where { .address.zip == "00233" }.sort { .id }.select { id, street: .address.street }"#, r#"{"id":"c2","street":"2 Ring Rd"} {"id":"c5","street":"5 Oxford St"}"#),
        yes("List holds a value", "'vip' = ANY(tags)", "{ tags: \"vip\" }",
            r#"customers.where { "vip" in .tags }.sort { .id }.select { id }"#, r#"{"id":"c1"} {"id":"c3"}"#),
        yes("List holds all of some values", "tags @> ARRAY['vip', 'early']", "{ tags: { $all: [...] } }",
            r#"customers.where { .tags contains_all ["vip", "early"] }.select { id }"#, r#"{"id":"c1"}"#),
        yes("Length of a list", "cardinality(tags)", "$size",
            "customers.where { .tags.count() == 0 }.sort { .id }.select { id }", r#"{"id":"c2"} {"id":"c5"}"#),
        yes("Element by position", "tags[1]", "$arrayElemAt, $first, $last",
            r#"customers.where { .id == "c1" }.select { first: .tags[0], last: .tags[-1], none: .tags[9] }"#, r#"{"first":"vip","last":"early","none":null}"#),
        yes("One element meets several conditions", "EXISTS (SELECT 1 FROM unnest(items) ...)", "$elemMatch",
            "orders.where { .items.any { .qty > 5 && .price < 5 } }.sort { .id }.select { id }", r#"{"id":"o1"} {"id":"o4"}"#),
        yes("Every element meets a condition", "NOT EXISTS (... WHERE NOT ...)", "{ $not: { $elemMatch: ... } }",
            "orders.where { .items.all { .qty >= 2 } }.sort { .id }.select { id }", r#"{"id":"o3"} {"id":"o6"}"#),
        yes("One row per element", "CROSS JOIN LATERAL unnest(items)", "$unwind",
            "orders.items.sort { orders.id, .qty.desc }.limit(3).select { order: orders.id, product: .product.name, qty }", r#"{"order":"o1","product":"Pen","qty":7} {"order":"o1","product":"Book","qty":1} {"order":"o2","product":"Desk","qty":1}"#),
        yes("Group over elements", "unnest then GROUP BY", "$unwind then $group",
            "orders.items.group_by { product: .product.id }.select { product, sold: sum(.qty), revenue: sum(.qty * .price) }.sort { .product }", r#"{"product":"p1","revenue":26.0,"sold":13} {"product":"p2","revenue":50.0,"sold":4} {"product":"p3","revenue":90.0,"sold":3} {"product":"p4","revenue":150.0,"sold":1}"#),
        yes("One field of the elements that match", "(subquery over unnest)", "$filter then $map",
            r#"customers.where { .id == "c1" }.select { big: .orders.where { .total > 100 }.total, dear: .orders.items.where { .price > 10 }.product.name }"#, r#"{"big":[150.0],"dear":["Book","Desk"]}"#),
        yes("Some of a list in the answer", "(subquery over unnest)", "$filter",
            r#"orders.where { .id == "o4" }.select { id, small: .items.where { .price < 10 }.select { qty, price } }"#, r#"{"id":"o4","small":[{"price":2.0,"qty":6}]}"#),
        yes("Each element reshaped", "(subquery over unnest)", "$map",
            r#"orders.where { .id == "o1" }.select { id, lines: .items.select { name: .product.name, cost: .qty * .price } }"#, r#"{"id":"o1","lines":[{"cost":14.0,"name":"Pen"},{"cost":12.5,"name":"Book"}]}"#),
        yes("List reduced to a value", "(subquery over unnest)", "$reduce, $sum",
            "orders.sort { .id }.limit(2).select { id, units: .items.sum(.qty), value: .items.sum(.qty * .price) }", r#"{"id":"o1","units":8,"value":26.5} {"id":"o2","units":1,"value":150.0}"#),
        yes("Part of a list", "tags[1:1]", "$slice",
            r#"orders.where { .id == "o1" }.select { id, first: .items[:1].select { qty }, rest: .items[1:].select { qty } }"#, r#"{"first":[{"qty":7}],"id":"o1","rest":[{"qty":1}]}"#),
        yes("List sorted", "(subquery with ORDER BY)", "$sortArray",
            r#"orders.where { .id == "o4" }.select { id, by_qty: .items.sort { .qty.desc }.select { qty } }"#, r#"{"by_qty":[{"qty":6},{"qty":1}],"id":"o4"}"#),
        yes("Lists joined", "a || b", "$concatArrays",
            r#"customers.where { .id == "c1" }.select { all: .tags + ["checked"], size: (.tags + ["checked"]).count() }"#, r#"{"all":["vip","early","checked"],"size":3}"#),
        yes("Row with fields added", "to_jsonb(c) || '{...}'", "$addFields, $mergeObjects",
            r#"products.where { .id == "p5" }.select { *, worth: .price * .stock }"#, r#"{"category":"kitchen","id":"p5","name":"Mug","price":8.0,"stock":25,"worth":200.0}"#),
        yes("Collection with no declared shape", "(a jsonb column)", "(the default)",
            "notes.insert [ { id: \"n1\", text: \"hello\", stars: 3 }, { id: \"n2\", about: { any: [1, \"two\"] } } ].select { id, stars, inner: .about.any[1] }",
            r#"{"id":"n1","inner":null,"stars":3} {"id":"n2","inner":"two","stars":null}"#),
        yes("Names of an object's fields as data", "jsonb_each, jsonb_object_keys", "$objectToArray",
            r#"customers.where { .id == "c1" }.select { names: keys(.address) }"#, r#"{"names":["street","zip"]}"#),
    ]
}

fn writing() -> Vec<Row> {
    vec![
        yes("Insert a row", "INSERT INTO products VALUES (...)", "insertOne()",
            r#"products.insert { id: "p6", name: "Clip", price: 0.5, stock: 900, category: "stationery" }.select { name, stock }"#, r#"{"name":"Clip","stock":900}"#)
            .note("A write gives back the rows it wrote, so what follows it reads them."),
        yes("Insert many rows", "INSERT ... VALUES (...), (...)", "insertMany()",
            r#"products.insert [ { id: "p6", name: "Clip", price: 0.5, stock: 900, category: "stationery" }, { id: "p7", name: "Rug", price: 80.0, stock: 3, category: "home" } ].select { id, name }"#, r#"{"id":"p6","name":"Clip"} {"id":"p7","name":"Rug"}"#),
        yes("Update the rows a condition picks", "UPDATE ... SET ... WHERE ...", "updateMany()",
            r#"orders.where { .status == "open" }.update { status: "paid" }.select { id, status }"#, r#"{"id":"o3","status":"paid"}"#),
        yes("Update from the row's own values", "SET stock = stock - 1, price = price * 1.1", "$inc, $mul",
            r#"products.where { .id == "p2" }.update { stock -= 1, price: .price * 2 }.select { stock, price }"#, r#"{"price":25.0,"stock":39}"#),
        yes("Update from related rows", "UPDATE ... SET total = (SELECT sum(...) ...)", "(read, then write)",
            r#"orders.where { .id == "o1" }.update { total: .items.sum(.qty * .price) + .customer.orders.count() }.select { total }"#, r#"{"total":28.5}"#),
        yes("Update a field inside an object", "jsonb_set(address, '{zip}', ...)", "{ $set: { \"address.zip\": ... } }",
            r#"customers.where { .id == "c1" }.update { address.zip: "100002" }.select { address }"#, r#"{"address":{"street":"1 Marina","zip":"100002"}}"#),
        yes("Clear a field", "SET email = NULL", "{ $set: { email: null } }",
            r#"customers.where { .id == "c1" }.update { email: null }.select { id, email }"#, r#"{"email":null,"id":"c1"}"#),
        yes("Take a field off a row", "(columns always exist)", "$unset",
            r#"customers.where { .id == "c1" }.update { -email }.select { id, has_email: exists(.email) }"#, r#"{"has_email":false,"id":"c1"}"#)
            .note("`-email` takes the field off the row. `email: null` keeps it, holding null."),
        yes("Add to a list", "SET tags = tags || 'x'", "$push",
            r#"customers.where { .id == "c2" }.update { tags += "back" }.select { tags }"#, r#"{"tags":["back"]}"#),
        yes("Add to a list once", "(CASE WHEN ... )", "$addToSet",
            r#"customers.where { .id == "c1" }.update { tags: (.tags + ["vip", "gold"]).distinct() }.select { tags }"#, r#"{"tags":["vip","early","gold"]}"#),
        yes("Take out of a list", "array_remove(tags, 'vip')", "$pull",
            r#"customers.where { .id == "c1" }.update { tags -= "vip" }.select { tags }"#, r#"{"tags":["early"]}"#),
        yes("Change the elements of a list that match", "(rebuild the array)", "\"items.$[e].qty\" with arrayFilters",
            r#"orders.where { .id == "o1" }.update { items.where { .price < 5 }.qty += 1 }.select { qty: .items.qty }"#, r#"{"qty":[8,1]}"#),
        yes("Delete the rows a condition picks", "DELETE FROM ... WHERE ...", "deleteMany()",
            r#"orders.where { .status == "cancelled" }.delete().select { id }"#, r#"{"id":"o5"}"#),
        yes("Delete every row", "TRUNCATE, DELETE FROM", "deleteMany({})",
            "orders.delete().count()", r#"{"count":6}"#)
            .note("The rows deleted are given back, here counted."),
        yes("Insert or replace", "INSERT ... ON CONFLICT (id) DO UPDATE", "replaceOne(..., { upsert: true })",
            r#"products.upsert on id { id: "p1", name: "Pencil", price: 1.0, stock: 10, category: "stationery" }
products.upsert on id { id: "p9", name: "Tape", price: 3.0, stock: 10, category: "stationery" }"#, r#"{"id":"p1","name":"Pencil"} {"id":"p9","name":"Tape"}"#)
            .check(r#"products.where { .id in ["p1", "p9"] }.sort { .id }.select { id, name }"#),
        yes("Rows a write changed", "UPDATE ... RETURNING *", "findOneAndUpdate()",
            r#"products.where { .category == "home" }.update { stock += 1 }.select { id, stock }"#, r#"{"id":"p3","stock":1} {"id":"p4","stock":6}"#),
        yes("Insert the rows of a query", "INSERT INTO ... SELECT ...", "$out, $merge",
            r#"archive.insert(orders.where { .status == "cancelled" }).select { id, status }"#, r#"{"id":"o5","status":"cancelled"}"#),
        no("One statement that inserts, updates and deletes by a join", "MERGE", "$merge with whenMatched",
            "`upsert` covers insert-or-replace. Anything more is separate statements in a `transaction` block."),
    ]
}

fn schema() -> Vec<Row> {
    vec![
        yes("Typed fields", "CREATE TABLE t (n integer, ...)", "$jsonSchema validator",
            "schema gauges { id: String, n: Integer, ratio: Float, live: Boolean, at: DateTime, names: List(String) }\ngauges.insert { id: \"g1\", n: 7, ratio: 0.5, live: true, at: \"2026-01-01\", names: [\"a\"] }.select { n: type_of(.n), ratio: type_of(.ratio), live: type_of(.live), at: type_of(.at), names: type_of(.names) }", r#"{"at":"DateTime","live":"Boolean","n":"Integer","names":"List","ratio":"Float"}"#)
            .refusing("schema gauges { id: String, n: Integer }\ngauges.insert { id: \"g1\", n: \"seven\" }"),
        yes("Optional and required fields", "NOT NULL", "required: [...]",
            r#"customers.insert { id: "c9", name: "Zed", city: "Lagos", joined: "2026-01-01", tags: [], address: { street: "9", zip: "9" } }.select { id, email }"#, r#"{"email":null,"id":"c9"}"#)
            .refusing(r#"customers.insert { id: "c9", name: "Zed", joined: "2026-01-01", tags: [], address: { street: "9", zip: "9" } }"#)
            .note("`email: String?` may be left out. `city: String` may not."),
        yes("Defaults", "DEFAULT 'free'", "(application code)",
            "schema plans { id: String, tier: String = \"free\", label: String = upper(.id), made: DateTime = now() }\nplans.insert { id: \"a\" }.select { tier, label }", r#"{"label":"A","tier":"free"}"#),
        yes("Unique fields", "UNIQUE (email)", "createIndex(..., { unique: true })",
            "schema logins { id: String, email: String unique, unique(id, email) }\nlogins.insert { id: \"l1\", email: \"a@x.io\" }", r#"{"count":1}"#)
            .check("logins.count()")
            .refusing("schema logins { id: String, email: String unique }\nlogins.insert [ { id: \"l1\", email: \"a@x.io\" }, { id: \"l2\", email: \"a@x.io\" } ]"),
        yes("Conditions every row must meet", "CHECK (age >= 0)", "$jsonSchema, $expr validator",
            "schema ages { id: String, age: Integer, check(.age >= 0 && .age < 150) }\nages.insert { id: \"a1\", age: 36 }", r#"{"count":1}"#)
            .check("ages.count()")
            .refusing("schema ages { id: String, age: Integer, check(.age >= 0 && .age < 150) }\nages.insert { id: \"a1\", age: -1 }"),
        yes("One of a fixed set of values", "CREATE TYPE ... AS ENUM", "enum: [...]",
            "schema tickets { id: String, state: String, check(.state in [\"open\", \"closed\"]) }\ntickets.insert { id: \"t1\", state: \"open\" }", r#"{"count":1}"#)
            .check("tickets.count()")
            .differently()
            .note("A `check` on the field. There is no separate enum type."),
        yes("References that must resolve", "FOREIGN KEY", "(not enforced)",
            r#"orders.where { .id == "o1" }.select { id, customer: .customer.id }"#, r#"{"customer":"c1","id":"o1"}"#)
            .refusing(r#"orders.insert { id: "o9", customer: "nobody", status: "open", placed: "2026-01-01", total: 1.0, items: [] }"#),
        yes("What deleting a referenced row does", "ON DELETE CASCADE, SET NULL, RESTRICT", "(application code)",
            "schema folders { id: String }\nschema files { id: String, folder: Ref(folders) on delete cascade, owner: Optional(Ref(customers)) on delete nullify }\nfolders.insert { id: \"f1\" }\nfiles.insert { id: \"x\", folder: \"f1\" }\nfolders.delete()", r#"{"count":0}"#)
            .check("files.count()"),
        yes("Identity of a row", "PRIMARY KEY (id)", "_id",
            r#"customers.where { .id == "c1" }.select { id }"#, r#"{"id":"c1"}"#)
            .note("Every row has an `id`, unique in its collection and never changed."),
        no("Identity made of several fields", "PRIMARY KEY (tenant, id)", "_id: { tenant: ..., id: ... }",
            "The identity is the one `id` field. Several fields together can be declared `unique`."),
        yes("Identity chosen by the database", "SERIAL, GENERATED AS IDENTITY", "ObjectId",
            r#"notes.insert { text: "no id given" }.select { has_id: .id != null }"#, r#"{"has_id":true}"#),
        yes("Add, rename, retype and drop fields", "ALTER TABLE ... ADD, RENAME, ALTER TYPE, DROP COLUMN", "$set, $rename, $unset over every document",
            "schema products { id: String, title: String from name, price: Float, stock: Float, category: removed, rating: Integer = 5, note: String? }", r#"{"id":"p1","price":2.0,"rating":5,"stock":100.0,"title":"Pen"}"#)
            .check(r#"products.where { .id == "p1" }"#),
        yes("Drop a collection", "DROP TABLE", "drop()",
            "schema scraps { id: String }\nscraps.insert { id: \"s\" }\ndrop scraps", r#"{"count":0}"#)
            .check("scraps.count()"),
        yes("Stored functions", "CREATE FUNCTION", "(application code)",
            "fn with_tax(amount: Float, rate: Float = 0.5) { amount * (1 + rate) }\nfn paid(rows) { rows.where { .status == \"paid\" } }", r#"{"gross":39.75,"id":"o1"} {"gross":225.0,"id":"o2"}"#)
            .check("orders.paid().sort { .id }.limit(2).select { id, gross: with_tax(.total) }"),
        yes("Saved query", "CREATE VIEW", "createView()",
            "fn paid(rows) { rows.where { .status == \"paid\" } }", r#"{"count":4}"#)
            .check("orders.paid().count()")
            .differently()
            .note("A function of rows, used as a step in any query. It is not a named collection."),
        no("Stored result kept up to date", "CREATE MATERIALIZED VIEW", "$merge on a schedule, on-demand views",
            "Nothing stores a query's result and refreshes it."),
        no("Value computed from other fields on every write", "GENERATED ALWAYS AS (...) STORED", "(application code)",
            "A default may be computed, but only when the row is first written."),
        no("Code that runs when rows change", "CREATE TRIGGER", "change streams, Atlas triggers",
            "No triggers."),
        no("One collection split by a key", "PARTITION BY RANGE (...)", "sharding, time series collections",
            "A collection is not partitioned."),
    ]
}

fn indexes() -> Vec<Row> {
    vec![
        yes("Index on a field", "CREATE INDEX ON jobs (state)", "createIndex({ state: 1 })",
            "schema jobs { id: String, state: String, index(.state) }\njobs.insert [ { id: \"j1\", state: \"open\" }, { id: \"j2\", state: \"done\" } ]",
            r#""uses_index":true"#)
            .check(r#"jobs.where { .state == "open" }.explain()"#)
            .holding(),
        yes("Index on several fields", "CREATE INDEX ON jobs (state, n)", "createIndex({ state: 1, n: 1 })",
            "schema jobs { id: String, state: String, n: Integer, index(.state, .n) }\njobs.insert [ { id: \"j1\", state: \"open\", n: 1 }, { id: \"j2\", state: \"open\", n: 2 } ]",
            r#""uses_index":true"#)
            .check(r#"jobs.where { .state == "open" && .n == 2 }.explain()"#)
            .holding(),
        no("Index on a field inside an object", "CREATE INDEX ON places ((address->>'zip'))", "createIndex({ \"address.zip\": 1 })",
            "The declaration is accepted, but a filter on the field still reads every row.")
            .shown("schema places { id: String, address: { zip: String }, index(.address.zip) }\nplaces.insert { id: \"a\", address: { zip: \"1\" } }",
                r#"places.where { .address.zip == "1" }.explain()"#, r#""uses_index":false"#),
        yes("Index that keeps values unique", "CREATE UNIQUE INDEX", "createIndex(..., { unique: true })",
            "schema logins { id: String, email: String unique }\nlogins.insert { id: \"l1\", email: \"a@x.io\" }",
            r#""uses_index":true"#)
            .check(r#"logins.where { .email == "a@x.io" }.explain()"#)
            .holding(),
        yes("Index for following references backwards", "CREATE INDEX ON orders (customer)", "createIndex({ customer: 1 })",
            r#"customers.where { .id == "c1" }.select { n: .orders.count() }"#, r#"{"n":2}"#)
            .note("Kept for every reference without being declared."),
        yes("How a query will run", "EXPLAIN", "explain()",
            r#"orders.where { .status == "paid" }.explain()"#, r#""uses_index":false"#)
            .holding(),
        no("Index on the elements of a list", "CREATE INDEX ... USING gin (tags)", "multikey index",
            "A list cannot be an index key, so `\"vip\" in .tags` reads every row.")
            .refusing("schema posts { id: String, tags: List(String), index(.tags) }\nposts.insert { id: \"a\", tags: [\"x\"] }"),
        no("Index on an expression", "CREATE INDEX ON t (lower(name))", "(store the value in a field)",
            "An index is on fields as stored.")
            .refusing("schema names { id: String, name: String, index(lower(.name)) }"),
        no("Index over some of the rows", "CREATE INDEX ... WHERE state = 'open'", "partialFilterExpression",
            "An index covers every row of its collection.")
            .refusing("schema jobs { id: String, state: String, index(.state, where: .state == \"open\") }"),
    ]
}

fn transactions() -> Vec<Row> {
    vec![
        yes("Several writes kept or dropped together", "BEGIN; ...; COMMIT;", "session.withTransaction()",
            "transaction {\n  products.where { .id == \"p1\" }.update { stock -= 10 }\n  products.where { .id == \"p2\" }.update { stock += 10 }\n  products.where { .id in [\"p1\", \"p2\"] }.sort { .id }.select { stock }\n}", r#"{"stock":90} {"stock":50}"#)
            .note("The block answers with the rows of its last statement."),
        tested("A failure undoes the whole block", "ROLLBACK on error", "abortTransaction()",
            "tests/oql_language_conformance_test.rs", "a_transaction_block_is_all_or_nothing"),
        tested("A transaction held open across statements", "BEGIN ... COMMIT over one connection", "startSession(), startTransaction()",
            "tests/concurrent_transactions_test.rs", "a_transaction_sees_its_own_writes_and_nobody_else_does"),
        tested("Explicit rollback", "ROLLBACK", "abortTransaction()",
            "tests/concurrent_transactions_test.rs", "rolling_back_leaves_nothing_and_the_transaction_is_over"),
        tested("Two writers of one row cannot both win", "serialization failure, row locks", "WriteConflict",
            "tests/concurrent_transactions_test.rs", "the_second_to_commit_a_change_to_the_same_document_is_refused"),
        tested("Read-then-write in one statement is atomic", "UPDATE ... SET n = n + 1", "$inc",
            "tests/concurrent_transactions_test.rs", "concurrent_increments_are_all_counted"),
        tested("A commit that was acknowledged survives a crash", "durability", "writeConcern: { j: true }",
            "tests/crash_simulation_test.rs", "acknowledged_writes_survive_a_crash_at_any_point"),
        no("Partial rollback inside a transaction", "SAVEPOINT, ROLLBACK TO", "(none)",
            "A transaction is kept whole or dropped whole."),
        no("Choice of isolation level", "SET TRANSACTION ISOLATION LEVEL ...", "readConcern, writeConcern",
            "One level: a transaction reads the database as of its start and is refused at commit if a row it changed was changed by another."),
        no("Locking rows for a later write", "SELECT ... FOR UPDATE", "(none)",
            "No row locks are taken by reads. A conflicting commit is refused and retried instead."),
    ]
}

fn search() -> Vec<Row> {
    vec![
        yes("Nearest vectors", "ORDER BY embedding <-> $1 (pgvector)", "$vectorSearch (Atlas)",
            "schema docs { id: String, embedding: Vector(3), index(.embedding, metric: \"cosine\") }\ndocs.insert [ { id: \"d1\", embedding: [1.0, 0.0, 0.0] }, { id: \"d2\", embedding: [0.0, 1.0, 0.0] } ]", r#"{"id":"d1"}"#)
            .check(r#"docs.nearest { field: .embedding, query: [0.9, 0.1, 0.0], metric: "cosine", k: 1 }.select { id }"#),
        no("Full-text search", "to_tsvector, @@, ts_rank", "$text, $search (Atlas)",
            "No word index, stemming or ranking. Text is matched with `contains` and `matches`, which read the rows.")
            .refusing(r#"customers.search("ada")"#),
        no("Places and distances", "PostGIS", "2dsphere, $near, $geoWithin",
            "No geographic types or indexes.")
            .refusing("customers.where { near(.address, [3.4, 6.5], 1000) }"),
        yes("Time series in buckets", "date_trunc, date_bin", "$dateTrunc, time series collections",
            r#"orders.group_by { week: .placed.format("%G-W%V") }.select { week, n: count(), total: sum(.total) }.sort { .week }.limit(3)"#, r#"{"n":1,"total":26.5,"week":"2026-W02"} {"n":2,"total":210.0,"week":"2026-W07"} {"n":1,"total":42.0,"week":"2026-W09"}"#),
        no("Rows that expire", "(a scheduled DELETE)", "TTL index",
            "Nothing deletes rows by age. It is a `delete()` run on a schedule."),
        no("Being told when rows change", "LISTEN, NOTIFY, logical replication", "change streams",
            "There is change-stream code in the repository, but no test drives it, so it is not counted."),
    ]
}

fn operations() -> Vec<Row> {
    vec![
        tested("Backup and restore", "pg_dump, pg_basebackup", "mongodump, mongorestore",
            "tests/backup_restore_test.rs", "a_restored_backup_answers_as_the_original_did"),
        tested("Backup while writes continue", "pg_basebackup", "mongodump --oplog",
            "tests/backup_restore_test.rs", "a_backup_taken_during_writes_is_one_moment_of_the_database"),
        tested("Readers never see half of a write", "MVCC", "WiredTiger snapshots",
            "tests/concurrent_transactions_test.rs", "updates_to_many_rows_at_once_do_not_tear"),
        no("Users, roles and permissions", "CREATE ROLE, GRANT, row-level security", "createUser(), roles",
            "Whoever can open the database can read and write all of it."),
        no("Copies of the database on other machines", "streaming replication", "replica sets",
            "One process owns a database. There is replication code in the repository, but no test drives it, so it is not counted."),
        no("One database spread over machines", "(extensions such as Citus)", "sharding",
            "A database lives on one machine."),
    ]
}

fn defects() -> Vec<Defect> {
    // The five found when the matrix was first written are fixed, and are
    // held by `what_the_matrix_found_stays_fixed` in the conformance tests.
    Vec::new()
}
