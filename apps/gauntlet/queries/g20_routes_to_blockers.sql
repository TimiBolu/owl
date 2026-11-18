WITH RECURSIVE r(id, depth, path) AS (
  SELECT l.blocker, 1, '/' || l.blocked || '/' || l.blocker || '/' FROM links l
  WHERE l.blocked IN (SELECT id FROM tasks ORDER BY id LIMIT 400)
  UNION ALL
  SELECT l.blocker, r.depth + 1, r.path || l.blocker || '/' FROM links l JOIN r ON l.blocked = r.id
  WHERE r.depth < 5 AND instr(r.path, '/' || l.blocker || '/') = 0)
SELECT id, COUNT(*) AS routes, MIN(depth) AS nearest FROM r GROUP BY id ORDER BY routes DESC, id ASC LIMIT 20
