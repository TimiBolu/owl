MATCH (po:Org)<-[:OF_ORG]-(:Person)-[w:WORKS_ON]->(:Project)-[:OF_ORG]->(jo:Org)
WHERE po <> jo
RETURN jo.id AS org, count(*) AS n, sum(w.hours) AS hours
ORDER BY org
