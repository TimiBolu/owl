SELECT a.person AS id, COUNT(*) AS projects, SUM(a.hours) AS hours
FROM assignments a
GROUP BY a.person HAVING COUNT(*) >= 3 AND SUM(a.hours) > 100
ORDER BY hours DESC, id ASC LIMIT 20
