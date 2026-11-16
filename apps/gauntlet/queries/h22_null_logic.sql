SELECT COUNT(*) AS count FROM tasks t JOIN tasks p ON p.id = t.parent
WHERE t.assignee IS NULL AND p.assignee IS NOT NULL AND t.closed IS NULL
