-- Each gold customer with their three latest orders
SELECT c.id, c.name,
  (SELECT json_group_array(json_object('id', l.id, 'placed', l.placed, 'total', l.total) ORDER BY l.placed DESC, l.id ASC)
     FROM (SELECT id, placed, total FROM orders
            WHERE customer = c.id ORDER BY placed DESC, id ASC LIMIT 3) l) AS latest
FROM (SELECT id, name FROM customers WHERE tier = 'gold' ORDER BY id ASC LIMIT 50) c
ORDER BY c.id ASC
