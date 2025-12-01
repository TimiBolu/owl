// Comprehensive tests for lock-free concurrency implementation
//
// Tests correctness, performance, and memory safety of lock-free transaction map
//
// Requirements: 12

use owl_db::lockfree_concurrency::{LockFreeConcurrencyManager, LockFreeTransactionMap, LockType};
use owl_db::simulation::RealClock;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use std::collections::HashSet;

#[test]
fn test_basic_transaction_operations() {
    let clock = Arc::new(RealClock::new());
    let map = LockFreeTransactionMap::new(clock);
    
    // Test registration
    map.register_transaction("txn1").unwrap();
    map.register_transaction("txn2").unwrap();
    
    assert_eq!(map.size(), 2);
    assert!(map.contains_transaction("txn1"));
    assert!(map.contains_transaction("txn2"));
    assert!(!map.contains_transaction("txn3"));
    
    // Test unregistration
    map.unregister_transaction("txn1").unwrap();
    assert_eq!(map.size(), 1);
    assert!(!map.contains_transaction("txn1"));
    assert!(map.contains_transaction("txn2"));
    
    map.unregister_transaction("txn2").unwrap();
    assert_eq!(map.size(), 0);
    assert!(!map.contains_transaction("txn2"));
}

#[test]
fn test_concurrent_registration_unregistration() {
    let clock = Arc::new(RealClock::new());
    let map = Arc::new(LockFreeTransactionMap::new(clock));
    let num_threads = 8;
    let transactions_per_thread = 100;
    
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();
    
    // Spawn threads that register and unregister transactions concurrently
    for thread_id in 0..num_threads {
        let map_clone = map.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait(); // Synchronize start
            
            let mut registered_txns = Vec::new();
            
            // Register transactions
            for i in 0..transactions_per_thread {
                let txn_id = format!("txn_{}_{}", thread_id, i);
                map_clone.register_transaction(&txn_id).unwrap();
                registered_txns.push(txn_id);
            }
            
            // Verify they exist
            for txn_id in &registered_txns {
                assert!(map_clone.contains_transaction(txn_id));
            }
            
            // Unregister half of them
            for i in 0..transactions_per_thread / 2 {
                map_clone.unregister_transaction(&registered_txns[i]).unwrap();
            }
            
            // Verify the unregistered ones are gone
            for i in 0..transactions_per_thread / 2 {
                assert!(!map_clone.contains_transaction(&registered_txns[i]));
            }
            
            // Verify the remaining ones still exist
            for i in transactions_per_thread / 2..transactions_per_thread {
                assert!(map_clone.contains_transaction(&registered_txns[i]));
            }
            
            registered_txns
        });
        
        handles.push(handle);
    }
    
    // Collect all registered transaction IDs
    let mut all_remaining_txns = HashSet::new();
    for handle in handles {
        let registered_txns = handle.join().unwrap();
        for i in transactions_per_thread / 2..transactions_per_thread {
            all_remaining_txns.insert(registered_txns[i].clone());
        }
    }
    
    // Verify final state
    let expected_remaining = num_threads * (transactions_per_thread - transactions_per_thread / 2);
    assert_eq!(map.size(), expected_remaining);
    
    // Verify all remaining transactions exist
    for txn_id in &all_remaining_txns {
        assert!(map.contains_transaction(txn_id));
    }
    
    // Clean up remaining transactions
    for txn_id in &all_remaining_txns {
        map.unregister_transaction(txn_id).unwrap();
    }
    
    assert_eq!(map.size(), 0);
}

#[test]
fn test_high_contention_scenario() {
    let clock = Arc::new(RealClock::new());
    let map = Arc::new(LockFreeTransactionMap::new(clock));
    let num_threads = 16;
    let operations_per_thread = 50;
    
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();
    
    // High contention: all threads operating on overlapping transaction IDs
    for thread_id in 0..num_threads {
        let map_clone = map.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait(); // Synchronize start
            
            for i in 0..operations_per_thread {
                // Use overlapping transaction IDs to create contention
                let txn_id = format!("shared_txn_{}", i % 10);
                
                // Try to register (may fail if already exists)
                let _ = map_clone.register_transaction(&txn_id);
                
                // Check if it exists
                let exists = map_clone.contains_transaction(&txn_id);
                
                // Try to unregister (may fail if already removed)
                if exists {
                    let _ = map_clone.unregister_transaction(&txn_id);
                }
            }
        });
        
        handles.push(handle);
    }
    
    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }
    
    // System should be in a consistent state
    // (exact final state is non-deterministic due to race conditions, which is expected)
    println!("Final size after high contention test: {}", map.size());
}

