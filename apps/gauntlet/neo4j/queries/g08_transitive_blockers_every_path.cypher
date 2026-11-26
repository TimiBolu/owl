MATCH p = (b:Task)-[:BLOCKS*]->(:Task {id: 't0002717'})
RETURN b.id AS id, b.state AS state, min(length(p)) AS depth
ORDER BY id
