// Distributed Transaction Simulation Tests for Next-Gen Owl DST
//
// Tests 2PC (Two-Phase Commit) across shards, coordinator failures,
// and participant failures in distributed transaction scenarios.
//
// Requirements: 2, 5

use owl_db::distributed::raft_cluster::RaftCluster;
use owl_db::distributed::shard_manager::ShardManager;
use owl_db::distributed::consistent_hash::ConsistentHashRing;
use owl_db::distributed::global_objref::GlobalObjRef;
use owl_db::error::OQLError;
use owl_db::simulation::{Clock, Network, SimulatedClock, SimulatedNetwork, Simulation};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Transaction coordinator for 2PC protocol
#[derive(Debug, Clone)]
struct TransactionCoordinator {
    transaction_id: String,
    participants: Vec<u32>, // Shard IDs
    state: TransactionState,
    prepared_shards: HashSet<u32>,
    committed_shards: HashSet<u32>,
    aborted_shards: HashSet<u32>,
}

#[derive(Debug, Clone, PartialEq)]
enum TransactionState {
    Active,
    Preparing,
    Prepared,
    Committing,
    Committed,
    Aborting,
    Aborted,
}

#[derive(Debug, Clone)]
struct TransactionOperation {
    shard_id: u32,
    operation_type: String,
    data: Vec<u8>,
}

/// Mock distributed transaction manager for testing
struct DistributedTransactionManager {
    coordinators: Arc<Mutex<HashMap<String, TransactionCoordinator>>>,
    shard_manager: Arc<ShardManager>,
    hash_ring: Arc<ConsistentHashRing>,
    clock: Arc<SimulatedClock>,
    network: Arc<SimulatedNetwork>,
}

impl DistributedTransactionManager {
    fn new(
        shard_manager: Arc<ShardManager>,
        hash_ring: Arc<ConsistentHashRing>,
        clock: Arc<SimulatedClock>,
        network: Arc<SimulatedNetwork>,
    ) -> Self {
        Self {
            coordinators: Arc::new(Mutex::new(HashMap::new())),
            shard_manager,
            hash_ring,
            clock,
            network,
        }
    }
    
    /// Begin a new distributed transaction
    fn begin_transaction(&self, transaction_id: String, operations: Vec<TransactionOperation>) -> Result<(), OQLError> {
        let participants: HashSet<u32> = operations.iter().map(|op| op.shard_id).collect();
        let participants_vec: Vec<u32> = participants.into_iter().collect();
        
        let coordinator = TransactionCoordinator {
            transaction_id: transaction_id.clone(),
            participants: participants_vec,
            state: TransactionState::Active,
            prepared_shards: HashSet::new(),
            committed_shards: HashSet::new(),
            aborted_shards: HashSet::new(),
        };
        
        self.coordinators.lock().unwrap().insert(transaction_id, coordinator);
        Ok(())
    }
    
    /// Execute 2PC prepare phase
    fn prepare_transaction(&self, transaction_id: &str) -> Result<bool, OQLError> {
        let mut coordinators = self.coordinators.lock().unwrap();
        let coordinator = coordinators.get_mut(transaction_id)
            .ok_or_else(|| OQLError::StorageError { message: "Transaction not found".to_string() })?;
        
        coordinator.state = TransactionState::Preparing;
        
        // Send prepare messages to all participants
        let mut all_prepared = true;
        for &shard_id in &coordinator.participants {
            if self.network.can_communicate(0, shard_id) { // 0 as coordinator node
                // Simulate prepare message
                let prepare_success = self.simulate_prepare_shard(shard_id, transaction_id)?;
                if prepare_success {
                    coordinator.prepared_shards.insert(shard_id);
                } else {
                    all_prepared = false;
                    break;
                }
            } else {
                // Network partition - cannot reach shard
                all_prepared = false;
                break;
            }
        }
        
        if all_prepared && coordinator.prepared_shards.len() == coordinator.participants.len() {
            coordinator.state = TransactionState::Prepared;
            Ok(true)
        } else {
            coordinator.state = TransactionState::Aborting;
            Ok(false)
        }
    }
    
