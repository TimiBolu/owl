// Concurrency Simulation Tests for Next-Gen Owl DST
//
// Tests concurrent OQL queries with lock contention, deadlock detection,
// optimistic locking conflicts, and timeout scenarios with deterministic timing.
//
// Requirements: 5, 7

use owl_db::concurrency::{ConcurrencyManager, LockType, DeadlockResult};
use owl_db::document::{Document, Value};
use owl_db::error::{OQLError, OQLResult};
use owl_db::executor::QueryExecutor;
use owl_db::parser::parse_oql;
use owl_db::simulation::{Clock, SimulatedClock, SimulatedScheduler, Scheduler, Simulation, RealClock};
use owl_db::storage::{create_storage, DocumentStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

/// Test concurrent OQL queries with lock contention
#[test]
fn test_concurrent_oql_queries_with_lock_contention() -> OQLResult<()> {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    // Create test storage and executor
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test_concurrency.owl");
    let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
    let executor = Arc::new(QueryExecutor::new(store, sim.clock()));
    
    // Set up test data
    setup_test_data(&executor)?;
    
    // Create concurrency manager with short timeouts for testing
    let concurrency_manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        sim.clock(),
        Duration::from_millis(100), // Short lock timeout
        Duration::from_millis(10),  // Frequent deadlock checks
    ));
    
    // Track execution results
    let results = Arc::new(Mutex::new(Vec::new()));
    
    // Spawn concurrent transactions that will contend for the same document
    let doc_key = "users#user1";
    
    // Transaction 1: Try to acquire write lock
    let executor1 = executor.clone();
    let manager1 = concurrency_manager.clone();
    let results1 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "txn1";
        manager1.register_transaction(txn_id).unwrap();
        
        match manager1.acquire_lock(txn_id, doc_key, LockType::Write, None) {
            Ok(()) => {
                // Hold lock for a while
                sim.clock().sleep(Duration::from_millis(50));
                
                // Execute OQL query while holding lock
                let query = parse_oql(r#"users.where { .id == "user1" }.update { name: "Alice Updated" }"#).unwrap();
                let result = executor1.execute_query(&query);
                
                results1.lock().unwrap().push(("txn1", result.is_ok()));
                
                manager1.release_lock(txn_id, doc_key).unwrap();
                manager1.unregister_transaction(txn_id).unwrap();
            }
            Err(_) => {
                results1.lock().unwrap().push(("txn1", false));
                manager1.unregister_transaction(txn_id).unwrap();
            }
        }
    });
    
    // Transaction 2: Try to acquire conflicting write lock
    let executor2 = executor.clone();
    let manager2 = concurrency_manager.clone();
    let results2 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "txn2";
        manager2.register_transaction(txn_id).unwrap();
        
        // Small delay to ensure txn1 gets lock first
        sim.clock().sleep(Duration::from_millis(5));
        
        match manager2.acquire_lock(txn_id, doc_key, LockType::Write, None) {
            Ok(()) => {
                // Execute OQL query
                let query = parse_oql(r#"users.where { .id == "user1" }.update { age: 31 }"#).unwrap();
                let result = executor2.execute_query(&query);
                
                results2.lock().unwrap().push(("txn2", result.is_ok()));
                
                manager2.release_lock(txn_id, doc_key).unwrap();
                manager2.unregister_transaction(txn_id).unwrap();
            }
            Err(_) => {
                // Expected to timeout due to lock contention
                results2.lock().unwrap().push(("txn2", false));
                manager2.unregister_transaction(txn_id).unwrap();
            }
        }
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify results
    let final_results = results.lock().unwrap();
    assert_eq!(final_results.len(), 2);
    
    // Transaction 1 should succeed (gets lock first)
    let txn1_result = final_results.iter().find(|(id, _)| id == &"txn1").unwrap();
    assert!(txn1_result.1, "Transaction 1 should succeed");
    
    // Transaction 2 should fail due to timeout
    let txn2_result = final_results.iter().find(|(id, _)| id == &"txn2").unwrap();
    assert!(!txn2_result.1, "Transaction 2 should timeout");
    
    Ok(())
}

