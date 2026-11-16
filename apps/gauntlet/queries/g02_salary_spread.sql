WITH ranked AS (
  SELECT level, salary,
    ROW_NUMBER() OVER (PARTITION BY level ORDER BY salary) - 1 AS i,
    COUNT(*) OVER (PARTITION BY level) AS n
  FROM people),
at AS (
  SELECT level, salary, i, n,
    0.5 * (n - 1) AS m, CAST(0.5 * (n - 1) AS INTEGER) AS mi,
    0.9 * (n - 1) AS q, CAST(0.9 * (n - 1) AS INTEGER) AS qi
  FROM ranked)
SELECT level, MAX(n) AS n,
  SUM(CASE WHEN i = mi THEN salary * (1 - (m - mi)) WHEN i = mi + 1 THEN salary * (m - mi) ELSE 0 END) AS median,
  SUM(CASE WHEN i = qi THEN salary * (1 - (q - qi)) WHEN i = qi + 1 THEN salary * (q - qi) ELSE 0 END) AS p90
FROM at GROUP BY level ORDER BY level
