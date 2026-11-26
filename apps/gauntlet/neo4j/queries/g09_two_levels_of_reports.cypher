MATCH (p:Person) WHERE p.level >= 6
WITH p ORDER BY p.id LIMIT 20
RETURN p.id AS id, COUNT { (p)<-[:REPORTS_TO*1..2]-(:Person) } AS team
ORDER BY id
