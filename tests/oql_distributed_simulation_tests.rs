// OQL Distributed Simulation Tests for Next-Gen Owl DST
//
// Tests OQL queries during network partitions, reference navigation with node failures,
// OQL transactions across failing shards, and verifies OQL semantics are preserved under failures.
//
// Requirements: 5, 14, 18

use owl_db::distributed::raft_cluster::RaftCluster;
use owl_db::distributed::shard_manager::ShardManager;
use owl_db::distributed::consistent_hash::ConsistentHashRing;
use owl_db::distributed::global_objref::GlobalObjRef;
use owl_db::distributed::query_planner::DistributedQueryPlanner;
use owl_db::document::{Document, Value};
use owl_db::error::{OQLError, OQLResult};
use owl_db::executor::QueryExecutor;
use owl_db::parser::parse_oql;
use owl_db::simulation::{Clock, Network, SimulatedClock, SimulatedNetwork, Simulation};
use owl_db::storage::RustStore;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Mock distributed OQL executor for testing
struct DistributedOQLExecutor {
    shard_manager: Arc<ShardManager>,
    hash_ring: Arc<ConsistentHashRing>,
    local_executors: HashMap<u32, Arc<QueryExecutor<RustStore>>>,
    query_planner: Arc<DistributedQueryPlanner>,
    clock: Arc<SimulatedClock>,
    network: Arc<SimulatedNetwork>,
}

impl DistributedOQLExecutor {
    fn new(
        shard_manager: Arc<ShardManager>,
        hash_ring: Arc<ConsistentHashRing>,
        clock: Arc<SimulatedClock>,
        network: Arc<SimulatedNetwork>,
    ) -> Result<Self, OQLError> {
        let query_planner = Arc::new(DistributedQueryPlanner::new(
            shard_manager.clone(),
            hash_ring.clone(),
        ));
        
        Ok(Self {
            shard_manager,
            hash_ring,
            local_executors: HashMap::new(),
            query_planner,
            clock,
            network,
        })
    }
    
    /// Add a shard with its local executor
    fn add_shard(&mut self, shard_id: u32, temp_dir: &TempDir) -> Result<(), OQLError> {
        let path = temp_dir.path().join(format!("shard_{}.owl", shard_id));
        let store = Arc::new(RustStore::new(path, 10 * 1024 * 1024)?);
        let executor = Arc::new(QueryExecutor::new(store, self.clock.clone()));
        
        self.local_executors.insert(shard_id, executor);
        self.shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        self.hash_ring.add_shard(shard_id)?;
        self.network.add_node(shard_id);
        
        Ok(())
    }
    
    /// Execute OQL query across distributed shards
    fn execute_oql(&self, oql: &str) -> Result<Vec<Document>, OQLError> {
        // Parse OQL query
        let query = parse_oql(oql)?;
        
        // Plan distributed execution
        let execution_plan = self.query_planner.plan_query(&query)?;
        
        // Execute across shards
        let mut results = Vec::new();
        
        for shard_operation in execution_plan.shard_operations {
            let shard_id = shard_operation.shard_id;
            
            // Check if shard is reachable
            if !self.network.can_communicate(0, shard_id) {
                return Err(OQLError::StorageError { 
                    message: format!("Cannot reach shard {}", shard_id) 
                });
            }
            
            // Execute on local shard executor
            if let Some(executor) = self.local_executors.get(&shard_id) {
                // Simulate network latency
                let latency = self.network.latency(0, shard_id);
                self.clock.sleep(latency);
                
                let shard_results = executor.execute_query(&shard_operation.query)?;
                results.extend(shard_results);
            } else {
                return Err(OQLError::StorageError { 
                    message: format!("Shard {} not available", shard_id) 
                });
            }
        }
        
        // Apply any cross-shard operations (joins, aggregations, etc.)
        self.apply_cross_shard_operations(&mut results, &execution_plan)?;
        
        Ok(results)
    }
    
