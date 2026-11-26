MATCH (j:Project {state: 'active'})-[:OWNED_BY]->(:Person)-[:IN_UNIT]->(u:Unit)
WHERE NOT EXISTS { (j)<-[:WORKS_ON]-(:Person)-[:IN_UNIT]->(u) }
RETURN count(j) AS count
