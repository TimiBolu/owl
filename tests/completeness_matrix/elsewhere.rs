// How SQL and MongoDB say what each row's OQL says, written out whole.
// Included by `completeness_matrix_test.rs`.
//
// A row's own `sql` and `mongo` name the feature in a few words, for the
// table in the written matrix. These are the statements a person would
// type to ask the same question of the same shop, for the website, so that
// a whole OQL query is set beside a whole SQL statement and a whole
// MongoDB one, not beside a keyword.
//
// The matrix test does not run these. `tools/matrix_check/check.py` does:
// it runs each on PostgreSQL 17 and MongoDB 7 against the same shop and
// compares its rows with the rows the test holds Owl to. Run it after
// changing anything here.
//
// The SQL is PostgreSQL over the shop kept the relational way: `tags` is
// `text[]`, `address` is `jsonb`, and an order's items are rows of
// `order_items (order_id, position, product, qty, price)`. The MongoDB is
// mongosh over the same documents as Owl, with the id in `_id` and a
// reference held as the id it points to.
//
// Lines are kept short so that three columns fit side by side.

fn elsewhere() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        // Filtering
        ("Equality",
r#"SELECT id FROM orders
WHERE status = 'paid';"#,
r#"db.orders.find(
  { status: "paid" },
  { _id: 1 }
)"#),
        ("Comparison",
r#"SELECT id FROM orders
WHERE total > 50;"#,
r#"db.orders.find(
  { total: { $gt: 50 } },
  { _id: 1 }
)"#),
        ("And, or, not",
r#"SELECT id FROM orders
WHERE total > 20
  AND (status = 'open'
       OR NOT total < 100);"#,
r#"db.orders.find(
  { total: { $gt: 20 },
    $or: [
      { status: "open" },
      { total: { $not: { $lt: 100 } } }
    ] },
  { _id: 1 }
)"#),
        ("In a list",
r#"SELECT id FROM customers
WHERE city IN ('Lagos', 'Nairobi');"#,
r#"db.customers.find(
  { city: { $in: ["Lagos", "Nairobi"] } },
  { _id: 1 }
)"#),
        ("Not in a list",
r#"SELECT id FROM customers
WHERE city NOT IN ('Lagos', 'Nairobi');"#,
r#"db.customers.find(
  { city: { $nin: ["Lagos", "Nairobi"] } },
  { _id: 1 }
)"#),
        ("Between",
r#"SELECT id FROM orders
WHERE total BETWEEN 25 AND 60;"#,
r#"db.orders.find(
  { total: { $gte: 25, $lte: 60 } },
  { _id: 1 }
)"#),
        ("Text starts with, ends with, contains",
r#"SELECT id FROM customers
WHERE name LIKE 'A%'
   OR email LIKE '%.org'
   OR city LIKE '%rob%';"#,
r#"db.customers.find(
  { $or: [
      { name: /^A/ },
      { email: /\.org$/ },
      { city: /rob/ }
    ] },
  { _id: 1 }
)"#),
        ("Regular expression",
r#"SELECT id FROM customers
WHERE email ~ '^[a-c].*io$';"#,
r#"db.customers.find(
  { email: /^[a-c].*io$/ },
  { _id: 1 }
)"#),
        ("Ignoring case",
r#"SELECT id FROM customers
WHERE lower(name) = 'ada'
   OR name ~* '^EVE$';"#,
r#"db.customers.find(
  { $or: [
      { $expr: { $eq: [
          { $toLower: "$name" }, "ada"
        ] } },
      { name: /^EVE$/i }
    ] },
  { _id: 1 }
)"#),
        ("Is null",
r#"SELECT id FROM customers
WHERE email IS NULL;"#,
r#"db.customers.find(
  { email: null },
  { _id: 1 }
)"#),
        ("Is not null",
r#"SELECT id FROM customers
WHERE referrer IS NOT NULL;"#,
r#"db.customers.find(
  { referrer: { $ne: null } },
  { _id: 1 }
)"#),
        ("Field is present",
r#"-- a column is on every row,
-- so the nearest thing is
SELECT id FROM customers
WHERE email IS NOT NULL;"#,
r#"db.customers.find(
  { email: { $exists: true } },
  { _id: 1 }
)"#),
        ("One field against another",
r#"SELECT id FROM products
WHERE price > stock;"#,
r#"db.products.find(
  { $expr: {
      $gt: ["$price", "$stock"]
    } },
  { _id: 1 }
)"#),
        ("Arithmetic in a condition",
r#"SELECT id FROM products
WHERE price * stock > 400;"#,
r#"db.products.find(
  { $expr: { $gt: [
      { $multiply: ["$price", "$stock"] },
      400
    ] } },
  { _id: 1 }
)"#),
        ("Dates and parts of dates",
r#"SELECT id FROM customers
WHERE EXTRACT(YEAR FROM joined) = 2025
  AND joined < '2025-07-01';"#,
r#"db.customers.find(
  { $expr: { $eq: [
      { $year: "$joined" }, 2025
    ] },
    joined: {
      $lt: ISODate("2025-07-01")
    } },
  { _id: 1 }
)"#),
        ("Kind of value",
r#"-- a column has one type,
-- so only null differs
SELECT count(*) FROM customers
WHERE email IS NOT NULL;"#,
r#"db.customers.countDocuments(
  { email: { $type: "string" } }
)"#),

        // Shaping and ordering
        ("Chosen fields",
r#"SELECT id, name FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { name: 1 }
)"#),
        ("Every field",
r#"SELECT * FROM products
WHERE id = 'p1';"#,
r#"db.products.find({ _id: "p1" })"#),
        ("Every field but some",
r#"-- no "all but": list the rest
SELECT id, name, price
FROM products
WHERE id = 'p1';"#,
r#"db.products.find(
  { _id: "p1" },
  { stock: 0, category: 0 }
)"#),
        ("Renamed fields",
r#"SELECT name AS customer
FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { _id: 0, customer: "$name" }
)"#),
        ("Computed fields",
r#"SELECT id, price * stock AS worth
FROM products
WHERE id = 'p2';"#,
r#"db.products.find(
  { _id: "p2" },
  { worth: {
      $multiply: ["$price", "$stock"]
    } }
)"#),
        ("Distinct rows",
r#"SELECT DISTINCT city
FROM customers
ORDER BY city;"#,
r#"db.customers.distinct("city").sort()"#),
        ("Conditional value",
r#"SELECT id,
  CASE WHEN stock = 0 THEN 'out'
       WHEN stock < 50 THEN 'low'
       ELSE 'fine'
  END AS level
FROM products
ORDER BY id
LIMIT 3;"#,
r#"db.products.aggregate([
  { $sort: { _id: 1 } },
  { $limit: 3 },
  { $project: { level: { $switch: {
      branches: [
        { case: { $eq: ["$stock", 0] },
          then: "out" },
        { case: { $lt: ["$stock", 50] },
          then: "low" }
      ],
      default: "fine"
    } } } }
])"#),
        ("Matching on a value",
r#"SELECT id,
  CASE status WHEN 'paid' THEN true
              ELSE false
  END AS done
FROM orders
ORDER BY id
LIMIT 3;"#,
r#"db.orders.aggregate([
  { $sort: { _id: 1 } },
  { $limit: 3 },
  { $project: { done: { $switch: {
      branches: [
        { case: { $eq: ["$status", "paid"] },
          then: true }
      ],
      default: false
    } } } }
])"#),
        ("First value that is present",
r#"SELECT id,
  COALESCE(email, 'none') AS email
FROM customers
WHERE id IN ('c1', 'c2')
ORDER BY id;"#,
r#"db.customers.find(
  { _id: { $in: ["c1", "c2"] } },
  { email: { $ifNull: ["$email", "none"] } }
).sort({ _id: 1 })"#),
        ("Conversion between types",
r#"SELECT stock::text AS text,
       price::int AS whole,
       stock::float AS real,
       '42'::int AS number
FROM products
WHERE id = 'p1';"#,
r#"db.products.find(
  { _id: "p1" },
  { _id: 0,
    text: { $toString: "$stock" },
    whole: { $toInt: "$price" },
    real: { $toDouble: "$stock" },
    number: { $toInt: "42" } }
)"#),
        ("Text functions",
r#"SELECT upper(name) AS up,
  length(email) AS size,
  substring(email, 1, 3) AS part,
  replace(email, 'x.io', 'owl.dev')
    AS swapped,
  name || ' of ' || city AS joined,
  string_to_array(email, '@') AS pieces
FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { _id: 0,
    up: { $toUpper: "$name" },
    size: { $strLenCP: "$email" },
    part: { $substrCP: ["$email", 0, 3] },
    swapped: { $replaceAll: {
      input: "$email", find: "x.io",
      replacement: "owl.dev" } },
    joined: { $concat: [
      "$name", " of ", "$city"] },
    pieces: { $split: ["$email", "@"] } }
)"#),
        ("Number functions",
r#"SELECT round(price::numeric) AS rounded,
       floor(price) AS down,
       ceil(price) AS up,
       sqrt(stock + 9) AS root,
       power(price, 2) AS squared,
       abs(stock - 100) AS away
FROM products
WHERE id = 'p2';"#,
r#"db.products.find(
  { _id: "p2" },
  { _id: 0,
    // $round takes a half to the even
    rounded: { $floor: {
      $add: ["$price", 0.5] } },
    down: { $floor: "$price" },
    up: { $ceil: "$price" },
    root: { $sqrt: { $add: ["$stock", 9] } },
    squared: { $pow: ["$price", 2] },
    away: { $abs: {
      $subtract: ["$stock", 100] } } }
)"#),
        ("Date arithmetic",
r#"SELECT
  to_char(c.joined + INTERVAL '30 days',
          'YYYY-MM-DD') AS later,
  c.joined::date - r.joined::date
    AS waited
FROM customers c
JOIN customers r ON r.id = c.referrer
WHERE c.id = 'c3';"#,
r#"db.customers.aggregate([
  { $match: { _id: "c3" } },
  { $lookup: { from: "customers",
      localField: "referrer",
      foreignField: "_id", as: "r" } },
  { $unwind: "$r" },
  { $project: { _id: 0,
      later: { $dateToString: {
        format: "%Y-%m-%d",
        date: { $dateAdd: {
          startDate: "$joined",
          unit: "day", amount: 30 } } } },
      waited: { $dateDiff: {
        startDate: "$r.joined",
        endDate: "$joined",
        unit: "day" } } } }
])"#),
        ("Date formatting",
r#"SELECT to_char(joined, 'YYYY-MM')
  AS month
FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { _id: 0,
    month: { $dateToString: {
      format: "%Y-%m", date: "$joined" } } }
)"#),
        ("Nested output",
r#"SELECT id, json_build_object(
  'name', name,
  'place', json_build_object(
    'city', city,
    'zip', address->>'zip')) AS who
FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { who: {
      name: "$name",
      place: { city: "$city",
               zip: "$address.zip" } } }
)"#),
        ("List in the output",
r#"SELECT id,
  ARRAY[price, stock] AS figures
FROM products
WHERE id = 'p1';"#,
r#"db.products.find(
  { _id: "p1" },
  { figures: ["$price", "$stock"] }
)"#),
        ("Order by several keys",
r#"SELECT id FROM customers
ORDER BY city ASC, joined DESC;"#,
r#"db.customers.find({}, { _id: 1 })
  .sort({ city: 1, joined: -1 })"#),
        ("Order by an expression",
r#"SELECT id FROM products
ORDER BY price * stock DESC, id;"#,
r#"db.products.aggregate([
  { $addFields: { worth: {
      $multiply: ["$price", "$stock"] } } },
  { $sort: { worth: -1, _id: 1 } },
  { $project: { _id: 1 } }
])"#),
        ("Where absent values sort",
r#"SELECT id FROM customers
ORDER BY email NULLS LAST;"#,
r#"db.customers.aggregate([
  { $addFields: { absent: { $eq: [
      { $ifNull: ["$email", null] },
      null ] } } },
  { $sort: { absent: 1, email: 1 } },
  { $project: { _id: 1 } }
])"#),
        ("Limit and offset",
r#"SELECT id FROM products
ORDER BY price DESC
LIMIT 2 OFFSET 1;"#,
r#"db.products.find({}, { _id: 1 })
  .sort({ price: -1 })
  .skip(1)
  .limit(2)"#),
        ("Paging from the last row seen",
r#"SELECT id FROM products
WHERE (price, id) > (12.5, 'p2')
ORDER BY price, id
LIMIT 2;"#,
r#"db.products.find(
  { $or: [
      { price: { $gt: 12.5 } },
      { price: 12.5, _id: { $gt: "p2" } }
    ] },
  { _id: 1 }
).sort({ price: 1, _id: 1 })
 .limit(2)"#),
        ("Random sample",
r#"SELECT count(*) FROM (
  SELECT id FROM products
  ORDER BY random()
  LIMIT 2
) AS picked;"#,
r#"db.products.aggregate([
  { $sample: { size: 2 } },
  { $count: "count" }
])"#),

        // Aggregation
        ("Count",
r#"SELECT count(*) FROM orders;"#,
r#"db.orders.countDocuments()"#),
        ("Sum, average, least, greatest",
r#"SELECT sum(total) AS sum,
       avg(total) AS avg,
       min(total) AS least,
       max(total) AS most
FROM orders;"#,
r#"db.orders.aggregate([
  { $group: { _id: null,
      sum: { $sum: "$total" },
      avg: { $avg: "$total" },
      least: { $min: "$total" },
      most: { $max: "$total" } } }
])"#),
        ("Group by",
r#"SELECT status, count(*) AS n,
       sum(total) AS total
FROM orders
GROUP BY status
ORDER BY status;"#,
r#"db.orders.aggregate([
  { $group: { _id: "$status",
      n: { $sum: 1 },
      total: { $sum: "$total" } } },
  { $sort: { _id: 1 } }
])"#),
        ("Group by several keys",
r#"SELECT c.city, o.status,
       count(*) AS n
FROM orders o
JOIN customers c ON c.id = o.customer
GROUP BY c.city, o.status
ORDER BY c.city, o.status;"#,
r#"db.orders.aggregate([
  { $lookup: { from: "customers",
      localField: "customer",
      foreignField: "_id", as: "c" } },
  { $unwind: "$c" },
  { $group: {
      _id: { city: "$c.city",
             status: "$status" },
      n: { $sum: 1 } } },
  { $sort: { "_id.city": 1,
             "_id.status": 1 } }
])"#),
        ("Group by an expression",
r#"SELECT to_char(placed, 'YYYY-MM')
         AS month,
       count(*) AS n
FROM orders
GROUP BY month
ORDER BY month;"#,
r#"db.orders.aggregate([
  { $group: {
      _id: { $dateToString: {
        format: "%Y-%m",
        date: "$placed" } },
      n: { $sum: 1 } } },
  { $sort: { _id: 1 } }
])"#),
        ("Filter on a group",
r#"SELECT customer, count(*) AS n
FROM orders
GROUP BY customer
HAVING count(*) > 1
ORDER BY customer;"#,
r#"db.orders.aggregate([
  { $group: { _id: "$customer",
      n: { $sum: 1 } } },
  { $match: { n: { $gt: 1 } } },
  { $sort: { _id: 1 } }
])"#),
        ("Count of distinct values",
r#"SELECT count(DISTINCT customer)
FROM orders;"#,
r#"db.orders.distinct("customer").length"#),
        ("Aggregate over some of the rows",
r#"SELECT customer,
  count(*) FILTER (
    WHERE status = 'paid') AS paid,
  coalesce(sum(total) FILTER (
    WHERE status = 'paid'), 0)
    AS paid_total
FROM orders
GROUP BY customer
ORDER BY customer;"#,
r#"db.orders.aggregate([
  { $group: { _id: "$customer",
      paid: { $sum: { $cond: [
        { $eq: ["$status", "paid"] },
        1, 0 ] } },
      paid_total: { $sum: { $cond: [
        { $eq: ["$status", "paid"] },
        "$total", 0 ] } } } },
  { $sort: { _id: 1 } }
])"#),
        ("Median and percentiles",
r#"SELECT
  percentile_cont(0.5) WITHIN GROUP
    (ORDER BY total) AS median,
  percentile_cont(0.9) WITHIN GROUP
    (ORDER BY total) AS p90
FROM orders;"#,
r#"// picks a value that is there:
// 26.5 and [150], not 34.25 and 105
db.orders.aggregate([
  { $group: { _id: null,
      median: { $median: {
        input: "$total",
        method: "approximate" } },
      p90: { $percentile: {
        input: "$total", p: [0.9],
        method: "approximate" } } } }
])"#),
        ("Spread",
r#"SELECT category,
       var_samp(stock) AS sample,
       var_pop(stock) AS whole,
       stddev_pop(stock) AS sd
FROM products
GROUP BY category
ORDER BY category;"#,
r#"db.products.aggregate([
  { $group: { _id: "$category",
      ss: { $stdDevSamp: "$stock" },
      sd: { $stdDevPop: "$stock" } } },
  { $project: { sd: 1,
      sample: { $pow: ["$ss", 2] },
      whole: { $pow: ["$sd", 2] } } },
  { $sort: { _id: 1 } }
])"#),
        ("Values of a group as a list",
r#"SELECT status,
       array_agg(total) AS totals
FROM orders
GROUP BY status
ORDER BY status;"#,
r#"db.orders.aggregate([
  { $group: { _id: "$status",
      totals: { $push: "$total" } } },
  { $sort: { _id: 1 } }
])"#),
        ("Distinct values of a group as a list",
r#"SELECT o.status,
  array_agg(DISTINCT c.city) AS cities
FROM orders o
JOIN customers c ON c.id = o.customer
GROUP BY o.status
ORDER BY o.status;"#,
r#"db.orders.aggregate([
  { $lookup: { from: "customers",
      localField: "customer",
      foreignField: "_id", as: "c" } },
  { $unwind: "$c" },
  { $group: { _id: "$status",
      cities: { $addToSet: "$c.city" } } },
  // a set comes in no order
  { $set: { cities: { $sortArray: {
      input: "$cities", sortBy: 1 } } } },
  { $sort: { _id: 1 } }
])"#),
        ("First or last row of each group",
r#"SELECT DISTINCT ON (customer)
       customer, id AS latest
FROM orders
ORDER BY customer, placed DESC;"#,
r#"db.orders.aggregate([
  { $sort: { placed: -1 } },
  { $group: { _id: "$customer",
      latest: { $first: "$_id" } } },
  { $sort: { _id: 1 } }
])"#),
        ("Ranges as groups",
r#"SELECT
  CASE WHEN total < 30 THEN 'small'
       WHEN total < 100 THEN 'medium'
       ELSE 'large'
  END AS size,
  count(*) AS n
FROM orders
GROUP BY size
ORDER BY size;"#,
r#"db.orders.aggregate([
  { $bucket: {
      groupBy: "$total",
      boundaries: [0, 30, 100],
      default: "large",
      output: { n: { $sum: 1 } } } }
])"#),
        ("Several summaries in one answer",
r#"SELECT
  (SELECT json_agg(s) FROM (
     SELECT status, count(*) AS n
     FROM orders
     GROUP BY status
     ORDER BY status) s) AS by_status,
  (SELECT json_agg(d) FROM (
     SELECT id FROM orders
     ORDER BY total DESC
     LIMIT 2) d) AS dearest;"#,
r#"db.orders.aggregate([
  { $facet: {
      by_status: [
        { $group: { _id: "$status",
            n: { $sum: 1 } } },
        { $sort: { _id: 1 } }
      ],
      dearest: [
        { $sort: { total: -1 } },
        { $limit: 2 },
        { $project: { _id: 1 } }
      ] } }
])"#),
        ("Subtotals and grand totals",
r#"SELECT c.city, o.status,
       count(*) AS n
FROM orders o
JOIN customers c ON c.id = o.customer
GROUP BY ROLLUP (c.city, o.status);"#,
"(a $facet with one $group for each level of total)"),

        // Relationships and joins
        ("Row and the row it refers to",
r#"SELECT o.id, c.name AS customer,
       c.city
FROM orders o
JOIN customers c ON c.id = o.customer
WHERE o.id = 'o3';"#,
r#"db.orders.aggregate([
  { $match: { _id: "o3" } },
  { $lookup: { from: "customers",
      localField: "customer",
      foreignField: "_id", as: "c" } },
  { $unwind: "$c" },
  { $project: { customer: "$c.name",
                city: "$c.city" } }
])"#),
        ("Filter through a reference",
r#"SELECT o.id
FROM orders o
JOIN customers c ON c.id = o.customer
WHERE c.city = 'Lagos';"#,
r#"db.orders.aggregate([
  { $lookup: { from: "customers",
      localField: "customer",
      foreignField: "_id", as: "c" } },
  { $match: { "c.city": "Lagos" } },
  { $project: { _id: 1 } }
])"#),
        ("Optional reference",
r#"SELECT c.id, r.name AS referrer
FROM customers c
LEFT JOIN customers r
  ON r.id = c.referrer
ORDER BY c.id
LIMIT 2;"#,
r#"db.customers.aggregate([
  { $sort: { _id: 1 } },
  { $limit: 2 },
  { $lookup: { from: "customers",
      localField: "referrer",
      foreignField: "_id", as: "r" } },
  { $project: { referrer: {
      $ifNull: [{ $first: "$r.name" },
                null] } } }
])"#),
        ("Rows that refer to a row",
r#"SELECT c.id, count(o.id) AS orders,
  coalesce(sum(o.total), 0) AS spent
FROM customers c
LEFT JOIN orders o
  ON o.customer = c.id
GROUP BY c.id
ORDER BY c.id;"#,
r#"db.customers.aggregate([
  { $lookup: { from: "orders",
      localField: "_id",
      foreignField: "customer",
      as: "o" } },
  { $project: {
      orders: { $size: "$o" },
      spent: { $sum: "$o.total" } } },
  { $sort: { _id: 1 } }
])"#),
        ("Related rows nested in the answer",
r#"SELECT c.id, (
  SELECT json_agg(json_build_object(
    'id', o.id, 'status', o.status)
    ORDER BY o.id)
  FROM orders o
  WHERE o.customer = c.id) AS orders
