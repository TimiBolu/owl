SELECT COUNT(*) AS count FROM (
  SELECT j.owner FROM projects j
  INTERSECT
  SELECT r.manager FROM people r WHERE r.manager IS NOT NULL)
