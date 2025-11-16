// Comprehensive tests for concurrent access control
// Tests optimistic locking, deadlock detection, and multi-client operations

use owl_db::concurrency::{ConcurrencyManager, LockType};
use owl_db::document::{Document, Value};
use owl_db::error::OQLError;
use owl_db::storage::DocumentStore;
use owl_db::transaction::{IsolationLevel, TransactionManager, TransactionalDocumentStore};
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn create_test_document(id: &str, name: &str, version: u64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    
    let mut doc = Document::with_id(id.to_string(), "test_docs".to_string(), fields);
    doc.version = version;
    doc
}

#[test]
fn test_optimistic_locking_basic() {
    let manager = ConcurrencyManager::new();
    let doc_key = "doc1";
    let txn_id = "txn1";

    manager.register_transaction(txn_id).unwrap();

    // Test successful version validation
    assert!(manager.validate_version(doc_key, 1, 1, txn_id).is_ok());

    // Test version conflict
    let result = manager.validate_version(doc_key, 1, 2, txn_id);
    assert!(result.is_err());
    
    if let Err(OQLError::EnhancedConcurrencyError { message, .. }) = result {
        assert!(message.contains("Optimistic locking conflict"));
        assert!(message.contains("expected version 1"));
        assert!(message.contains("document has version 2"));
    } else {
        panic!("Expected concurrency error");
    }

    manager.unregister_transaction(txn_id).unwrap();
}

#[test]
fn test_read_write_lock_compatibility() {
    // Use shorter timeout for testing
    let manager = ConcurrencyManager::new_with_timeouts(
        Duration::from_millis(500), 
        Duration::from_millis(50)
    );
    let doc_key = "doc1";
    let txn1 = "txn1";
    let txn2 = "txn2";
    let txn3 = "txn3";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();
    manager.register_transaction(txn3).unwrap();

    // Multiple read locks should be allowed
    manager.acquire_lock(txn1, doc_key, LockType::Read, None).unwrap();
    manager.acquire_lock(txn2, doc_key, LockType::Read, None).unwrap();

    // Write lock should conflict with existing read locks and timeout
    let result = manager.acquire_lock(txn3, doc_key, LockType::Write, None);
    assert!(result.is_err(), "Write lock should fail when read locks exist");

    // Release read locks
    manager.release_lock(txn1, doc_key).unwrap();
    manager.release_lock(txn2, doc_key).unwrap();

    // Now write lock should be acquirable
    manager.acquire_lock(txn3, doc_key, LockType::Write, None).unwrap();

    // Cleanup
    manager.release_lock(txn3, doc_key).unwrap();
    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
    manager.unregister_transaction(txn3).unwrap();
}

#[test]
fn test_lock_upgrade() {
    let manager = ConcurrencyManager::new();
    let doc_key = "doc1";
    let txn_id = "txn1";

    manager.register_transaction(txn_id).unwrap();

    // Acquire read lock
    manager.acquire_lock(txn_id, doc_key, LockType::Read, None).unwrap();

    // Upgrade to write lock (should succeed since we're the only reader)
    manager.acquire_lock(txn_id, doc_key, LockType::Write, None).unwrap();

    // Verify we have a write lock
    let lock_info = manager.get_lock_info(doc_key).unwrap();
    assert_eq!(lock_info.len(), 1);
    assert_eq!(lock_info[0].lock_type, LockType::Write);
    assert_eq!(lock_info[0].transaction_id, txn_id);

    manager.unregister_transaction(txn_id).unwrap();
}

