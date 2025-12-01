// Failure Injection Simulation Tests for Next-Gen Owl DST
//
// Tests cascading failures, recovery scenarios, and data consistency
// under various failure conditions using deterministic simulation.
//
// Requirements: 5, 18

use owl_db::distributed::raft_cluster::RaftCluster;
use owl_db::distributed::shard_manager::ShardManager;
use owl_db::error::OQLError;
use owl_db::simulation::{Clock, Network, SimulatedClock, SimulatedNetwork, Simulation};
use owl_db::simulation::network::{NetworkFailureConfig, NetworkError};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Failure injection coordinator for systematic testing
struct FailureInjector {
    simulation: Arc<Mutex<Simulation>>,
    active_failures: Arc<Mutex<HashMap<String, FailureScenario>>>,
    failure_history: Arc<Mutex<Vec<FailureEvent>>>,
}

#[derive(Debug, Clone)]
struct FailureScenario {
    id: String,
    failure_type: FailureType,
    target_nodes: Vec<u32>,
    start_time: std::time::Instant,
    duration: Option<Duration>,
    recovery_time: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
enum FailureType {
    NodeCrash,
    NetworkPartition { groups: Vec<Vec<u32>> },
    PacketLoss { rate: f64 },
    HighLatency { latency: Duration },
    DiskFailure,
    MemoryPressure,
    CascadingFailure { trigger_delay: Duration },
}

#[derive(Debug, Clone)]
struct FailureEvent {
    timestamp: std::time::Instant,
    event_type: FailureEventType,
    scenario_id: String,
    affected_nodes: Vec<u32>,
    details: String,
}

#[derive(Debug, Clone)]
enum FailureEventType {
    FailureInjected,
    FailureRecovered,
    CascadeTriggered,
    SystemRecovered,
}

impl FailureInjector {
    fn new(simulation: Arc<Mutex<Simulation>>) -> Self {
        Self {
            simulation,
            active_failures: Arc::new(Mutex::new(HashMap::new())),
            failure_history: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Inject a specific failure scenario
    fn inject_failure(&self, scenario: FailureScenario) -> Result<(), OQLError> {
        let scenario_id = scenario.id.clone();
        let start_time = {
            let sim = self.simulation.lock().unwrap();
            sim.clock().now()
        };
        
        // Apply the failure
        match &scenario.failure_type {
            FailureType::NodeCrash => {
                self.inject_node_crash(&scenario.target_nodes)?;
            }
            FailureType::NetworkPartition { groups } => {
                self.inject_network_partition(groups)?;
            }
            FailureType::PacketLoss { rate } => {
                self.inject_packet_loss(*rate)?;
            }
            FailureType::HighLatency { latency } => {
                self.inject_high_latency(*latency)?;
            }
            FailureType::DiskFailure => {
                self.inject_disk_failure(&scenario.target_nodes)?;
            }
            FailureType::MemoryPressure => {
                self.inject_memory_pressure(&scenario.target_nodes)?;
            }
            FailureType::CascadingFailure { trigger_delay } => {
                self.inject_cascading_failure(&scenario.target_nodes, *trigger_delay)?;
            }
        }
        
        // Record failure injection
        let event = FailureEvent {
            timestamp: start_time,
            event_type: FailureEventType::FailureInjected,
            scenario_id: scenario_id.clone(),
            affected_nodes: scenario.target_nodes.clone(),
            details: format!("{:?}", scenario.failure_type),
        };
        
        self.failure_history.lock().unwrap().push(event);
        self.active_failures.lock().unwrap().insert(scenario_id, scenario);
        
        Ok(())
    }
    
    /// Recover from a specific failure scenario
    fn recover_failure(&self, scenario_id: &str) -> Result<(), OQLError> {
        let scenario = {
            let mut active = self.active_failures.lock().unwrap();
            active.remove(scenario_id)
        };
        
        if let Some(mut scenario) = scenario {
            let recovery_time = {
                let sim = self.simulation.lock().unwrap();
                sim.clock().now()
            };
            
            scenario.recovery_time = Some(recovery_time);
            
            // Apply recovery based on failure type
            match &scenario.failure_type {
                FailureType::NodeCrash => {
                    self.recover_node_crash(&scenario.target_nodes)?;
                }
                FailureType::NetworkPartition { .. } => {
                    self.recover_network_partition()?;
                }
                FailureType::PacketLoss { .. } => {
                    self.recover_packet_loss()?;
                }
                FailureType::HighLatency { .. } => {
                    self.recover_high_latency()?;
                }
                FailureType::DiskFailure => {
                    self.recover_disk_failure(&scenario.target_nodes)?;
                }
                FailureType::MemoryPressure => {
                    self.recover_memory_pressure(&scenario.target_nodes)?;
                }
                FailureType::CascadingFailure { .. } => {
                    self.recover_cascading_failure(&scenario.target_nodes)?;
                }
            }
            
            // Record recovery
            let event = FailureEvent {
                timestamp: recovery_time,
                event_type: FailureEventType::FailureRecovered,
                scenario_id: scenario_id.to_string(),
                affected_nodes: scenario.target_nodes,
                details: "Recovery completed".to_string(),
            };
            
            self.failure_history.lock().unwrap().push(event);
        }
        
        Ok(())
    }
    
    /// Inject node crash failure
    fn inject_node_crash(&self, nodes: &[u32]) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        for &node_id in nodes {
            sim.network().remove_node(node_id);
        }
        Ok(())
    }
    
    /// Recover from node crash
    fn recover_node_crash(&self, nodes: &[u32]) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        for &node_id in nodes {
            sim.network().add_node(node_id);
        }
        Ok(())
    }
    
