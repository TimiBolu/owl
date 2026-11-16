SELECT j.org AS org, t.state AS state, COUNT(*) AS n, SUM(t.points) AS points, AVG(t.points) AS mean
FROM tasks t JOIN projects j ON j.id = t.project
GROUP BY j.org, t.state HAVING COUNT(*) > 50
ORDER BY j.org, t.state
