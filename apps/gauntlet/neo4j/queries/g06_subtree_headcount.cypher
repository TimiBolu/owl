MATCH (u:Unit) WHERE NOT EXISTS { (u)-[:CHILD_OF]->(:Unit) }
OPTIONAL MATCH p = (u)<-[:CHILD_OF*]-(c:Unit)
WITH u, collect(DISTINCT c) AS below, max(length(p)) AS deepest
CALL (below) {
  UNWIND below AS c
  MATCH (c)<-[:IN_UNIT]-(x:Person)
  RETURN count(x) AS under
}
RETURN u.id AS id, size(below) AS units, deepest,
       COUNT { (u)<-[:IN_UNIT]-(:Person) } + under AS people
ORDER BY id
