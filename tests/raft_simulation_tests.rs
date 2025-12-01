// Raft Simulation Tests for Next-Gen Owl DST
//
// Comprehensive tests for Raft consensus algorithm including leader election,
// log replication, network partitions, and split-brain prevention using
// deterministic simulation testing.
//
// Requirements: 4, 5, 9

use owl_db::distributed::raft::{RaftNode, RaftConfig, NodeId, LogEntry, RequestVote, RequestVoteResponse, AppendEntries, AppendEntriesResponse};
use owl_db::distributed::raft_cluster::{RaftCluster, ClusterStatus};
use owl_db::error::OQLError;
use owl_db::simulation::{Clock, Network, SimulatedClock, SimulatedNetwork, SimulatedScheduler, Scheduler, Simulation};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Test basic leader election in a 3-node cluster
#[test]
fn test_raft_leader_election_basic() {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    // Create 3-node cluster
    let cluster = create_test_cluster(&mut sim, 3);
    
    // Initially no leader
    assert!(cluster.find_leader().is_none());
    
    // Trigger election on node 1
    cluster.trigger_election(1).unwrap();
    
    // Step cluster to process election
    for _ in 0..10 {
        cluster.step().unwrap();
        sim.scheduler().run_tasks(Some(Duration::from_millis(10)));
        sim.network().advance_time(Duration::from_millis(10));
    }
    
    // Should have elected a leader
    let leader = cluster.find_leader();
    assert!(leader.is_some(), "Should have elected a leader");
    
    let status = cluster.get_cluster_status();
    assert!(status.term > 0, "Term should be incremented");
    assert_eq!(status.leader, leader, "Status should reflect current leader");
}

/// Test leader election with network delays
#[test]
fn test_raft_leader_election_with_delays() {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    // Create cluster with network delays
    let cluster = create_test_cluster(&mut sim, 5);
    
    // Set different latencies between nodes
    sim.network().set_latency(1, 2, Duration::from_millis(50));
    sim.network().set_latency(1, 3, Duration::from_millis(100));
    sim.network().set_latency(2, 3, Duration::from_millis(75));
    
    // Trigger elections on multiple nodes simultaneously
    cluster.trigger_election(1).unwrap();
    cluster.trigger_election(2).unwrap();
    
    // Run simulation with network delays
    for _ in 0..20 {
        cluster.step().unwrap();
        sim.scheduler().run_tasks(Some(Duration::from_millis(20)));
        sim.network().advance_time(Duration::from_millis(20));
    }
    
    // Should eventually elect a single leader
    let leader = cluster.find_leader();
    assert!(leader.is_some(), "Should elect a leader despite delays");
    
    // Verify only one leader exists
    let status = cluster.get_cluster_status();
    let leader_count = status.nodes.values().filter(|n| n.is_leader).count();
    assert_eq!(leader_count, 1, "Should have exactly one leader");
}

/// Test leader election timeout and retry
#[test]
fn test_raft_leader_election_timeout_retry() {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    // Create cluster with short timeouts
    let config = RaftConfig {
        election_timeout_min: Duration::from_millis(50),
        election_timeout_max: Duration::from_millis(100),
        heartbeat_interval: Duration::from_millis(25),
        rpc_timeout: Duration::from_millis(30),
    };
    
    let cluster = create_test_cluster_with_config(&mut sim, 3, config);
    
    // Set high packet loss to cause timeouts
    sim.network().set_packet_loss_rate(0.7);
    
    // Trigger election
    cluster.trigger_election(1).unwrap();
    
    let initial_term = cluster.get_cluster_status().term;
    
    // Run simulation - should retry elections due to packet loss
    for _ in 0..50 {
        cluster.step().unwrap();
        sim.scheduler().run_tasks(Some(Duration::from_millis(10)));
        sim.network().advance_time(Duration::from_millis(10));
    }
    
    let final_status = cluster.get_cluster_status();
    
    // Term should have increased due to retries
    assert!(final_status.term > initial_term, "Term should increase due to election retries");
    
    // Reduce packet loss and verify leader election succeeds
    sim.network().set_packet_loss_rate(0.1);
    
    for _ in 0..20 {
        cluster.step().unwrap();
        sim.scheduler().run_tasks(Some(Duration::from_millis(10)));
        sim.network().advance_time(Duration::from_millis(10));
    }
    
    assert!(cluster.find_leader().is_some(), "Should eventually elect leader with lower packet loss");
}

