MATCH (m:Person)<-[:REPORTS_TO]-(r:Person)
WITH m, max(r.salary) AS best_report, count(r) AS reports
WHERE best_report > m.salary
RETURN m.id AS id, m.salary AS salary, best_report, reports
ORDER BY id
LIMIT 20
