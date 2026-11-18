SELECT a.id AS a, b.id AS b
FROM people a JOIN people b ON b.unit = a.unit AND b.level = a.level AND b.id > a.id
ORDER BY a.id, b.id LIMIT 200
