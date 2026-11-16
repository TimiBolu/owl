SELECT l.value AS label, COUNT(*) AS n
FROM tasks_labels l JOIN tasks t ON t.id = l.parent
WHERE t.state <> 'done'
GROUP BY l.value ORDER BY n DESC, label ASC
