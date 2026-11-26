MATCH p = (root:Unit)<-[:CHILD_OF*]-(c:Unit)
WHERE NOT EXISTS { (root)-[:CHILD_OF]->(:Unit) }
RETURN c.id AS id, length(p) AS depth,
       reduce(sum = 0.0, n IN nodes(p) | sum + n.budget) AS chain
ORDER BY chain DESC, id
LIMIT 20