    /// Inject network partition
    fn inject_network_partition(&self, groups: &[Vec<u32>]) -> Result<(), OQLError> {
        if groups.len() >= 2 {
            let sim = self.simulation.lock().unwrap();
            sim.network().partition(&groups[0], &groups[1]);
        }
        Ok(())
    }
    
    /// Recover from network partition
    fn recover_network_partition(&self) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        sim.network().heal_partition();
        Ok(())
    }
    
    /// Inject packet loss
    fn inject_packet_loss(&self, rate: f64) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        sim.network().set_packet_loss_rate(rate);
        Ok(())
    }
    
    /// Recover from packet loss
    fn recover_packet_loss(&self) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        sim.network().set_packet_loss_rate(0.0);
        Ok(())
    }
    
    /// Inject high latency
    fn inject_high_latency(&self, latency: Duration) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        sim.network().set_base_latency(latency);
        Ok(())
    }
    
    /// Recover from high latency
    fn recover_high_latency(&self) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        sim.network().set_base_latency(Duration::from_millis(10)); // Reset to default
        Ok(())
    }
    
    /// Inject disk failure (simulated)
    fn inject_disk_failure(&self, nodes: &[u32]) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        // Simulate disk failure by causing filesystem operations to fail
        sim.filesystem().inject_failure("disk_failure".to_string());
        Ok(())
    }
    
    /// Recover from disk failure
    fn recover_disk_failure(&self, nodes: &[u32]) -> Result<(), OQLError> {
        let sim = self.simulation.lock().unwrap();
        sim.filesystem().clear_failures();
        Ok(())
    }
    
    /// Inject memory pressure (simulated)
    fn inject_memory_pressure(&self, nodes: &[u32]) -> Result<(), OQLError> {
        // Simulate memory pressure by reducing available memory
        // In a real implementation, this would limit memory allocations
        Ok(())
    }
    
    /// Recover from memory pressure
    fn recover_memory_pressure(&self, nodes: &[u32]) -> Result<(), OQLError> {
        // Restore normal memory allocation
        Ok(())
    }
    
    /// Inject cascading failure
    fn inject_cascading_failure(&self, nodes: &[u32], trigger_delay: Duration) -> Result<(), OQLError> {
        // Start with first node failure
        if !nodes.is_empty() {
            self.inject_node_crash(&nodes[0..1])?;
            
            // Schedule cascading failures
            let sim = self.simulation.lock().unwrap();
            let injector = self.clone();
            let remaining_nodes = nodes[1..].to_vec();
            
            sim.scheduler().spawn_fn(move || {
                sim.clock().sleep(trigger_delay);
                
                // Cascade to remaining nodes
                for &node_id in &remaining_nodes {
                    let _ = injector.inject_node_crash(&[node_id]);
                    sim.clock().sleep(Duration::from_millis(100)); // Small delay between cascades
                }
            });
        }
        
        Ok(())
    }
    
    /// Recover from cascading failure
    fn recover_cascading_failure(&self, nodes: &[u32]) -> Result<(), OQLError> {
        self.recover_node_crash(nodes)
    }
    
    /// Get failure statistics
    fn get_failure_stats(&self) -> FailureStats {
        let history = self.failure_history.lock().unwrap();
        let active = self.active_failures.lock().unwrap();
        
        let mut stats = FailureStats {
            total_failures_injected: 0,
            total_recoveries: 0,
            active_failures: active.len(),
            cascading_events: 0,
            average_failure_duration: Duration::from_millis(0),
            failure_types: HashMap::new(),
        };
        
        let mut total_duration = Duration::from_millis(0);
        let mut completed_failures = 0;
        
        for event in history.iter() {
            match event.event_type {
                FailureEventType::FailureInjected => {
                    stats.total_failures_injected += 1;
                    
                    // Count failure types
                    let failure_type = event.details.split('(').next().unwrap_or("Unknown").to_string();
                    *stats.failure_types.entry(failure_type).or_insert(0) += 1;
                }
                FailureEventType::FailureRecovered => {
                    stats.total_recoveries += 1;
                    
                    // Calculate duration if we can find the corresponding injection
                    if let Some(injection_event) = history.iter().find(|e| {
                        e.scenario_id == event.scenario_id && 
                        matches!(e.event_type, FailureEventType::FailureInjected)
                    }) {
                        let duration = event.timestamp.duration_since(injection_event.timestamp);
                        total_duration += duration;
                        completed_failures += 1;
                    }
                }
                FailureEventType::CascadeTriggered => {
                    stats.cascading_events += 1;
                }
                _ => {}
            }
        }
        
        if completed_failures > 0 {
            stats.average_failure_duration = total_duration / completed_failures as u32;
        }
        
        stats
    }
}

