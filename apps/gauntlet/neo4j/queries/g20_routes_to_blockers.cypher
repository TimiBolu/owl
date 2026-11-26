MATCH (t:Task) WITH t ORDER BY t.id LIMIT 400
MATCH p = (b:Task)-[:BLOCKS*1..5]->(t)
WHERE all(n IN nodes(p) WHERE single(m IN nodes(p) WHERE m = n))
RETURN b.id AS id, count(*) AS routes, min(length(p)) AS nearest
ORDER BY routes DESC, id
LIMIT 20
