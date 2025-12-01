// OQL Reference Navigation Failure Tests for Next-Gen Owl DST
//
// Tests OQL reference navigation under node failures, automatic failover to replica shards,
// verification of navigation completion, and testing with deep navigation paths (5+ levels).
//
// Requirements: 1, 4, 5, 14

use owl_db::distributed::global_objref::GlobalObjRef;
use owl_db::distributed::shard_manager::ShardManager;
use owl_db::distributed::consistent_hash::ConsistentHashRing;
use owl_db::document::{Document, Value};
use owl_db::error::{OQLError, OQLResult};
use owl_db::simulation::{Clock, Network, SimulatedClock, SimulatedNetwork, Simulation};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mock distributed reference navigator for testing
struct DistributedReferenceNavigator {
    shard_manager: Arc<ShardManager>,
    hash_ring: Arc<ConsistentHashRing>,
    object_store: Arc<Mutex<HashMap<GlobalObjRef, MockObject>>>,
    replica_map: Arc<Mutex<HashMap<u32, Vec<u32>>>>, // shard_id -> replica_shard_ids
    clock: Arc<SimulatedClock>,
    network: Arc<SimulatedNetwork>,
    navigation_stats: Arc<Mutex<NavigationStats>>,
}

#[derive(Debug, Clone)]
struct MockObject {
    id: String,
    shard_id: u32,
    references: HashMap<String, GlobalObjRef>, // field_name -> target_objref
    data: HashMap<String, Value>,
}

#[derive(Debug, Default)]
struct NavigationStats {
    total_navigations: usize,
    successful_navigations: usize,
    failed_navigations: usize,
    failover_navigations: usize,
    average_navigation_time: Duration,
    deep_navigation_count: usize,
}

impl DistributedReferenceNavigator {
    fn new(
        shard_manager: Arc<ShardManager>,
        hash_ring: Arc<ConsistentHashRing>,
        clock: Arc<SimulatedClock>,
        network: Arc<SimulatedNetwork>,
    ) -> Self {
        Self {
            shard_manager,
            hash_ring,
            object_store: Arc::new(Mutex::new(HashMap::new())),
            replica_map: Arc::new(Mutex::new(HashMap::new())),
            clock,
            network,
            navigation_stats: Arc::new(Mutex::new(NavigationStats::default())),
        }
    }
    
    /// Add a shard with replicas
    fn add_shard_with_replicas(&self, shard_id: u32, replica_ids: Vec<u32>) {
        self.shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        self.network.add_node(shard_id);
        
        // Add replicas
        for &replica_id in &replica_ids {
            self.network.add_node(replica_id);
        }
        
        self.replica_map.lock().unwrap().insert(shard_id, replica_ids);
    }
    
    /// Create a mock object with references
    fn create_object(
        &self,
        objref: GlobalObjRef,
        id: String,
        references: HashMap<String, GlobalObjRef>,
        data: HashMap<String, Value>,
    ) {
        let object = MockObject {
            id,
            shard_id: objref.shard_id(),
            references,
            data,
        };
        
        self.object_store.lock().unwrap().insert(objref, object);
    }
    
    /// Navigate a reference path with automatic failover
    fn navigate_path(&self, start_objref: GlobalObjRef, path: &str) -> Result<GlobalObjRef, OQLError> {
        let start_time = self.clock.now();
        let mut stats = self.navigation_stats.lock().unwrap();
        stats.total_navigations += 1;
        
        // Count path depth
        let path_depth = path.split('.').filter(|s| !s.is_empty()).count();
        if path_depth >= 5 {
            stats.deep_navigation_count += 1;
        }
        drop(stats);
        
        let result = self.navigate_path_internal(start_objref, path);
        
        let navigation_time = self.clock.now().duration_since(start_time);
        let mut stats = self.navigation_stats.lock().unwrap();
        
        match &result {
            Ok(_) => {
                stats.successful_navigations += 1;
                
                // Update average navigation time
                let total_time = stats.average_navigation_time * stats.successful_navigations as u32 + navigation_time;
                stats.average_navigation_time = total_time / (stats.successful_navigations as u32);
            }
            Err(_) => {
                stats.failed_navigations += 1;
            }
        }
        
        result
    }
    
