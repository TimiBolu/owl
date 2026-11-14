-- One order with its customer and line items
SELECT o.id, o.status, o.total, c.name AS customer, c.country AS country,
  (SELECT json_group_array(json_object('product', p.name, 'qty', i.qty, 'price', i.price) ORDER BY i.line)
     FROM order_items i JOIN products p ON p.id = i.product
    WHERE i.order_id = o.id) AS items
FROM orders o JOIN customers c ON c.id = o.customer
WHERE o.id = 'o00000042'
