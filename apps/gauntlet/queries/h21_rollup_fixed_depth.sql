SELECT u.id,
  (SELECT COUNT(*) FROM people p WHERE p.unit = u.id) AS direct,
  (SELECT COUNT(*) FROM people p JOIN units a ON a.id = p.unit WHERE a.parent = u.id) AS below_1,
  (SELECT COUNT(*) FROM people p JOIN units a ON a.id = p.unit JOIN units b ON b.id = a.parent
    WHERE b.parent = u.id) AS below_2,
  (SELECT COUNT(*) FROM people p JOIN units a ON a.id = p.unit JOIN units b ON b.id = a.parent
    JOIN units c ON c.id = b.parent WHERE c.parent = u.id) AS below_3
FROM units u WHERE u.parent IS NULL ORDER BY u.id