    /// Internal navigation with failover logic
    fn navigate_path_internal(&self, start_objref: GlobalObjRef, path: &str) -> Result<GlobalObjRef, OQLError> {
        let path_segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
        let mut current_objref = start_objref;
        
        for (i, segment) in path_segments.iter().enumerate() {
            // Parse segment (handle array access like "orders[0]")
            let (field_name, array_index) = self.parse_path_segment(segment);
            
            // Navigate to next object
            current_objref = self.navigate_single_reference(current_objref, &field_name, array_index)?;
            
            // Simulate navigation latency (O(1) for local, higher for remote)
            let navigation_latency = if i == 0 {
                Duration::from_nanos(50) // <50ns for local navigation
            } else {
                Duration::from_micros(1) // <1μs for cross-shard navigation
            };
            
            self.clock.sleep(navigation_latency);
        }
        
        Ok(current_objref)
    }
    
    /// Navigate a single reference with failover
    fn navigate_single_reference(
        &self,
        objref: GlobalObjRef,
        field_name: &str,
        array_index: Option<usize>,
    ) -> Result<GlobalObjRef, OQLError> {
        let shard_id = objref.shard_id();
        
        // Try primary shard first
        if self.network.can_communicate(0, shard_id) {
            match self.get_reference_from_shard(objref, field_name, array_index, shard_id) {
                Ok(target_ref) => return Ok(target_ref),
                Err(_) => {
                    // Primary shard failed, try replicas
                }
            }
        }
        
        // Try replica shards
        let replica_ids = {
            self.replica_map.lock().unwrap()
                .get(&shard_id)
                .cloned()
                .unwrap_or_default()
        };
        
        for replica_id in replica_ids {
            if self.network.can_communicate(0, replica_id) {
                match self.get_reference_from_shard(objref, field_name, array_index, replica_id) {
                    Ok(target_ref) => {
                        // Successfully failed over to replica
                        self.navigation_stats.lock().unwrap().failover_navigations += 1;
                        return Ok(target_ref);
                    }
                    Err(_) => {
                        // This replica also failed, try next
                        continue;
                    }
                }
            }
        }
        
        // All shards (primary + replicas) failed
        Err(OQLError::StorageError {
            message: format!("Cannot navigate reference on shard {} - all replicas unavailable", shard_id),
        })
    }
    
    /// Get reference from a specific shard
    fn get_reference_from_shard(
        &self,
        objref: GlobalObjRef,
        field_name: &str,
        array_index: Option<usize>,
        shard_id: u32,
    ) -> Result<GlobalObjRef, OQLError> {
        // Simulate network latency to shard
        let latency = self.network.latency(0, shard_id);
        self.clock.sleep(latency);
        
        // Get object from store
        let store = self.object_store.lock().unwrap();
        let object = store.get(&objref)
            .ok_or_else(|| OQLError::StorageError {
                message: format!("Object not found: {:?}", objref),
            })?;
        
        // Get reference field
        let target_ref = object.references.get(field_name)
            .ok_or_else(|| OQLError::StorageError {
                message: format!("Reference field '{}' not found", field_name),
            })?;
        
        // Handle array indexing if specified
        if let Some(index) = array_index {
            // For simplicity, just offset the reference by index
            let offset_ref = GlobalObjRef::new(
                target_ref.shard_id(),
                target_ref.local_offset() + index as u64,
                target_ref.flags(),
            );
            Ok(offset_ref)
        } else {
            Ok(*target_ref)
        }
    }
    
    /// Parse path segment to extract field name and optional array index
    fn parse_path_segment(&self, segment: &str) -> (String, Option<usize>) {
        if let Some(bracket_pos) = segment.find('[') {
            let field_name = segment[..bracket_pos].to_string();
            let index_str = &segment[bracket_pos + 1..segment.len() - 1];
            let index = index_str.parse().unwrap_or(0);
            (field_name, Some(index))
        } else {
            (segment.to_string(), None)
        }
    }
    
    /// Get navigation statistics
    fn get_stats(&self) -> NavigationStats {
        self.navigation_stats.lock().unwrap().clone()
    }
    