FROM customers c
WHERE c.id = 'c3';"#,
r#"db.customers.aggregate([
  { $match: { _id: "c3" } },
  { $lookup: { from: "orders",
      localField: "_id",
      foreignField: "customer",
      pipeline: [
        { $sort: { _id: 1 } },
        { $project: { status: 1 } }
      ],
      as: "orders" } },
  { $project: { orders: 1 } }
])"#),
        ("Several steps",
r#"SELECT c.id, r2.name AS through, (
  SELECT array_agg(p.name)
  FROM orders o
  JOIN order_items i
    ON i.order_id = o.id
  JOIN products p ON p.id = i.product
  WHERE o.customer = c.id) AS bought
FROM customers c
LEFT JOIN customers r1
  ON r1.id = c.referrer
LEFT JOIN customers r2
  ON r2.id = r1.referrer
WHERE c.id = 'c4';"#,
r#"db.customers.aggregate([
  { $match: { _id: "c4" } },
  { $lookup: { from: "customers",
      localField: "referrer",
      foreignField: "_id", as: "r1" } },
  { $lookup: { from: "customers",
      localField: "r1.referrer",
      foreignField: "_id", as: "r2" } },
  { $lookup: { from: "orders",
      localField: "_id",
      foreignField: "customer",
      as: "o" } },
  { $lookup: { from: "products",
      localField: "o.items.product",
      foreignField: "_id", as: "p" } },
  { $project: {
      through: { $first: "$r2.name" },
      bought: "$p.name" } }
])"#),
        ("Row related to its own collection",
r#"SELECT r.id, count(*) AS referred
FROM customers c
JOIN customers r ON r.id = c.referrer
GROUP BY r.id
ORDER BY r.id;"#,
r#"db.customers.aggregate([
  { $match: { referrer: { $ne: null } } },
  { $group: { _id: "$referrer",
      referred: { $sum: 1 } } },
  { $sort: { _id: 1 } }
])"#),
        ("Many to many",
r#"SELECT p.id,
  count(DISTINCT o.customer) AS buyers
FROM products p
LEFT JOIN order_items i
  ON i.product = p.id
LEFT JOIN orders o
  ON o.id = i.order_id
GROUP BY p.id
ORDER BY p.id;"#,
r#"db.products.aggregate([
  { $lookup: { from: "orders",
      localField: "_id",
      foreignField: "items.product",
      as: "o" } },
  { $project: { buyers: { $size: {
      $setUnion: ["$o.customer"] } } } },
  { $sort: { _id: 1 } }
])"#),
        ("Rows with a related row",
r#"SELECT c.id FROM customers c
WHERE EXISTS (
  SELECT 1 FROM orders o
  WHERE o.customer = c.id
    AND o.status = 'paid')
ORDER BY c.id;"#,
r#"db.orders.distinct(
  "customer",
  { status: "paid" }
).sort()"#),
        ("Rows with no related row",
r#"SELECT c.id FROM customers c
WHERE NOT EXISTS (
  SELECT 1 FROM orders o
  WHERE o.customer = c.id);"#,
r#"db.customers.aggregate([
  { $lookup: { from: "orders",
      localField: "_id",
      foreignField: "customer",
      as: "o" } },
  { $match: { o: [] } },
  { $project: { _id: 1 } }
])"#),
        ("Join on any condition",
r#"SELECT a.id, coalesce(
  json_agg(json_build_object(
    'id', b.id))
  FILTER (WHERE b.id IS NOT NULL),
  '[]') AS neighbours
