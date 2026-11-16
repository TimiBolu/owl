SELECT COUNT(*) AS count FROM people p
WHERE p.level >= 6
   OR (SELECT COUNT(*) FROM people r WHERE r.manager = p.id) > 8
   OR (SELECT COUNT(*) FROM projects j WHERE j.owner = p.id AND j.state = 'active') >= 2