    /// Reset statistics
    fn reset_stats(&self) {
        *self.navigation_stats.lock().unwrap() = NavigationStats::default();
    }
}

/// Test basic reference navigation under node failure
#[test]
fn test_basic_reference_navigation_node_failure() {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    let navigator = DistributedReferenceNavigator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Set up shards with replicas
    navigator.add_shard_with_replicas(1, vec![11]); // Shard 1 with replica 11
    navigator.add_shard_with_replicas(2, vec![12]); // Shard 2 with replica 12
    
    // Create test objects with references
    let user_ref = GlobalObjRef::new(1, 1000, 0);
    let profile_ref = GlobalObjRef::new(2, 2000, 0);
    
    // User object with reference to profile
    let mut user_refs = HashMap::new();
    user_refs.insert("profile".to_string(), profile_ref);
    navigator.create_object(
        user_ref,
        "user1".to_string(),
        user_refs,
        HashMap::new(),
    );
    
    // Profile object
    navigator.create_object(
        profile_ref,
        "profile1".to_string(),
        HashMap::new(),
        HashMap::new(),
    );
    
    // Test normal navigation
    let normal_result = navigator.navigate_path(user_ref, ".profile");
    assert!(normal_result.is_ok(), "Normal navigation should succeed");
    assert_eq!(normal_result.unwrap(), profile_ref);
    
    // Simulate node failure on shard 2
    sim.network().remove_node(2);
    
    // Test navigation with failover to replica
    let failover_result = navigator.navigate_path(user_ref, ".profile");
    assert!(failover_result.is_ok(), "Navigation should succeed with failover to replica");
    
    let stats = navigator.get_stats();
    assert!(stats.failover_navigations > 0, "Should have recorded failover navigation");
    assert_eq!(stats.failed_navigations, 0, "Should not have failed navigations with replica available");
    
    println!("Navigation stats after failover: {:?}", stats);
}

/// Test deep navigation paths (5+ levels) under failures
#[test]
fn test_deep_navigation_paths_under_failures() {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    let navigator = DistributedReferenceNavigator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Set up shards across multiple nodes with replicas
    for shard_id in 1..=6 {
        let replica_id = shard_id + 10;
        navigator.add_shard_with_replicas(shard_id, vec![replica_id]);
    }
    
    // Create deep reference chain: user -> profile -> preferences -> theme -> colors -> primary
    let refs = [
        GlobalObjRef::new(1, 1000, 0), // user
        GlobalObjRef::new(2, 2000, 0), // profile
        GlobalObjRef::new(3, 3000, 0), // preferences
        GlobalObjRef::new(4, 4000, 0), // theme
        GlobalObjRef::new(5, 5000, 0), // colors
        GlobalObjRef::new(6, 6000, 0), // primary
    ];
    
    let field_names = ["profile", "preferences", "theme", "colors", "primary"];
    
    // Create objects with reference chain
    for i in 0..refs.len() - 1 {
        let mut object_refs = HashMap::new();
        object_refs.insert(field_names[i].to_string(), refs[i + 1]);
        
        navigator.create_object(
            refs[i],
            format!("object_{}", i),
            object_refs,
            HashMap::new(),
        );
    }
    
    // Create final object (no outgoing references)
    navigator.create_object(
        refs[refs.len() - 1],
        "final_object".to_string(),
        HashMap::new(),
        HashMap::new(),
    );
    
    // Test deep navigation under normal conditions
    let deep_path = ".profile.preferences.theme.colors.primary";
    let normal_result = navigator.navigate_path(refs[0], deep_path);
    assert!(normal_result.is_ok(), "Deep navigation should succeed normally");
    assert_eq!(normal_result.unwrap(), refs[refs.len() - 1]);
    
    let stats_normal = navigator.get_stats();
    assert!(stats_normal.deep_navigation_count > 0, "Should count deep navigation");
    assert!(stats_normal.average_navigation_time > Duration::from_nanos(0), "Should measure navigation time");
    
    // Simulate failure of intermediate shard (shard 3)
    sim.network().remove_node(3);
    
    navigator.reset_stats();
    
    // Test deep navigation with failover
    let failover_result = navigator.navigate_path(refs[0], deep_path);
    assert!(failover_result.is_ok(), "Deep navigation should succeed with failover");
    
    let stats_failover = navigator.get_stats();
    assert!(stats_failover.failover_navigations > 0, "Should have failover navigations");
    assert!(stats_failover.average_navigation_time > stats_normal.average_navigation_time, 
        "Failover navigation should take longer");
    
    // Simulate multiple node failures
    sim.network().remove_node(4);
    sim.network().remove_node(14); // Remove replica of shard 4
    
    navigator.reset_stats();
    
    // Test navigation with multiple failures
    let multi_failure_result = navigator.navigate_path(refs[0], deep_path);
    
    match multi_failure_result {
        Ok(_) => {
            println!("Deep navigation succeeded despite multiple failures");
            let stats = navigator.get_stats();
            assert!(stats.failover_navigations > 0, "Should have multiple failovers");
        }
        Err(e) => {
            println!("Deep navigation failed with multiple node failures: {:?}", e);
            let stats = navigator.get_stats();
            assert!(stats.failed_navigations > 0, "Should record navigation failure");
        }
    }
    
    println!("Deep navigation stats: {:?}", navigator.get_stats());
}

