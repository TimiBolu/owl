WITH RECURSIVE tree(root, id, depth) AS (
  SELECT u.id, c.id, 1 FROM units u JOIN units c ON c.parent = u.id WHERE u.parent IS NULL
  UNION ALL
  SELECT t.root, c.id, t.depth + 1 FROM tree t JOIN units c ON c.parent = t.id)
SELECT u.id,
  (SELECT COUNT(*) FROM tree t WHERE t.root = u.id) AS units,
  (SELECT MAX(t.depth) FROM tree t WHERE t.root = u.id) AS deepest,
  (SELECT COUNT(*) FROM people p WHERE p.unit = u.id)
    + (SELECT COUNT(*) FROM people p JOIN tree t ON t.id = p.unit WHERE t.root = u.id) AS people
FROM units u WHERE u.parent IS NULL ORDER BY u.id
