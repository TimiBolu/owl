SELECT p.id,
  (SELECT COUNT(*) FROM people r WHERE r.manager = p.id)
    + (SELECT COUNT(*) FROM people r JOIN people s ON s.manager = r.id WHERE r.manager = p.id) AS team
FROM people p WHERE p.level >= 6 ORDER BY p.id LIMIT 20