#[test]
fn test_deadlock_detection_simple() {
    let manager = Arc::new(ConcurrencyManager::new());
    let doc1 = "doc1";
    let doc2 = "doc2";
    let txn1 = "txn1";
    let txn2 = "txn2";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();

    // Create a deadlock scenario
    // txn1 locks doc1, txn2 locks doc2
    manager.acquire_lock(txn1, doc1, LockType::Write, None).unwrap();
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();

    let manager1 = manager.clone();
    let manager2 = manager.clone();

    let barrier = Arc::new(Barrier::new(2));
    let barrier1 = barrier.clone();
    let barrier2 = barrier.clone();

    // Now try to create circular dependency
    let handle1 = thread::spawn(move || {
        barrier1.wait();
        // txn1 tries to lock doc2 (held by txn2)
        manager1.acquire_lock(txn1, doc2, LockType::Write, None)
    });

    let handle2 = thread::spawn(move || {
        barrier2.wait();
        // txn2 tries to lock doc1 (held by txn1)
        manager2.acquire_lock(txn2, doc1, LockType::Write, None)
    });

    // Both should fail due to deadlock detection or timeout
    let result1 = handle1.join().unwrap();
    let result2 = handle2.join().unwrap();

    // At least one should fail
    assert!(result1.is_err() || result2.is_err());

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
}

#[test]
fn test_concurrent_document_updates() {
    let temp_dir = TempDir::new().unwrap();
    let store = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Insert initial document
    let initial_doc = create_test_document("doc1", "Initial", 1);
    let txn_id = store.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    store.insert_document_in_transaction(&txn_id, initial_doc).unwrap();
    store.commit_transaction(&txn_id).unwrap();

    let num_threads = 4;
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();

    // Spawn multiple threads trying to update the same document
    for i in 0..num_threads {
        let store_clone = store.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let txn_id = store_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
            
            // Try to read and update the document
            let result = store_clone.get_document_in_transaction(&txn_id, "test_docs", &"doc1".to_string());
            
            if let Ok(Some(mut doc)) = result {
                // Modify the document
                doc.set_field("name".to_string(), Value::String(format!("Updated by thread {}", i)));
                
                // Try to update with optimistic locking
                let update_result = store_clone.update_document_in_transaction(
                    &txn_id,
                    "test_docs",
                    &"doc1".to_string(),
                    doc,
                    Some(1), // Expected version
                );
                
                if update_result.is_ok() {
                    store_clone.commit_transaction(&txn_id)
                } else {
                    store_clone.rollback_transaction(&txn_id).unwrap_or(());
                    update_result
                }
            } else {
                store_clone.rollback_transaction(&txn_id).unwrap_or(());
                Err(owl_db::error::OQLError::Generic { 
                    message: "Document not found".to_string() 
                })
            }
        });
        
        handles.push(handle);
    }

    // Collect results
    let mut successful_updates = 0;
    let mut conflicts = 0;

    for handle in handles {
        match handle.join().unwrap() {
            Ok(_) => successful_updates += 1,
            Err(_) => conflicts += 1,
        }
    }

    // Only one thread should succeed due to optimistic locking
    assert_eq!(successful_updates, 1);
    assert_eq!(conflicts, num_threads - 1);
}

#[test]
fn test_transaction_isolation() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());

    // Insert initial document
    let doc = create_test_document("doc1", "Original", 1);
    let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    manager.insert_document_in_transaction(&txn_id, doc).unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    // Test sequential transactions (since our current implementation uses pessimistic locking)
    let txn1 = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    
    // Read and update in transaction 1
    let doc1 = manager.get_document_in_transaction(&txn1, "test_docs", &"doc1".to_string()).unwrap().unwrap();
    assert_eq!(doc1.get_field("name"), Some(&Value::String("Original".to_string())));
    
    let mut updated_doc1 = doc1.clone();
    updated_doc1.set_field("name".to_string(), Value::String("Updated by txn1".to_string()));
    manager.update_document_in_transaction(&txn1, "test_docs", &"doc1".to_string(), updated_doc1, Some(doc1.version)).unwrap();

    // Commit transaction 1
    manager.commit_transaction(&txn1).unwrap();

    // Start transaction 2 after transaction 1 commits
    let txn2 = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    let doc2 = manager.get_document_in_transaction(&txn2, "test_docs", &"doc1".to_string()).unwrap().unwrap();
    
    // Transaction 2 should see the committed changes from transaction 1
    assert_eq!(doc2.get_field("name"), Some(&Value::String("Updated by txn1".to_string())));
    
    manager.commit_transaction(&txn2).unwrap();
}

