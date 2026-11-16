SELECT t.id, t.points, a.name AS who
FROM tasks t JOIN projects j ON j.id = t.project JOIN people a ON a.id = t.assignee
WHERE j.org = 'g00' AND t.state = 'open'
ORDER BY t.points DESC, t.opened ASC, t.id ASC LIMIT 20
