WITH ranked AS (
  SELECT unit, salary,
    ROW_NUMBER() OVER (PARTITION BY unit ORDER BY salary) - 1 AS i,
    COUNT(*) OVER (PARTITION BY unit) AS n
  FROM people),
medians AS (
  SELECT unit, MAX(n) AS n,
    SUM(CASE WHEN i = CAST(0.5 * (n - 1) AS INTEGER) THEN salary * (1 - (0.5 * (n - 1) - CAST(0.5 * (n - 1) AS INTEGER)))
             WHEN i = CAST(0.5 * (n - 1) AS INTEGER) + 1 THEN salary * (0.5 * (n - 1) - CAST(0.5 * (n - 1) AS INTEGER))
             ELSE 0 END) AS median
  FROM ranked GROUP BY unit)
SELECT m.unit AS id, m.median,
  (SELECT COUNT(DISTINCT p.level) FROM people p WHERE p.unit = m.unit) AS skills
FROM medians m WHERE m.n >= 20 AND m.median > 50000 ORDER BY m.unit
