SELECT COUNT(DISTINCT t.assignee) AS count FROM tasks t WHERE t.state = 'open'
