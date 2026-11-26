MATCH (p:Person)
WHERE p.level >= 6
   OR COUNT { (p)<-[:REPORTS_TO]-(:Person) } > 8
   OR COUNT { (p)<-[:OWNED_BY]-(:Project {state: 'active'}) } >= 2
RETURN count(p) AS count
