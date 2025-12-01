// Rebalancing Simulation Tests for Next-Gen Owl DST
//
// Tests adding nodes, removing nodes, and data migration during
// cluster rebalancing operations with deterministic simulation.
//
// Requirements: 2, 5

use owl_db::distributed::shard_manager::ShardManager;
use owl_db::distributed::consistent_hash::ConsistentHashRing;
use owl_db::distributed::global_objref::GlobalObjRef;
use owl_db::error::OQLError;
use owl_db::simulation::{Clock, Network, SimulatedClock, SimulatedNetwork, Simulation};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mock data object for rebalancing tests
#[derive(Debug, Clone)]
struct MockDataObject {
    id: String,
    shard_id: u32,
    data: Vec<u8>,
    size: usize,
}

/// Mock rebalancing coordinator
struct RebalancingCoordinator {
    shard_manager: Arc<ShardManager>,
    hash_ring: Arc<Mutex<ConsistentHashRing>>,
    data_objects: Arc<Mutex<HashMap<String, MockDataObject>>>,
    migration_progress: Arc<Mutex<HashMap<String, MigrationStatus>>>,
    clock: Arc<SimulatedClock>,
    network: Arc<SimulatedNetwork>,
}

#[derive(Debug, Clone)]
struct MigrationStatus {
    object_id: String,
    from_shard: u32,
    to_shard: u32,
    state: MigrationState,
    started_at: std::time::Instant,
    completed_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone, PartialEq)]
enum MigrationState {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl RebalancingCoordinator {
    fn new(
        shard_manager: Arc<ShardManager>,
        hash_ring: Arc<Mutex<ConsistentHashRing>>,
        clock: Arc<SimulatedClock>,
        network: Arc<SimulatedNetwork>,
    ) -> Self {
        Self {
            shard_manager,
            hash_ring,
            data_objects: Arc::new(Mutex::new(HashMap::new())),
            migration_progress: Arc::new(Mutex::new(HashMap::new())),
            clock,
            network,
        }
    }
    
    /// Add initial data objects to the cluster
    fn add_data_object(&self, id: String, data: Vec<u8>) -> Result<(), OQLError> {
        let shard_id = {
            let ring = self.hash_ring.lock().unwrap();
            ring.get_shard_for_object(&id)
        };
        
        let object = MockDataObject {
            id: id.clone(),
            shard_id,
            data,
            size: data.len(),
        };
        
        self.data_objects.lock().unwrap().insert(id, object);
        Ok(())
    }
    
    /// Add a new shard to the cluster and trigger rebalancing
    fn add_shard(&self, shard_id: u32) -> Result<Vec<String>, OQLError> {
        // Add shard to manager and hash ring
        self.shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        self.network.add_node(shard_id);
        
        {
            let mut ring = self.hash_ring.lock().unwrap();
            ring.add_shard(shard_id)?;
        }
        
        // Identify objects that need to be migrated
        let objects_to_migrate = self.identify_objects_to_migrate()?;
        
        // Start migration process
        self.start_migrations(&objects_to_migrate)?;
        
        Ok(objects_to_migrate)
    }
    
    /// Remove a shard from the cluster and migrate its data
    fn remove_shard(&self, shard_id: u32) -> Result<Vec<String>, OQLError> {
        // Identify all objects on the shard to be removed
        let objects_to_migrate: Vec<String> = {
            let objects = self.data_objects.lock().unwrap();
            objects.values()
                .filter(|obj| obj.shard_id == shard_id)
                .map(|obj| obj.id.clone())
                .collect()
        };
        
        // Start migration process before removing shard
        self.start_migrations(&objects_to_migrate)?;
        
        // Wait for migrations to complete
        self.wait_for_migrations_to_complete(&objects_to_migrate)?;
        
        // Remove shard from hash ring and manager
        {
            let mut ring = self.hash_ring.lock().unwrap();
            ring.remove_shard(shard_id)?;
        }
        
        self.shard_manager.remove_shard(shard_id);
        self.network.remove_node(shard_id);
        
        Ok(objects_to_migrate)
    }
    
