-- The twenty most recent paid orders
SELECT id, placed, total FROM orders
WHERE status = 'paid'
ORDER BY placed DESC, id ASC LIMIT 20
