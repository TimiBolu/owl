-- Customers ranked by lifetime paid spend
SELECT c.id, c.name, COUNT(o.id) AS orders,
       COALESCE(SUM(CASE WHEN o.status = 'paid' THEN o.total END), 0) AS spent
FROM customers c LEFT JOIN orders o ON o.customer = c.id
GROUP BY c.id, c.name
ORDER BY spent DESC, c.id ASC LIMIT 20