/// Test navigation with array indexing under failures
#[test]
fn test_navigation_array_indexing_under_failures() {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    let navigator = DistributedReferenceNavigator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Set up shards
    navigator.add_shard_with_replicas(1, vec![11]);
    navigator.add_shard_with_replicas(2, vec![12]);
    navigator.add_shard_with_replicas(3, vec![13]);
    
    // Create user with orders array reference
    let user_ref = GlobalObjRef::new(1, 1000, 0);
    let orders_ref = GlobalObjRef::new(2, 2000, 0); // Base reference for orders array
    let items_ref = GlobalObjRef::new(3, 3000, 0);  // Base reference for items array
    
    // User -> orders array
    let mut user_refs = HashMap::new();
    user_refs.insert("orders".to_string(), orders_ref);
    navigator.create_object(user_ref, "user1".to_string(), user_refs, HashMap::new());
    
    // Orders[0] -> items array
    let mut order_refs = HashMap::new();
    order_refs.insert("items".to_string(), items_ref);
    navigator.create_object(orders_ref, "order1".to_string(), order_refs, HashMap::new());
    
    // Items[2] -> final object
    let final_ref = GlobalObjRef::new(3, 3002, 0); // Offset by array index
    navigator.create_object(final_ref, "item3".to_string(), HashMap::new(), HashMap::new());
    
    // Test complex array navigation: user.orders[0].items[2]
    let array_path = ".orders[0].items[2]";
    let normal_result = navigator.navigate_path(user_ref, array_path);
    assert!(normal_result.is_ok(), "Array navigation should succeed normally");
    
    // Simulate failure of shard 2 (orders)
    sim.network().remove_node(2);
    
    // Test navigation with failover
    let failover_result = navigator.navigate_path(user_ref, array_path);
    assert!(failover_result.is_ok(), "Array navigation should succeed with failover");
    
    let stats = navigator.get_stats();
    assert!(stats.failover_navigations > 0, "Should use replica for array navigation");
    
    println!("Array navigation stats: {:?}", stats);
}