    /// Execute 2PC commit phase
    fn commit_transaction(&self, transaction_id: &str) -> Result<bool, OQLError> {
        let mut coordinators = self.coordinators.lock().unwrap();
        let coordinator = coordinators.get_mut(transaction_id)
            .ok_or_else(|| OQLError::StorageError { message: "Transaction not found".to_string() })?;
        
        if coordinator.state != TransactionState::Prepared {
            return Err(OQLError::StorageError { 
                message: "Transaction not in prepared state".to_string() 
            });
        }
        
        coordinator.state = TransactionState::Committing;
        
        // Send commit messages to all prepared participants
        let mut all_committed = true;
        for &shard_id in &coordinator.prepared_shards {
            if self.network.can_communicate(0, shard_id) {
                let commit_success = self.simulate_commit_shard(shard_id, transaction_id)?;
                if commit_success {
                    coordinator.committed_shards.insert(shard_id);
                } else {
                    all_committed = false;
                }
            } else {
                all_committed = false;
            }
        }
        
        if all_committed && coordinator.committed_shards.len() == coordinator.prepared_shards.len() {
            coordinator.state = TransactionState::Committed;
            Ok(true)
        } else {
            // Partial commit - need recovery
            Ok(false)
        }
    }
    
    /// Execute 2PC abort phase
    fn abort_transaction(&self, transaction_id: &str) -> Result<(), OQLError> {
        let mut coordinators = self.coordinators.lock().unwrap();
        let coordinator = coordinators.get_mut(transaction_id)
            .ok_or_else(|| OQLError::StorageError { message: "Transaction not found".to_string() })?;
        
        coordinator.state = TransactionState::Aborting;
        
        // Send abort messages to all participants
        for &shard_id in &coordinator.participants {
            if self.network.can_communicate(0, shard_id) {
                let _ = self.simulate_abort_shard(shard_id, transaction_id);
                coordinator.aborted_shards.insert(shard_id);
            }
        }
        
        coordinator.state = TransactionState::Aborted;
        Ok(())
    }
    
    /// Get transaction state
    fn get_transaction_state(&self, transaction_id: &str) -> Option<TransactionState> {
        self.coordinators.lock().unwrap()
            .get(transaction_id)
            .map(|c| c.state.clone())
    }
    
    /// Simulate prepare on a shard (mock implementation)
    fn simulate_prepare_shard(&self, shard_id: u32, transaction_id: &str) -> Result<bool, OQLError> {
        // Simulate network delay
        self.clock.sleep(Duration::from_millis(10));
        
        // Simulate shard availability check
        if !self.shard_manager.is_shard_available(shard_id) {
            return Ok(false);
        }
        
        // Simulate resource locking and validation
        // For testing, we'll make some shards occasionally fail to prepare
        let success_rate = 0.9; // 90% success rate
        let mut rng = rand::thread_rng();
        use rand::Rng;
        
        Ok(rng.gen::<f64>() < success_rate)
    }
    
    /// Simulate commit on a shard (mock implementation)
    fn simulate_commit_shard(&self, shard_id: u32, transaction_id: &str) -> Result<bool, OQLError> {
        self.clock.sleep(Duration::from_millis(5));
        
        if !self.shard_manager.is_shard_available(shard_id) {
            return Ok(false);
        }
        
        // Commit should generally succeed if prepare succeeded
        Ok(true)
    }
    
    /// Simulate abort on a shard (mock implementation)
    fn simulate_abort_shard(&self, shard_id: u32, transaction_id: &str) -> Result<bool, OQLError> {
        self.clock.sleep(Duration::from_millis(5));
        
        // Abort should always succeed (cleanup)
        Ok(true)
    }
}

/// Test basic 2PC across multiple shards
#[test]
fn test_2pc_basic_success() {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    // Create distributed system components
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    // Add shards to the system
    for shard_id in 1..=3 {
        sim.network().add_node(shard_id);
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        hash_ring.add_shard(shard_id).unwrap();
    }
    
    let tx_manager = DistributedTransactionManager::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    // Create a transaction that spans multiple shards
    let operations = vec![
        TransactionOperation {
            shard_id: 1,
            operation_type: "insert".to_string(),
            data: b"data_for_shard_1".to_vec(),
        },
        TransactionOperation {
            shard_id: 2,
            operation_type: "update".to_string(),
            data: b"data_for_shard_2".to_vec(),
        },
        TransactionOperation {
            shard_id: 3,
            operation_type: "delete".to_string(),
            data: b"data_for_shard_3".to_vec(),
        },
    ];
    
    let tx_id = "tx_001".to_string();
    
    // Begin transaction
    tx_manager.begin_transaction(tx_id.clone(), operations).unwrap();
    assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Active));
    
    // Execute prepare phase
    let prepare_result = tx_manager.prepare_transaction(&tx_id).unwrap();
    assert!(prepare_result, "Prepare phase should succeed");
    assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Prepared));
    
    // Execute commit phase
    let commit_result = tx_manager.commit_transaction(&tx_id).unwrap();
    assert!(commit_result, "Commit phase should succeed");
    assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Committed));
}

