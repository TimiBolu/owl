-- How many countries placed a paid order in one month
SELECT COUNT(DISTINCT c.country) AS count
FROM orders o JOIN customers c ON c.id = o.customer
WHERE o.status = 'paid' AND o.placed >= '2026-01-01' AND o.placed < '2026-02-01'
