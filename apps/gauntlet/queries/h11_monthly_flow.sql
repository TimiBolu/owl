SELECT strftime('%Y-%m', t.opened) AS month,
  COUNT(*) AS opened,
  SUM(t.closed IS NOT NULL) AS closed,
  SUM(t.assignee IS NULL) AS unowned,
  SUM(EXISTS (SELECT 1 FROM tasks_labels l WHERE l.parent = t.id AND l.value = 'urgent')) AS urgent,
  SUM(t.points) AS points
FROM tasks t GROUP BY month ORDER BY month
