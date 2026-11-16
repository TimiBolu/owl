SELECT w.author AS author_id, COUNT(*) AS entries, SUM(w.minutes) AS minutes
FROM tasks_worklog w GROUP BY w.author
ORDER BY minutes DESC, author_id ASC LIMIT 20