/// Test deadlock detection in complex transaction scenarios
#[test]
fn test_deadlock_detection_complex_scenario() -> OQLResult<()> {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    // Create concurrency manager
    let concurrency_manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        sim.clock(),
        Duration::from_secs(1),    // Longer timeout to allow deadlock detection
        Duration::from_millis(10), // Frequent deadlock checks
    ));
    
    // Track deadlock detection results
    let deadlock_detected = Arc::new(Mutex::new(false));
    let victim_transaction = Arc::new(Mutex::new(String::new()));
    
    // Create a classic deadlock scenario:
    // Transaction A: locks doc1, then tries to lock doc2
    // Transaction B: locks doc2, then tries to lock doc1
    
    let doc1_key = "users#user1";
    let doc2_key = "users#user2";
    
    // Transaction A
    let manager_a = concurrency_manager.clone();
    let deadlock_a = deadlock_detected.clone();
    let victim_a = victim_transaction.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "txn_a";
        manager_a.register_transaction(txn_id).unwrap();
        
        // Acquire lock on doc1
        if manager_a.acquire_lock(txn_id, doc1_key, LockType::Write, None).is_ok() {
            // Small delay to let transaction B acquire doc2
            sim.clock().sleep(Duration::from_millis(20));
            
            // Try to acquire lock on doc2 (will create deadlock)
            match manager_a.acquire_lock(txn_id, doc2_key, LockType::Write, None) {
                Ok(()) => {
                    // Should not reach here in deadlock scenario
                    manager_a.release_lock(txn_id, doc2_key).unwrap();
                    manager_a.release_lock(txn_id, doc1_key).unwrap();
                }
                Err(_) => {
                    // Check if deadlock was detected
                    if let Ok(DeadlockResult::DeadlockDetected { victim, .. }) = manager_a.detect_deadlock() {
                        *deadlock_a.lock().unwrap() = true;
                        *victim_a.lock().unwrap() = victim;
                    }
                    manager_a.release_lock(txn_id, doc1_key).unwrap();
                }
            }
        }
        
        manager_a.unregister_transaction(txn_id).unwrap();
    });
    
    // Transaction B
    let manager_b = concurrency_manager.clone();
    let deadlock_b = deadlock_detected.clone();
    let victim_b = victim_transaction.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "txn_b";
        manager_b.register_transaction(txn_id).unwrap();
        
        // Small delay to let transaction A acquire doc1
        sim.clock().sleep(Duration::from_millis(10));
        
        // Acquire lock on doc2
        if manager_b.acquire_lock(txn_id, doc2_key, LockType::Write, None).is_ok() {
            // Small delay
            sim.clock().sleep(Duration::from_millis(20));
            
            // Try to acquire lock on doc1 (will create deadlock)
            match manager_b.acquire_lock(txn_id, doc1_key, LockType::Write, None) {
                Ok(()) => {
                    // Should not reach here in deadlock scenario
                    manager_b.release_lock(txn_id, doc1_key).unwrap();
                    manager_b.release_lock(txn_id, doc2_key).unwrap();
                }
                Err(_) => {
                    // Check if deadlock was detected
                    if let Ok(DeadlockResult::DeadlockDetected { victim, .. }) = manager_b.detect_deadlock() {
                        *deadlock_b.lock().unwrap() = true;
                        *victim_b.lock().unwrap() = victim;
                    }
                    manager_b.release_lock(txn_id, doc2_key).unwrap();
                }
            }
        }
        
        manager_b.unregister_transaction(txn_id).unwrap();
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify deadlock was detected
    assert!(*deadlock_detected.lock().unwrap(), "Deadlock should be detected");
    
    let victim = victim_transaction.lock().unwrap();
    assert!(victim == "txn_a" || victim == "txn_b", "One transaction should be chosen as victim");
    
    Ok(())
}