FROM customers a
LEFT JOIN customers b
  ON b.city = a.city AND b.id <> a.id
GROUP BY a.id
ORDER BY a.id;"#,
r#"db.customers.aggregate([
  { $lookup: { from: "customers",
      let: { city: "$city", me: "$_id" },
      pipeline: [
        { $match: { $expr: { $and: [
            { $eq: ["$city", "$$city"] },
            { $ne: ["$_id", "$$me"] }
          ] } } },
        { $project: { _id: 1 } }
      ],
      as: "neighbours" } },
  { $project: { neighbours: 1 } },
  { $sort: { _id: 1 } }
])"#),
        ("Every row with every row",
r#"SELECT c.id, p.id AS "with"
FROM customers c
CROSS JOIN products p
WHERE c.city = 'Accra'
  AND p.price > 20
ORDER BY c.id, p.id;"#,
r#"db.customers.aggregate([
  { $match: { city: "Accra" } },
  { $sort: { _id: 1 } },
  { $lookup: { from: "products",
      pipeline: [
        { $match: { price: { $gt: 20 } } },
        { $sort: { _id: 1 } },
        { $project: { _id: 1 } }
      ],
      as: "with" } },
  { $project: { with: 1 } }
])"#),
        ("Unmatched rows from both sides",
r#"SELECT c.id AS customer,
       o.id AS "order"
FROM customers c
FULL OUTER JOIN orders o
  ON o.customer = c.id;"#,
"(two $lookup stages put together with $unionWith)"),
        ("Hierarchy of any depth",
r#"WITH RECURSIVE up AS (
  SELECT referrer, 1 AS depth
  FROM customers
  WHERE id = 'c4'
  UNION ALL
  SELECT c.referrer, up.depth + 1
  FROM up
  JOIN customers c ON c.id = up.referrer
)
SELECT referrer AS id, depth
FROM up
WHERE referrer IS NOT NULL
ORDER BY depth;"#,
r#"db.customers.aggregate([
  { $match: { _id: "c4" } },
  { $graphLookup: {
      from: "customers",
      startWith: "$referrer",
      connectFromField: "referrer",
      connectToField: "_id",
      as: "up",
      depthField: "depth" } },
  { $unwind: "$up" },
  { $sort: { "up.depth": 1 } },
  { $project: { _id: "$up._id",
      depth: { $add: ["$up.depth", 1] } } }
])"#),
        ("Everything below a row",
r#"WITH RECURSIVE down AS (
  SELECT id, 1 AS depth
  FROM customers
  WHERE referrer = 'c1'
  UNION ALL
  SELECT c.id, down.depth + 1
  FROM down
  JOIN customers c
    ON c.referrer = down.id
)
SELECT 'c1' AS id,
       count(*) AS below,
       max(depth) AS deepest
FROM down;"#,
r#"db.customers.aggregate([
  { $match: { _id: "c1" } },
  { $graphLookup: {
      from: "customers",
      startWith: "$_id",
      connectFromField: "_id",
      connectToField: "referrer",
      as: "down",
      depthField: "depth" } },
  { $project: {
      below: { $size: "$down" },
      deepest: { $add: [
        { $max: "$down.depth" }, 1] } } }
])"#),

        // Subqueries and sets
        ("One value from another query",
r#"SELECT id FROM orders
WHERE total > (
  SELECT avg(total) FROM orders);"#,
r#"db.orders.aggregate([
  { $setWindowFields: { output: {
      mean: { $avg: "$total" } } } },
  { $match: { $expr: {
      $gt: ["$total", "$mean"] } } },
  { $project: { _id: 1 } }
])"#),
        ("Query that depends on the row",
r#"SELECT o.id FROM orders o
WHERE o.total = (
  SELECT max(total) FROM orders
  WHERE customer = o.customer)
ORDER BY o.id;"#,
r#"db.orders.aggregate([
  { $setWindowFields: {
      partitionBy: "$customer",
      output: {
        most: { $max: "$total" } } } },
  { $match: { $expr: {
      $eq: ["$total", "$most"] } } },
  { $sort: { _id: 1 } },
  { $project: { _id: 1 } }
])"#),
        ("In the rows of another query",
r#"SELECT id FROM customers
WHERE id IN (
  SELECT customer FROM orders
  WHERE total > 50);"#,
r#"db.customers.find(
  { _id: { $in: db.orders.distinct(
      "customer",
      { total: { $gt: 50 } }) } },
  { _id: 1 }
)"#),
        ("Rows of either query",
r#"SELECT id FROM customers
WHERE city = 'Nairobi'
UNION
SELECT id FROM customers
WHERE 'vip' = ANY(tags)
ORDER BY id;"#,
r#"db.customers.aggregate([
  { $match: { city: "Nairobi" } },
  { $unionWith: { coll: "customers",
      pipeline: [
        { $match: { tags: "vip" } }
      ] } },
  { $group: { _id: "$_id" } },
  { $sort: { _id: 1 } }
])"#),
        ("Rows of both, repeats kept",
r#"SELECT count(*) FROM (
  SELECT id FROM customers
  WHERE city = 'Lagos'
  UNION ALL
  SELECT id FROM customers
  WHERE 'vip' = ANY(tags)
) AS u;"#,
r#"db.customers.aggregate([
  { $match: { city: "Lagos" } },
  { $unionWith: { coll: "customers",
      pipeline: [
        { $match: { tags: "vip" } }
      ] } },
  { $count: "count" }
])"#),
        ("Rows in both queries",
r#"SELECT id FROM customers
WHERE city = 'Accra'
INTERSECT
SELECT id FROM customers
WHERE email IS NOT NULL;"#,
r#"// no stage for it; over one
// collection, both conditions
db.customers.find(
  { city: "Accra",
    email: { $ne: null } },
  { _id: 1 }
)"#),
        ("Rows in one query and not the other",
r#"SELECT id FROM customers
WHERE city = 'Accra'
EXCEPT
SELECT id FROM customers
WHERE email IS NOT NULL;"#,
r#"// no stage for it; over one
// collection, the second negated
db.customers.find(
  { city: "Accra", email: null },
  { _id: 1 }
)"#),

        // Window functions
        ("Position in a partition",
r#"SELECT id, row_number() OVER (
  PARTITION BY customer
  ORDER BY placed) AS n
FROM orders
ORDER BY customer, placed;"#,
r#"db.orders.aggregate([
  { $setWindowFields: {
      partitionBy: "$customer",
      sortBy: { placed: 1 },
      output: {
        n: { $documentNumber: {} } } } },
  { $sort: { customer: 1, placed: 1 } },
  { $project: { n: 1 } }
])"#),
        ("Rank, with and without gaps",
r#"SELECT id,
       rank() OVER w AS rank,
       dense_rank() OVER w AS dense
FROM products
WINDOW w AS (
  PARTITION BY category
  ORDER BY price DESC)
ORDER BY category, price DESC;"#,
r#"db.products.aggregate([
  { $setWindowFields: {
      partitionBy: "$category",
      sortBy: { price: -1 },
      output: {
        rank: { $rank: {} },
        dense: { $denseRank: {} } } } },
  { $sort: { category: 1, price: -1 } },
  { $project: { rank: 1, dense: 1 } }
])"#),
        ("Row before and row after",
r#"SELECT id,
  lag(total) OVER w AS before,
  lead(total) OVER w AS after
FROM orders
WINDOW w AS (ORDER BY placed)
ORDER BY placed
LIMIT 3;"#,
r#"db.orders.aggregate([
  { $setWindowFields: {
      sortBy: { placed: 1 },
      output: {
        before: { $shift: {
          output: "$total", by: -1 } },
        after: { $shift: {
          output: "$total", by: 1 } } } } },
  { $limit: 3 },
  { $project: { before: 1, after: 1 } }
])"#),
        ("Running total",
r#"SELECT id, sum(total) OVER (
  ORDER BY placed) AS running
