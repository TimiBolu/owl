MATCH (u:Unit) WHERE NOT EXISTS { (u)-[:CHILD_OF]->(:Unit) }
RETURN u.id AS id,
  COUNT { (u)<-[:IN_UNIT]-(:Person) } AS direct,
  COUNT { (u)<-[:CHILD_OF]-(:Unit)<-[:IN_UNIT]-(:Person) } AS below_1,
  COUNT { (u)<-[:CHILD_OF*2]-(:Unit)<-[:IN_UNIT]-(:Person) } AS below_2,
  COUNT { (u)<-[:CHILD_OF*3]-(:Unit)<-[:IN_UNIT]-(:Person) } AS below_3
ORDER BY id
