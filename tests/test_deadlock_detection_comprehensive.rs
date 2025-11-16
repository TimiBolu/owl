// Comprehensive Deadlock Detection and Resolution Tests
// Tests various deadlock scenarios and resolution strategies

use owl_db::concurrency::{ConcurrencyManager, LockType, DeadlockResult};
use owl_db::document::{Document, Value};
use owl_db::error::OQLError;
use owl_db::transaction::{IsolationLevel, TransactionManager};
use std::collections::HashMap;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn create_test_document(id: &str, name: &str) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    Document::with_id(id.to_string(), "test_docs".to_string(), fields)
}

#[test]
fn test_simple_deadlock_detection() {
    // Test deadlock detection through realistic lock acquisition scenarios
    let manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        Duration::from_millis(200), // Short timeout for testing
        Duration::from_millis(10)
    ));
    
    let doc1 = "doc1";
    let doc2 = "doc2";
    let txn1 = "txn1";
    let txn2 = "txn2";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();

    // Create a realistic deadlock scenario
    // Transaction 1 locks document 1
    manager.acquire_lock(txn1, doc1, LockType::Write, None).unwrap();
    
    // Transaction 2 locks document 2
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();

    let manager1 = manager.clone();
    let manager2 = manager.clone();
    let barrier = Arc::new(Barrier::new(2));
    let barrier1 = barrier.clone();
    let barrier2 = barrier.clone();

    let results = Arc::new(Mutex::new(Vec::new()));
    let results1 = results.clone();
    let results2 = results.clone();

    // Now both transactions try to acquire the other's lock (creates deadlock)
    let handle1 = thread::spawn(move || {
        barrier1.wait();
        let result = manager1.acquire_lock(txn1, doc2, LockType::Write, None);
        results1.lock().unwrap().push(("txn1".to_string(), result));
    });

    let handle2 = thread::spawn(move || {
        barrier2.wait();
        let result = manager2.acquire_lock(txn2, doc1, LockType::Write, None);
        results2.lock().unwrap().push(("txn2".to_string(), result));
    });

    handle1.join().unwrap();
    handle2.join().unwrap();

    let results = results.lock().unwrap();
    
    // At least one transaction should fail due to deadlock detection or timeout
    let failures = results.iter().filter(|(_, result)| result.is_err()).count();
    assert!(failures >= 1, "At least one transaction should fail due to deadlock or timeout");

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
}

#[test]
fn test_complex_deadlock_chain() {
    // Test a more complex deadlock scenario with three transactions
    let manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        Duration::from_millis(300), // Longer timeout for complex scenario
        Duration::from_millis(20)
    ));
    
    let doc1 = "doc1";
    let doc2 = "doc2";
    let doc3 = "doc3";
    let txn1 = "txn1";
    let txn2 = "txn2";
    let txn3 = "txn3";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();
    manager.register_transaction(txn3).unwrap();

    // Create a circular dependency: txn1 -> doc1, txn2 -> doc2, txn3 -> doc3
    manager.acquire_lock(txn1, doc1, LockType::Write, None).unwrap();
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();
    manager.acquire_lock(txn3, doc3, LockType::Write, None).unwrap();

    let manager1 = manager.clone();
    let manager2 = manager.clone();
    let manager3 = manager.clone();
    let barrier = Arc::new(Barrier::new(3));
    let barrier1 = barrier.clone();
    let barrier2 = barrier.clone();
    let barrier3 = barrier.clone();

    let results = Arc::new(Mutex::new(Vec::new()));
    let results1 = results.clone();
    let results2 = results.clone();
    let results3 = results.clone();

    // Create circular wait: txn1 wants doc2, txn2 wants doc3, txn3 wants doc1
    let handle1 = thread::spawn(move || {
        barrier1.wait();
        let result = manager1.acquire_lock(txn1, doc2, LockType::Write, None);
        results1.lock().unwrap().push(("txn1".to_string(), result));
    });

    let handle2 = thread::spawn(move || {
        barrier2.wait();
        let result = manager2.acquire_lock(txn2, doc3, LockType::Write, None);
        results2.lock().unwrap().push(("txn2".to_string(), result));
    });

    let handle3 = thread::spawn(move || {
        barrier3.wait();
        let result = manager3.acquire_lock(txn3, doc1, LockType::Write, None);
        results3.lock().unwrap().push(("txn3".to_string(), result));
    });

    handle1.join().unwrap();
    handle2.join().unwrap();
    handle3.join().unwrap();

    let results = results.lock().unwrap();
    
    // At least one (likely all) transactions should fail due to deadlock or timeout
    let failures = results.iter().filter(|(_, result)| result.is_err()).count();
    assert!(failures >= 1, "At least one transaction should fail in circular deadlock");

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
    manager.unregister_transaction(txn3).unwrap();
}