/// Test optimistic locking conflicts with retry logic
#[test]
fn test_optimistic_locking_conflicts() -> OQLResult<()> {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    // Create test storage and executor
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test_optimistic.owl");
    let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
    let executor = Arc::new(QueryExecutor::new(store, sim.clock()));
    
    // Set up test data
    setup_test_data(&executor)?;
    
    // Create concurrency manager
    let concurrency_manager = Arc::new(ConcurrencyManager::new(sim.clock()));
    
    // Track results
    let results = Arc::new(Mutex::new(Vec::new()));
    
    let doc_key = "users#user1";
    let expected_version = 1u64; // Assume document starts at version 1
    
    // Transaction 1: Update with correct version
    let manager1 = concurrency_manager.clone();
    let results1 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "txn1";
        manager1.register_transaction(txn_id).unwrap();
        
        match manager1.acquire_lock(txn_id, doc_key, LockType::Write, Some(expected_version)) {
            Ok(()) => {
                // Simulate version validation success
                match manager1.validate_version(doc_key, expected_version, expected_version, txn_id) {
                    Ok(()) => {
                        results1.lock().unwrap().push(("txn1", true));
                    }
                    Err(_) => {
                        results1.lock().unwrap().push(("txn1", false));
                    }
                }
                manager1.release_lock(txn_id, doc_key).unwrap();
            }
            Err(_) => {
                results1.lock().unwrap().push(("txn1", false));
            }
        }
        
        manager1.unregister_transaction(txn_id).unwrap();
    });
    
    // Transaction 2: Update with incorrect version (optimistic conflict)
    let manager2 = concurrency_manager.clone();
    let results2 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "txn2";
        manager2.register_transaction(txn_id).unwrap();
        
        // Small delay to ensure txn1 runs first
        sim.clock().sleep(Duration::from_millis(10));
        
        let wrong_version = expected_version + 1; // Wrong version
        
        match manager2.acquire_lock(txn_id, doc_key, LockType::Write, Some(wrong_version)) {
            Ok(()) => {
                // Simulate version validation failure
                match manager2.validate_version(doc_key, wrong_version, expected_version, txn_id) {
                    Ok(()) => {
                        results2.lock().unwrap().push(("txn2", true));
                    }
                    Err(_) => {
                        // Expected optimistic locking conflict
                        results2.lock().unwrap().push(("txn2", false));
                    }
                }
                manager2.release_lock(txn_id, doc_key).unwrap();
            }
            Err(_) => {
                results2.lock().unwrap().push(("txn2", false));
            }
        }
        
        manager2.unregister_transaction(txn_id).unwrap();
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify results
    let final_results = results.lock().unwrap();
    assert_eq!(final_results.len(), 2);
    
    // Transaction 1 should succeed (correct version)
    let txn1_result = final_results.iter().find(|(id, _)| id == &"txn1").unwrap();
    assert!(txn1_result.1, "Transaction 1 should succeed with correct version");
    
    // Transaction 2 should fail (optimistic conflict)
    let txn2_result = final_results.iter().find(|(id, _)| id == &"txn2").unwrap();
    assert!(!txn2_result.1, "Transaction 2 should fail due to optimistic conflict");
    
    Ok(())
}

