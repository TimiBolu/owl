SELECT id, unit, salary, position, rank, dense FROM (
  SELECT p.id, p.unit, p.salary,
    ROW_NUMBER() OVER (PARTITION BY p.unit ORDER BY p.salary DESC, p.id) AS position,
    RANK() OVER (PARTITION BY p.unit ORDER BY p.salary DESC) AS rank,
    DENSE_RANK() OVER (PARTITION BY p.unit ORDER BY p.salary DESC) AS dense
  FROM people p)
WHERE position <= 3 ORDER BY unit, salary DESC, id
