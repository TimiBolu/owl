SELECT COUNT(*) AS count FROM (
  SELECT w.parent FROM tasks_worklog w WHERE w.minutes > 400
  GROUP BY w.parent HAVING COUNT(*) >= 3)
