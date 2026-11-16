SELECT g.id,
  (SELECT COUNT(*) FROM people p WHERE p.org = g.id) AS people,
  (SELECT COALESCE(SUM(p.salary), 0) FROM people p WHERE p.org = g.id) AS payroll,
  (SELECT COUNT(*) FROM projects j WHERE j.org = g.id) AS projects,
  (SELECT COUNT(*) FROM tasks t JOIN projects j ON j.id = t.project WHERE j.org = g.id) AS tasks,
  (SELECT COALESCE(SUM(t.points), 0) FROM tasks t JOIN projects j ON j.id = t.project
    WHERE j.org = g.id AND t.state = 'open') AS open_points,
  (SELECT COALESCE(SUM(a.hours), 0) FROM assignments a JOIN projects j ON j.id = a.project
    WHERE j.org = g.id) AS hours
FROM orgs g ORDER BY g.id
