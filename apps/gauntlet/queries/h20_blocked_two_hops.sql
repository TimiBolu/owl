SELECT COUNT(*) AS count FROM tasks t
WHERE EXISTS (
  SELECT 1 FROM links a JOIN links b ON b.blocked = a.blocker JOIN tasks s ON s.id = b.blocker
  WHERE a.blocked = t.id AND s.state = 'open')