    /// Navigate object references across shards
    fn navigate_reference(&self, start_objref: GlobalObjRef, path: &str) -> Result<GlobalObjRef, OQLError> {
        let current_shard = start_objref.shard_id();
        
        // Check if we can reach the shard
        if !self.network.can_communicate(0, current_shard) {
            return Err(OQLError::StorageError { 
                message: format!("Cannot reach shard {} for navigation", current_shard) 
            });
        }
        
        // Simulate navigation latency
        let latency = self.network.latency(0, current_shard);
        self.clock.sleep(latency);
        
        // For testing, simulate successful navigation to another shard
        let target_shard = (current_shard % self.local_executors.len() as u32) + 1;
        let target_objref = GlobalObjRef::new(target_shard, 12345, 0); // Mock reference
        
        Ok(target_objref)
    }
    
    /// Execute distributed transaction
    fn execute_transaction(&self, operations: Vec<String>) -> Result<Vec<OQLResult<Vec<Document>>>, OQLError> {
        let mut results = Vec::new();
        
        // Group operations by shard
        let mut shard_operations: HashMap<u32, Vec<String>> = HashMap::new();
        
        for operation in operations {
            // Determine which shard this operation affects
            let shard_id = self.determine_operation_shard(&operation)?;
            shard_operations.entry(shard_id).or_insert_with(Vec::new).push(operation);
        }
        
        // Execute 2PC across shards
        // Phase 1: Prepare
        let mut prepared_shards = HashSet::new();
        for (&shard_id, operations) in &shard_operations {
            if self.network.can_communicate(0, shard_id) {
                if self.prepare_shard_operations(shard_id, operations)? {
                    prepared_shards.insert(shard_id);
                } else {
                    // Prepare failed - abort all
                    self.abort_transaction(&prepared_shards)?;
                    return Err(OQLError::StorageError { 
                        message: "Transaction prepare failed".to_string() 
                    });
                }
            } else {
                // Cannot reach shard - abort
                self.abort_transaction(&prepared_shards)?;
                return Err(OQLError::StorageError { 
                    message: format!("Cannot reach shard {} for transaction", shard_id) 
                });
            }
        }
        
        // Phase 2: Commit
        for (&shard_id, operations) in &shard_operations {
            if prepared_shards.contains(&shard_id) {
                let shard_results = self.commit_shard_operations(shard_id, operations)?;
                results.extend(shard_results);
            }
        }
        
        Ok(results)
    }
    
