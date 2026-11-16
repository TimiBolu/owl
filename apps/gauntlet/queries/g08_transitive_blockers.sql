WITH RECURSIVE up(id, depth) AS (
  SELECT l.blocker, 1 FROM links l WHERE l.blocked = 't0002717'
  UNION
  SELECT l.blocker, up.depth + 1 FROM up JOIN links l ON l.blocked = up.id)
SELECT t.id, t.state, MIN(up.depth) AS depth
FROM up JOIN tasks t ON t.id = up.id GROUP BY t.id ORDER BY t.id
