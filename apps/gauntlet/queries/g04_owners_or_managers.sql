SELECT p.id, p.name FROM people p
WHERE p.id IN (
  SELECT j.owner FROM projects j WHERE j.state = 'active'
  UNION
  SELECT r.manager FROM people r GROUP BY r.manager HAVING COUNT(*) >= 5
  EXCEPT
  SELECT d.id FROM people d WHERE d.departed IS NOT NULL)
ORDER BY p.id LIMIT 30