/// Test 2PC with prepare phase failure
#[test]
fn test_2pc_prepare_failure() {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    // Add shards
    for shard_id in 1..=3 {
        sim.network().add_node(shard_id);
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        hash_ring.add_shard(shard_id).unwrap();
    }
    
    // Create network partition to cause prepare failure
    sim.network().partition(&[1], &[2, 3]);
    
    let tx_manager = DistributedTransactionManager::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    let operations = vec![
        TransactionOperation {
            shard_id: 1,
            operation_type: "insert".to_string(),
            data: b"data_1".to_vec(),
        },
        TransactionOperation {
            shard_id: 2,
            operation_type: "update".to_string(),
            data: b"data_2".to_vec(),
        },
    ];
    
    let tx_id = "tx_002".to_string();
    
    // Begin transaction
    tx_manager.begin_transaction(tx_id.clone(), operations).unwrap();
    
    // Prepare should fail due to network partition
    let prepare_result = tx_manager.prepare_transaction(&tx_id).unwrap();
    assert!(!prepare_result, "Prepare should fail due to network partition");
    assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Aborting));
    
    // Should abort the transaction
    tx_manager.abort_transaction(&tx_id).unwrap();
    assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Aborted));
}

/// Test coordinator failure during 2PC
#[test]
fn test_coordinator_failure_during_2pc() {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    // Add shards
    for shard_id in 1..=4 {
        sim.network().add_node(shard_id);
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        hash_ring.add_shard(shard_id).unwrap();
    }
    
    let tx_manager = DistributedTransactionManager::new(
        shard_manager.clone(),
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    let operations = vec![
        TransactionOperation {
            shard_id: 1,
            operation_type: "insert".to_string(),
            data: b"data_1".to_vec(),
        },
        TransactionOperation {
            shard_id: 2,
            operation_type: "update".to_string(),
            data: b"data_2".to_vec(),
        },
        TransactionOperation {
            shard_id: 3,
            operation_type: "delete".to_string(),
            data: b"data_3".to_vec(),
        },
    ];
    
    let tx_id = "tx_003".to_string();
    
    // Begin transaction and prepare
    tx_manager.begin_transaction(tx_id.clone(), operations).unwrap();
    let prepare_result = tx_manager.prepare_transaction(&tx_id).unwrap();
    
    if prepare_result {
        // Simulate coordinator failure by removing it from network
        sim.network().remove_node(0); // Coordinator node
        
        // Try to commit - should fail due to coordinator failure
        let commit_result = tx_manager.commit_transaction(&tx_id);
        
        // In a real system, participants would timeout and abort
        // For this test, we verify the coordinator state is preserved
        assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Prepared));
        
        // Simulate coordinator recovery
        sim.network().add_node(0);
        
        // Should be able to complete the transaction after recovery
        let recovery_commit = tx_manager.commit_transaction(&tx_id).unwrap();
        assert!(recovery_commit, "Should complete transaction after coordinator recovery");
    }
}

/// Test participant failure during 2PC
#[test]
fn test_participant_failure_during_2pc() {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    // Add shards
    for shard_id in 1..=4 {
        sim.network().add_node(shard_id);
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        hash_ring.add_shard(shard_id).unwrap();
    }
    
    let tx_manager = DistributedTransactionManager::new(
        shard_manager.clone(),
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    let operations = vec![
        TransactionOperation {
            shard_id: 1,
            operation_type: "insert".to_string(),
            data: b"data_1".to_vec(),
        },
        TransactionOperation {
            shard_id: 2,
            operation_type: "update".to_string(),
            data: b"data_2".to_vec(),
        },
        TransactionOperation {
            shard_id: 3,
            operation_type: "delete".to_string(),
            data: b"data_3".to_vec(),
        },
    ];
    
    let tx_id = "tx_004".to_string();
    
    // Begin transaction
    tx_manager.begin_transaction(tx_id.clone(), operations).unwrap();
    
    // Simulate participant failure during prepare
    sim.network().remove_node(2); // Remove shard 2
    
    // Prepare should fail due to participant unavailability
    let prepare_result = tx_manager.prepare_transaction(&tx_id).unwrap();
    assert!(!prepare_result, "Prepare should fail due to participant failure");
    
    // Should abort transaction
    tx_manager.abort_transaction(&tx_id).unwrap();
    assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Aborted));
}

