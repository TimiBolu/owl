MATCH (u:Unit)-[:OF_ORG]->(:Org {id: 'g03'})
WHERE NOT EXISTS { (u)-[:CHILD_OF]->(:Unit) }
RETURN u.id AS id, u.name AS name,
  COLLECT {
    MATCH (u)<-[:CHILD_OF]-(c:Unit)
    WITH c ORDER BY c.id
    RETURN {
      id: c.id,
      headcount: COUNT { (c)<-[:IN_UNIT]-(:Person) },
      people: COLLECT {
        MATCH (c)<-[:IN_UNIT]-(p:Person)
        WITH p ORDER BY p.salary DESC, p.id LIMIT 3
        RETURN {
          id: p.id, salary: p.salary,
          projects: COLLECT {
            MATCH (p)-[w:WORKS_ON]->(j:Project)
            WITH w, j ORDER BY w.id
            RETURN { project: j.name, hours: w.hours }
          }
        }
      }
    }
  } AS children
