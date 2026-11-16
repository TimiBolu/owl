WITH RECURSIVE chain(id, depth) AS (
  SELECT p.manager, 1 FROM people p WHERE p.id = 'p001998' AND p.manager IS NOT NULL
  UNION ALL
  SELECT p.manager, c.depth + 1 FROM chain c JOIN people p ON p.id = c.id WHERE p.manager IS NOT NULL)
SELECT p.id, p.level, c.depth FROM chain c JOIN people p ON p.id = c.id ORDER BY c.depth
