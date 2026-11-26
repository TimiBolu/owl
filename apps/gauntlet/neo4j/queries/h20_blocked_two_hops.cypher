MATCH (t:Task)
WHERE EXISTS { (t)<-[:BLOCKS]-(:Task)<-[:BLOCKS]-(:Task {state: 'open'}) }
RETURN count(t) AS count