/// Test timeout scenarios with deterministic timing
#[test]
fn test_timeout_scenarios_deterministic() -> OQLResult<()> {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    // Create concurrency manager with very short timeout
    let short_timeout = Duration::from_millis(50);
    let concurrency_manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        sim.clock(),
        short_timeout,
        Duration::from_millis(5), // Frequent checks
    ));
    
    // Track timeout occurrences
    let timeout_count = Arc::new(Mutex::new(0));
    
    let doc_key = "users#user1";
    
    // Transaction 1: Hold lock for longer than timeout
    let manager1 = concurrency_manager.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "holder";
        manager1.register_transaction(txn_id).unwrap();
        
        // Acquire lock and hold it
        if manager1.acquire_lock(txn_id, doc_key, LockType::Write, None).is_ok() {
            // Hold lock for longer than timeout period
            sim.clock().sleep(Duration::from_millis(100));
            manager1.release_lock(txn_id, doc_key).unwrap();
        }
        
        manager1.unregister_transaction(txn_id).unwrap();
    });
    
    // Multiple transactions that will timeout
    for i in 0..3 {
        let manager = concurrency_manager.clone();
        let timeout_counter = timeout_count.clone();
        sim.scheduler().spawn_fn(move || {
            let txn_id = format!("waiter_{}", i);
            manager.register_transaction(&txn_id).unwrap();
            
            // Small delay to ensure holder gets lock first
            sim.clock().sleep(Duration::from_millis(5));
            
            // Try to acquire lock - should timeout
            match manager.acquire_lock(&txn_id, doc_key, LockType::Read, None) {
                Ok(()) => {
                    // Should not succeed
                    manager.release_lock(&txn_id, doc_key).unwrap();
                }
                Err(OQLError::ConcurrencyError { .. }) | Err(OQLError::EnhancedConcurrencyError { .. }) => {
                    // Expected timeout
                    *timeout_counter.lock().unwrap() += 1;
                }
                Err(_) => {
                    // Other error
                }
            }
            
            manager.unregister_transaction(&txn_id).unwrap();
        });
    }
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify that timeouts occurred
    let final_timeout_count = *timeout_count.lock().unwrap();
    assert_eq!(final_timeout_count, 3, "All waiting transactions should timeout");
    
    Ok(())
}

/// Test concurrent read locks (should not conflict)
#[test]
fn test_concurrent_read_locks_no_conflict() -> OQLResult<()> {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    // Create test storage and executor
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test_read_locks.owl");
    let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
    let executor = Arc::new(QueryExecutor::new(store, sim.clock()));
    
    // Set up test data
    setup_test_data(&executor)?;
    
    // Create concurrency manager
    let concurrency_manager = Arc::new(ConcurrencyManager::new(sim.clock()));
    
    // Track successful read operations
    let success_count = Arc::new(Mutex::new(0));
    
    let doc_key = "users#user1";
    
    // Spawn multiple concurrent read transactions
    for i in 0..5 {
        let manager = concurrency_manager.clone();
        let executor_clone = executor.clone();
        let counter = success_count.clone();
        
        sim.scheduler().spawn_fn(move || {
            let txn_id = format!("reader_{}", i);
            manager.register_transaction(&txn_id).unwrap();
            
            // All should be able to acquire read locks simultaneously
            match manager.acquire_lock(&txn_id, doc_key, LockType::Read, None) {
                Ok(()) => {
                    // Execute read query
                    let query = parse_oql(r#"users.where { .id == "user1" }"#).unwrap();
                    if executor_clone.execute_query(&query).is_ok() {
                        *counter.lock().unwrap() += 1;
                    }
                    
                    manager.release_lock(&txn_id, doc_key).unwrap();
                }
                Err(_) => {
                    // Should not fail for read locks
                }
            }
            
            manager.unregister_transaction(&txn_id).unwrap();
        });
    }
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify all read operations succeeded
    let final_count = *success_count.lock().unwrap();
    assert_eq!(final_count, 5, "All concurrent read operations should succeed");
    
    Ok(())
}

/// Helper function to set up test data
fn setup_test_data(executor: &QueryExecutor<dyn DocumentStore>) -> OQLResult<()> {
    // Create test documents
    let mut fields1 = HashMap::new();
    fields1.insert("id".to_string(), Value::String("user1".to_string()));
    fields1.insert("name".to_string(), Value::String("Alice".to_string()));
    fields1.insert("age".to_string(), Value::Integer(30));
    
    let mut fields2 = HashMap::new();
    fields2.insert("id".to_string(), Value::String("user2".to_string()));
    fields2.insert("name".to_string(), Value::String("Bob".to_string()));
    fields2.insert("age".to_string(), Value::Integer(25));
    
    let doc1 = Document::with_id("user1".to_string(), "users".to_string(), fields1);
    let doc2 = Document::with_id("user2".to_string(), "users".to_string(), fields2);
    
    executor.store().put_document(&doc1)?;
    executor.store().put_document(&doc2)?;
    
    Ok(())
}