    /// Identify objects that need migration after topology change
    fn identify_objects_to_migrate(&self) -> Result<Vec<String>, OQLError> {
        let mut objects_to_migrate = Vec::new();
        let objects = self.data_objects.lock().unwrap();
        let ring = self.hash_ring.lock().unwrap();
        
        for (object_id, object) in objects.iter() {
            let new_shard = ring.get_shard_for_object(object_id);
            if new_shard != object.shard_id {
                objects_to_migrate.push(object_id.clone());
            }
        }
        
        Ok(objects_to_migrate)
    }
    
    /// Start migration process for specified objects
    fn start_migrations(&self, object_ids: &[String]) -> Result<(), OQLError> {
        let ring = self.hash_ring.lock().unwrap();
        let mut progress = self.migration_progress.lock().unwrap();
        
        for object_id in object_ids {
            let current_shard = {
                let objects = self.data_objects.lock().unwrap();
                objects.get(object_id)
                    .map(|obj| obj.shard_id)
                    .ok_or_else(|| OQLError::StorageError { 
                        message: format!("Object {} not found", object_id) 
                    })?
            };
            
            let target_shard = ring.get_shard_for_object(object_id);
            
            if current_shard != target_shard {
                let migration = MigrationStatus {
                    object_id: object_id.clone(),
                    from_shard: current_shard,
                    to_shard: target_shard,
                    state: MigrationState::Pending,
                    started_at: self.clock.now(),
                    completed_at: None,
                };
                
                progress.insert(object_id.clone(), migration);
            }
        }
        
        Ok(())
    }
    
    /// Execute migration for a single object
    fn migrate_object(&self, object_id: &str) -> Result<bool, OQLError> {
        // Update migration state to in progress
        {
            let mut progress = self.migration_progress.lock().unwrap();
            if let Some(migration) = progress.get_mut(object_id) {
                migration.state = MigrationState::InProgress;
            }
        }
        
        // Simulate network transfer time based on object size
        let object_size = {
            let objects = self.data_objects.lock().unwrap();
            objects.get(object_id)
                .map(|obj| obj.size)
                .unwrap_or(0)
        };
        
        // Simulate transfer time: 1ms per KB
        let transfer_time = Duration::from_millis(object_size as u64 / 1024 + 1);
        self.clock.sleep(transfer_time);
        
        // Check if migration should succeed (simulate network issues)
        let migration_info = {
            self.migration_progress.lock().unwrap()
                .get(object_id)
                .cloned()
        };
        
        if let Some(migration) = migration_info {
            // Check network connectivity
            if !self.network.can_communicate(migration.from_shard, migration.to_shard) {
                // Migration failed due to network partition
                let mut progress = self.migration_progress.lock().unwrap();
                if let Some(m) = progress.get_mut(object_id) {
                    m.state = MigrationState::Failed;
                }
                return Ok(false);
            }
            
            // Simulate occasional migration failures (5% failure rate)
            use rand::Rng;
            let mut rng = rand::thread_rng();
            if rng.gen::<f64>() < 0.05 {
                let mut progress = self.migration_progress.lock().unwrap();
                if let Some(m) = progress.get_mut(object_id) {
                    m.state = MigrationState::Failed;
                }
                return Ok(false);
            }
            
            // Migration successful - update object location
            {
                let mut objects = self.data_objects.lock().unwrap();
                if let Some(object) = objects.get_mut(object_id) {
                    object.shard_id = migration.to_shard;
                }
            }
            
            // Update migration status
            {
                let mut progress = self.migration_progress.lock().unwrap();
                if let Some(m) = progress.get_mut(object_id) {
                    m.state = MigrationState::Completed;
                    m.completed_at = Some(self.clock.now());
                }
            }
            
            Ok(true)
        } else {
            Err(OQLError::StorageError { 
                message: format!("Migration not found for object {}", object_id) 
            })
        }
    }
    
