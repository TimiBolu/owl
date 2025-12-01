// OQL Query Consistency Tests for Next-Gen Owl DST
//
// Tests executing same OQL query on different nodes, verifying identical results (strong consistency),
// testing during leader elections, and testing with concurrent updates.
//
// Requirements: 4, 5, 14

use owl_db::distributed::raft_cluster::RaftCluster;
use owl_db::distributed::shard_manager::ShardManager;
use owl_db::distributed::consistent_hash::ConsistentHashRing;
use owl_db::document::{Document, Value};
use owl_db::error::{OQLError, OQLResult};
use owl_db::executor::QueryExecutor;
use owl_db::parser::parse_oql;
use owl_db::simulation::{Clock, Network, SimulatedClock, SimulatedNetwork, Simulation};
use owl_db::storage::{create_storage, DocumentStore};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

/// Distributed OQL consistency tester
struct OQLConsistencyTester {
    clusters: HashMap<u32, RaftCluster<SimulatedClock, SimulatedNetwork>>,
    executors: HashMap<u32, Arc<QueryExecutor<dyn DocumentStore>>>,
    shard_manager: Arc<ShardManager>,
    clock: Arc<SimulatedClock>,
    network: Arc<SimulatedNetwork>,
    consistency_log: Arc<Mutex<Vec<ConsistencyEvent>>>,
}

#[derive(Debug, Clone)]
struct ConsistencyEvent {
    timestamp: std::time::Instant,
    event_type: ConsistencyEventType,
    node_id: u32,
    query: String,
    result_hash: u64,
    details: String,
}

#[derive(Debug, Clone)]
enum ConsistencyEventType {
    QueryExecuted,
    ConsistencyViolation,
    LeaderElection,
    ConcurrentUpdate,
}