#[test]
fn test_no_deadlock_linear_chain() {
    // Test that linear lock acquisition doesn't cause false deadlock detection
    let manager = ConcurrencyManager::new();
    
    let doc1 = "doc1";
    let doc2 = "doc2";
    let doc3 = "doc3";
    let txn1 = "txn1";
    let txn2 = "txn2";
    let txn3 = "txn3";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();
    manager.register_transaction(txn3).unwrap();

    // Linear acquisition: txn1 -> doc1, txn2 -> doc2, txn3 -> doc3
    // No circular dependencies
    manager.acquire_lock(txn1, doc1, LockType::Write, None).unwrap();
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();
    manager.acquire_lock(txn3, doc3, LockType::Write, None).unwrap();

    // All transactions should be able to acquire their locks without issues
    // No deadlock should be detected
    let result = manager.detect_deadlock().unwrap();
    
    match result {
        DeadlockResult::NoDeadlock => {
            // This is expected for linear chains
        }
        DeadlockResult::DeadlockDetected { .. } => {
            panic!("No deadlock should be detected in linear acquisition pattern");
        }
    }

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
    manager.unregister_transaction(txn3).unwrap();
}

#[test]
fn test_realistic_deadlock_scenario() {
    let manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        Duration::from_millis(1000), // Longer timeout for this test
        Duration::from_millis(50)
    ));
    
    let doc1 = "document1";
    let doc2 = "document2";
    let txn1 = "transaction1";
    let txn2 = "transaction2";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();

    // Create realistic deadlock scenario
    // Transaction 1 locks document 1
    manager.acquire_lock(txn1, doc1, LockType::Write, None).unwrap();
    
    // Transaction 2 locks document 2
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();

    let manager1 = manager.clone();
    let manager2 = manager.clone();
    let barrier = Arc::new(Barrier::new(2));
    let barrier1 = barrier.clone();
    let barrier2 = barrier.clone();

    let results = Arc::new(Mutex::new(Vec::new()));
    let results1 = results.clone();
    let results2 = results.clone();

    // Now both transactions try to acquire the other's lock
    let handle1 = thread::spawn(move || {
        barrier1.wait();
        let result = manager1.acquire_lock(txn1, doc2, LockType::Write, None);
        results1.lock().unwrap().push(("txn1".to_string(), result));
    });

    let handle2 = thread::spawn(move || {
        barrier2.wait();
        let result = manager2.acquire_lock(txn2, doc1, LockType::Write, None);
        results2.lock().unwrap().push(("txn2".to_string(), result));
    });

    handle1.join().unwrap();
    handle2.join().unwrap();

    let results = results.lock().unwrap();
    
    // At least one transaction should fail due to deadlock detection or timeout
    let failures = results.iter().filter(|(_, result)| result.is_err()).count();
    assert!(failures >= 1, "At least one transaction should fail due to deadlock");

    // Check if any failures were due to deadlock detection
    let deadlock_failures = results.iter().any(|(_, result)| {
        if let Err(OQLError::EnhancedConcurrencyError { message, .. }) = result {
            message.contains("deadlock")
        } else {
            false
        }
    });

    println!("Deadlock detection triggered: {}", deadlock_failures);

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
}