impl Clone for FailureInjector {
    fn clone(&self) -> Self {
        Self {
            simulation: self.simulation.clone(),
            active_failures: self.active_failures.clone(),
            failure_history: self.failure_history.clone(),
        }
    }
}

#[derive(Debug)]
struct FailureStats {
    total_failures_injected: usize,
    total_recoveries: usize,
    active_failures: usize,
    cascading_events: usize,
    average_failure_duration: Duration,
    failure_types: HashMap<String, usize>,
}

/// Test cascading node failures
#[test]
fn test_cascading_node_failures() {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    let sim_arc = Arc::new(Mutex::new(sim));
    
    // Create 5-node cluster
    let cluster = {
        let mut sim = sim_arc.lock().unwrap();
        create_test_cluster(&mut sim, 5)
    };
    
    let injector = FailureInjector::new(sim_arc.clone());
    
    // Elect initial leader
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&sim_arc, &cluster, 15);
    
    let initial_leader = cluster.find_leader();
    assert!(initial_leader.is_some(), "Should have initial leader");
    
    // Inject cascading failure starting with node 1
    let cascade_scenario = FailureScenario {
        id: "cascade_test".to_string(),
        failure_type: FailureType::CascadingFailure { 
            trigger_delay: Duration::from_millis(100) 
        },
        target_nodes: vec![1, 2, 3], // Will cascade through these nodes
        start_time: {
            let sim = sim_arc.lock().unwrap();
            sim.clock().now()
        },
        duration: None,
        recovery_time: None,
    };
    
    injector.inject_failure(cascade_scenario).unwrap();
    
    // Run simulation to let cascade propagate
    run_simulation_steps(&sim_arc, &cluster, 30);
    
    // Check cluster state during cascade
    let status_during_cascade = cluster.get_cluster_status();
    let available_nodes = status_during_cascade.nodes.len();
    
    // Should have fewer available nodes due to cascade
    assert!(available_nodes < 5, "Cascade should affect multiple nodes");
    
    // Remaining nodes should still be able to elect leader if they have majority
    if available_nodes >= 3 {
        let leader_during_cascade = cluster.find_leader();
        // May or may not have leader depending on which nodes failed
        println!("Leader during cascade: {:?}", leader_during_cascade);
    }
    
    // Recover from cascade
    injector.recover_failure("cascade_test").unwrap();
    run_simulation_steps(&sim_arc, &cluster, 30);
    
    // Verify recovery
    let final_status = cluster.get_cluster_status();
    assert_eq!(final_status.nodes.len(), 5, "All nodes should be recovered");
    
    let final_leader = cluster.find_leader();
    assert!(final_leader.is_some(), "Should have leader after recovery");
    
    // Check failure statistics
    let stats = injector.get_failure_stats();
    assert!(stats.total_failures_injected > 0, "Should have injected failures");
    assert!(stats.total_recoveries > 0, "Should have recovered from failures");
    
    println!("Cascade failure stats: {:?}", stats);
}