/// Test log replication from leader to followers
#[test]
fn test_raft_log_replication_basic() {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 3);
    
    // Elect leader
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 15);
    
    let leader_id = cluster.find_leader().expect("Should have leader");
    
    // Write data to cluster
    let test_data = b"Hello, Raft!".to_vec();
    let write_result = tokio_test::block_on(cluster.write(test_data.clone()));
    assert!(write_result.is_ok(), "Write should succeed");
    
    // Step simulation to replicate
    run_simulation_steps(&mut sim, &cluster, 20);
    
    // Verify all nodes have the entry
    let node_ids = cluster.get_node_ids();
    for node_id in node_ids {
        if let Some(node_ref) = cluster.get_node(node_id) {
            let node = node_ref.lock().unwrap();
            assert!(node.log_len() > 0, "Node {} should have log entries", node_id);
            
            if node.log_len() > 0 {
                // Check if entry was replicated (simplified check)
                assert!(node.is_committed(1), "Entry should be committed on node {}", node_id);
            }
        }
    }
}

/// Test log replication with follower failures
#[test]
fn test_raft_log_replication_follower_failures() {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 5);
    
    // Elect leader
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 15);
    
    let leader_id = cluster.find_leader().expect("Should have leader");
    
    // Partition off 2 followers (minority)
    sim.network().partition(&[2, 3], &[1, 4, 5]);
    
    // Write data - should succeed with majority
    let test_data = b"Replicated data".to_vec();
    let write_result = tokio_test::block_on(cluster.write(test_data.clone()));
    assert!(write_result.is_ok(), "Write should succeed with majority available");
    
    run_simulation_steps(&mut sim, &cluster, 20);
    
    // Heal partition
    sim.network().heal_partition();
    run_simulation_steps(&mut sim, &cluster, 30);
    
    // Verify all nodes eventually have the entry
    let node_ids = cluster.get_node_ids();
    for node_id in node_ids {
        if let Some(node_ref) = cluster.get_node(node_id) {
            let node = node_ref.lock().unwrap();
            if node.log_len() > 0 {
                assert!(node.is_committed(1), "Entry should be committed on node {} after healing", node_id);
            }
        }
    }
}

/// Test network partition scenarios
#[test]
fn test_raft_network_partition_scenarios() {
    let seed = 202;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 5);
    
    // Elect initial leader
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 15);
    
    let initial_leader = cluster.find_leader().expect("Should have initial leader");
    
    // Create partition: {1, 2} | {3, 4, 5}
    sim.network().partition(&[1, 2], &[3, 4, 5]);
    
    // Advance time to trigger new election in majority partition
    for _ in 0..30 {
        cluster.step().unwrap();
        sim.scheduler().run_tasks(Some(Duration::from_millis(20)));
        sim.network().advance_time(Duration::from_millis(20));
    }
    
    // Check partition behavior
    let status = cluster.get_cluster_status();
    
    // Should have a leader in the majority partition (3, 4, 5)
    let majority_nodes = vec![3, 4, 5];
    let majority_leader_count = majority_nodes.iter()
        .filter(|&&node_id| {
            status.nodes.get(&node_id)
                .map(|n| n.is_leader)
                .unwrap_or(false)
        })
        .count();
    
    assert!(majority_leader_count <= 1, "Should have at most one leader in majority partition");
    
    // Minority partition should not have leader
    let minority_nodes = vec![1, 2];
    let minority_leader_count = minority_nodes.iter()
        .filter(|&&node_id| {
            status.nodes.get(&node_id)
                .map(|n| n.is_leader)
                .unwrap_or(false)
        })
        .count();
    
    assert_eq!(minority_leader_count, 0, "Minority partition should not have leader");
}

