-- Top-level categories with their children and product statistics
SELECT r.id, r.name,
  (SELECT COUNT(*) FROM products p JOIN categories k ON k.id = p.category WHERE k.parent = r.id) AS products,
  (SELECT json_group_array(json_object(
            'id', k.id,
            'products', (SELECT COUNT(*) FROM products p WHERE p.category = k.id),
            'average_price', (SELECT AVG(p.price) FROM products p WHERE p.category = k.id)) ORDER BY k.id ASC)
     FROM categories k WHERE k.parent = r.id) AS children
FROM categories r WHERE r.parent IS NULL ORDER BY r.id ASC