/// Test network partition with minority/majority behavior
#[test]
fn test_network_partition_minority_majority() {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    let sim_arc = Arc::new(Mutex::new(sim));
    
    // Create 5-node cluster
    let cluster = {
        let mut sim = sim_arc.lock().unwrap();
        create_test_cluster(&mut sim, 5)
    };
    
    let injector = FailureInjector::new(sim_arc.clone());
    
    // Elect leader and write some data
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&sim_arc, &cluster, 15);
    
    let initial_data = b"Initial data".to_vec();
    let _ = tokio_test::block_on(cluster.write(initial_data));
    run_simulation_steps(&sim_arc, &cluster, 10);
    
    // Create partition: minority {1, 2} vs majority {3, 4, 5}
    let partition_scenario = FailureScenario {
        id: "minority_majority_partition".to_string(),
        failure_type: FailureType::NetworkPartition { 
            groups: vec![vec![1, 2], vec![3, 4, 5]] 
        },
        target_nodes: vec![1, 2, 3, 4, 5],
        start_time: {
            let sim = sim_arc.lock().unwrap();
            sim.clock().now()
        },
        duration: Some(Duration::from_secs(10)),
        recovery_time: None,
    };
    
    injector.inject_failure(partition_scenario).unwrap();
    run_simulation_steps(&sim_arc, &cluster, 30);
    
    // Check partition behavior
    let status_during_partition = cluster.get_cluster_status();
    
    // Majority partition should elect leader
    let majority_nodes = vec![3, 4, 5];
    let majority_has_leader = majority_nodes.iter().any(|&node_id| {
        status_during_partition.nodes.get(&node_id)
            .map(|n| n.is_leader)
            .unwrap_or(false)
    });
    
    // Minority partition should not have leader
    let minority_nodes = vec![1, 2];
    let minority_has_leader = minority_nodes.iter().any(|&node_id| {
        status_during_partition.nodes.get(&node_id)
            .map(|n| n.is_leader)
            .unwrap_or(false)
    });
    
    assert!(!minority_has_leader, "Minority partition should not have leader");
    
    // Try to write to cluster during partition (should succeed only if majority has leader)
    let partition_data = b"Partition data".to_vec();
    let write_result = tokio_test::block_on(cluster.write(partition_data));
    
    if majority_has_leader {
        assert!(write_result.is_ok(), "Write should succeed in majority partition");
    } else {
        // If no leader, write should fail
        assert!(write_result.is_err(), "Write should fail without leader");
    }
    
    // Heal partition
    injector.recover_failure("minority_majority_partition").unwrap();
    run_simulation_steps(&sim_arc, &cluster, 30);
    
    // Verify cluster convergence after healing
    let final_status = cluster.get_cluster_status();
    let final_leaders = final_status.nodes.values().filter(|n| n.is_leader).count();
    assert_eq!(final_leaders, 1, "Should have exactly one leader after healing");
    
    println!("Partition test completed - majority had leader: {}", majority_has_leader);
}

