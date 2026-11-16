SELECT DISTINCT p.id, p.name
FROM people p
JOIN assignments a ON a.person = p.id
JOIN assignments b ON b.project = a.project
WHERE b.person = 'p000001' AND p.id <> 'p000001'
ORDER BY p.id LIMIT 20
