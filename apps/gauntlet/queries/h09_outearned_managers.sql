SELECT m.id, m.salary,
  (SELECT MAX(r.salary) FROM people r WHERE r.manager = m.id) AS best_report,
  (SELECT COUNT(*) FROM people r WHERE r.manager = m.id) AS reports
FROM people m
WHERE (SELECT MAX(r.salary) FROM people r WHERE r.manager = m.id) > m.salary
ORDER BY m.id LIMIT 20
