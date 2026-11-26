MATCH (p:Person)-[w:WORKS_ON]->(:Project)
WITH p, count(w) AS projects, sum(w.hours) AS hours
WHERE projects >= 3 AND hours > 100
RETURN p.id AS id, projects, hours
ORDER BY hours DESC, id
LIMIT 20
