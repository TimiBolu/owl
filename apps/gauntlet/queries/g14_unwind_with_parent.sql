SELECT t.state AS task_state, w.state AS entry_state, COUNT(*) AS entries, SUM(w.minutes) AS minutes
FROM tasks_worklog w JOIN tasks t ON t.id = w.parent
GROUP BY t.state, w.state ORDER BY t.state, w.state
