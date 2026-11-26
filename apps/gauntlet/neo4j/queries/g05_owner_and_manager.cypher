MATCH (p:Person)
WHERE EXISTS { (p)<-[:OWNED_BY]-(:Project) }
  AND EXISTS { (p)<-[:REPORTS_TO]-(:Person) }
RETURN count(p) AS count