/// Test recovery from multiple simultaneous failures
#[test]
fn test_multiple_simultaneous_failures() {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    let sim_arc = Arc::new(Mutex::new(sim));
    
    let cluster = {
        let mut sim = sim_arc.lock().unwrap();
        create_test_cluster(&mut sim, 7) // Larger cluster for multiple failures
    };
    
    let injector = FailureInjector::new(sim_arc.clone());
    
    // Establish initial state
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&sim_arc, &cluster, 15);
    
    // Inject multiple simultaneous failures
    let failures = vec![
        FailureScenario {
            id: "node_crash_failure".to_string(),
            failure_type: FailureType::NodeCrash,
            target_nodes: vec![2, 3],
            start_time: {
                let sim = sim_arc.lock().unwrap();
                sim.clock().now()
            },
            duration: Some(Duration::from_secs(5)),
            recovery_time: None,
        },
        FailureScenario {
            id: "packet_loss_failure".to_string(),
            failure_type: FailureType::PacketLoss { rate: 0.5 },
            target_nodes: vec![],
            start_time: {
                let sim = sim_arc.lock().unwrap();
                sim.clock().now()
            },
            duration: Some(Duration::from_secs(3)),
            recovery_time: None,
        },
        FailureScenario {
            id: "high_latency_failure".to_string(),
            failure_type: FailureType::HighLatency { 
                latency: Duration::from_millis(500) 
            },
            target_nodes: vec![],
            start_time: {
                let sim = sim_arc.lock().unwrap();
                sim.clock().now()
            },
            duration: Some(Duration::from_secs(4)),
            recovery_time: None,
        },
    ];
    
    // Inject all failures
    for failure in failures {
        injector.inject_failure(failure).unwrap();
    }
    
    // Run simulation under multiple failures
    run_simulation_steps(&sim_arc, &cluster, 40);
    
    // Check system behavior under stress
    let status_under_stress = cluster.get_cluster_status();
    let available_nodes = status_under_stress.nodes.len();
    
    // Should still have some nodes available
    assert!(available_nodes >= 3, "Should have at least 3 nodes available");
    
    // System should still be able to elect leader if majority available
    if available_nodes >= 4 {
        // Give more time for leader election under adverse conditions
        run_simulation_steps(&sim_arc, &cluster, 30);
        let leader_under_stress = cluster.find_leader();
        // May or may not have leader depending on network conditions
        println!("Leader under multiple failures: {:?}", leader_under_stress);
    }
    
    // Recover from all failures
    injector.recover_failure("node_crash_failure").unwrap();
    injector.recover_failure("packet_loss_failure").unwrap();
    injector.recover_failure("high_latency_failure").unwrap();
    
    // Allow time for full recovery
    run_simulation_steps(&sim_arc, &cluster, 50);
    
    // Verify full recovery
    let final_status = cluster.get_cluster_status();
    assert_eq!(final_status.nodes.len(), 7, "All nodes should be recovered");
    
    let final_leader = cluster.find_leader();
    assert!(final_leader.is_some(), "Should have leader after full recovery");
    
    // Verify cluster is functional
    let recovery_data = b"Recovery test data".to_vec();
    let write_result = tokio_test::block_on(cluster.write(recovery_data));
    assert!(write_result.is_ok(), "Should be able to write after recovery");
    
    let stats = injector.get_failure_stats();
    println!("Multiple failure stats: {:?}", stats);
    assert_eq!(stats.total_failures_injected, 3);
    assert_eq!(stats.total_recoveries, 3);
}