FROM orders
ORDER BY placed;"#,
r#"db.orders.aggregate([
  { $setWindowFields: {
      sortBy: { placed: 1 },
      output: { running: {
        $sum: "$total",
        window: { documents: [
          "unbounded", "current"] } } } } },
  { $project: { running: 1 } }
])"#),
        ("Moving window",
r#"SELECT id, avg(total) OVER (
  ORDER BY placed
  ROWS 1 PRECEDING) AS recent
FROM orders
ORDER BY placed;"#,
r#"db.orders.aggregate([
  { $setWindowFields: {
      sortBy: { placed: 1 },
      output: { recent: {
        $avg: "$total",
        window: {
          documents: [-1, 0] } } } } },
  { $project: { recent: 1 } }
])"#),
        ("Share of a partition",
r#"SELECT id,
  sum(total) OVER w AS "of",
  count(*) OVER w AS size
FROM orders
WINDOW w AS (PARTITION BY customer)
ORDER BY id
LIMIT 3;"#,
r#"db.orders.aggregate([
  { $setWindowFields: {
      partitionBy: "$customer",
      output: {
        of: { $sum: "$total" },
        size: { $count: {} } } } },
  { $sort: { _id: 1 } },
  { $limit: 3 },
  { $project: { of: 1, size: 1 } }
])"#),
        ("First and last of a partition",
r#"SELECT id,
  first_value(id) OVER w
    AS opened_with,
  last_value(id) OVER w AS latest
FROM orders
WINDOW w AS (
  PARTITION BY customer
  ORDER BY placed
  ROWS BETWEEN UNBOUNDED PRECEDING
       AND UNBOUNDED FOLLOWING)
ORDER BY customer, placed
LIMIT 3;"#,
r#"db.orders.aggregate([
  { $setWindowFields: {
      partitionBy: "$customer",
      sortBy: { placed: 1 },
      output: {
        opened_with: { $first: "$_id" },
        latest: { $last: "$_id" } } } },
  { $sort: { customer: 1, placed: 1 } },
  { $limit: 3 },
  { $project: { opened_with: 1,
                latest: 1 } }
])"#),
        ("Top rows of each partition",
r#"SELECT id FROM (
  SELECT id, customer,
    row_number() OVER (
      PARTITION BY customer
      ORDER BY total DESC) AS n
  FROM orders
) ranked
WHERE n = 1
ORDER BY customer;"#,
r#"db.orders.aggregate([
  { $group: { _id: "$customer",
      top: { $topN: { n: 1,
        sortBy: { total: -1 },
        output: "$_id" } } } },
  { $sort: { _id: 1 } },
  { $project: {
      _id: { $first: "$top" } } }
])"#),
        ("Equal-sized buckets and relative rank",
r#"SELECT id, ntile(4) OVER (
  ORDER BY total) AS quarter
FROM orders;"#,
"(no operator for it)"),

        // Documents and lists
        ("Field inside an object",
r#"SELECT id,
  address->>'street' AS street
FROM customers
WHERE address->>'zip' = '00233'
ORDER BY id;"#,
r#"db.customers.find(
  { "address.zip": "00233" },
  { street: "$address.street" }
).sort({ _id: 1 })"#),
        ("List holds a value",
r#"SELECT id FROM customers
WHERE 'vip' = ANY(tags)
ORDER BY id;"#,
r#"db.customers.find(
  { tags: "vip" },
  { _id: 1 }
).sort({ _id: 1 })"#),
        ("List holds all of some values",
r#"SELECT id FROM customers
WHERE tags @> ARRAY['vip', 'early'];"#,
r#"db.customers.find(
  { tags: { $all: ["vip", "early"] } },
  { _id: 1 }
)"#),
        ("Length of a list",
r#"SELECT id FROM customers
WHERE cardinality(tags) = 0
ORDER BY id;"#,
r#"db.customers.find(
  { tags: { $size: 0 } },
  { _id: 1 }
).sort({ _id: 1 })"#),
        ("Element by position",
r#"SELECT tags[1] AS first,
  tags[cardinality(tags)] AS last,
  tags[10] AS none
FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { _id: 0,
    first: { $arrayElemAt: ["$tags", 0] },
    last: { $arrayElemAt: ["$tags", -1] },
    none: { $arrayElemAt: ["$tags", 9] } }
)"#),
        ("One element meets several conditions",
r#"SELECT o.id FROM orders o
WHERE EXISTS (
  SELECT 1 FROM order_items i
  WHERE i.order_id = o.id
    AND i.qty > 5 AND i.price < 5)
ORDER BY o.id;"#,
r#"db.orders.find(
  { items: { $elemMatch: {
      qty: { $gt: 5 },
      price: { $lt: 5 } } } },
  { _id: 1 }
).sort({ _id: 1 })"#),
        ("Every element meets a condition",
r#"SELECT o.id FROM orders o
WHERE NOT EXISTS (
  SELECT 1 FROM order_items i
  WHERE i.order_id = o.id
    AND i.qty < 2)
ORDER BY o.id;"#,
r#"db.orders.find(
  { items: { $not: { $elemMatch: {
      qty: { $lt: 2 } } } } },
  { _id: 1 }
).sort({ _id: 1 })"#),
        ("One row per element",
r#"SELECT i.order_id AS "order",
       p.name AS product, i.qty
FROM order_items i
JOIN products p ON p.id = i.product
ORDER BY i.order_id, i.qty DESC
LIMIT 3;"#,
r#"db.orders.aggregate([
  { $unwind: "$items" },
  { $sort: { _id: 1, "items.qty": -1 } },
  { $limit: 3 },
  { $lookup: { from: "products",
      localField: "items.product",
      foreignField: "_id", as: "p" } },
  { $project: { _id: 0,
      order: "$_id",
      product: { $first: "$p.name" },
      qty: "$items.qty" } }
])"#),
        ("Group over elements",
r#"SELECT product,
       sum(qty) AS sold,
       sum(qty * price) AS revenue
FROM order_items
GROUP BY product
ORDER BY product;"#,
r#"db.orders.aggregate([
  { $unwind: "$items" },
  { $group: { _id: "$items.product",
      sold: { $sum: "$items.qty" },
      revenue: { $sum: { $multiply: [
        "$items.qty", "$items.price"
      ] } } } },
  { $sort: { _id: 1 } }
])"#),
        ("One field of the elements that match",
r#"SELECT
  (SELECT array_agg(o.total)
   FROM orders o
   WHERE o.customer = c.id
     AND o.total > 100) AS big,
  (SELECT array_agg(p.name)
   FROM orders o
   JOIN order_items i
     ON i.order_id = o.id
   JOIN products p
     ON p.id = i.product
   WHERE o.customer = c.id
     AND i.price > 10) AS dear
FROM customers c
WHERE c.id = 'c1';"#,
r#"db.customers.aggregate([
  { $match: { _id: "c1" } },
  { $lookup: { from: "orders",
      localField: "_id",
      foreignField: "customer",
      as: "o" } },
  { $set: { dear: { $filter: {
      input: { $reduce: {
        input: "$o.items",
        initialValue: [],
        in: { $concatArrays: [
          "$$value", "$$this"] } } },
      cond: { $gt: [
        "$$this.price", 10] } } } } },
  { $lookup: { from: "products",
      localField: "dear.product",
      foreignField: "_id", as: "p" } },
  { $project: { _id: 0,
      big: { $map: {
        input: { $filter: {
          input: "$o",
          cond: { $gt: [
            "$$this.total", 100] } } },
        in: "$$this.total" } },
      dear: "$p.name" } }
])"#),
        ("Some of a list in the answer",
r#"SELECT o.id, (
  SELECT json_agg(json_build_object(
    'qty', i.qty, 'price', i.price))
  FROM order_items i
  WHERE i.order_id = o.id
    AND i.price < 10) AS small