#[test]
fn test_memory_safety_stress() {
    let clock = Arc::new(RealClock::new());
    let map = Arc::new(LockFreeTransactionMap::new(clock));
    let num_iterations = 1000;
    
    // Rapidly register and unregister transactions to stress memory management
    for iteration in 0..num_iterations {
        let txn_id = format!("stress_txn_{}", iteration);
        
        map.register_transaction(&txn_id).unwrap();
        assert!(map.contains_transaction(&txn_id));
        
        map.unregister_transaction(&txn_id).unwrap();
        assert!(!map.contains_transaction(&txn_id));
    }
    
    assert_eq!(map.size(), 0);
}

#[test]
fn test_concurrent_readers() {
    let clock = Arc::new(RealClock::new());
    let map = Arc::new(LockFreeTransactionMap::new(clock));
    
    // Pre-populate with transactions
    let num_transactions = 100;
    for i in 0..num_transactions {
        let txn_id = format!("reader_txn_{}", i);
        map.register_transaction(&txn_id).unwrap();
    }
    
    let num_reader_threads = 8;
    let reads_per_thread = 1000;
    let barrier = Arc::new(Barrier::new(num_reader_threads));
    let mut handles = Vec::new();
    
    // Spawn multiple reader threads
    for thread_id in 0..num_reader_threads {
        let map_clone = map.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait(); // Synchronize start
            
            let mut found_count = 0;
            
            for i in 0..reads_per_thread {
                let txn_id = format!("reader_txn_{}", i % num_transactions);
                if map_clone.contains_transaction(&txn_id) {
                    found_count += 1;
                }
            }
            
            found_count
        });
        
        handles.push(handle);
    }
    
    // Collect results
    let mut total_found = 0;
    for handle in handles {
        total_found += handle.join().unwrap();
    }
    
    // All reads should have found the transactions
    let expected_total = num_reader_threads * reads_per_thread;
    assert_eq!(total_found, expected_total);
    
    // Clean up
    for i in 0..num_transactions {
        let txn_id = format!("reader_txn_{}", i);
        map.unregister_transaction(&txn_id).unwrap();
    }
}

#[test]
fn test_lock_free_concurrency_manager() {
    let clock = Arc::new(RealClock::new());
    let manager = LockFreeConcurrencyManager::new(clock);
    
    // Test basic operations
    manager.register_transaction("txn1").unwrap();
    manager.register_transaction("txn2").unwrap();
    
    // Test lock acquisition
    manager.acquire_lock("txn1", "doc1", LockType::Read, None).unwrap();
    manager.acquire_lock("txn2", "doc2", LockType::Write, Some(1)).unwrap();
    
    // Test statistics
    let stats = manager.get_statistics();
    assert_eq!(stats.active_transactions, 2);
    
    // Test lock release
    manager.release_lock("txn1", "doc1").unwrap();
    manager.release_lock("txn2", "doc2").unwrap();
    
    // Test cleanup
    manager.cleanup();
    
    // Test unregistration
    manager.unregister_transaction("txn1").unwrap();
    manager.unregister_transaction("txn2").unwrap();
    
    let stats = manager.get_statistics();
    assert_eq!(stats.active_transactions, 0);
}

#[test]
fn test_error_handling() {
    let clock = Arc::new(RealClock::new());
    let manager = LockFreeConcurrencyManager::new(clock);
    
    // Test acquiring lock for non-existent transaction
    let result = manager.acquire_lock("non_existent", "doc1", LockType::Read, None);
    assert!(result.is_err());
    
    // Test unregistering non-existent transaction
    let result = manager.unregister_transaction("non_existent");
    // This should succeed (idempotent operation)
    assert!(result.is_ok());
}

