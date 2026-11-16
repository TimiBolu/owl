WITH flagged AS (
  SELECT e.person, e.at, e.id,
    CASE WHEN LAG(e.at) OVER w IS NULL
           OR strftime('%s', e.at) - strftime('%s', LAG(e.at) OVER w) >= 21600
         THEN 1 ELSE 0 END AS fresh
  FROM events e WINDOW w AS (PARTITION BY e.person ORDER BY e.at, e.id)),
numbered AS (
  SELECT person, SUM(fresh) OVER (PARTITION BY person ORDER BY at, id) AS session FROM flagged),
sized AS (SELECT person, session, COUNT(*) AS events FROM numbered GROUP BY person, session)
SELECT person, COUNT(*) AS sessions, MAX(events) AS longest
FROM sized GROUP BY person ORDER BY sessions DESC, person ASC LIMIT 20