FROM orders o
WHERE o.id = 'o4';"#,
r#"db.orders.find(
  { _id: "o4" },
  { small: { $map: {
      input: { $filter: {
        input: "$items",
        cond: { $lt: [
          "$$this.price", 10] } } },
      in: { qty: "$$this.qty",
            price: "$$this.price" } } } }
)"#),
        ("Each element reshaped",
r#"SELECT o.id, (
  SELECT json_agg(json_build_object(
    'name', p.name,
    'cost', i.qty * i.price))
  FROM order_items i
  JOIN products p ON p.id = i.product
  WHERE i.order_id = o.id) AS lines
FROM orders o
WHERE o.id = 'o1';"#,
r#"db.orders.aggregate([
  { $match: { _id: "o1" } },
  { $lookup: { from: "products",
      localField: "items.product",
      foreignField: "_id", as: "p" } },
  { $project: { lines: { $map: {
      input: "$items",
      in: {
        name: { $arrayElemAt: [
          "$p.name",
          { $indexOfArray: [
            "$p._id", "$$this.product"
          ] } ] },
        cost: { $multiply: [
          "$$this.qty", "$$this.price"
        ] } } } } } }
])"#),
        ("List reduced to a value",
r#"SELECT order_id AS id,
       sum(qty) AS units,
       sum(qty * price) AS value
FROM order_items
GROUP BY order_id
ORDER BY order_id
LIMIT 2;"#,
r#"db.orders.find({}, {
  units: { $sum: "$items.qty" },
  value: { $sum: { $map: {
    input: "$items",
    in: { $multiply: [
      "$$this.qty", "$$this.price"
    ] } } } }
}).sort({ _id: 1 }).limit(2)"#),
        ("Part of a list",
r#"SELECT o.id,
  (SELECT json_agg(f) FROM (
     SELECT qty FROM order_items
     WHERE order_id = o.id
     ORDER BY position
     LIMIT 1) f) AS first,
  (SELECT json_agg(r) FROM (
     SELECT qty FROM order_items
     WHERE order_id = o.id
     ORDER BY position
     OFFSET 1) r) AS rest
FROM orders o
WHERE o.id = 'o1';"#,
r#"db.orders.find(
  { _id: "o1" },
  { first: { $map: {
      input: { $slice: ["$items", 1] },
      in: { qty: "$$this.qty" } } },
    rest: { $map: {
      input: { $slice: ["$items", 1,
        { $size: "$items" }] },
      in: { qty: "$$this.qty" } } } }
)"#),
        ("List sorted",
r#"SELECT o.id, (
  SELECT json_agg(
    json_build_object('qty', qty)
    ORDER BY qty DESC)
  FROM order_items
  WHERE order_id = o.id) AS by_qty
FROM orders o
WHERE o.id = 'o4';"#,
r#"db.orders.find(
  { _id: "o4" },
  { by_qty: { $map: {
      input: { $sortArray: {
        input: "$items",
        sortBy: { qty: -1 } } },
      in: { qty: "$$this.qty" } } } }
)"#),
        ("Lists joined",
r#"SELECT
  tags || ARRAY['checked'] AS "all",
  cardinality(
    tags || ARRAY['checked']) AS size
FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { _id: 0,
    all: { $concatArrays: [
      "$tags", ["checked"]] },
    size: { $size: { $concatArrays: [
      "$tags", ["checked"]] } } }
)"#),
        ("Row with fields added",
r#"SELECT *, price * stock AS worth
FROM products
WHERE id = 'p5';"#,
r#"db.products.aggregate([
  { $match: { _id: "p5" } },
  { $addFields: { worth: {
      $multiply: ["$price", "$stock"] } } }
])"#),
        ("Collection with no declared shape",
r#"CREATE TABLE notes (
  id text PRIMARY KEY, doc jsonb);
INSERT INTO notes VALUES
  ('n1', '{"text": "hello",
           "stars": 3}'),
  ('n2', '{"about": {
           "any": [1, "two"]}}');"#,
