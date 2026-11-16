SELECT g.id,
  (SELECT json_group_array(json(x.o)) FROM (
     SELECT json_object(
       'id', j.id,
       'open_points', (SELECT COALESCE(SUM(t.points), 0) FROM tasks t
                        WHERE t.project = j.id AND t.state <> 'done'),
       'team', json((SELECT json_group_array(json(y.o)) FROM (
          SELECT json_object('who', p.name, 'hours', a.hours) AS o
          FROM assignments a JOIN people p ON p.id = a.person
          WHERE a.project = j.id ORDER BY a.hours DESC, a.id LIMIT 2) y))) AS o
     FROM projects j
     WHERE j.org = g.id AND EXISTS (
       SELECT 1 FROM tasks t WHERE t.project = j.id AND t.state <> 'done'
         AND EXISTS (SELECT 1 FROM links l WHERE l.blocked = t.id))
     ORDER BY (SELECT COALESCE(SUM(t.points), 0) FROM tasks t
                WHERE t.project = j.id AND t.state <> 'done') DESC, j.id
     LIMIT 3) x) AS top
FROM orgs g ORDER BY g.id