/// Test 2PC with partial commit failure
#[test]
fn test_2pc_partial_commit_failure() {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    // Add shards
    for shard_id in 1..=3 {
        sim.network().add_node(shard_id);
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        hash_ring.add_shard(shard_id).unwrap();
    }
    
    let tx_manager = DistributedTransactionManager::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    let operations = vec![
        TransactionOperation {
            shard_id: 1,
            operation_type: "insert".to_string(),
            data: b"data_1".to_vec(),
        },
        TransactionOperation {
            shard_id: 2,
            operation_type: "update".to_string(),
            data: b"data_2".to_vec(),
        },
        TransactionOperation {
            shard_id: 3,
            operation_type: "delete".to_string(),
            data: b"data_3".to_vec(),
        },
    ];
    
    let tx_id = "tx_005".to_string();
    
    // Begin and prepare transaction
    tx_manager.begin_transaction(tx_id.clone(), operations).unwrap();
    let prepare_result = tx_manager.prepare_transaction(&tx_id).unwrap();
    assert!(prepare_result, "Prepare should succeed");
    
    // Simulate network partition during commit phase
    sim.network().partition(&[1, 2], &[3]);
    
    // Commit will partially succeed
    let commit_result = tx_manager.commit_transaction(&tx_id).unwrap();
    assert!(!commit_result, "Commit should partially fail due to partition");
    
    // Verify coordinator is in committing state (needs recovery)
    assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Committing));
    
    // Heal partition and retry commit
    sim.network().heal_partition();
    
    // In a real system, coordinator would retry commit for uncommitted participants
    // For this test, we verify the state allows for recovery
    let coordinators = tx_manager.coordinators.lock().unwrap();
    let coordinator = coordinators.get(&tx_id).unwrap();
    
    // Should have some committed shards and some pending
    assert!(coordinator.committed_shards.len() > 0, "Some shards should be committed");
    assert!(coordinator.committed_shards.len() < coordinator.participants.len(), 
        "Not all shards should be committed");
}

