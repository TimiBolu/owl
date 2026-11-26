MATCH p = SHORTEST 1 (b:Task)-[:BLOCKS]->+(:Task {id: 't0002717'})
RETURN b.id AS id, b.state AS state, length(p) AS depth
ORDER BY id