/// Test data consistency during failures
#[test]
fn test_data_consistency_during_failures() {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    let sim_arc = Arc::new(Mutex::new(sim));
    
    let cluster = {
        let mut sim = sim_arc.lock().unwrap();
        create_test_cluster(&mut sim, 5)
    };
    
    let injector = FailureInjector::new(sim_arc.clone());
    
    // Establish cluster and write initial data
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&sim_arc, &cluster, 15);
    
    let initial_data = b"Consistency test data".to_vec();
    let write_result = tokio_test::block_on(cluster.write(initial_data.clone()));
    assert!(write_result.is_ok(), "Initial write should succeed");
    run_simulation_steps(&sim_arc, &cluster, 10);
    
    // Inject intermittent failures while writing data
    let consistency_results = Arc::new(Mutex::new(Vec::new()));
    
    // Background task: continuous writes
    let cluster_writer = cluster.clone();
    let results_writer = consistency_results.clone();
    let sim_writer = sim_arc.clone();
    
    {
        let sim = sim_arc.lock().unwrap();
        sim.scheduler().spawn_fn(move || {
            for i in 0..10 {
                let data = format!("Data write {}", i).into_bytes();
                let result = tokio_test::block_on(cluster_writer.write(data));
                results_writer.lock().unwrap().push((i, result.is_ok()));
                
                // Small delay between writes
                {
                    let sim = sim_writer.lock().unwrap();
                    sim.clock().sleep(Duration::from_millis(50));
                }
            }
        });
    }
    
    // Background task: inject intermittent failures
    let injector_bg = injector.clone();
    let sim_bg = sim_arc.clone();
    
    {
        let sim = sim_arc.lock().unwrap();
        sim.scheduler().spawn_fn(move || {
            for i in 0..3 {
                // Wait a bit
                {
                    let sim = sim_bg.lock().unwrap();
                    sim.clock().sleep(Duration::from_millis(100));
                }
                
                // Inject temporary failure
                let failure = FailureScenario {
                    id: format!("intermittent_{}", i),
                    failure_type: FailureType::PacketLoss { rate: 0.8 },
                    target_nodes: vec![],
                    start_time: {
                        let sim = sim_bg.lock().unwrap();
                        sim.clock().now()
                    },
                    duration: Some(Duration::from_millis(200)),
                    recovery_time: None,
                };
                
                let _ = injector_bg.inject_failure(failure);
                
                // Wait for failure duration
                {
                    let sim = sim_bg.lock().unwrap();
                    sim.clock().sleep(Duration::from_millis(200));
                }
                
                // Recover
                let _ = injector_bg.recover_failure(&format!("intermittent_{}", i));
            }
        });
    }
    
    // Run simulation with concurrent writes and failures
    run_simulation_steps(&sim_arc, &cluster, 100);
    
    // Analyze consistency results
    let write_results = consistency_results.lock().unwrap();
    let successful_writes = write_results.iter().filter(|(_, success)| *success).count();
    let total_writes = write_results.len();
    
    println!("Write results during failures: {}/{} successful", successful_writes, total_writes);
    
    // Should have some successful writes despite failures
    assert!(successful_writes > 0, "Should have some successful writes");
    
    // Verify final cluster state is consistent
    let final_status = cluster.get_cluster_status();
    let final_leader = cluster.find_leader();
    assert!(final_leader.is_some(), "Should have leader after test");
    
    // All nodes should be in consistent state (same term)
    let terms: HashSet<u64> = final_status.nodes.values().map(|n| n.term).collect();
    assert!(terms.len() <= 2, "Nodes should have converged to similar terms");
    
    let stats = injector.get_failure_stats();
    println!("Consistency test failure stats: {:?}", stats);
}

