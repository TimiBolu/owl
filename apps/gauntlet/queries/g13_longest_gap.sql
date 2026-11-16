WITH paired AS (
  SELECT e.person,
    strftime('%s', e.at) - strftime('%s', LAG(e.at) OVER w) AS gap,
    LEAD(e.kind) OVER w AS after
  FROM events e WINDOW w AS (PARTITION BY e.person ORDER BY e.at, e.id))
SELECT person, MAX(gap) AS longest, SUM(after = 'deploy') AS deploys_next
FROM paired WHERE gap IS NOT NULL
GROUP BY person ORDER BY longest DESC, person ASC LIMIT 20
