-- Customers who have paid for something and never opened a ticket
SELECT COUNT(*) AS count FROM customers c
WHERE EXISTS (SELECT 1 FROM orders o WHERE o.customer = c.id AND o.status = 'paid')
  AND NOT EXISTS (SELECT 1 FROM tickets t WHERE t.customer = c.id)