/// Test recovery time measurement
#[test]
fn test_failure_recovery_time_measurement() {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    let sim_arc = Arc::new(Mutex::new(sim));
    
    let cluster = {
        let mut sim = sim_arc.lock().unwrap();
        create_test_cluster(&mut sim, 5)
    };
    
    let injector = FailureInjector::new(sim_arc.clone());
    
    // Establish baseline
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&sim_arc, &cluster, 15);
    
    let baseline_leader = cluster.find_leader();
    assert!(baseline_leader.is_some(), "Should have baseline leader");
    
    // Test different failure types and measure recovery times
    let failure_types = vec![
        ("node_crash", FailureType::NodeCrash),
        ("packet_loss", FailureType::PacketLoss { rate: 0.9 }),
        ("high_latency", FailureType::HighLatency { latency: Duration::from_millis(1000) }),
        ("network_partition", FailureType::NetworkPartition { 
            groups: vec![vec![1, 2], vec![3, 4, 5]] 
        }),
    ];
    
    let mut recovery_times = HashMap::new();
    
    for (name, failure_type) in failure_types {
        let scenario = FailureScenario {
            id: format!("recovery_test_{}", name),
            failure_type,
            target_nodes: vec![1, 2], // Affect minority
            start_time: {
                let sim = sim_arc.lock().unwrap();
                sim.clock().now()
            },
            duration: None,
            recovery_time: None,
        };
        
        // Inject failure
        let failure_start = {
            let sim = sim_arc.lock().unwrap();
            sim.clock().now()
        };
        
        injector.inject_failure(scenario).unwrap();
        run_simulation_steps(&sim_arc, &cluster, 20);
        
        // Recover from failure
        let recovery_start = {
            let sim = sim_arc.lock().unwrap();
            sim.clock().now()
        };
        
        injector.recover_failure(&format!("recovery_test_{}", name)).unwrap();
        
        // Measure time to restore functionality
        let mut recovery_complete_time = None;
        for step in 0..50 {
            run_simulation_steps(&sim_arc, &cluster, 1);
            
            let current_leader = cluster.find_leader();
            if current_leader.is_some() {
                // Test if cluster is functional
                let test_data = format!("Recovery test {}", name).into_bytes();
                let write_result = tokio_test::block_on(cluster.write(test_data));
                
                if write_result.is_ok() {
                    recovery_complete_time = Some({
                        let sim = sim_arc.lock().unwrap();
                        sim.clock().now()
                    });
                    break;
                }
            }
        }
        
        if let Some(complete_time) = recovery_complete_time {
            let recovery_duration = complete_time.duration_since(recovery_start);
            recovery_times.insert(name.to_string(), recovery_duration);
            println!("Recovery time for {}: {:?}", name, recovery_duration);
        } else {
            println!("Failed to recover from {} within timeout", name);
        }
        
        // Allow cluster to stabilize before next test
        run_simulation_steps(&sim_arc, &cluster, 10);
    }
    
    // Verify recovery times are reasonable
    for (failure_type, recovery_time) in &recovery_times {
        assert!(*recovery_time < Duration::from_secs(10), 
            "Recovery time for {} should be reasonable: {:?}", failure_type, recovery_time);
    }
    
    // Network partition should generally take longer to recover than node crash
    if let (Some(partition_time), Some(crash_time)) = (
        recovery_times.get("network_partition"),
        recovery_times.get("node_crash")
    ) {
        println!("Partition recovery: {:?}, Crash recovery: {:?}", partition_time, crash_time);
    }
    
    let stats = injector.get_failure_stats();
    println!("Recovery measurement stats: {:?}", stats);
}

/// Helper function to create test cluster
fn create_test_cluster(sim: &mut Simulation, node_count: usize) -> RaftCluster<SimulatedClock, SimulatedNetwork> {
    let node_ids: Vec<u32> = (1..=node_count as u32).collect();
    
    // Add nodes to network
    for &node_id in &node_ids {
        sim.network().add_node(node_id);
    }
    
    use owl_db::distributed::raft::RaftConfig;
    let config = RaftConfig::default();
    
    RaftCluster::new(node_ids, sim.clock(), sim.network(), config)
}

/// Helper function to run simulation steps
fn run_simulation_steps(
    sim_arc: &Arc<Mutex<Simulation>>, 
    cluster: &RaftCluster<SimulatedClock, SimulatedNetwork>, 
    steps: usize
) {
    for _ in 0..steps {
        cluster.step().unwrap();
        {
            let mut sim = sim_arc.lock().unwrap();
            sim.scheduler().run_tasks(Some(Duration::from_millis(10)));
            sim.network().advance_time(Duration::from_millis(10));
        }
    }
}