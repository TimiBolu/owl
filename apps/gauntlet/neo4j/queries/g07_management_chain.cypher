MATCH p = (:Person {id: 'p001998'})-[:REPORTS_TO*]->(m:Person)
RETURN m.id AS id, m.level AS level, length(p) AS depth
ORDER BY depth