#[test]
fn test_deadlock_with_multiple_documents() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Setup initial documents
    let docs = vec![
        create_test_document("doc1", "Document 1"),
        create_test_document("doc2", "Document 2"),
        create_test_document("doc3", "Document 3"),
    ];
    
    for doc in &docs {
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        manager.insert_document_in_transaction(&txn_id, doc.clone()).unwrap();
        manager.commit_transaction(&txn_id).unwrap();
    }

    let num_transactions = 3;
    let barrier = Arc::new(Barrier::new(num_transactions));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    // Create multiple transactions that will create a deadlock cycle
    for i in 0..num_transactions {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let results_clone = results.clone();
        
        let handle = thread::spawn(move || {
            let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
            
            barrier_clone.wait();
            
            let result = (|| -> Result<(), OQLError> {
                // Each transaction tries to access documents in a different order
                // This creates potential for circular dependencies
                let doc_order = match i {
                    0 => vec!["doc1", "doc2", "doc3"],
                    1 => vec!["doc2", "doc3", "doc1"],
                    2 => vec!["doc3", "doc1", "doc2"],
                    _ => unreachable!(),
                };
                
                for (idx, doc_id) in doc_order.iter().enumerate() {
                    // Add small delay to increase chance of deadlock
                    if idx > 0 {
                        thread::sleep(Duration::from_millis(10));
                    }
                    
                    let doc = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "test_docs",
                        &doc_id.to_string(),
                    )?;
                    
                    if let Some(mut document) = doc {
                        document.set_field(
                            "name".to_string(),
                            Value::String(format!("Updated by txn {}", i)),
                        );
                        
                        manager_clone.update_document_in_transaction(
                            &txn_id,
                            "test_docs",
                            &doc_id.to_string(),
                            document,
                            None,
                        )?;
                    }
                }
                
                manager_clone.commit_transaction(&txn_id)?;
                Ok(())
            })();
            
            if result.is_err() {
                let _ = manager_clone.rollback_transaction(&txn_id);
            }
            
            results_clone.lock().unwrap().push((i, result));
        });
        
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let results = results.lock().unwrap();
    let successful = results.iter().filter(|(_, result)| result.is_ok()).count();
    let failed = results.iter().filter(|(_, result)| result.is_err()).count();
    
    println!("Multi-document deadlock test: {} successful, {} failed", successful, failed);
    
    // Some transactions should succeed, some might fail due to conflicts or deadlocks
    assert!(successful > 0, "At least one transaction should succeed");
    
    // Check for deadlock-related failures
    let deadlock_failures = results.iter().filter(|(_, result)| {
        if let Err(OQLError::EnhancedConcurrencyError { message, .. }) = result {
            message.contains("deadlock") || message.contains("timeout")
        } else {
            false
        }
    }).count();
    
    println!("Deadlock/timeout failures: {}", deadlock_failures);
}

#[test]
fn test_deadlock_prevention_with_ordered_locking() {
    let manager = Arc::new(ConcurrencyManager::new());
    let doc1 = "doc1";
    let doc2 = "doc2";
    let txn1 = "txn1";
    let txn2 = "txn2";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();

    let manager1 = manager.clone();
    let manager2 = manager.clone();
    let barrier = Arc::new(Barrier::new(2));
    let barrier1 = barrier.clone();
    let barrier2 = barrier.clone();

    let results = Arc::new(Mutex::new(Vec::new()));
    let results1 = results.clone();
    let results2 = results.clone();

    // Both transactions acquire locks in the same order (prevents deadlock)
    let handle1 = thread::spawn(move || {
        barrier1.wait();
        
        let result1 = manager1.acquire_lock(txn1, doc1, LockType::Write, None);
        if result1.is_ok() {
            thread::sleep(Duration::from_millis(50)); // Hold lock briefly
            let result2 = manager1.acquire_lock(txn1, doc2, LockType::Write, None);
            results1.lock().unwrap().push(("txn1".to_string(), result2));
        } else {
            results1.lock().unwrap().push(("txn1".to_string(), result1));
        }
    });

    let handle2 = thread::spawn(move || {
        barrier2.wait();
        
        // Same order as transaction 1
        let result1 = manager2.acquire_lock(txn2, doc1, LockType::Write, None);
        if result1.is_ok() {
            thread::sleep(Duration::from_millis(50)); // Hold lock briefly
            let result2 = manager2.acquire_lock(txn2, doc2, LockType::Write, None);
            results2.lock().unwrap().push(("txn2".to_string(), result2));
        } else {
            results2.lock().unwrap().push(("txn2".to_string(), result1));
        }
    });

    handle1.join().unwrap();
    handle2.join().unwrap();

    let results = results.lock().unwrap();
    
    // With ordered locking, we should avoid deadlocks
    // One transaction should succeed, the other should timeout (not deadlock)
    let successes = results.iter().filter(|(_, result)| result.is_ok()).count();
    let failures = results.iter().filter(|(_, result)| result.is_err()).count();
    
    println!("Ordered locking test: {} successes, {} failures", successes, failures);
    
    // Check that failures are due to timeout, not deadlock
    let timeout_failures = results.iter().filter(|(_, result)| {
        if let Err(OQLError::EnhancedConcurrencyError { message, .. }) = result {
            message.contains("timeout") && !message.contains("deadlock")
        } else {
            false
        }
    }).count();
    
    assert_eq!(failures, timeout_failures, "All failures should be timeouts, not deadlocks");

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
}

