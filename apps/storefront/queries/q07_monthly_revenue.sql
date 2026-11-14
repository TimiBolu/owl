-- Paid revenue by month
SELECT substr(placed, 1, 7) AS month, COUNT(*) AS orders, SUM(total) AS revenue
FROM orders WHERE status = 'paid'
GROUP BY month ORDER BY month ASC
