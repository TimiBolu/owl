MATCH (:Person {id: 'p000001'})-[:WORKS_ON]->(:Project)<-[:WORKS_ON]-(c:Person)
WHERE c.id <> 'p000001'
RETURN DISTINCT c.id AS id, c.name AS name
ORDER BY id
LIMIT 20
