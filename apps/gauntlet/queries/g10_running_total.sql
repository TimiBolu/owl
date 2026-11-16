SELECT e.id, ROW_NUMBER() OVER w AS n, SUM(e.amount) OVER w AS running
FROM events e WHERE e.kind = 'deploy'
WINDOW w AS (PARTITION BY e.person ORDER BY e.at, e.id)
ORDER BY e.person, e.at, e.id LIMIT 500
