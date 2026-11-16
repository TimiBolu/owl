SELECT j.org AS org, COUNT(*) AS n, SUM(a.hours) AS hours
FROM assignments a
JOIN people p ON p.id = a.person
JOIN projects j ON j.id = a.project
WHERE p.org <> j.org
GROUP BY j.org ORDER BY j.org
