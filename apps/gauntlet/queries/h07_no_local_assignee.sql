SELECT COUNT(*) AS count
FROM projects j JOIN people o ON o.id = j.owner
WHERE j.state = 'active' AND NOT EXISTS (
  SELECT 1 FROM assignments a JOIN people p ON p.id = a.person
  WHERE a.project = j.id AND p.unit = o.unit)
