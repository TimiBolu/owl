-- Orders larger than their own customer's average order
SELECT COUNT(*) AS count FROM orders o
WHERE o.total > (SELECT AVG(x.total) FROM orders x WHERE x.customer = o.customer)