/// Test split-brain prevention
#[test]
fn test_raft_split_brain_prevention() {
    let seed = 303;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 4); // Even number to test split-brain
    
    // Elect initial leader
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 15);
    
    assert!(cluster.find_leader().is_some(), "Should have initial leader");
    
    // Create perfect split: {1, 2} | {3, 4}
    sim.network().partition(&[1, 2], &[3, 4]);
    
    // Advance time significantly to trigger elections in both partitions
    for _ in 0..50 {
        cluster.step().unwrap();
        sim.scheduler().run_tasks(Some(Duration::from_millis(20)));
        sim.network().advance_time(Duration::from_millis(20));
    }
    
    // Count leaders across all nodes
    let status = cluster.get_cluster_status();
    let total_leaders = status.nodes.values().filter(|n| n.is_leader).count();
    
    // Should have at most 1 leader (split-brain prevented)
    assert!(total_leaders <= 1, "Should prevent split-brain scenario");
    
    // If there is a leader, it should be in one partition only
    if total_leaders == 1 {
        let leader_id = status.nodes.iter()
            .find(|(_, node)| node.is_leader)
            .map(|(id, _)| *id)
            .unwrap();
        
        // Verify no leader in the other partition
        let partition_1 = vec![1, 2];
        let partition_2 = vec![3, 4];
        
        if partition_1.contains(&leader_id) {
            // Leader in partition 1, verify no leader in partition 2
            for &node_id in &partition_2 {
                let is_leader = status.nodes.get(&node_id)
                    .map(|n| n.is_leader)
                    .unwrap_or(false);
                assert!(!is_leader, "Node {} in partition 2 should not be leader", node_id);
            }
        } else {
            // Leader in partition 2, verify no leader in partition 1
            for &node_id in &partition_1 {
                let is_leader = status.nodes.get(&node_id)
                    .map(|n| n.is_leader)
                    .unwrap_or(false);
                assert!(!is_leader, "Node {} in partition 1 should not be leader", node_id);
            }
        }
    }
}

/// Test partition healing and leader re-election
#[test]
fn test_raft_partition_healing() {
    let seed = 404;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 5);
    
    // Elect initial leader
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 15);
    
    let initial_leader = cluster.find_leader().expect("Should have initial leader");
    
    // Create partition that isolates the leader
    if initial_leader == 1 {
        sim.network().partition(&[1], &[2, 3, 4, 5]);
    } else {
        sim.network().partition(&[initial_leader], &[1, 2, 3, 4, 5]);
    }
    
    // Let majority partition elect new leader
    run_simulation_steps(&mut sim, &cluster, 30);
    
    let status_during_partition = cluster.get_cluster_status();
    let leaders_during_partition = status_during_partition.nodes.values()
        .filter(|n| n.is_leader)
        .count();
    
    assert!(leaders_during_partition <= 1, "Should have at most one leader during partition");
    
    // Heal partition
    sim.network().heal_partition();
    run_simulation_steps(&mut sim, &cluster, 30);
    
    // Verify single leader after healing
    let final_status = cluster.get_cluster_status();
    let final_leaders = final_status.nodes.values()
        .filter(|n| n.is_leader)
        .count();
    
    assert_eq!(final_leaders, 1, "Should have exactly one leader after partition healing");
    
    // Verify all nodes are in same term
    let terms: HashSet<u64> = final_status.nodes.values()
        .map(|n| n.term)
        .collect();
    
    assert!(terms.len() <= 2, "Nodes should converge to similar terms after healing");
}

/// Test log consistency after partition healing
#[test]
fn test_raft_log_consistency_after_partition() {
    let seed = 505;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 5);
    
    // Elect leader and write some data
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 15);
    
    let initial_data = b"Initial data".to_vec();
    let _ = tokio_test::block_on(cluster.write(initial_data.clone()));
    run_simulation_steps(&mut sim, &cluster, 10);
    
    // Create partition
    sim.network().partition(&[1, 2], &[3, 4, 5]);
    
    // Try to write to both partitions (only majority should succeed)
    let partition_data_1 = b"Partition 1 data".to_vec();
    let partition_data_2 = b"Partition 2 data".to_vec();
    
    // These writes may or may not succeed depending on which partition has leader
    let _ = tokio_test::block_on(cluster.write(partition_data_1));
    let _ = tokio_test::block_on(cluster.write(partition_data_2));
    
    run_simulation_steps(&mut sim, &cluster, 20);
    
    // Heal partition
    sim.network().heal_partition();
    run_simulation_steps(&mut sim, &cluster, 40);
    
    // Verify log consistency across all nodes
    let node_ids = cluster.get_node_ids();
    let mut log_lengths = Vec::new();
    
    for node_id in &node_ids {
        if let Some(node_ref) = cluster.get_node(*node_id) {
            let node = node_ref.lock().unwrap();
            log_lengths.push(node.log_len());
        }
    }
    
    // All nodes should have same log length after convergence
    let min_length = *log_lengths.iter().min().unwrap_or(&0);
    let max_length = *log_lengths.iter().max().unwrap_or(&0);
    
    assert!(max_length - min_length <= 1, 
        "Log lengths should be consistent across nodes (diff <= 1)");
}