/// Test navigation performance under various failure conditions
#[test]
fn test_navigation_performance_under_failures() {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    let navigator = DistributedReferenceNavigator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Set up larger cluster
    for shard_id in 1..=10 {
        let replica_id = shard_id + 20;
        navigator.add_shard_with_replicas(shard_id, vec![replica_id]);
    }
    
    // Create test objects for performance testing
    let mut test_refs = Vec::new();
    for i in 0..10 {
        let objref = GlobalObjRef::new((i % 10) + 1, (i + 1) * 1000, 0);
        test_refs.push(objref);
        
        let mut refs = HashMap::new();
        if i < 9 {
            refs.insert("next".to_string(), test_refs[i + 1]);
        }
        
        navigator.create_object(objref, format!("obj_{}", i), refs, HashMap::new());
    }
    
    // Benchmark normal navigation performance
    navigator.reset_stats();
    let normal_start = sim.clock().now();
    
    for _ in 0..100 {
        let _ = navigator.navigate_path(test_refs[0], ".next.next.next");
    }
    
    let normal_time = sim.clock().now().duration_since(normal_start);
    let normal_stats = navigator.get_stats();
    
    // Introduce network latency
    sim.network().set_base_latency(Duration::from_millis(10));
    
    navigator.reset_stats();
    let latency_start = sim.clock().now();
    
    for _ in 0..100 {
        let _ = navigator.navigate_path(test_refs[0], ".next.next.next");
    }
    
    let latency_time = sim.clock().now().duration_since(latency_start);
    let latency_stats = navigator.get_stats();
    
    // Introduce node failures
    sim.network().remove_node(2);
    sim.network().remove_node(4);
    
    navigator.reset_stats();
    let failure_start = sim.clock().now();
    
    for _ in 0..100 {
        let _ = navigator.navigate_path(test_refs[0], ".next.next.next");
    }
    
    let failure_time = sim.clock().now().duration_since(failure_start);
    let failure_stats = navigator.get_stats();
    
    // Verify performance characteristics
    println!("Normal navigation: {:?} total, {:?} avg per navigation", 
        normal_time, normal_stats.average_navigation_time);
    println!("With latency: {:?} total, {:?} avg per navigation", 
        latency_time, latency_stats.average_navigation_time);
    println!("With failures: {:?} total, {:?} avg per navigation", 
        failure_time, failure_stats.average_navigation_time);
    
    // Normal navigation should be fastest
    assert!(normal_stats.average_navigation_time < latency_stats.average_navigation_time,
        "Normal navigation should be faster than high-latency navigation");
    
    // Failure navigation should be slowest due to failover overhead
    assert!(failure_stats.average_navigation_time >= latency_stats.average_navigation_time,
        "Failure navigation should be slower due to failover");
    
    // Should still maintain reasonable performance even with failures
    assert!(failure_stats.average_navigation_time < Duration::from_millis(100),
        "Navigation with failures should still be reasonably fast");
    
    // Verify failover occurred
    assert!(failure_stats.failover_navigations > 0, "Should have failover navigations");
    
    println!("Performance test completed - failovers: {}", failure_stats.failover_navigations);
}

/// Test concurrent navigation under failures
#[test]
fn test_concurrent_navigation_under_failures() {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    let navigator = Arc::new(DistributedReferenceNavigator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    ));
    
    // Set up shards
    for shard_id in 1..=5 {
        navigator.add_shard_with_replicas(shard_id, vec![shard_id + 10]);
    }
    
    // Create test objects
    let base_ref = GlobalObjRef::new(1, 1000, 0);
    let target_ref = GlobalObjRef::new(2, 2000, 0);
    
    let mut refs = HashMap::new();
    refs.insert("target".to_string(), target_ref);
    navigator.create_object(base_ref, "base".to_string(), refs, HashMap::new());
    navigator.create_object(target_ref, "target".to_string(), HashMap::new(), HashMap::new());
    
    // Spawn concurrent navigation tasks
    let results = Arc::new(Mutex::new(Vec::new()));
    
    for i in 0..10 {
        let nav = navigator.clone();
        let res = results.clone();
        
        sim.scheduler().spawn_fn(move || {
            // Add some variation in timing
            sim.clock().sleep(Duration::from_millis(i * 10));
            
            let result = nav.navigate_path(base_ref, ".target");
            res.lock().unwrap().push((i, result.is_ok()));
        });
    }
    
    // Introduce failure during concurrent navigation
    sim.scheduler().spawn_fn(move || {
        sim.clock().sleep(Duration::from_millis(50));
        sim.network().remove_node(2); // Remove primary shard
    });
    
    // Run concurrent navigation
    sim.scheduler().run_tasks(None);
    
    // Analyze results
    let final_results = results.lock().unwrap();
    let success_count = final_results.iter().filter(|(_, success)| *success).count();
    
    println!("Concurrent navigation results: {}/{} successful", success_count, final_results.len());
    
    // Most navigations should succeed due to failover
    assert!(success_count >= 7, "Most concurrent navigations should succeed with failover");
    
    let stats = navigator.get_stats();
    assert!(stats.failover_navigations > 0, "Should have failover navigations in concurrent test");
    
    println!("Concurrent navigation stats: {:?}", stats);
}