    /// Wait for all migrations to complete
    fn wait_for_migrations_to_complete(&self, object_ids: &[String]) -> Result<(), OQLError> {
        let timeout = Duration::from_secs(30);
        let start_time = self.clock.now();
        
        loop {
            let all_completed = {
                let progress = self.migration_progress.lock().unwrap();
                object_ids.iter().all(|id| {
                    progress.get(id)
                        .map(|m| m.state == MigrationState::Completed || m.state == MigrationState::Failed)
                        .unwrap_or(true)
                })
            };
            
            if all_completed {
                break;
            }
            
            if self.clock.now().duration_since(start_time) > timeout {
                return Err(OQLError::StorageError { 
                    message: "Migration timeout".to_string() 
                });
            }
            
            // Process pending migrations
            for object_id in object_ids {
                let should_migrate = {
                    let progress = self.migration_progress.lock().unwrap();
                    progress.get(object_id)
                        .map(|m| m.state == MigrationState::Pending)
                        .unwrap_or(false)
                };
                
                if should_migrate {
                    let _ = self.migrate_object(object_id);
                }
            }
            
            self.clock.sleep(Duration::from_millis(10));
        }
        
        Ok(())
    }
    
    /// Get migration statistics
    fn get_migration_stats(&self) -> MigrationStats {
        let progress = self.migration_progress.lock().unwrap();
        
        let mut stats = MigrationStats {
            total_migrations: progress.len(),
            completed: 0,
            failed: 0,
            in_progress: 0,
            pending: 0,
            total_data_migrated: 0,
            average_migration_time: Duration::from_millis(0),
        };
        
        let mut total_time = Duration::from_millis(0);
        let mut completed_count = 0;
        
        for migration in progress.values() {
            match migration.state {
                MigrationState::Completed => {
                    stats.completed += 1;
                    if let Some(completed_at) = migration.completed_at {
                        total_time += completed_at.duration_since(migration.started_at);
                        completed_count += 1;
                    }
                }
                MigrationState::Failed => stats.failed += 1,
                MigrationState::InProgress => stats.in_progress += 1,
                MigrationState::Pending => stats.pending += 1,
            }
        }
        
        if completed_count > 0 {
            stats.average_migration_time = total_time / completed_count as u32;
        }
        
        // Calculate total data migrated
        let objects = self.data_objects.lock().unwrap();
        for migration in progress.values() {
            if migration.state == MigrationState::Completed {
                if let Some(object) = objects.get(&migration.object_id) {
                    stats.total_data_migrated += object.size;
                }
            }
        }
        
        stats
    }
    