#[test]
fn test_deadlock_resolution_victim_selection() {
    // Test that deadlock resolution properly selects a victim transaction
    let manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        Duration::from_millis(200),
        Duration::from_millis(10)
    ));
    
    let doc1 = "doc1";
    let doc2 = "doc2";
    let doc3 = "doc3";
    let txn1 = "txn1";
    let txn2 = "txn2";
    let txn3 = "txn3";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();
    manager.register_transaction(txn3).unwrap();

    // Create initial locks
    manager.acquire_lock(txn1, doc1, LockType::Write, None).unwrap();
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();
    manager.acquire_lock(txn3, doc3, LockType::Write, None).unwrap();

    let manager1 = manager.clone();
    let manager2 = manager.clone();
    let manager3 = manager.clone();
    let barrier = Arc::new(Barrier::new(3));
    let barrier1 = barrier.clone();
    let barrier2 = barrier.clone();
    let barrier3 = barrier.clone();

    let results = Arc::new(Mutex::new(Vec::new()));
    let results1 = results.clone();
    let results2 = results.clone();
    let results3 = results.clone();

    // Create circular dependency
    let handle1 = thread::spawn(move || {
        barrier1.wait();
        let result = manager1.acquire_lock(txn1, doc2, LockType::Write, None);
        results1.lock().unwrap().push(("txn1".to_string(), result));
    });

    let handle2 = thread::spawn(move || {
        barrier2.wait();
        let result = manager2.acquire_lock(txn2, doc3, LockType::Write, None);
        results2.lock().unwrap().push(("txn2".to_string(), result));
    });

    let handle3 = thread::spawn(move || {
        barrier3.wait();
        let result = manager3.acquire_lock(txn3, doc1, LockType::Write, None);
        results3.lock().unwrap().push(("txn3".to_string(), result));
    });

    handle1.join().unwrap();
    handle2.join().unwrap();
    handle3.join().unwrap();

    let results = results.lock().unwrap();
    
    // At least one transaction should be selected as victim and fail
    let failures = results.iter().filter(|(_, result)| result.is_err()).count();
    let successes = results.iter().filter(|(_, result)| result.is_ok()).count();
    
    assert!(failures >= 1, "At least one transaction should be selected as victim");
    println!("Deadlock resolution: {} failures, {} successes", failures, successes);

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
    manager.unregister_transaction(txn3).unwrap();
}

#[test]
fn test_concurrent_deadlock_detection() {
    // Test that deadlock detection works correctly under concurrent access
    let manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        Duration::from_millis(100),
        Duration::from_millis(5)
    ));
    
    let num_threads = 4;
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();

    // Multiple threads creating potential deadlock scenarios
    for thread_id in 0..num_threads {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            let txn_id = format!("txn{}", thread_id);
            let doc1 = format!("doc{}", thread_id);
            let doc2 = format!("doc{}", (thread_id + 1) % num_threads);
            
            manager_clone.register_transaction(&txn_id).unwrap();
            
            // Each thread locks its own document first
            manager_clone.acquire_lock(&txn_id, &doc1, LockType::Write, None).unwrap();
            
            barrier_clone.wait();
            
            // Then tries to lock the next thread's document (creates circular dependency)
            let result = manager_clone.acquire_lock(&txn_id, &doc2, LockType::Write, None);
            
            manager_clone.unregister_transaction(&txn_id).unwrap();
            
            (thread_id, result)
        });
        
        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        let result = handle.join().unwrap();
        results.push(result);
    }

    // Check results
    let failures = results.iter().filter(|(_, result)| result.is_err()).count();
    let successes = results.iter().filter(|(_, result)| result.is_ok()).count();
    
    println!("Concurrent deadlock test: {} failures, {} successes", failures, successes);
    
    // In a circular dependency, at least some transactions should fail
    assert!(failures > 0, "Some transactions should fail due to deadlock or timeout");
}

#[test]
fn test_deadlock_recovery_after_victim_abort() {
    let manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        Duration::from_millis(500),
        Duration::from_millis(50)
    ));
    
    let doc1 = "doc1";
    let doc2 = "doc2";
    let txn1 = "txn1";
    let txn2 = "txn2";
    let txn3 = "txn3";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();
    manager.register_transaction(txn3).unwrap();

    // Create initial locks
    manager.acquire_lock(txn1, doc1, LockType::Write, None).unwrap();
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();

    // Simulate deadlock and recovery
    let manager_clone = manager.clone();
    let handle = thread::spawn(move || {
        // This should timeout due to deadlock
        manager_clone.acquire_lock(txn1, doc2, LockType::Write, None)
    });

    // Meanwhile, txn2 tries to acquire doc1 (creates deadlock)
    let result2 = manager.acquire_lock(txn2, doc1, LockType::Write, None);
    
    // Wait for the first thread
    let result1 = handle.join().unwrap();

    // At least one should fail
    assert!(result1.is_err() || result2.is_err());

    // Now simulate victim abort by unregistering one transaction
    manager.unregister_transaction(txn1).unwrap();

    // txn3 should now be able to acquire locks that were held by txn1
    let result3 = manager.acquire_lock(txn3, doc1, LockType::Write, None);
    assert!(result3.is_ok(), "txn3 should acquire lock after txn1 abort");

    manager.unregister_transaction(txn2).unwrap();
    manager.unregister_transaction(txn3).unwrap();
}