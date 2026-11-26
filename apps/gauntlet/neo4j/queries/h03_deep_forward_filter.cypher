MATCH (t:Task)-[:ASSIGNED_TO]->(:Person)-[:IN_UNIT]->(:Unit)-[:CHILD_OF]->(:Unit)-[:OF_ORG]->(:Org {plan: 'enterprise'}),
      (t)-[:IN_PROJECT]->(:Project)-[:OWNED_BY]->(:Person)-[:REPORTS_TO]->(m:Person),
      (t)-[:SUBTASK_OF]->(:Task {state: 'done'})
WHERE m.level >= 3
RETURN count(t) AS count
