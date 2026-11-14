-- Order count and revenue by status
SELECT status, COUNT(*) AS n, SUM(total) AS revenue, AVG(total) AS average, MAX(total) AS largest
FROM orders GROUP BY status ORDER BY status ASC
