SELECT p.id,
  SUM(p.salary) OVER u AS unit_pay,
  COUNT(*) OVER u AS size,
  MAX(p.salary) OVER u AS best,
  SUM(p.level >= 5) OVER u AS seniors,
  ROW_NUMBER() OVER (PARTITION BY p.unit ORDER BY p.id) AS k
FROM people p WINDOW u AS (PARTITION BY p.unit)
ORDER BY p.id LIMIT 300