r#"db.notes.insertMany([
  { _id: "n1", text: "hello",
    stars: 3 },
  { _id: "n2",
    about: { any: [1, "two"] } }
])"#),
        ("Names of an object's fields as data",
r#"SELECT jsonb_object_keys(address)
  AS names
FROM customers
LIMIT 1;"#,
r#"db.customers.aggregate([
  { $limit: 1 },
  { $project: { names: { $map: {
      input: {
        $objectToArray: "$address" },
      in: "$$this.k" } } } }
])"#),

        // Writing
        ("Insert a row",
r#"INSERT INTO products
  (id, name, price, stock, category)
VALUES
  ('p6', 'Clip', 0.5, 900,
   'stationery');"#,
r#"db.products.insertOne(
  { _id: "p6", name: "Clip",
    price: 0.5, stock: 900,
    category: "stationery" }
)"#),
        ("Insert many rows",
r#"INSERT INTO products
  (id, name, price, stock, category)
VALUES
  ('p6', 'Clip', 0.5, 900,
   'stationery'),
  ('p7', 'Rug', 80.0, 3, 'home');"#,
r#"db.products.insertMany([
  { _id: "p6", name: "Clip",
    price: 0.5, stock: 900,
    category: "stationery" },
  { _id: "p7", name: "Rug",
    price: 80.0, stock: 3,
    category: "home" }
])"#),
        ("Update the rows a condition picks",
r#"UPDATE orders
SET status = 'paid'
WHERE status = 'open';"#,
r#"db.orders.updateMany(
  { status: "open" },
  { $set: { status: "paid" } }
)"#),
        ("Update from the row's own values",
r#"UPDATE products
SET stock = stock - 1,
    price = price * 2
WHERE id = 'p2';"#,
r#"db.products.updateOne(
  { _id: "p2" },
  { $inc: { stock: -1 },
    $mul: { price: 2 } }
)"#),
        ("Update from related rows",
r#"UPDATE orders o
SET total = (
    SELECT sum(qty * price)
    FROM order_items
    WHERE order_id = o.id)
  + (
    SELECT count(*) FROM orders
    WHERE customer = o.customer)
WHERE o.id = 'o1';"#,
r#"// read, then write
const o = db.orders.findOne(
  { _id: "o1" })
const lines = o.items.reduce(
  (sum, i) => sum + i.qty * i.price, 0)
const count = db.orders.countDocuments(
  { customer: o.customer })
db.orders.updateOne(
  { _id: "o1" },
  { $set: { total: lines + count } }
)"#),
        ("Update a field inside an object",
r#"UPDATE customers
SET address = jsonb_set(
  address, '{zip}', '"100002"')
WHERE id = 'c1';"#,
r#"db.customers.updateOne(
  { _id: "c1" },
  { $set: { "address.zip": "100002" } }
)"#),
        ("Clear a field",
r#"UPDATE customers
SET email = NULL
WHERE id = 'c1';"#,
r#"db.customers.updateOne(
  { _id: "c1" },
  { $set: { email: null } }
)"#),
        ("Take a field off a row",
"(a column is on every row or on none)",
r#"db.customers.updateOne(
  { _id: "c1" },
  { $unset: { email: "" } }
)"#),
        ("Add to a list",
r#"UPDATE customers
SET tags = array_append(tags, 'back')
WHERE id = 'c2';"#,
r#"db.customers.updateOne(
  { _id: "c2" },
  { $push: { tags: "back" } }
)"#),
        ("Add to a list once",
r#"UPDATE customers
SET tags = ARRAY(
  SELECT DISTINCT unnest(
    tags || ARRAY['vip', 'gold']))
WHERE id = 'c1';"#,
r#"db.customers.updateOne(
  { _id: "c1" },
  { $addToSet: { tags: {
      $each: ["vip", "gold"] } } }
)"#),
        ("Take out of a list",
r#"UPDATE customers
SET tags = array_remove(tags, 'vip')
WHERE id = 'c1';"#,
r#"db.customers.updateOne(
  { _id: "c1" },
  { $pull: { tags: "vip" } }
)"#),
        ("Change the elements of a list that match",
r#"UPDATE order_items
SET qty = qty + 1
WHERE order_id = 'o1'
  AND price < 5;"#,
r#"db.orders.updateOne(
  { _id: "o1" },
  { $inc: { "items.$[cheap].qty": 1 } },
  { arrayFilters: [
      { "cheap.price": { $lt: 5 } }
    ] }
)"#),
        ("Delete the rows a condition picks",
r#"DELETE FROM orders
WHERE status = 'cancelled';"#,
r#"db.orders.deleteMany(
  { status: "cancelled" }
)"#),
        ("Delete every row",
r#"TRUNCATE orders CASCADE;"#,
r#"db.orders.deleteMany({})"#),
        ("Insert or replace",
r#"INSERT INTO products
  (id, name, price, stock, category)
VALUES
  ('p1', 'Pencil', 1.0, 10,
   'stationery'),
  ('p9', 'Tape', 3.0, 10,
   'stationery')
ON CONFLICT (id) DO UPDATE SET
  name = EXCLUDED.name,
  price = EXCLUDED.price,
  stock = EXCLUDED.stock,
  category = EXCLUDED.category;"#,
r#"db.products.replaceOne(
  { _id: "p1" },
  { name: "Pencil", price: 1.0,
    stock: 10,
    category: "stationery" },
  { upsert: true }
)
db.products.replaceOne(
  { _id: "p9" },
  { name: "Tape", price: 3.0,
    stock: 10,
    category: "stationery" },
  { upsert: true }
)"#),
        ("Rows a write changed",
r#"UPDATE products
SET stock = stock + 1
WHERE category = 'home'
RETURNING id, stock;"#,
r#"// updateMany gives back counts,
// so the rows are read after
db.products.updateMany(
  { category: "home" },
  { $inc: { stock: 1 } }
)
db.products.find(
  { category: "home" },
  { stock: 1 }
)"#),
        ("Insert the rows of a query",
r#"INSERT INTO archive
SELECT * FROM orders
WHERE status = 'cancelled';"#,
r#"db.orders.aggregate([
  { $match: { status: "cancelled" } },
  { $merge: { into: "archive" } }
])"#),
        ("One statement that inserts, updates and deletes by a join",
r#"MERGE INTO products p
USING incoming i ON p.id = i.id
WHEN MATCHED AND i.stock = 0 THEN
  DELETE
WHEN MATCHED THEN
  UPDATE SET stock = i.stock
WHEN NOT MATCHED THEN
  INSERT VALUES (i.id, i.name,
    i.price, i.stock, i.category);"#,
r#"db.incoming.aggregate([
  { $merge: {
      into: "products",
      on: "_id",
      whenMatched: "replace",
      whenNotMatched: "insert" } }
])"#),

        // Schema
        ("Typed fields",
r#"CREATE TABLE gauges (
  id text PRIMARY KEY,
  n integer,
  ratio double precision,
  live boolean,
  at timestamptz,
  names text[]
);
INSERT INTO gauges VALUES
  ('g1', 7, 0.5, true,
   '2026-01-01', ARRAY['a']);"#,
r#"db.createCollection("gauges", {
  validator: { $jsonSchema: {
    properties: {
      n: { bsonType: "int" },
      ratio: { bsonType: "double" },
      live: { bsonType: "bool" },
      at: { bsonType: "date" },
      names: { bsonType: "array",
        items: { bsonType: "string" } }
    } } }
})
db.gauges.insertOne(
  { _id: "g1", n: 7, ratio: 0.5,
    live: true,
    at: ISODate("2026-01-01"),
    names: ["a"] }
)"#),
        ("Optional and required fields",
r#"-- email text, city text NOT NULL
INSERT INTO customers
  (id, name, city, joined, tags,
   address)
VALUES
  ('c9', 'Zed', 'Lagos',
   '2026-01-01', '{}',
   '{"street": "9", "zip": "9"}');"#,
r#"db.runCommand({
  collMod: "customers",
  validator: { $jsonSchema: {
    required: ["name", "city",
      "joined", "tags", "address"]
  } }
})
db.customers.insertOne(
  { _id: "c9", name: "Zed",
    city: "Lagos",
    joined: ISODate("2026-01-01"),
    tags: [],
    address: { street: "9", zip: "9" } }
)"#),
        ("Defaults",
r#"CREATE TABLE plans (
  id text PRIMARY KEY,
  tier text DEFAULT 'free',
  label text GENERATED ALWAYS AS
    (upper(id)) STORED,
  made timestamptz DEFAULT now()
);
INSERT INTO plans (id) VALUES ('a');"#,
r#"// no defaults in the database:
// the application fills them in
db.plans.insertOne(
  { _id: "a", tier: "free",
    label: "A", made: new Date() }
)"#),
        ("Unique fields",
r#"CREATE TABLE logins (
  id text PRIMARY KEY,
  email text UNIQUE,
  UNIQUE (id, email)
);
INSERT INTO logins
VALUES ('l1', 'a@x.io');"#,
r#"db.logins.createIndex(
  { email: 1 }, { unique: true })
db.logins.createIndex(
  { _id: 1, email: 1 },
  { unique: true })
db.logins.insertOne(
  { _id: "l1", email: "a@x.io" }
)"#),
        ("Conditions every row must meet",
r#"CREATE TABLE ages (
  id text PRIMARY KEY,
  age integer
    CHECK (age >= 0 AND age < 150)
);
INSERT INTO ages VALUES ('a1', 36);"#,
r#"db.createCollection("ages", {
  validator: {
    age: { $gte: 0, $lt: 150 } }
})
db.ages.insertOne(
  { _id: "a1", age: 36 }
)"#),
        ("One of a fixed set of values",
r#"CREATE TYPE ticket_state
  AS ENUM ('open', 'closed');
CREATE TABLE tickets (
  id text PRIMARY KEY,
  state ticket_state
);
INSERT INTO tickets
VALUES ('t1', 'open');"#,
r#"db.createCollection("tickets", {
  validator: { $jsonSchema: {
    properties: { state: {
      enum: ["open", "closed"] } }
  } }
})
db.tickets.insertOne(
  { _id: "t1", state: "open" }
)"#),
        ("References that must resolve",
r#"-- customer text
--   REFERENCES customers (id)
SELECT id, customer FROM orders
WHERE id = 'o1';"#,
r#"// not enforced: an order may name
// a customer that does not exist
db.orders.find(
  { _id: "o1" },
  { customer: 1 }
)"#),
        ("What deleting a referenced row does",
r#"CREATE TABLE folders (
  id text PRIMARY KEY);
CREATE TABLE files (
  id text PRIMARY KEY,
  folder text REFERENCES folders
    ON DELETE CASCADE,
  owner text REFERENCES customers
    ON DELETE SET NULL
);
INSERT INTO folders VALUES ('f1');
INSERT INTO files (id, folder)
VALUES ('x', 'f1');
DELETE FROM folders;"#,
r#"// not in the database: the
// application deletes both
db.folders.insertOne({ _id: "f1" })
db.files.insertOne(
  { _id: "x", folder: "f1" })
db.files.deleteMany({ folder: "f1" })
db.folders.deleteOne({ _id: "f1" })"#),
        ("Identity of a row",
r#"-- id text PRIMARY KEY
SELECT id FROM customers
WHERE id = 'c1';"#,
r#"db.customers.find(
  { _id: "c1" },
  { _id: 1 }
)"#),
        ("Identity made of several fields",
r#"CREATE TABLE seats (
  tenant text,
  id text,
  PRIMARY KEY (tenant, id)
);"#,
r#"db.seats.insertOne(
  { _id: { tenant: "t1", id: "s1" } }
)"#),
        ("Identity chosen by the database",
r#"CREATE TABLE notes (
  id bigint GENERATED ALWAYS
    AS IDENTITY PRIMARY KEY,
  text text
);
INSERT INTO notes (text)
VALUES ('no id given');"#,
r#"// _id becomes a new ObjectId
db.notes.insertOne(
  { text: "no id given" }
)"#),
        ("Add, rename, retype and drop fields",
r#"ALTER TABLE products
  RENAME COLUMN name TO title;
ALTER TABLE products
  ALTER COLUMN stock
    TYPE double precision,
  DROP COLUMN category,
  ADD COLUMN rating integer
    DEFAULT 5,
  ADD COLUMN note text;"#,
r#"db.products.updateMany({}, [
  { $set: {
      title: "$name",
      stock: { $toDouble: "$stock" },
      rating: 5 } },
  { $unset: ["name", "category"] }
])"#),
        ("Drop a collection",
r#"CREATE TABLE scraps (
  id text PRIMARY KEY);
