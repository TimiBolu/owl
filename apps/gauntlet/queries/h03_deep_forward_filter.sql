SELECT COUNT(*) AS count
FROM tasks t
JOIN people a ON a.id = t.assignee
JOIN units u ON u.id = a.unit
JOIN units up ON up.id = u.parent
JOIN orgs g ON g.id = up.org
JOIN projects j ON j.id = t.project
JOIN people o ON o.id = j.owner
JOIN people m ON m.id = o.manager
JOIN tasks pt ON pt.id = t.parent
WHERE g.plan = 'enterprise' AND m.level >= 3 AND pt.state = 'done'
