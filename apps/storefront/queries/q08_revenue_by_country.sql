-- Paid revenue grouped through the customer reference
SELECT c.country AS country, COUNT(*) AS orders, SUM(o.total) AS revenue
FROM orders o JOIN customers c ON c.id = o.customer
WHERE o.status = 'paid'
GROUP BY c.country ORDER BY revenue DESC, country ASC