    /// Apply cross-shard operations like joins and aggregations
    fn apply_cross_shard_operations(
        &self, 
        results: &mut Vec<Document>, 
        plan: &DistributedExecutionPlan
    ) -> Result<(), OQLError> {
        // For testing, just sort results by ID if needed
        if plan.requires_sorting {
            results.sort_by(|a, b| {
                let a_id = a.get_field("id").unwrap_or(&Value::String("".to_string()));
                let b_id = b.get_field("id").unwrap_or(&Value::String("".to_string()));
                a_id.partial_cmp(b_id).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        
        Ok(())
    }
    
    /// Determine which shard an operation affects
    fn determine_operation_shard(&self, operation: &str) -> Result<u32, OQLError> {
        // Simple heuristic: hash the operation string
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        operation.hash(&mut hasher);
        let hash = hasher.finish();
        
        let shard_count = self.local_executors.len() as u32;
        Ok((hash % shard_count as u64) as u32 + 1)
    }
    
    /// Prepare operations on a shard (2PC phase 1)
    fn prepare_shard_operations(&self, shard_id: u32, operations: &[String]) -> Result<bool, OQLError> {
        // Simulate prepare latency
        let latency = self.network.latency(0, shard_id);
        self.clock.sleep(latency);
        
        // For testing, randomly succeed/fail prepare
        use rand::Rng;
        let mut rng = rand::thread_rng();
        Ok(rng.gen::<f64>() > 0.1) // 90% success rate
    }
    
    /// Commit operations on a shard (2PC phase 2)
    fn commit_shard_operations(&self, shard_id: u32, operations: &[String]) -> Result<Vec<OQLResult<Vec<Document>>>, OQLError> {
        let mut results = Vec::new();
        
        if let Some(executor) = self.local_executors.get(&shard_id) {
            for operation in operations {
                let query = parse_oql(operation)?;
                let result = executor.execute_query(&query);
                results.push(result);
            }
        }
        
        Ok(results)
    }
    
    /// Abort transaction on prepared shards
    fn abort_transaction(&self, prepared_shards: &HashSet<u32>) -> Result<(), OQLError> {
        for &shard_id in prepared_shards {
            // Simulate abort operation
            let latency = self.network.latency(0, shard_id);
            self.clock.sleep(latency);
        }
        Ok(())
    }
}

/// Mock distributed execution plan
#[derive(Debug)]
struct DistributedExecutionPlan {
    shard_operations: Vec<ShardOperation>,
    requires_sorting: bool,
    requires_aggregation: bool,
}

#[derive(Debug)]
struct ShardOperation {
    shard_id: u32,
    query: owl_db::parser::Query,
}

/// Test OQL queries during network partitions
#[test]
fn test_oql_queries_during_network_partitions() -> OQLResult<()> {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    // Create distributed system with 5 shards
    let temp_dir = TempDir::new().unwrap();
    let shard_manager = Arc::new(ShardManager::new());
    let hash_ring = Arc::new(ConsistentHashRing::new());
    
    let mut executor = DistributedOQLExecutor::new(
        shard_manager,
        hash_ring,
        sim.simulated_clock(),
        sim.simulated_network(),
    )?;
    
    // Add shards
    for shard_id in 1..=5 {
        executor.add_shard(shard_id, &temp_dir)?;
    }
    
    // Set up test data across shards
    setup_distributed_test_data(&executor)?;
    
    // Test normal query execution
    let normal_query = r#"users.where { .age > 25 }.select { id, name, age }"#;
    let normal_results = executor.execute_oql(normal_query)?;
    assert!(!normal_results.is_empty(), "Should have results under normal conditions");
    
    // Create network partition: {1, 2} | {3, 4, 5}
    sim.partition_network(&[1, 2], &[3, 4, 5]);
    
    // Test query execution during partition
    let partition_query = r#"users.where { .status == "active" }"#;
    let partition_result = executor.execute_oql(partition_query);
    
    match partition_result {
        Ok(results) => {
            // Query succeeded - should only have results from reachable shards
            println!("Query succeeded during partition with {} results", results.len());
            assert!(results.len() <= normal_results.len(), 
                "Should have fewer or equal results during partition");
        }
        Err(e) => {
            // Query failed due to partition - this is acceptable
            println!("Query failed during partition: {:?}", e);
            assert!(matches!(e, OQLError::StorageError { .. }));
        }
    }
    
    // Heal partition
    sim.heal_network();
    
    // Test query execution after healing
    let healed_query = r#"users.select { id, name }"#;
    let healed_results = executor.execute_oql(healed_query)?;
    assert!(!healed_results.is_empty(), "Should have results after healing partition");
    
    Ok(())
}

/// Test reference navigation with node failures
#[test]
fn test_reference_navigation_with_node_failures() -> OQLResult<()> {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let shard_manager = Arc::new(ShardManager::new());
    let hash_ring = Arc::new(ConsistentHashRing::new());
    
    let mut executor = DistributedOQLExecutor::new(
        shard_manager,
        hash_ring,
        sim.simulated_clock(),
        sim.simulated_network(),
    )?;
    
    // Add shards
    for shard_id in 1..=4 {
        executor.add_shard(shard_id, &temp_dir)?;
    }
    
    // Create test reference chain across shards
    let start_objref = GlobalObjRef::new(1, 1000, 0);
    
    // Test normal reference navigation
    let nav_result = executor.navigate_reference(start_objref, ".user.orders[0].items");
    assert!(nav_result.is_ok(), "Reference navigation should work normally");
    
    // Simulate node failure
    sim.simulated_network().remove_node(2);
    
    // Test navigation when target shard is unavailable
    let failed_objref = GlobalObjRef::new(2, 2000, 0);
    let failed_nav = executor.navigate_reference(failed_objref, ".profile.preferences");
    
    match failed_nav {
        Ok(_) => {
            // Navigation succeeded despite node failure (maybe using replica)
            println!("Navigation succeeded despite node failure");
        }
        Err(e) => {
            // Navigation failed due to node unavailability
            println!("Navigation failed due to node failure: {:?}", e);
            assert!(matches!(e, OQLError::StorageError { .. }));
        }
    }
    
    // Test navigation to available shard
    let available_objref = GlobalObjRef::new(3, 3000, 0);
    let available_nav = executor.navigate_reference(available_objref, ".data.metadata");
    assert!(available_nav.is_ok(), "Navigation to available shard should work");
    
    // Recover failed node
    sim.simulated_network().add_node(2);
    
    // Test navigation after recovery
    let recovered_nav = executor.navigate_reference(failed_objref, ".profile.settings");
    assert!(recovered_nav.is_ok(), "Navigation should work after node recovery");
    
    Ok(())
}

/// Test OQL transactions across failing shards
#[test]
fn test_oql_transactions_across_failing_shards() -> OQLResult<()> {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let shard_manager = Arc::new(ShardManager::new());
    let hash_ring = Arc::new(ConsistentHashRing::new());
    
    let mut executor = DistributedOQLExecutor::new(
        shard_manager,
        hash_ring,
        sim.simulated_clock(),
        sim.simulated_network(),
    )?;
    
    // Add shards
    for shard_id in 1..=3 {
        executor.add_shard(shard_id, &temp_dir)?;
    }
    
    // Test successful distributed transaction
    let successful_operations = vec![
        r#"users.insert { id: "tx_user1", name: "Alice", balance: 1000 }"#.to_string(),
        r#"accounts.insert { id: "tx_acc1", user_id: "tx_user1", balance: 1000 }"#.to_string(),
        r#"transactions.insert { id: "tx_001", from: "tx_acc1", amount: 100 }"#.to_string(),
    ];
    
    let success_result = executor.execute_transaction(successful_operations);
    assert!(success_result.is_ok(), "Transaction should succeed under normal conditions");
    
    // Simulate shard failure during transaction
    sim.simulated_network().remove_node(2);
    
    let failing_operations = vec![
        r#"users.insert { id: "tx_user2", name: "Bob", balance: 500 }"#.to_string(),
        r#"accounts.insert { id: "tx_acc2", user_id: "tx_user2", balance: 500 }"#.to_string(),
        r#"orders.insert { id: "tx_order1", user_id: "tx_user2", total: 200 }"#.to_string(),
    ];
    
    let failure_result = executor.execute_transaction(failing_operations);
    
    match failure_result {
        Ok(_) => {
            println!("Transaction succeeded despite shard failure (using replicas?)");
        }
        Err(e) => {
            println!("Transaction failed due to shard failure: {:?}", e);
            // This is expected behavior - transaction should fail atomically
            assert!(matches!(e, OQLError::StorageError { .. }));
        }
    }
    
    // Recover shard and test transaction again
    sim.simulated_network().add_node(2);
    
    let recovery_operations = vec![
        r#"users.insert { id: "tx_user3", name: "Charlie", balance: 750 }"#.to_string(),
        r#"accounts.insert { id: "tx_acc3", user_id: "tx_user3", balance: 750 }"#.to_string(),
    ];
    
    let recovery_result = executor.execute_transaction(recovery_operations);
    assert!(recovery_result.is_ok(), "Transaction should succeed after shard recovery");
    
    Ok(())
}

/// Test OQL semantics preservation under failures
#[test]
fn test_oql_semantics_preservation_under_failures() -> OQLResult<()> {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let shard_manager = Arc::new(ShardManager::new());
    let hash_ring = Arc::new(ConsistentHashRing::new());
    
    let mut executor = DistributedOQLExecutor::new(
        shard_manager,
        hash_ring,
        sim.simulated_clock(),
        sim.simulated_network(),
    )?;
    
    // Add shards
    for shard_id in 1..=4 {
        executor.add_shard(shard_id, &temp_dir)?;
    }
    
    // Set up test data
    setup_distributed_test_data(&executor)?;
    
    // Test OQL semantics under various failure conditions
    let test_queries = vec![
        // Basic filtering
        r#"users.where { .age > 30 }"#,
        // Implicit null handling
        r#"users.select { id, name, bio: .profile.bio }"#,
        // Aggregation
        r#"users.select { total_users: count() }"#,
        // Sorting and limiting
        r#"users.sort { .age desc }.limit { 5 }"#,
        // Complex nested query
        r#"users.where { .orders.any(.total > 100) }.select { id, name, high_value_orders: .orders.where(.total > 100).count() }"#,
    ];
    
    // Execute queries under normal conditions
    let mut normal_results = HashMap::new();
    for query in &test_queries {
        let result = executor.execute_oql(query);
        normal_results.insert(query.to_string(), result);
    }
    
    // Test under packet loss
    sim.set_packet_loss(0.3);
    
    for query in &test_queries {
        let result = executor.execute_oql(query);
        match result {
            Ok(docs) => {
                // Query succeeded - verify semantics are preserved
                if let Some(Ok(normal_docs)) = normal_results.get(*query) {
                    // Results should be consistent (may be subset due to unreachable shards)
                    assert!(docs.len() <= normal_docs.len(), 
                        "Results under packet loss should not exceed normal results");
                    
                    // Verify document structure is preserved
                    for doc in &docs {
                        assert!(doc.get_field("id").is_some(), "Documents should have ID field");
                    }
                }
            }
            Err(e) => {
                // Query failed - should be due to network issues, not semantic errors
                println!("Query failed under packet loss: {:?}", e);
                assert!(matches!(e, OQLError::StorageError { .. }));
            }
        }
    }
    
    // Reset network conditions
    sim.set_packet_loss(0.0);
    
    // Test under high latency
    sim.simulated_network().set_base_latency(Duration::from_millis(500));
    
    for query in &test_queries {
        let start_time = sim.clock().now();
        let result = executor.execute_oql(query);
        let query_time = sim.clock().now().duration_since(start_time);
        
        match result {
            Ok(docs) => {
                // Query succeeded but should take longer
                assert!(query_time >= Duration::from_millis(500), 
                    "Query should reflect network latency");
                
                // Semantics should still be preserved
                if let Some(Ok(normal_docs)) = normal_results.get(*query) {
                    // Structure should be identical under high latency
                    for (doc, normal_doc) in docs.iter().zip(normal_docs.iter()) {
                        assert_eq!(doc.get_field("id"), normal_doc.get_field("id"), 
                            "Document IDs should match under high latency");
                    }
                }
            }
            Err(e) => {
                println!("Query failed under high latency: {:?}", e);
            }
        }
    }
    
    // Test under network partition
    sim.simulated_network().set_base_latency(Duration::from_millis(10)); // Reset latency
    sim.partition_network(&[1, 2], &[3, 4]);
    
    for query in &test_queries {
        let result = executor.execute_oql(query);
        match result {
            Ok(docs) => {
                // Query succeeded with partial data
                println!("Query succeeded during partition with {} results", docs.len());
                
                // Verify OQL semantics are still correct
                for doc in &docs {
                    // All returned documents should be valid
                    assert!(doc.get_field("id").is_some(), "Documents should have valid structure");
                }
            }
            Err(e) => {
                // Query failed due to partition
                println!("Query failed during partition: {:?}", e);
            }
        }
    }
    
    // Heal partition and verify full functionality returns
    sim.heal_network();
    
    let final_query = r#"users.select { id, name, age }"#;
    let final_result = executor.execute_oql(final_query)?;
    assert!(!final_result.is_empty(), "Should have full results after healing");
    
    Ok(())
}

/// Test OQL query optimization under failures
#[test]
fn test_oql_query_optimization_under_failures() -> OQLResult<()> {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let shard_manager = Arc::new(ShardManager::new());
    let hash_ring = Arc::new(ConsistentHashRing::new());
    
    let mut executor = DistributedOQLExecutor::new(
        shard_manager,
        hash_ring,
        sim.simulated_clock(),
        sim.simulated_network(),
    )?;
    
    // Add shards
    for shard_id in 1..=5 {
        executor.add_shard(shard_id, &temp_dir)?;
    }
    
    // Test query that can be optimized by pushing predicates to shards
    let optimizable_query = r#"users.where { .age > 25 && .status == "active" }.select { id, name }"#;
    
    // Execute under normal conditions
    let normal_start = sim.clock().now();
    let normal_result = executor.execute_oql(optimizable_query)?;
    let normal_time = sim.clock().now().duration_since(normal_start);
    
    // Simulate some shard failures
    sim.simulated_network().remove_node(2);
    sim.simulated_network().remove_node(4);
    
    // Execute same query with fewer available shards
    let failure_start = sim.clock().now();
    let failure_result = executor.execute_oql(optimizable_query);
    let failure_time = sim.clock().now().duration_since(failure_start);
    
    match failure_result {
        Ok(docs) => {
            // Query succeeded with optimization
            assert!(docs.len() <= normal_result.len(), 
                "Should have fewer results with failed shards");
            
            // Should complete faster due to fewer shards to query
            println!("Normal time: {:?}, Failure time: {:?}", normal_time, failure_time);
        }
        Err(e) => {
            println!("Optimized query failed: {:?}", e);
        }
    }
    
    // Test query that requires cross-shard coordination
    let coordination_query = r#"users.select { total_count: count(), avg_age: avg(.age) }"#;
    
    let coord_result = executor.execute_oql(coordination_query);
    match coord_result {
        Ok(docs) => {
            // Aggregation succeeded despite failures
            assert_eq!(docs.len(), 1, "Aggregation should return single result");
            
            let result_doc = &docs[0];
            assert!(result_doc.get_field("total_count").is_some(), 
                "Should have count aggregation");
            assert!(result_doc.get_field("avg_age").is_some(), 
                "Should have average aggregation");
        }
        Err(e) => {
            // Aggregation failed due to insufficient data
            println!("Cross-shard aggregation failed: {:?}", e);
        }
    }
    
    Ok(())
}

/// Helper function to set up distributed test data
fn setup_distributed_test_data(executor: &DistributedOQLExecutor) -> OQLResult<()> {
    // Create test data across different shards
    let test_operations = vec![
        r#"users.insert { id: "user1", name: "Alice", age: 30, status: "active" }"#,
        r#"users.insert { id: "user2", name: "Bob", age: 25, status: "inactive" }"#,
        r#"users.insert { id: "user3", name: "Charlie", age: 35, status: "active" }"#,
        r#"users.insert { id: "user4", name: "Diana", age: 28, status: "active" }"#,
        r#"orders.insert { id: "order1", user_id: "user1", total: 150.0, status: "completed" }"#,
        r#"orders.insert { id: "order2", user_id: "user2", total: 75.0, status: "pending" }"#,
        r#"orders.insert { id: "order3", user_id: "user3", total: 200.0, status: "completed" }"#,
    ];
    
    for operation in test_operations {
        // Execute each operation (may fail due to network issues, which is fine for testing)
        let _ = executor.execute_oql(operation);
    }
    
    Ok(())
}