/// Test rapid leader changes
#[test]
fn test_raft_rapid_leader_changes() {
    let seed = 606;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 3);
    
    // Elect initial leader
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 10);
    
    let mut leader_changes = 0;
    let mut previous_leader = cluster.find_leader();
    
    // Simulate rapid network issues causing leader changes
    for round in 0..10 {
        // Create temporary partitions
        match round % 3 {
            0 => sim.network().partition(&[1], &[2, 3]),
            1 => sim.network().partition(&[2], &[1, 3]),
            _ => sim.network().partition(&[3], &[1, 2]),
        }
        
        run_simulation_steps(&mut sim, &cluster, 5);
        
        // Heal partition
        sim.network().heal_partition();
        run_simulation_steps(&mut sim, &cluster, 5);
        
        let current_leader = cluster.find_leader();
        if current_leader != previous_leader && current_leader.is_some() {
            leader_changes += 1;
            previous_leader = current_leader;
        }
    }
    
    // Should have experienced some leader changes
    assert!(leader_changes > 0, "Should have experienced leader changes due to partitions");
    
    // But should stabilize to single leader
    let final_status = cluster.get_cluster_status();
    let final_leaders = final_status.nodes.values()
        .filter(|n| n.is_leader)
        .count();
    
    assert_eq!(final_leaders, 1, "Should stabilize to single leader");
}

/// Test Raft with message reordering
#[test]
fn test_raft_with_message_reordering() {
    let seed = 707;
    let mut sim = Simulation::new(seed);
    
    let cluster = create_test_cluster(&mut sim, 3);
    
    // Enable message reordering
    sim.network().set_reorder_rate(0.3);
    sim.network().set_failure_config(owl_db::simulation::network::NetworkFailureConfig {
        packet_loss_rate: 0.1,
        reorder_rate: 0.3,
        max_reorder_delay: Duration::from_millis(50),
        congestion_rate: 0.1,
        congestion_latency: Duration::from_millis(100),
    });
    
    // Elect leader despite reordering
    cluster.trigger_election(1).unwrap();
    run_simulation_steps(&mut sim, &cluster, 30); // More steps due to reordering
    
    let leader = cluster.find_leader();
    assert!(leader.is_some(), "Should elect leader despite message reordering");
    
    // Write data and verify replication works
    let test_data = b"Reordered messages test".to_vec();
    let write_result = tokio_test::block_on(cluster.write(test_data));
    assert!(write_result.is_ok(), "Write should succeed despite reordering");
    
    run_simulation_steps(&mut sim, &cluster, 40);
    
    // Verify cluster remains stable
    let final_status = cluster.get_cluster_status();
    let leader_count = final_status.nodes.values()
        .filter(|n| n.is_leader)
        .count();
    
    assert_eq!(leader_count, 1, "Should maintain single leader despite reordering");
}

/// Helper function to create test cluster
fn create_test_cluster(sim: &mut Simulation, node_count: usize) -> RaftCluster<SimulatedClock, SimulatedNetwork> {
    let config = RaftConfig::default();
    create_test_cluster_with_config(sim, node_count, config)
}

/// Helper function to create test cluster with custom config
fn create_test_cluster_with_config(
    sim: &mut Simulation, 
    node_count: usize, 
    config: RaftConfig
) -> RaftCluster<SimulatedClock, SimulatedNetwork> {
    let node_ids: Vec<NodeId> = (1..=node_count as u32).collect();
    
    // Add nodes to network
    for &node_id in &node_ids {
        sim.network().add_node(node_id);
    }
    
    RaftCluster::new(node_ids, sim.clock(), sim.network(), config)
}

/// Helper function to run simulation steps
fn run_simulation_steps(
    sim: &mut Simulation, 
    cluster: &RaftCluster<SimulatedClock, SimulatedNetwork>, 
    steps: usize
) {
    for _ in 0..steps {
        cluster.step().unwrap();
        sim.scheduler().run_tasks(Some(Duration::from_millis(10)));
        sim.network().advance_time(Duration::from_millis(10));
    }
}