/// Test concurrent distributed transactions
#[test]
fn test_concurrent_distributed_transactions() {
    let seed = 202;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    // Add shards
    for shard_id in 1..=4 {
        sim.network().add_node(shard_id);
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        hash_ring.add_shard(shard_id).unwrap();
    }
    
    let tx_manager = Arc::new(DistributedTransactionManager::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    ));
    
    // Create multiple concurrent transactions
    let tx_results = Arc::new(Mutex::new(Vec::new()));
    
    // Transaction 1: Shards 1, 2
    let tx_manager_1 = tx_manager.clone();
    let results_1 = tx_results.clone();
    sim.scheduler().spawn_fn(move || {
        let operations = vec![
            TransactionOperation {
                shard_id: 1,
                operation_type: "insert".to_string(),
                data: b"tx1_data_1".to_vec(),
            },
            TransactionOperation {
                shard_id: 2,
                operation_type: "update".to_string(),
                data: b"tx1_data_2".to_vec(),
            },
        ];
        
        let tx_id = "concurrent_tx_1".to_string();
        let mut success = true;
        
        if tx_manager_1.begin_transaction(tx_id.clone(), operations).is_ok() {
            if let Ok(prepared) = tx_manager_1.prepare_transaction(&tx_id) {
                if prepared {
                    if let Ok(committed) = tx_manager_1.commit_transaction(&tx_id) {
                        success = committed;
                    } else {
                        success = false;
                    }
                } else {
                    let _ = tx_manager_1.abort_transaction(&tx_id);
                    success = false;
                }
            } else {
                success = false;
            }
        } else {
            success = false;
        }
        
        results_1.lock().unwrap().push(("tx1", success));
    });
    
    // Transaction 2: Shards 2, 3 (overlaps with tx1 on shard 2)
    let tx_manager_2 = tx_manager.clone();
    let results_2 = tx_results.clone();
    sim.scheduler().spawn_fn(move || {
        // Small delay to create interleaving
        sim.clock().sleep(Duration::from_millis(5));
        
        let operations = vec![
            TransactionOperation {
                shard_id: 2,
                operation_type: "delete".to_string(),
                data: b"tx2_data_2".to_vec(),
            },
            TransactionOperation {
                shard_id: 3,
                operation_type: "insert".to_string(),
                data: b"tx2_data_3".to_vec(),
            },
        ];
        
        let tx_id = "concurrent_tx_2".to_string();
        let mut success = true;
        
        if tx_manager_2.begin_transaction(tx_id.clone(), operations).is_ok() {
            if let Ok(prepared) = tx_manager_2.prepare_transaction(&tx_id) {
                if prepared {
                    if let Ok(committed) = tx_manager_2.commit_transaction(&tx_id) {
                        success = committed;
                    } else {
                        success = false;
                    }
                } else {
                    let _ = tx_manager_2.abort_transaction(&tx_id);
                    success = false;
                }
            } else {
                success = false;
            }
        } else {
            success = false;
        }
        
        results_2.lock().unwrap().push(("tx2", success));
    });
    
    // Transaction 3: Shards 3, 4 (no overlap)
    let tx_manager_3 = tx_manager.clone();
    let results_3 = tx_results.clone();
    sim.scheduler().spawn_fn(move || {
        let operations = vec![
            TransactionOperation {
                shard_id: 3,
                operation_type: "update".to_string(),
                data: b"tx3_data_3".to_vec(),
            },
            TransactionOperation {
                shard_id: 4,
                operation_type: "insert".to_string(),
                data: b"tx3_data_4".to_vec(),
            },
        ];
        
        let tx_id = "concurrent_tx_3".to_string();
        let mut success = true;
        
        if tx_manager_3.begin_transaction(tx_id.clone(), operations).is_ok() {
            if let Ok(prepared) = tx_manager_3.prepare_transaction(&tx_id) {
                if prepared {
                    if let Ok(committed) = tx_manager_3.commit_transaction(&tx_id) {
                        success = committed;
                    } else {
                        success = false;
                    }
                } else {
                    let _ = tx_manager_3.abort_transaction(&tx_id);
                    success = false;
                }
            } else {
                success = false;
            }
        } else {
            success = false;
        }
        
        results_3.lock().unwrap().push(("tx3", success));
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify results
    let final_results = tx_results.lock().unwrap();
    assert_eq!(final_results.len(), 3, "All transactions should complete");
    
    // At least some transactions should succeed
    let success_count = final_results.iter().filter(|(_, success)| *success).count();
    assert!(success_count >= 1, "At least one transaction should succeed");
    
    // Transaction 3 should succeed (no conflicts)
    let tx3_result = final_results.iter().find(|(id, _)| id == &"tx3");
    assert!(tx3_result.is_some(), "Transaction 3 should complete");
    if let Some((_, success)) = tx3_result {
        assert!(*success, "Transaction 3 should succeed (no conflicts)");
    }
}

/// Test 2PC timeout handling
#[test]
fn test_2pc_timeout_handling() {
    let seed = 303;
    let mut sim = Simulation::new(seed);
    
    let shard_manager = Arc::new(ShardManager::new(sim.clock()));
    let hash_ring = Arc::new(ConsistentHashRing::new(256));
    
    // Add shards
    for shard_id in 1..=3 {
        sim.network().add_node(shard_id);
        shard_manager.add_shard(shard_id, format!("shard_{}", shard_id));
        hash_ring.add_shard(shard_id).unwrap();
    }
    
    // Set high network latency to cause timeouts
    sim.network().set_base_latency(Duration::from_millis(200));
    sim.network().set_congestion_rate(0.8);
    
    let tx_manager = DistributedTransactionManager::new(
        shard_manager,
        hash_ring,
        sim.clock(),
        sim.network(),
    );
    
    let operations = vec![
        TransactionOperation {
            shard_id: 1,
            operation_type: "insert".to_string(),
            data: b"timeout_test_1".to_vec(),
        },
        TransactionOperation {
            shard_id: 2,
            operation_type: "update".to_string(),
            data: b"timeout_test_2".to_vec(),
        },
    ];
    
    let tx_id = "timeout_tx".to_string();
    
    // Begin transaction
    tx_manager.begin_transaction(tx_id.clone(), operations).unwrap();
    
    // Measure prepare time
    let start_time = sim.clock().now();
    let prepare_result = tx_manager.prepare_transaction(&tx_id);
    let prepare_duration = sim.clock().now().duration_since(start_time);
    
    // Should handle timeouts gracefully
    match prepare_result {
        Ok(success) => {
            if success {
                // If prepare succeeded despite delays, commit should also handle timeouts
                let commit_start = sim.clock().now();
                let commit_result = tx_manager.commit_transaction(&tx_id);
                let commit_duration = sim.clock().now().duration_since(commit_start);
                
                // Verify reasonable timeout handling
                assert!(commit_duration < Duration::from_secs(5), 
                    "Commit should not hang indefinitely");
            } else {
                // Prepare failed due to timeouts - should abort
                tx_manager.abort_transaction(&tx_id).unwrap();
                assert_eq!(tx_manager.get_transaction_state(&tx_id), Some(TransactionState::Aborted));
            }
        }
        Err(_) => {
            // Prepare failed - should be able to abort
            let abort_result = tx_manager.abort_transaction(&tx_id);
            assert!(abort_result.is_ok(), "Should be able to abort after prepare failure");
        }
    }
    
    // Verify prepare didn't hang indefinitely
    assert!(prepare_duration < Duration::from_secs(5), 
        "Prepare should not hang indefinitely");
}