    /// Verify data distribution after rebalancing
    fn verify_data_distribution(&self) -> Result<DistributionStats, OQLError> {
        let objects = self.data_objects.lock().unwrap();
        let mut shard_counts = HashMap::new();
        let mut shard_sizes = HashMap::new();
        
        for object in objects.values() {
            *shard_counts.entry(object.shard_id).or_insert(0) += 1;
            *shard_sizes.entry(object.shard_id).or_insert(0) += object.size;
        }
        
        let total_objects = objects.len();
        let total_size: usize = objects.values().map(|obj| obj.size).sum();
        let shard_count = shard_counts.len();
        
        let expected_objects_per_shard = total_objects as f64 / shard_count as f64;
        let expected_size_per_shard = total_size as f64 / shard_count as f64;
        
        // Calculate variance
        let object_variance = shard_counts.values()
            .map(|&count| {
                let diff = count as f64 - expected_objects_per_shard;
                diff * diff
            })
            .sum::<f64>() / shard_count as f64;
        
        let size_variance = shard_sizes.values()
            .map(|&size| {
                let diff = size as f64 - expected_size_per_shard;
                diff * diff
            })
            .sum::<f64>() / shard_count as f64;
        
        Ok(DistributionStats {
            total_objects,
            total_size,
            shard_count,
            objects_per_shard: shard_counts,
            size_per_shard: shard_sizes,
            object_distribution_variance: object_variance,
            size_distribution_variance: size_variance,
        })
    }
}

#[derive(Debug)]
struct MigrationStats {
    total_migrations: usize,
    completed: usize,
    failed: usize,
    in_progress: usize,
    pending: usize,
    total_data_migrated: usize,
    average_migration_time: Duration,
}

#[derive(Debug)]
struct DistributionStats {
    total_objects: usize,
    total_size: usize,
    shard_count: usize,
    objects_per_shard: HashMap<u32, usize>,
    size_per_shard: HashMap<u32, usize>,
    object_distribution_variance: f64,
    size_distribution_variance: f64,
}

/// Test adding a new node and rebalancing
#[test]
fn test_add_node_rebalancing() {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    // Create initial cluster with 3 shards
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(Mutex::new(ConsistentHashRing::new(256)));
    
    // Add initial shards
    for shard_id in 1..=3 {
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        sim.network().add_node(shard_id);
        hash_ring.lock().unwrap().add_shard(shard_id).unwrap();
    }
    
    let coordinator = RebalancingCoordinator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Add test data objects
    for i in 0..100 {
        let object_id = format!("object_{}", i);
        let data = vec![0u8; 1024]; // 1KB objects
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    // Verify initial distribution
    let initial_stats = coordinator.verify_data_distribution().unwrap();
    assert_eq!(initial_stats.shard_count, 3);
    assert_eq!(initial_stats.total_objects, 100);
    
    // Add a new shard (should trigger rebalancing)
    let migrated_objects = coordinator.add_shard(4).unwrap();
    
    // Wait for migrations to complete
    coordinator.wait_for_migrations_to_complete(&migrated_objects).unwrap();
    
    // Verify new distribution
    let final_stats = coordinator.verify_data_distribution().unwrap();
    assert_eq!(final_stats.shard_count, 4);
    assert_eq!(final_stats.total_objects, 100);
    
    // Check that data is more evenly distributed
    let expected_per_shard = 100.0 / 4.0; // 25 objects per shard
    for (&shard_id, &count) in &final_stats.objects_per_shard {
        let deviation = (count as f64 - expected_per_shard).abs() / expected_per_shard;
        assert!(deviation < 0.5, "Shard {} has poor distribution: {} objects", shard_id, count);
    }
    
    // Verify migration statistics
    let migration_stats = coordinator.get_migration_stats();
    assert!(migration_stats.completed > 0, "Should have completed some migrations");
    assert_eq!(migration_stats.failed + migration_stats.completed, migration_stats.total_migrations);
    
    println!("Migration stats: {:?}", migration_stats);
    println!("Final distribution: {:?}", final_stats);
}

/// Test removing a node and migrating its data
#[test]
fn test_remove_node_rebalancing() {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    // Create cluster with 4 shards
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(Mutex::new(ConsistentHashRing::new(256)));
    
    for shard_id in 1..=4 {
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        sim.network().add_node(shard_id);
        hash_ring.lock().unwrap().add_shard(shard_id).unwrap();
    }
    
    let coordinator = RebalancingCoordinator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Add test data
    for i in 0..80 {
        let object_id = format!("object_{}", i);
        let data = vec![0u8; 2048]; // 2KB objects
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    // Verify initial distribution
    let initial_stats = coordinator.verify_data_distribution().unwrap();
    assert_eq!(initial_stats.shard_count, 4);
    
    // Count objects on shard 2 before removal
    let objects_on_shard_2 = initial_stats.objects_per_shard.get(&2).copied().unwrap_or(0);
    
    // Remove shard 2
    let migrated_objects = coordinator.remove_shard(2).unwrap();
    
    // Verify final distribution
    let final_stats = coordinator.verify_data_distribution().unwrap();
    assert_eq!(final_stats.shard_count, 3);
    assert_eq!(final_stats.total_objects, 80);
    
    // Verify shard 2 no longer has any objects
    assert!(!final_stats.objects_per_shard.contains_key(&2));
    
    // Verify all objects were migrated
    assert_eq!(migrated_objects.len(), objects_on_shard_2);
    
    // Check migration statistics
    let migration_stats = coordinator.get_migration_stats();
    assert_eq!(migration_stats.completed, objects_on_shard_2);
    
    println!("Removed shard 2, migrated {} objects", objects_on_shard_2);
    println!("Final distribution: {:?}", final_stats);
}

/// Test rebalancing with network partitions
#[test]
fn test_rebalancing_with_network_partitions() {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(Mutex::new(ConsistentHashRing::new(256)));
    
    // Create initial cluster
    for shard_id in 1..=3 {
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        sim.network().add_node(shard_id);
        hash_ring.lock().unwrap().add_shard(shard_id).unwrap();
    }
    
    let coordinator = RebalancingCoordinator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Add test data
    for i in 0..60 {
        let object_id = format!("object_{}", i);
        let data = vec![0u8; 1024];
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    // Add new shard
    coordinator.add_shard(4).unwrap();
    
    // Create network partition that affects migration
    sim.network().partition(&[1, 2], &[3, 4]);
    
    // Try to complete migrations with partition
    let migrated_objects = coordinator.identify_objects_to_migrate().unwrap();
    coordinator.start_migrations(&migrated_objects).unwrap();
    
    // Some migrations should fail due to partition
    for object_id in &migrated_objects {
        let _ = coordinator.migrate_object(object_id);
    }
    
    let stats_during_partition = coordinator.get_migration_stats();
    assert!(stats_during_partition.failed > 0, "Some migrations should fail due to partition");
    
    // Heal partition
    sim.network().heal_partition();
    
    // Retry failed migrations
    let remaining_objects: Vec<String> = {
        let progress = coordinator.migration_progress.lock().unwrap();
        progress.iter()
            .filter(|(_, migration)| migration.state == MigrationState::Failed)
            .map(|(id, _)| id.clone())
            .collect()
    };
    
    for object_id in &remaining_objects {
        coordinator.migrate_object(object_id).unwrap();
    }
    
    let final_stats = coordinator.get_migration_stats();
    assert!(final_stats.completed > stats_during_partition.completed, 
        "Should complete more migrations after healing partition");
    
    println!("Migrations during partition: {} failed, {} completed", 
        stats_during_partition.failed, stats_during_partition.completed);
    println!("Final migrations: {} failed, {} completed", 
        final_stats.failed, final_stats.completed);
}

/// Test concurrent rebalancing operations
#[test]
fn test_concurrent_rebalancing_operations() {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(Mutex::new(ConsistentHashRing::new(256)));
    
    // Create initial cluster
    for shard_id in 1..=3 {
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        sim.network().add_node(shard_id);
        hash_ring.lock().unwrap().add_shard(shard_id).unwrap();
    }
    
    let coordinator = Arc::new(RebalancingCoordinator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    ));
    
    // Add test data
    for i in 0..120 {
        let object_id = format!("object_{}", i);
        let data = vec![0u8; 512];
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    let results = Arc::new(Mutex::new(Vec::new()));
    
    // Concurrent operation 1: Add shard 4
    let coordinator1 = coordinator.clone();
    let results1 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let migrated = coordinator1.add_shard(4).unwrap();
        let wait_result = coordinator1.wait_for_migrations_to_complete(&migrated);
        results1.lock().unwrap().push(("add_shard_4", wait_result.is_ok()));
    });
    
    // Concurrent operation 2: Add shard 5 (with delay)
    let coordinator2 = coordinator.clone();
    let results2 = results.clone();
    sim.scheduler().spawn_fn(move || {
        sim.clock().sleep(Duration::from_millis(50)); // Small delay
        let migrated = coordinator2.add_shard(5).unwrap();
        let wait_result = coordinator2.wait_for_migrations_to_complete(&migrated);
        results2.lock().unwrap().push(("add_shard_5", wait_result.is_ok()));
    });
    
    // Run concurrent operations
    sim.scheduler().run_tasks(None);
    
    // Verify results
    let final_results = results.lock().unwrap();
    assert_eq!(final_results.len(), 2);
    
    // Both operations should eventually succeed
    let success_count = final_results.iter().filter(|(_, success)| *success).count();
    assert!(success_count >= 1, "At least one rebalancing operation should succeed");
    
    // Verify final cluster state
    let final_stats = coordinator.verify_data_distribution().unwrap();
    assert!(final_stats.shard_count >= 4, "Should have at least 4 shards");
    assert_eq!(final_stats.total_objects, 120);
    
    println!("Concurrent rebalancing results: {:?}", *final_results);
    println!("Final distribution: {:?}", final_stats);
}

/// Test large-scale rebalancing performance
#[test]
fn test_large_scale_rebalancing_performance() {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(Mutex::new(ConsistentHashRing::new(512))); // More virtual nodes
    
    // Create initial cluster with 5 shards
    for shard_id in 1..=5 {
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        sim.network().add_node(shard_id);
        hash_ring.lock().unwrap().add_shard(shard_id).unwrap();
    }
    
    let coordinator = RebalancingCoordinator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Add large number of objects with varying sizes
    for i in 0..1000 {
        let object_id = format!("large_object_{}", i);
        let size = 1024 + (i % 10) * 512; // 1KB to 6KB objects
        let data = vec![0u8; size];
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    let initial_stats = coordinator.verify_data_distribution().unwrap();
    println!("Initial distribution with {} objects across {} shards", 
        initial_stats.total_objects, initial_stats.shard_count);
    
    // Measure rebalancing performance when adding multiple shards
    let start_time = sim.clock().now();
    
    // Add 3 new shards sequentially
    let mut all_migrated_objects = Vec::new();
    for shard_id in 6..=8 {
        let migrated = coordinator.add_shard(shard_id).unwrap();
        all_migrated_objects.extend(migrated);
    }
    
    // Wait for all migrations to complete
    coordinator.wait_for_migrations_to_complete(&all_migrated_objects).unwrap();
    
    let rebalancing_duration = sim.clock().now().duration_since(start_time);
    
    // Verify final distribution
    let final_stats = coordinator.verify_data_distribution().unwrap();
    assert_eq!(final_stats.shard_count, 8);
    assert_eq!(final_stats.total_objects, 1000);
    
    // Check distribution quality
    let expected_per_shard = 1000.0 / 8.0; // 125 objects per shard
    let max_deviation = final_stats.objects_per_shard.values()
        .map(|&count| (count as f64 - expected_per_shard).abs() / expected_per_shard)
        .fold(0.0, f64::max);
    
    assert!(max_deviation < 0.3, "Distribution should be reasonably balanced");
    
    // Check migration performance
    let migration_stats = coordinator.get_migration_stats();
    let migration_rate = migration_stats.completed as f64 / rebalancing_duration.as_secs_f64();
    
    println!("Rebalanced {} objects in {:?}", migration_stats.completed, rebalancing_duration);
    println!("Migration rate: {:.2} objects/second", migration_rate);
    println!("Average migration time: {:?}", migration_stats.average_migration_time);
    println!("Final distribution variance: {:.2}", final_stats.object_distribution_variance);
    
    // Performance assertions
    assert!(migration_rate > 10.0, "Migration rate should be reasonable");
    assert!(migration_stats.average_migration_time < Duration::from_millis(100), 
        "Average migration time should be reasonable");
}

/// Test rebalancing with mixed object sizes
#[test]
fn test_rebalancing_mixed_object_sizes() {
    let seed = 202;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(Mutex::new(ConsistentHashRing::new(256)));
    
    // Create initial cluster
    for shard_id in 1..=3 {
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        sim.network().add_node(shard_id);
        hash_ring.lock().unwrap().add_shard(shard_id).unwrap();
    }
    
    let coordinator = RebalancingCoordinator::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Add objects with very different sizes
    // Small objects (1KB)
    for i in 0..50 {
        let object_id = format!("small_{}", i);
        let data = vec![0u8; 1024];
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    // Medium objects (10KB)
    for i in 0..20 {
        let object_id = format!("medium_{}", i);
        let data = vec![0u8; 10 * 1024];
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    // Large objects (100KB)
    for i in 0..5 {
        let object_id = format!("large_{}", i);
        let data = vec![0u8; 100 * 1024];
        coordinator.add_data_object(object_id, data).unwrap();
    }
    
    let initial_stats = coordinator.verify_data_distribution().unwrap();
    println!("Initial size distribution: {:?}", initial_stats.size_per_shard);
    
    // Add new shard and rebalance
    let migrated_objects = coordinator.add_shard(4).unwrap();
    coordinator.wait_for_migrations_to_complete(&migrated_objects).unwrap();
    
    let final_stats = coordinator.verify_data_distribution().unwrap();
    println!("Final size distribution: {:?}", final_stats.size_per_shard);
    
    // Verify size distribution is reasonably balanced
    let total_size = final_stats.total_size;
    let expected_size_per_shard = total_size as f64 / 4.0;
    
    for (&shard_id, &size) in &final_stats.size_per_shard {
        let size_deviation = (size as f64 - expected_size_per_shard).abs() / expected_size_per_shard;
        assert!(size_deviation < 0.6, 
            "Shard {} has poor size distribution: {} bytes (deviation: {:.2})", 
            shard_id, size, size_deviation);
    }
    
    // Check that large objects were migrated successfully
    let migration_stats = coordinator.get_migration_stats();
    assert!(migration_stats.total_data_migrated > 0, "Should have migrated some data");
    
    println!("Total data migrated: {} bytes", migration_stats.total_data_migrated);
    println!("Size distribution variance: {:.2}", final_stats.size_distribution_variance);
}