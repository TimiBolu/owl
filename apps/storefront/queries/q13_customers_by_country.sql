-- Customers per country, with a conditional count
SELECT country, COUNT(*) AS customers,
       SUM(CASE WHEN tier = 'gold' THEN 1 ELSE 0 END) AS gold, MAX(joined) AS newest
FROM customers GROUP BY country ORDER BY country ASC