INSERT INTO scraps VALUES ('s');
DROP TABLE scraps;"#,
r#"db.scraps.insertOne({ _id: "s" })
db.scraps.drop()"#),
        ("Stored functions",
r#"CREATE FUNCTION with_tax(
  amount float, rate float DEFAULT 0.5)
RETURNS float LANGUAGE sql AS
  $$ SELECT amount * (1 + rate) $$;
CREATE VIEW paid AS
  SELECT * FROM orders
  WHERE status = 'paid';"#,
r#"// functions live in the application
const withTax = (amount, rate = 0.5) =>
  amount * (1 + rate)
const paid = () =>
  db.orders.find({ status: "paid" })"#),
        ("Saved query",
r#"CREATE VIEW paid AS
  SELECT * FROM orders
  WHERE status = 'paid';"#,
r#"db.createView("paid", "orders", [
  { $match: { status: "paid" } }
])"#),
        ("Stored result kept up to date",
r#"CREATE MATERIALIZED VIEW sales AS
  SELECT status, sum(total)
  FROM orders
  GROUP BY status;
REFRESH MATERIALIZED VIEW sales;"#,
r#"// run again to refresh
db.orders.aggregate([
  { $group: { _id: "$status",
      total: { $sum: "$total" } } },
  { $merge: { into: "sales" } }
])"#),
        ("Value computed from other fields on every write",
r#"ALTER TABLE products
  ADD COLUMN worth numeric
  GENERATED ALWAYS AS
    (price * stock) STORED;"#,
"(the application writes it with every change)"),
        ("Code that runs when rows change",
r#"CREATE TRIGGER stamp
  BEFORE UPDATE ON orders
  FOR EACH ROW
  EXECUTE FUNCTION touch();"#,
r#"db.orders.watch().on("change",
  change => { /* react */ })"#),
        ("One collection split by a key",
r#"CREATE TABLE events (
  id text, placed timestamptz
) PARTITION BY RANGE (placed);"#,
r#"sh.shardCollection(
  "shop.orders", { placed: 1 })"#),

        // Indexes
        ("Index on a field",
r#"CREATE INDEX ON jobs (state);"#,
r#"db.jobs.createIndex({ state: 1 })"#),
        ("Index on several fields",
r#"CREATE INDEX ON jobs (state, n);"#,
r#"db.jobs.createIndex(
  { state: 1, n: 1 })"#),
        ("Index on a field inside an object",
r#"CREATE INDEX ON places
  ((address->>'zip'));"#,
r#"db.places.createIndex(
  { "address.zip": 1 })"#),
        ("Index that keeps values unique",
r#"CREATE UNIQUE INDEX
  ON logins (email);"#,
r#"db.logins.createIndex(
  { email: 1 }, { unique: true })"#),
        ("Index for following references backwards",
r#"CREATE INDEX ON orders (customer);
SELECT count(*) AS n FROM orders
WHERE customer = 'c1';"#,
r#"db.orders.createIndex({ customer: 1 })
db.orders.countDocuments(
  { customer: "c1" })"#),
        ("How a query will run",
r#"EXPLAIN SELECT * FROM orders
WHERE status = 'paid';"#,
r#"db.orders.find({ status: "paid" })
  .explain()"#),
        ("Index on the elements of a list",
r#"CREATE INDEX ON posts
  USING gin (tags);"#,
r#"db.posts.createIndex({ tags: 1 })"#),
        ("Index on an expression",
r#"CREATE INDEX ON names
  (lower(name));"#,
"(store the lowered value in a field and index that)"),
        ("Index over some of the rows",
r#"CREATE INDEX ON jobs (state)
WHERE state = 'open';"#,
r#"db.jobs.createIndex(
  { state: 1 },
  { partialFilterExpression: {
      state: "open" } })"#),

        // Transactions
        ("Several writes kept or dropped together",
r#"BEGIN;
UPDATE products
SET stock = stock - 10
WHERE id = 'p1';
UPDATE products
SET stock = stock + 10
WHERE id = 'p2';
COMMIT;"#,
r#"const session =
  db.getMongo().startSession()
session.withTransaction(() => {
  const products = session
    .getDatabase("shop").products
  products.updateOne(
    { _id: "p1" },
    { $inc: { stock: -10 } })
  products.updateOne(
    { _id: "p2" },
    { $inc: { stock: 10 } })
})"#),
        ("Partial rollback inside a transaction",
r#"BEGIN;
SAVEPOINT before_stock;
UPDATE products SET stock = 0;
ROLLBACK TO before_stock;
COMMIT;"#,
"(none)"),
        ("Choice of isolation level",
r#"BEGIN ISOLATION LEVEL
  SERIALIZABLE;"#,
r#"session.startTransaction({
  readConcern: { level: "snapshot" },
  writeConcern: { w: "majority" }
})"#),
        ("Locking rows for a later write",
r#"SELECT * FROM products
WHERE id = 'p1'
FOR UPDATE;"#,
"(none)"),

        // Search and specialised data
        ("Nearest vectors",
r#"CREATE EXTENSION vector;
CREATE TABLE docs (
  id text PRIMARY KEY,
  embedding vector(3)
);
CREATE INDEX ON docs USING hnsw
  (embedding vector_cosine_ops);
INSERT INTO docs VALUES
  ('d1', '[1, 0, 0]'),
  ('d2', '[0, 1, 0]');"#,
r#"// Atlas only, with a vector
// index made in Atlas
db.docs.insertMany([
  { _id: "d1",
    embedding: [1.0, 0.0, 0.0] },
  { _id: "d2",
    embedding: [0.0, 1.0, 0.0] }
])"#),
        ("Full-text search",
r#"SELECT id FROM customers
WHERE to_tsvector(name)
   @@ to_tsquery('ada');"#,
r#"db.customers.find(
  { $text: { $search: "ada" } }
)"#),
        ("Places and distances",
r#"-- PostGIS
SELECT id FROM customers
WHERE ST_DWithin(
  location,
  ST_MakePoint(3.4, 6.5)::geography,
  1000);"#,
r#"db.customers.find({ location: {
  $near: {
    $geometry: { type: "Point",
      coordinates: [3.4, 6.5] },
    $maxDistance: 1000 } } })"#),
        ("Time series in buckets",
r#"SELECT
  to_char(placed, 'IYYY-"W"IW')
    AS week,
  count(*) AS n,
  sum(total) AS total
FROM orders
GROUP BY week
ORDER BY week
LIMIT 3;"#,
r#"db.orders.aggregate([
  { $group: {
      _id: { $dateToString: {
        format: "%G-W%V",
        date: "$placed" } },
      n: { $sum: 1 },
      total: { $sum: "$total" } } },
  { $sort: { _id: 1 } },
  { $limit: 3 }
])"#),
        ("Rows that expire",
r#"-- run on a schedule
DELETE FROM sessions
WHERE made < now()
  - INTERVAL '1 day';"#,
r#"db.sessions.createIndex(
  { made: 1 },
  { expireAfterSeconds: 86400 })"#),
        ("Being told when rows change",
r#"-- with a trigger that calls NOTIFY
LISTEN orders_changed;"#,
r#"db.orders.watch()"#),

        // Operations
        ("Users, roles and permissions",
r#"CREATE ROLE clerk LOGIN;
GRANT SELECT ON orders TO clerk;"#,
r#"db.createUser({
  user: "clerk",
  pwd: passwordPrompt(),
  roles: [
    { role: "read", db: "shop" }
  ]
})"#),
    ]
}
