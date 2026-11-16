SELECT u.id,
  (SELECT json_group_array(json(x.o)) FROM (
     SELECT json_object('id', p.id, 'salary', p.salary) AS o
     FROM people p WHERE p.unit = u.id ORDER BY p.salary DESC, p.id LIMIT 3) x) AS top
FROM units u ORDER BY u.id
