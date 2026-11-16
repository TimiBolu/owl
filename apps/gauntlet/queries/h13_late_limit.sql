SELECT j.id,
  (SELECT COUNT(*) FROM assignments a WHERE a.project = j.id) AS team,
  (SELECT json_group_array(json(x.o)) FROM (
     SELECT json_object('id', t.id, 'points', t.points) AS o
     FROM tasks t WHERE t.project = j.id ORDER BY t.points DESC, t.id LIMIT 1) x) AS biggest
FROM projects j ORDER BY j.id LIMIT 10
