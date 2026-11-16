SELECT COUNT(*) AS count FROM people p
WHERE p.salary > 1.2 * (SELECT AVG(x.salary) FROM people x WHERE x.unit = p.unit)
