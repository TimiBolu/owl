SELECT u.id, u.name,
  (SELECT json_group_array(json(x.o)) FROM (
     SELECT json_object(
       'id', c.id,
       'headcount', (SELECT COUNT(*) FROM people p WHERE p.unit = c.id),
       'people', json((SELECT json_group_array(json(y.o)) FROM (
          SELECT json_object(
            'id', p.id, 'salary', p.salary,
            'projects', json((SELECT json_group_array(json(z.o)) FROM (
               SELECT json_object('project', j.name, 'hours', a.hours) AS o
               FROM assignments a JOIN projects j ON j.id = a.project
               WHERE a.person = p.id ORDER BY a.id) z))) AS o
          FROM people p WHERE p.unit = c.id ORDER BY p.salary DESC, p.id LIMIT 3) y))) AS o
     FROM units c WHERE c.parent = u.id ORDER BY c.id) x) AS children
FROM units u WHERE u.org = 'g03' AND u.parent IS NULL