#[test]
fn test_concurrency_statistics() {
    let manager = ConcurrencyManager::new();
    let txn1 = "txn1";
    let txn2 = "txn2";
    let doc1 = "doc1";
    let doc2 = "doc2";

    // Initial statistics
    let stats = manager.get_statistics().unwrap();
    assert_eq!(stats.active_transactions, 0);
    assert_eq!(stats.total_locks, 0);
    assert_eq!(stats.locked_documents, 0);

    // Register transactions and acquire locks
    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();
    manager.acquire_lock(txn1, doc1, LockType::Read, None).unwrap();
    manager.acquire_lock(txn2, doc2, LockType::Write, None).unwrap();

    let stats = manager.get_statistics().unwrap();
    assert_eq!(stats.active_transactions, 2);
    assert_eq!(stats.total_locks, 2);
    assert_eq!(stats.locked_documents, 2);

    // Cleanup
    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();

    let stats = manager.get_statistics().unwrap();
    assert_eq!(stats.active_transactions, 0);
    assert_eq!(stats.total_locks, 0);
    assert_eq!(stats.locked_documents, 0);
}

#[test]
fn test_lock_timeout_behavior() {
    let manager = ConcurrencyManager::new();
    let doc_key = "doc1";
    let txn1 = "txn1";
    let txn2 = "txn2";

    manager.register_transaction(txn1).unwrap();
    manager.register_transaction(txn2).unwrap();

    // txn1 acquires write lock
    manager.acquire_lock(txn1, doc_key, LockType::Write, None).unwrap();

    let start_time = std::time::Instant::now();
    
    // txn2 tries to acquire conflicting lock - should timeout
    let result = manager.acquire_lock(txn2, doc_key, LockType::Read, None);
    
    let elapsed = start_time.elapsed();
    
    // Should fail due to timeout
    assert!(result.is_err());
    
    // Should have waited for some time (but not too long in tests)
    assert!(elapsed >= Duration::from_millis(50));

    manager.unregister_transaction(txn1).unwrap();
    manager.unregister_transaction(txn2).unwrap();
}

#[test]
fn test_multi_client_simulation() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Insert initial documents
    for i in 0..5 {
        let doc = create_test_document(&format!("doc{}", i), &format!("Initial {}", i), 1);
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        manager.insert_document_in_transaction(&txn_id, doc).unwrap();
        manager.commit_transaction(&txn_id).unwrap();
    }

    let num_clients = 3;
    let operations_per_client = 5;
    let barrier = Arc::new(Barrier::new(num_clients));
    let mut handles = Vec::new();

    // Simulate multiple clients performing concurrent operations
    for client_id in 0..num_clients {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_ops = 0;
            let mut failed_ops = 0;
            
            for op_id in 0..operations_per_client {
                let doc_id = format!("doc{}", op_id % 5); // Cycle through documents
                let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                
                let result = (|| -> Result<(), owl_db::error::OQLError> {
                    // Read document
                    let doc = manager_clone.get_document_in_transaction(
                        &txn_id, 
                        "test_docs", 
                        &doc_id
                    )?;
                    
                    if let Some(mut document) = doc {
                        // Update document
                        document.set_field(
                            "name".to_string(), 
                            Value::String(format!("Updated by client {} op {}", client_id, op_id))
                        );
                        
                        manager_clone.update_document_in_transaction(
                            &txn_id,
                            "test_docs",
                            &doc_id,
                            document,
                            None, // No specific version requirement
                        )?;
                        
                        manager_clone.commit_transaction(&txn_id)?;
                    }
                    
                    Ok(())
                })();
                
                match result {
                    Ok(_) => successful_ops += 1,
                    Err(_) => {
                        failed_ops += 1;
                        let _ = manager_clone.rollback_transaction(&txn_id);
                    }
                }
                
                // Small delay between operations
                thread::sleep(Duration::from_millis(10));
            }
            
            (successful_ops, failed_ops)
        });
        
        handles.push(handle);
    }

    // Collect results
    let mut total_successful = 0;
    let mut total_failed = 0;

    for handle in handles {
        let (successful, failed) = handle.join().unwrap();
        total_successful += successful;
        total_failed += failed;
    }

    // Some operations should succeed, some might fail due to conflicts
    assert!(total_successful > 0);
    println!("Multi-client test: {} successful, {} failed operations", 
             total_successful, total_failed);
}