#[test]
fn test_transaction_id_retrieval() {
    let clock = Arc::new(RealClock::new());
    let map = LockFreeTransactionMap::new(clock);
    
    // Register some transactions
    let transaction_ids = vec!["txn_a", "txn_b", "txn_c"];
    for txn_id in &transaction_ids {
        map.register_transaction(txn_id).unwrap();
    }
    
    // Get all transaction IDs
    let retrieved_ids = map.get_all_transaction_ids();
    
    // Should have all registered transactions
    assert_eq!(retrieved_ids.len(), transaction_ids.len());
    
    for txn_id in &transaction_ids {
        assert!(retrieved_ids.contains(&txn_id.to_string()));
    }
    
    // Clean up
    for txn_id in &transaction_ids {
        map.unregister_transaction(txn_id).unwrap();
    }
    
    // Should be empty now
    let retrieved_ids = map.get_all_transaction_ids();
    assert!(retrieved_ids.is_empty());
}

#[test]
fn test_performance_comparison() {
    use std::time::Instant;
    
    let num_operations = 10000;
    
    // Test lock-free implementation
    let clock = Arc::new(RealClock::new());
    let lock_free_map = LockFreeTransactionMap::new(clock);
    
    let start = Instant::now();
    for i in 0..num_operations {
        let txn_id = format!("perf_txn_{}", i);
        lock_free_map.register_transaction(&txn_id).unwrap();
        lock_free_map.unregister_transaction(&txn_id).unwrap();
    }
    let lock_free_duration = start.elapsed();
    
    println!("Lock-free implementation: {:?} for {} operations", 
        lock_free_duration, num_operations);
    
    // Test mutex-based implementation for comparison
    use owl_db::concurrency::ConcurrencyManager;
    
    let clock = Arc::new(RealClock::new());
    let mutex_manager = ConcurrencyManager::new(clock);
    
    let start = Instant::now();
    for i in 0..num_operations {
        let txn_id = format!("perf_txn_{}", i);
        mutex_manager.register_transaction(&txn_id).unwrap();
        mutex_manager.unregister_transaction(&txn_id).unwrap();
    }
    let mutex_duration = start.elapsed();
    
    println!("Mutex-based implementation: {:?} for {} operations", 
        mutex_duration, num_operations);
    
    // Lock-free should be faster or at least competitive
    let speedup = mutex_duration.as_nanos() as f64 / lock_free_duration.as_nanos() as f64;
    println!("Speedup: {:.2}x", speedup);
    
    // In single-threaded case, speedup might be minimal
    // The real benefits come with high concurrency
    assert!(speedup > 0.5, "Lock-free implementation should not be significantly slower");
}

#[test]
fn test_concurrent_mixed_operations() {
    let clock = Arc::new(RealClock::new());
    let map = Arc::new(LockFreeTransactionMap::new(clock));
    let num_threads = 4;
    let operations_per_thread = 250;
    
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();
    
    // Each thread performs a mix of operations
    for thread_id in 0..num_threads {
        let map_clone = map.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait(); // Synchronize start
            
            let mut operations_completed = 0;
            
            for i in 0..operations_per_thread {
                let txn_id = format!("mixed_txn_{}_{}", thread_id, i);
                
                // Register
                if map_clone.register_transaction(&txn_id).is_ok() {
                    operations_completed += 1;
                }
                
                // Check existence
                if map_clone.contains_transaction(&txn_id) {
                    operations_completed += 1;
                }
                
                // Get all IDs (expensive operation)
                if i % 10 == 0 {
                    let _all_ids = map_clone.get_all_transaction_ids();
                    operations_completed += 1;
                }
                
                // Unregister
                if map_clone.unregister_transaction(&txn_id).is_ok() {
                    operations_completed += 1;
                }
            }
            
            operations_completed
        });
        
        handles.push(handle);
    }
    
    // Wait for completion and collect results
    let mut total_operations = 0;
    for handle in handles {
        total_operations += handle.join().unwrap();
    }
    
    println!("Total operations completed: {}", total_operations);
    
    // Should have completed most operations successfully
    let expected_min = num_threads * operations_per_thread * 3; // At least 3 ops per iteration
    assert!(total_operations >= expected_min);
    
    // Map should be empty or nearly empty at the end
    println!("Final map size: {}", map.size());
}