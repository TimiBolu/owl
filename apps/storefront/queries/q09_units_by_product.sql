-- Best-selling products, from the line items inside paid orders
SELECT i.product AS product_id, SUM(i.qty) AS units, SUM(i.qty * i.price) AS revenue
FROM order_items i JOIN orders o ON o.id = i.order_id
WHERE o.status = 'paid'
GROUP BY i.product ORDER BY units DESC, product_id ASC LIMIT 20
