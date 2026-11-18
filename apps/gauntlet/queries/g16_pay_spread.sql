SELECT level,
  (SUM(salary * salary) - SUM(salary) * SUM(salary) / COUNT(*)) / (COUNT(*) - 1) AS sample,
  (SUM(salary * salary) - SUM(salary) * SUM(salary) / COUNT(*)) / COUNT(*) AS whole
FROM people GROUP BY level ORDER BY level
