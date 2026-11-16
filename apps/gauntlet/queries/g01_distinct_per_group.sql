SELECT j.org AS org, COUNT(DISTINCT a.person) AS people, COUNT(DISTINCT a.role) AS roles, COUNT(*) AS n
FROM assignments a JOIN projects j ON j.id = a.project
GROUP BY j.org ORDER BY j.org