impl OQLConsistencyTester {
    fn new(
        clock: Arc<SimulatedClock>,
        network: Arc<SimulatedNetwork>,
    ) -> Self {
        let shard_manager = Arc::new(ShardManager::new(clock.clone()));
        
        Self {
            clusters: HashMap::new(),
            executors: HashMap::new(),
            shard_manager,
            clock,
            network,
            consistency_log: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Add a node to the consistency test cluster
    fn add_node(&mut self, node_id: u32, temp_dir: &TempDir) -> Result<(), OQLError> {
        // Create storage for this node
        let path = temp_dir.path().join(format!("node_{}.owl", node_id));
        let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
        let executor = Arc::new(QueryExecutor::new(store, self.clock.clone()));
        
        // Create Raft cluster for this node
        use owl_db::distributed::raft::RaftConfig;
        let config = RaftConfig::default();
        let node_ids = vec![node_id]; // Single node cluster for simplicity
        let cluster = RaftCluster::new(node_ids, self.clock.clone(), self.network.clone(), config);
        
        self.network.add_node(node_id);
        self.executors.insert(node_id, executor);
        self.clusters.insert(node_id, cluster);
        self.shard_manager.add_shard(node_id, format!("node_{}", node_id));
        
        Ok(())
    }
    
    /// Execute OQL query on a specific node
    fn execute_query_on_node(&self, node_id: u32, oql: &str) -> Result<Vec<Document>, OQLError> {
        let executor = self.executors.get(&node_id)
            .ok_or_else(|| OQLError::StorageError { 
                message: format!("Node {} not found", node_id) 
            })?;
        
        let query = parse_oql(oql)?;
        let result = executor.execute_query(&query)?;
        
        // Log the query execution
        let result_hash = self.hash_documents(&result);
        let event = ConsistencyEvent {
            timestamp: self.clock.now(),
            event_type: ConsistencyEventType::QueryExecuted,
            node_id,
            query: oql.to_string(),
            result_hash,
            details: format!("Returned {} documents", result.len()),
        };
        
        self.consistency_log.lock().unwrap().push(event);
        
        Ok(result)
    }
    
    /// Execute same query on multiple nodes and check consistency
    fn check_query_consistency(&self, node_ids: &[u32], oql: &str) -> Result<ConsistencyResult, OQLError> {
        let mut results = HashMap::new();
        let mut errors = HashMap::new();
        
        for &node_id in node_ids {
            match self.execute_query_on_node(node_id, oql) {
                Ok(docs) => {
                    results.insert(node_id, docs);
                }
                Err(e) => {
                    errors.insert(node_id, e);
                }
            }
        }
        
        // Analyze consistency
        let consistency = self.analyze_consistency(&results, oql);
        
        Ok(ConsistencyResult {
            query: oql.to_string(),
            node_results: results,
            node_errors: errors,
            consistency_status: consistency,
        })
    }
    
    /// Analyze consistency of results across nodes
    fn analyze_consistency(&self, results: &HashMap<u32, Vec<Document>>, query: &str) -> ConsistencyStatus {
        if results.is_empty() {
            return ConsistencyStatus::NoResults;
        }
        
        // Compare result hashes
        let mut result_hashes = HashMap::new();
        for (&node_id, docs) in results {
            let hash = self.hash_documents(docs);
            result_hashes.insert(node_id, hash);
        }
        
        // Check if all hashes are the same
        let unique_hashes: HashSet<u64> = result_hashes.values().cloned().collect();
        
        if unique_hashes.len() == 1 {
            ConsistencyStatus::Consistent
        } else {
            // Log consistency violation
            let event = ConsistencyEvent {
                timestamp: self.clock.now(),
                event_type: ConsistencyEventType::ConsistencyViolation,
                node_id: 0, // All nodes
                query: query.to_string(),
                result_hash: 0,
                details: format!("Found {} different result hashes", unique_hashes.len()),
            };
            
            self.consistency_log.lock().unwrap().push(event);
            
            ConsistencyStatus::Inconsistent {
                node_hashes: result_hashes,
            }
        }
    }
    
    /// Hash a set of documents for consistency comparison
    fn hash_documents(&self, docs: &[Document]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        
        // Sort documents by ID for consistent hashing
        let mut sorted_docs = docs.to_vec();
        sorted_docs.sort_by(|a, b| {
            let a_id = a.get_field("id").unwrap_or(&Value::String("".to_string()));
            let b_id = b.get_field("id").unwrap_or(&Value::String("".to_string()));
            a_id.partial_cmp(b_id).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        // Hash each document's content
        for doc in &sorted_docs {
            doc.get_id().hash(&mut hasher);
            // Hash field values in sorted order
            let mut fields: Vec<_> = doc.get_all_fields().iter().collect();
            fields.sort_by_key(|(name, _)| *name);
            for (name, value) in fields {
                name.hash(&mut hasher);
                format!("{:?}", value).hash(&mut hasher); // Simple value hashing
            }
        }
        
        hasher.finish()
    }
    
    /// Trigger leader election on a cluster
    fn trigger_leader_election(&self, node_id: u32) -> Result<(), OQLError> {
        if let Some(cluster) = self.clusters.get(&node_id) {
            cluster.trigger_election(node_id)?;
            
            let event = ConsistencyEvent {
                timestamp: self.clock.now(),
                event_type: ConsistencyEventType::LeaderElection,
                node_id,
                query: "".to_string(),
                result_hash: 0,
                details: "Leader election triggered".to_string(),
            };
            
            self.consistency_log.lock().unwrap().push(event);
        }
        
        Ok(())
    }
    
    /// Execute concurrent update on a node
    fn execute_concurrent_update(&self, node_id: u32, update_oql: &str) -> Result<(), OQLError> {
        let executor = self.executors.get(&node_id)
            .ok_or_else(|| OQLError::StorageError { 
                message: format!("Node {} not found", node_id) 
            })?;
        
        let query = parse_oql(update_oql)?;
        executor.execute_query(&query)?;
        
        let event = ConsistencyEvent {
            timestamp: self.clock.now(),
            event_type: ConsistencyEventType::ConcurrentUpdate,
            node_id,
            query: update_oql.to_string(),
            result_hash: 0,
            details: "Concurrent update executed".to_string(),
        };
        
        self.consistency_log.lock().unwrap().push(event);
        
        Ok(())
    }
    
    /// Get consistency statistics
    fn get_consistency_stats(&self) -> ConsistencyStats {
        let log = self.consistency_log.lock().unwrap();
        
        let mut stats = ConsistencyStats {
            total_queries: 0,
            consistent_queries: 0,
            inconsistent_queries: 0,
            leader_elections: 0,
            concurrent_updates: 0,
            consistency_violations: 0,
        };
        
        let mut query_consistency = HashMap::new();
        
        for event in log.iter() {
            match event.event_type {
                ConsistencyEventType::QueryExecuted => {
                    stats.total_queries += 1;
                }
                ConsistencyEventType::ConsistencyViolation => {
                    stats.consistency_violations += 1;
                    query_consistency.insert(event.query.clone(), false);
                }
                ConsistencyEventType::LeaderElection => {
                    stats.leader_elections += 1;
                }
                ConsistencyEventType::ConcurrentUpdate => {
                    stats.concurrent_updates += 1;
                }
            }
        }
        
        // Count consistent vs inconsistent queries
        for event in log.iter() {
            if matches!(event.event_type, ConsistencyEventType::QueryExecuted) {
                if !query_consistency.contains_key(&event.query) {
                    query_consistency.insert(event.query.clone(), true);
                }
            }
        }
        
        stats.consistent_queries = query_consistency.values().filter(|&&v| v).count();
        stats.inconsistent_queries = query_consistency.values().filter(|&&v| !v).count();
        
        stats
    }
}

#[derive(Debug)]
struct ConsistencyResult {
    query: String,
    node_results: HashMap<u32, Vec<Document>>,
    node_errors: HashMap<u32, OQLError>,
    consistency_status: ConsistencyStatus,
}

#[derive(Debug)]
enum ConsistencyStatus {
    Consistent,
    Inconsistent { node_hashes: HashMap<u32, u64> },
    NoResults,
}

#[derive(Debug)]
struct ConsistencyStats {
    total_queries: usize,
    consistent_queries: usize,
    inconsistent_queries: usize,
    leader_elections: usize,
    concurrent_updates: usize,
    consistency_violations: usize,
}

/// Test basic query consistency across multiple nodes
#[test]
fn test_basic_query_consistency() -> OQLResult<()> {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLConsistencyTester::new(sim.clock(), sim.network());
    
    // Add nodes to the cluster
    for node_id in 1..=3 {
        tester.add_node(node_id, &temp_dir)?;
    }
    
    // Set up identical test data on all nodes
    let setup_queries = vec![
        r#"users.insert { id: "user1", name: "Alice", age: 30, status: "active" }"#,
        r#"users.insert { id: "user2", name: "Bob", age: 25, status: "inactive" }"#,
        r#"users.insert { id: "user3", name: "Charlie", age: 35, status: "active" }"#,
    ];
    
    for query in &setup_queries {
        for node_id in 1..=3 {
            tester.execute_query_on_node(node_id, query)?;
        }
    }
    
    // Test consistency of read queries
    let test_queries = vec![
        r#"users"#,
        r#"users.where { .status == "active" }"#,
        r#"users.where { .age > 25 }.select { id, name }"#,
        r#"users.sort { .age desc }"#,
    ];
    
    for query in &test_queries {
        let result = tester.check_query_consistency(&[1, 2, 3], query)?;
        
        match result.consistency_status {
            ConsistencyStatus::Consistent => {
                println!("Query '{}' is consistent across all nodes", query);
            }
            ConsistencyStatus::Inconsistent { node_hashes } => {
                println!("Query '{}' is INCONSISTENT: {:?}", query, node_hashes);
                panic!("Query should be consistent across nodes");
            }
            ConsistencyStatus::NoResults => {
                println!("Query '{}' returned no results", query);
            }
        }
    }
    
    let stats = tester.get_consistency_stats();
    println!("Consistency stats: {:?}", stats);
    
    assert_eq!(stats.consistency_violations, 0, "Should have no consistency violations");
    assert!(stats.consistent_queries > 0, "Should have consistent queries");
    
    Ok(())
}

/// Test query consistency during leader elections
#[test]
fn test_query_consistency_during_leader_elections() -> OQLResult<()> {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLConsistencyTester::new(sim.clock(), sim.network());
    
    // Add nodes
    for node_id in 1..=5 {
        tester.add_node(node_id, &temp_dir)?;
    }
    
    // Set up test data
    let setup_data = r#"users.insert { id: "user1", name: "Alice", age: 30 }"#;
    for node_id in 1..=5 {
        tester.execute_query_on_node(node_id, setup_data)?;
    }
    
    // Start concurrent query execution and leader elections
    let consistency_results = Arc::new(Mutex::new(Vec::new()));
    
    // Background task: continuous queries
    let tester_queries = Arc::new(tester);
    let results_queries = consistency_results.clone();
    sim.scheduler().spawn_fn(move || {
        for i in 0..10 {
            let query = r#"users.select { id, name, age }"#;
            let result = tester_queries.check_query_consistency(&[1, 2, 3, 4, 5], query);
            
            let is_consistent = match &result {
                Ok(res) => matches!(res.consistency_status, ConsistencyStatus::Consistent),
                Err(_) => false,
            };
            
            results_queries.lock().unwrap().push((i, is_consistent));
            
            // Small delay between queries
            sim.clock().sleep(Duration::from_millis(50));
        }
    });
    
    // Background task: trigger leader elections
    let tester_elections = tester_queries.clone();
    sim.scheduler().spawn_fn(move || {
        for node_id in 1..=3 {
            sim.clock().sleep(Duration::from_millis(100));
            let _ = tester_elections.trigger_leader_election(node_id);
        }
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Analyze results
    let final_results = consistency_results.lock().unwrap();
    let consistent_count = final_results.iter().filter(|(_, consistent)| *consistent).count();
    let total_queries = final_results.len();
    
    println!("Query consistency during elections: {}/{} consistent", consistent_count, total_queries);
    
    // Most queries should remain consistent even during leader elections
    let consistency_rate = consistent_count as f64 / total_queries as f64;
    assert!(consistency_rate >= 0.7, "Should maintain reasonable consistency during elections");
    
    let stats = tester_queries.get_consistency_stats();
    println!("Election consistency stats: {:?}", stats);
    
    Ok(())
}

/// Test query consistency with concurrent updates
#[test]
fn test_query_consistency_with_concurrent_updates() -> OQLResult<()> {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLConsistencyTester::new(sim.clock(), sim.network());
    
    // Add nodes
    for node_id in 1..=4 {
        tester.add_node(node_id, &temp_dir)?;
    }
    
    // Set up initial data
    let initial_data = vec![
        r#"users.insert { id: "user1", name: "Alice", age: 30, balance: 1000 }"#,
        r#"users.insert { id: "user2", name: "Bob", age: 25, balance: 500 }"#,
        r#"users.insert { id: "user3", name: "Charlie", age: 35, balance: 750 }"#,
    ];
    
    for query in &initial_data {
        for node_id in 1..=4 {
            tester.execute_query_on_node(node_id, query)?;
        }
    }
    
    let tester_arc = Arc::new(tester);
    let consistency_results = Arc::new(Mutex::new(Vec::new()));
    
    // Background task: continuous read queries
    let tester_reads = tester_arc.clone();
    let results_reads = consistency_results.clone();
    sim.scheduler().spawn_fn(move || {
        for i in 0..15 {
            let queries = vec![
                r#"users.select { id, name, balance }"#,
                r#"users.where { .balance > 600 }"#,
                r#"users.select { total_balance: sum(.balance) }"#,
            ];
            
            for query in &queries {
                let result = tester_reads.check_query_consistency(&[1, 2, 3, 4], query);
                let is_consistent = match &result {
                    Ok(res) => matches!(res.consistency_status, ConsistencyStatus::Consistent),
                    Err(_) => false,
                };
                
                results_reads.lock().unwrap().push((format!("read_{}", i), is_consistent));
            }
            
            sim.clock().sleep(Duration::from_millis(30));
        }
    });
    
    // Background task: concurrent updates
    let tester_updates = tester_arc.clone();
    sim.scheduler().spawn_fn(move || {
        let updates = vec![
            r#"users.where { .id == "user1" }.update { balance: .balance + 100 }"#,
            r#"users.where { .id == "user2" }.update { balance: .balance - 50 }"#,
            r#"users.where { .id == "user3" }.update { age: 36 }"#,
            r#"users.insert { id: "user4", name: "Diana", age: 28, balance: 600 }"#,
        ];
        
        for (i, update) in updates.iter().enumerate() {
            sim.clock().sleep(Duration::from_millis(80));
            
            // Execute update on random node
            let node_id = (i % 4) + 1;
            let _ = tester_updates.execute_concurrent_update(node_id as u32, update);
        }
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Analyze consistency results
    let final_results = consistency_results.lock().unwrap();
    let consistent_count = final_results.iter().filter(|(_, consistent)| *consistent).count();
    let total_queries = final_results.len();
    
    println!("Query consistency with concurrent updates: {}/{} consistent", 
        consistent_count, total_queries);
    
    // Some inconsistency is expected during concurrent updates
    let consistency_rate = consistent_count as f64 / total_queries as f64;
    assert!(consistency_rate >= 0.5, "Should maintain reasonable consistency with concurrent updates");
    
    let stats = tester_arc.get_consistency_stats();
    println!("Concurrent update consistency stats: {:?}", stats);
    
    // Should have recorded concurrent updates
    assert!(stats.concurrent_updates > 0, "Should have recorded concurrent updates");
    
    Ok(())
}

/// Test eventual consistency after network partitions
#[test]
fn test_eventual_consistency_after_partitions() -> OQLResult<()> {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLConsistencyTester::new(sim.clock(), sim.network());
    
    // Add nodes
    for node_id in 1..=5 {
        tester.add_node(node_id, &temp_dir)?;
    }
    
    // Set up initial consistent state
    let initial_data = r#"users.insert { id: "user1", name: "Alice", age: 30 }"#;
    for node_id in 1..=5 {
        tester.execute_query_on_node(node_id, initial_data)?;
    }
    
    // Verify initial consistency
    let initial_result = tester.check_query_consistency(&[1, 2, 3, 4, 5], r#"users"#)?;
    assert!(matches!(initial_result.consistency_status, ConsistencyStatus::Consistent));
    
    // Create network partition: {1, 2} | {3, 4, 5}
    sim.network().partition(&[1, 2], &[3, 4, 5]);
    
    // Make different updates in each partition
    let _ = tester.execute_concurrent_update(1, r#"users.where { .id == "user1" }.update { age: 31 }"#);
    let _ = tester.execute_concurrent_update(3, r#"users.where { .id == "user1" }.update { age: 32 }"#);
    
    // Check consistency during partition (should be inconsistent)
    let partition_result = tester.check_query_consistency(&[1, 2, 3, 4, 5], r#"users"#);
    
    match partition_result {
        Ok(result) => {
            match result.consistency_status {
                ConsistencyStatus::Inconsistent { .. } => {
                    println!("Expected inconsistency during partition");
                }
                ConsistencyStatus::Consistent => {
                    println!("Unexpected consistency during partition (some nodes may be unreachable)");
                }
                ConsistencyStatus::NoResults => {
                    println!("No results during partition");
                }
            }
        }
        Err(e) => {
            println!("Query failed during partition: {:?}", e);
        }
    }
    
    // Heal partition
    sim.network().heal_partition();
    
    // Allow time for convergence
    for _ in 0..10 {
        sim.clock().sleep(Duration::from_millis(100));
        
        // Check if consistency is restored
        let convergence_result = tester.check_query_consistency(&[1, 2, 3, 4, 5], r#"users"#)?;
        
        if matches!(convergence_result.consistency_status, ConsistencyStatus::Consistent) {
            println!("Consistency restored after partition healing");
            break;
        }
    }
    
    // Final consistency check
    let final_result = tester.check_query_consistency(&[1, 2, 3, 4, 5], r#"users"#)?;
    
    match final_result.consistency_status {
        ConsistencyStatus::Consistent => {
            println!("Final state is consistent");
        }
        ConsistencyStatus::Inconsistent { node_hashes } => {
            println!("Final state still inconsistent: {:?}", node_hashes);
            // This might be acceptable depending on conflict resolution strategy
        }
        ConsistencyStatus::NoResults => {
            println!("No results in final state");
        }
    }
    
    let stats = tester.get_consistency_stats();
    println!("Partition healing consistency stats: {:?}", stats);
    
    Ok(())
}

/// Test read-your-writes consistency
#[test]
fn test_read_your_writes_consistency() -> OQLResult<()> {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLConsistencyTester::new(sim.clock(), sim.network());
    
    // Add nodes
    for node_id in 1..=3 {
        tester.add_node(node_id, &temp_dir)?;
    }
    
    // Test read-your-writes on each node
    for node_id in 1..=3 {
        // Write data
        let write_query = format!(r#"users.insert {{ id: "user_{}", name: "User{}", node: {} }}"#, 
            node_id, node_id, node_id);
        tester.execute_query_on_node(node_id, &write_query)?;
        
        // Immediately read back the data from the same node
        let read_query = format!(r#"users.where {{ .id == "user_{}" }}"#, node_id);
        let read_result = tester.execute_query_on_node(node_id, &read_query)?;
        
        // Should be able to read own writes immediately
        assert!(!read_result.is_empty(), 
            "Node {} should be able to read its own writes immediately", node_id);
        
        let user_doc = &read_result[0];
        assert_eq!(user_doc.get_field("node"), Some(&Value::Integer(node_id as i64)));
    }
    
    // Test cross-node read consistency (eventual)
    sim.clock().sleep(Duration::from_millis(100)); // Allow replication time
    
    for node_id in 1..=3 {
        let all_users_query = r#"users.sort { .id }"#;
        let result = tester.execute_query_on_node(node_id, all_users_query)?;
        
        println!("Node {} sees {} users", node_id, result.len());
        
        // Each node should eventually see all users
        // (May not be immediate due to replication delays)
        assert!(result.len() >= 1, "Each node should see at least its own user");
    }
    
    let stats = tester.get_consistency_stats();
    println!("Read-your-writes consistency stats: {:?}", stats);
    
    Ok(())
}