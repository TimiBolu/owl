-- Large orders from one country, through the customer reference
SELECT o.id, o.total, c.name AS customer
FROM orders o JOIN customers c ON c.id = o.customer
WHERE c.country = 'NG' AND o.total > 900
ORDER BY o.total DESC, o.id ASC LIMIT 25
