WITH RECURSIVE down(id, depth, chain) AS (
  SELECT u.id, 0, u.budget FROM units u WHERE u.parent IS NULL
  UNION ALL
  SELECT c.id, d.depth + 1, d.chain + c.budget FROM units c JOIN down d ON c.parent = d.id)
SELECT id, depth, chain FROM down WHERE depth > 0 ORDER BY chain DESC, id ASC LIMIT 20
