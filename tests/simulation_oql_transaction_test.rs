// OQL Transaction Simulation Tests for Next-Gen Owl DST
//
// Tests concurrent OQL transactions with shared data, OQL transaction conflicts and rollback scenarios,
// OQL timeout scenarios with complex queries, and verifies OQL ACID properties under simulation.
//
// Requirements: 5, 7, 14

use owl_db::concurrency::{ConcurrencyManager, LockType};
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

/// Test concurrent OQL transactions with shared data
#[test]
fn test_concurrent_oql_transactions_shared_data() -> OQLResult<()> {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    // Create test storage and executor
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test_concurrent_oql.owl");
    let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
    let executor = Arc::new(QueryExecutor::new(store, sim.clock()));
    
    // Set up initial shared data
    setup_shared_test_data(&executor)?;
    
    // Create concurrency manager
    let concurrency_manager = Arc::new(ConcurrencyManager::new(sim.clock()));
    
    // Track transaction results
    let results = Arc::new(Mutex::new(Vec::new()));
    
    // Transaction 1: Update user balance and create order
    let executor1 = executor.clone();
    let manager1 = concurrency_manager.clone();
    let results1 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "balance_update_txn";
        manager1.register_transaction(txn_id).unwrap();
        
        let mut success = true;
        
        // Step 1: Read user balance
        let balance_query = parse_oql(r#"users.where { .id == "user1" }"#).unwrap();
        let user_result = executor1.execute_query(&balance_query);
        
        if let Ok(users) = user_result {
            if let Some(user) = users.first() {
                if let Some(Value::Integer(current_balance)) = user.get_field("balance") {
                    let new_balance = current_balance - 100; // Deduct 100
                    
                    if new_balance >= 0 {
                        // Step 2: Update user balance
                        let update_query = parse_oql(&format!(
                            r#"users.where {{ .id == "user1" }}.update {{ balance: {} }}"#,
                            new_balance
                        )).unwrap();
                        
                        if executor1.execute_query(&update_query).is_ok() {
                            // Step 3: Create order
                            let order_query = parse_oql(r#"orders.insert { 
                                id: "order1", 
                                user_id: "user1", 
                                amount: 100, 
                                status: "pending" 
                            }"#).unwrap();
                            
                            if executor1.execute_query(&order_query).is_err() {
                                success = false;
                            }
                        } else {
                            success = false;
                        }
                    } else {
                        success = false; // Insufficient balance
                    }
                } else {
                    success = false;
                }
            } else {
                success = false;
            }
        } else {
            success = false;
        }
        
        results1.lock().unwrap().push(("balance_update_txn", success));
        manager1.unregister_transaction(txn_id).unwrap();
    });
    
    // Transaction 2: Concurrent balance update (should conflict)
    let executor2 = executor.clone();
    let manager2 = concurrency_manager.clone();
    let results2 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "concurrent_balance_txn";
        manager2.register_transaction(txn_id).unwrap();
        
        // Small delay to create interleaving
        sim.clock().sleep(Duration::from_millis(10));
        
        let mut success = true;
        
        // Try to update the same user's balance
        let balance_query = parse_oql(r#"users.where { .id == "user1" }"#).unwrap();
        let user_result = executor2.execute_query(&balance_query);
        
        if let Ok(users) = user_result {
            if let Some(user) = users.first() {
                if let Some(Value::Integer(current_balance)) = user.get_field("balance") {
                    let new_balance = current_balance - 50; // Deduct 50
                    
                    if new_balance >= 0 {
                        let update_query = parse_oql(&format!(
                            r#"users.where {{ .id == "user1" }}.update {{ balance: {} }}"#,
                            new_balance
                        )).unwrap();
                        
                        if executor2.execute_query(&update_query).is_err() {
                            success = false; // Expected conflict
                        }
                    } else {
                        success = false;
                    }
                } else {
                    success = false;
                }
            } else {
                success = false;
            }
        } else {
            success = false;
        }
        
        results2.lock().unwrap().push(("concurrent_balance_txn", success));
        manager2.unregister_transaction(txn_id).unwrap();
    });
    
    // Transaction 3: Read-only transaction (should not conflict)
    let executor3 = executor.clone();
    let manager3 = concurrency_manager.clone();
    let results3 = results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "readonly_txn";
        manager3.register_transaction(txn_id).unwrap();
        
        // Read user data (should not conflict with writes)
        let read_query = parse_oql(r#"users.where { .id == "user1" }.select { id, name, balance }"#).unwrap();
        let success = executor3.execute_query(&read_query).is_ok();
        
        results3.lock().unwrap().push(("readonly_txn", success));
        manager3.unregister_transaction(txn_id).unwrap();
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify results
    let final_results = results.lock().unwrap();
    assert_eq!(final_results.len(), 3);
    
    // At least one transaction should succeed
    let success_count = final_results.iter().filter(|(_, success)| *success).count();
    assert!(success_count >= 1, "At least one transaction should succeed");
    
    // Read-only transaction should always succeed
    let readonly_result = final_results.iter().find(|(id, _)| id == &"readonly_txn").unwrap();
    assert!(readonly_result.1, "Read-only transaction should succeed");
    
    Ok(())
}

/// Test OQL transaction conflicts and rollback scenarios
#[test]
fn test_oql_transaction_conflicts_rollback() -> OQLResult<()> {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    // Create test storage and executor
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test_rollback.owl");
    let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
    let executor = Arc::new(QueryExecutor::new(store, sim.clock()));
    
    // Set up initial data
    setup_shared_test_data(&executor)?;
    
    // Create concurrency manager
    let concurrency_manager = Arc::new(ConcurrencyManager::new(sim.clock()));
    
    // Track rollback scenarios
    let rollback_results = Arc::new(Mutex::new(Vec::new()));
    
    // Transaction that will be rolled back due to conflict
    let executor1 = executor.clone();
    let manager1 = concurrency_manager.clone();
    let results1 = rollback_results.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "rollback_txn";
        manager1.register_transaction(txn_id).unwrap();
        
        let mut operations_completed = 0;
        let mut rollback_needed = false;
        
        // Step 1: Insert new order
        let insert_query = parse_oql(r#"orders.insert { 
            id: "temp_order", 
            user_id: "user1", 
            amount: 200, 
            status: "processing" 
        }"#).unwrap();
        
        if executor1.execute_query(&insert_query).is_ok() {
            operations_completed += 1;
            
            // Step 2: Try to update user balance (may conflict)
            let update_query = parse_oql(r#"users.where { .id == "user1" }.update { 
                balance: 800, 
                last_order: "temp_order" 
            }"#).unwrap();
            
            if executor1.execute_query(&update_query).is_ok() {
                operations_completed += 1;
                
                // Step 3: Update order status
                let status_query = parse_oql(r#"orders.where { .id == "temp_order" }.update { 
                    status: "completed" 
                }"#).unwrap();
                
                if executor1.execute_query(&status_query).is_ok() {
                    operations_completed += 1;
                } else {
                    rollback_needed = true;
                }
            } else {
                rollback_needed = true;
            }
        } else {
            rollback_needed = true;
        }
        
        // Simulate rollback if needed
        if rollback_needed {
            // In a real transaction system, this would automatically rollback
            // For simulation, we track that rollback was needed
            let _ = executor1.execute_query(&parse_oql(r#"orders.where { .id == "temp_order" }.delete"#).unwrap());
        }
        
        results1.lock().unwrap().push((operations_completed, rollback_needed));
        manager1.unregister_transaction(txn_id).unwrap();
    });
    
    // Conflicting transaction that will cause rollback
    let executor2 = executor.clone();
    let manager2 = concurrency_manager.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "conflict_txn";
        manager2.register_transaction(txn_id).unwrap();
        
        // Small delay then update the same user
        sim.clock().sleep(Duration::from_millis(15));
        
        let conflict_query = parse_oql(r#"users.where { .id == "user1" }.update { 
            balance: 900, 
            status: "premium" 
        }"#).unwrap();
        
        let _ = executor2.execute_query(&conflict_query);
        manager2.unregister_transaction(txn_id).unwrap();
    });
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify rollback behavior
    let results = rollback_results.lock().unwrap();
    assert_eq!(results.len(), 1);
    
    let (operations_completed, rollback_needed) = results[0];
    
    // Either the transaction completed successfully or rollback was needed
    if rollback_needed {
        // Verify that partial operations were detected
        assert!(operations_completed > 0, "Some operations should have completed before conflict");
        
        // Verify cleanup - temp order should not exist
        let check_query = parse_oql(r#"orders.where { .id == "temp_order" }"#).unwrap();
        let temp_orders = executor.execute_query(&check_query).unwrap();
        assert_eq!(temp_orders.len(), 0, "Temporary order should be cleaned up");
    } else {
        // Transaction completed successfully
        assert_eq!(operations_completed, 3, "All operations should complete if no conflict");
    }
    
    Ok(())
}

/// Test OQL timeout scenarios with complex queries
#[test]
fn test_oql_timeout_complex_queries() -> OQLResult<()> {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    // Create test storage and executor
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test_timeout.owl");
    let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
    let executor = Arc::new(QueryExecutor::new(store, sim.clock()));
    
    // Set up larger dataset for complex queries
    setup_large_test_dataset(&executor)?;
    
    // Create concurrency manager with short timeouts
    let concurrency_manager = Arc::new(ConcurrencyManager::new_with_timeouts(
        sim.clock(),
        Duration::from_millis(100), // Short timeout
        Duration::from_millis(5),   // Frequent checks
    ));
    
    // Track timeout results
    let timeout_results = Arc::new(Mutex::new(Vec::new()));
    
    // Long-running transaction that holds locks
    let executor1 = executor.clone();
    let manager1 = concurrency_manager.clone();
    sim.scheduler().spawn_fn(move || {
        let txn_id = "long_running_txn";
        manager1.register_transaction(txn_id).unwrap();
        
        // Acquire locks on multiple documents
        let lock_keys = vec!["users#user1", "users#user2", "orders#order1"];
        for key in &lock_keys {
            let _ = manager1.acquire_lock(txn_id, key, LockType::Write, None);
        }
        
        // Simulate complex processing
        sim.clock().sleep(Duration::from_millis(200));
        
        // Execute complex aggregation query
        let complex_query = parse_oql(r#"
            users.where { .balance > 500 }
                .select { 
                    id, 
                    name, 
                    balance,
                    order_count: count(.orders),
                    avg_order_amount: avg(.orders.amount)
                }
                .sort { .balance desc }
                .limit { 5 }
        "#).unwrap();
        
        let _ = executor1.execute_query(&complex_query);
        
        // Release locks
        for key in &lock_keys {
            let _ = manager1.release_lock(txn_id, key);
        }
        
        manager1.unregister_transaction(txn_id).unwrap();
    });
    
    // Multiple transactions that will timeout waiting for locks
    for i in 0..3 {
        let executor_i = executor.clone();
        let manager_i = concurrency_manager.clone();
        let results_i = timeout_results.clone();
        
        sim.scheduler().spawn_fn(move || {
            let txn_id = format!("timeout_txn_{}", i);
            manager_i.register_transaction(&txn_id).unwrap();
            
            // Small delay to ensure long-running transaction gets locks first
            sim.clock().sleep(Duration::from_millis(10));
            
            // Try to execute query that needs same locks
            let query = parse_oql(&format!(r#"
                users.where {{ .id == "user{}" }}
                    .update {{ 
                        last_access: "{}",
                        access_count: .access_count + 1
                    }}
            "#, (i % 2) + 1, txn_id)).unwrap();
            
            let start_time = sim.clock().now();
            let result = executor_i.execute_query(&query);
            let elapsed = sim.clock().now().duration_since(start_time);
            
            let timed_out = match result {
                Err(OQLError::ConcurrencyError { .. }) | Err(OQLError::EnhancedConcurrencyError { .. }) => true,
                _ => false,
            };
            
            results_i.lock().unwrap().push((txn_id, timed_out, elapsed));
            manager_i.unregister_transaction(&txn_id).unwrap();
        });
    }
    
    // Run simulation
    sim.scheduler().run_tasks(None);
    
    // Verify timeout behavior
    let results = timeout_results.lock().unwrap();
    assert_eq!(results.len(), 3);
    
    // All waiting transactions should timeout
    for (txn_id, timed_out, elapsed) in results.iter() {
        assert!(*timed_out, "Transaction {} should timeout", txn_id);
        assert!(*elapsed >= Duration::from_millis(100), 
            "Transaction {} should wait at least timeout duration", txn_id);
    }
    
    Ok(())
}

/// Test OQL ACID properties under simulation
#[test]
fn test_oql_acid_properties_simulation() -> OQLResult<()> {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    // Create test storage and executor
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test_acid.owl");
    let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
    let executor = Arc::new(QueryExecutor::new(store, sim.clock()));
    
    // Set up initial data for ACID testing
    setup_acid_test_data(&executor)?;
    
    // Test Atomicity: All operations in a transaction succeed or all fail
    test_atomicity(&executor, &mut sim)?;
    
    // Test Consistency: Database remains in valid state
    test_consistency(&executor, &mut sim)?;
    
    // Test Isolation: Concurrent transactions don't interfere
    test_isolation(&executor, &mut sim)?;
    
    // Test Durability: Committed changes survive crashes
    test_durability(&executor, &mut sim)?;
    
    Ok(())
}

/// Test Atomicity property
fn test_atomicity(executor: &Arc<QueryExecutor<dyn DocumentStore>>, sim: &mut Simulation) -> OQLResult<()> {
    // Create a transaction that should be atomic
    let concurrency_manager = Arc::new(ConcurrencyManager::new(sim.clock()));
    
    let executor_clone = executor.clone();
    let manager = concurrency_manager.clone();
    
    sim.scheduler().spawn_fn(move || {
        let txn_id = "atomicity_test";
        manager.register_transaction(txn_id).unwrap();
        
        let mut all_success = true;
        
        // Step 1: Transfer money between accounts
        let debit_query = parse_oql(r#"accounts.where { .id == "acc1" }.update { 
            balance: .balance - 100 
        }"#).unwrap();
        
        if executor_clone.execute_query(&debit_query).is_ok() {
            // Step 2: Credit to another account
            let credit_query = parse_oql(r#"accounts.where { .id == "acc2" }.update { 
                balance: .balance + 100 
            }"#).unwrap();
            
            if executor_clone.execute_query(&credit_query).is_err() {
                // If credit fails, we need to rollback debit
                all_success = false;
                
                // Rollback debit
                let rollback_query = parse_oql(r#"accounts.where { .id == "acc1" }.update { 
                    balance: .balance + 100 
                }"#).unwrap();
                let _ = executor_clone.execute_query(&rollback_query);
            }
        } else {
            all_success = false;
        }
        
        // Step 3: Log transaction
        if all_success {
            let log_query = parse_oql(r#"transaction_log.insert { 
                id: "tx_001", 
                type: "transfer", 
                amount: 100, 
                status: "completed" 
            }"#).unwrap();
            
            if executor_clone.execute_query(&log_query).is_err() {
                // If logging fails, rollback everything
                let _ = executor_clone.execute_query(&parse_oql(r#"accounts.where { .id == "acc1" }.update { 
                    balance: .balance + 100 
                }"#).unwrap());
                let _ = executor_clone.execute_query(&parse_oql(r#"accounts.where { .id == "acc2" }.update { 
                    balance: .balance - 100 
                }"#).unwrap());
            }
        }
        
        manager.unregister_transaction(txn_id).unwrap();
    });
    
    sim.scheduler().run_tasks(None);
    
    // Verify atomicity: Either all operations completed or none did
    let acc1_query = parse_oql(r#"accounts.where { .id == "acc1" }"#)?;
    let acc2_query = parse_oql(r#"accounts.where { .id == "acc2" }"#)?;
    let log_query = parse_oql(r#"transaction_log.where { .id == "tx_001" }"#)?;
    
    let acc1_result = executor.execute_query(&acc1_query)?;
    let acc2_result = executor.execute_query(&acc2_query)?;
    let log_result = executor.execute_query(&log_query)?;
    
    if !log_result.is_empty() {
        // Transaction completed - verify both accounts updated
        let acc1_balance = acc1_result[0].get_field("balance").unwrap();
        let acc2_balance = acc2_result[0].get_field("balance").unwrap();
        
        // Assuming initial balances were 1000 each
        assert_eq!(acc1_balance, &Value::Integer(900));
        assert_eq!(acc2_balance, &Value::Integer(1100));
    }
    
    Ok(())
}

/// Test Consistency property
fn test_consistency(executor: &Arc<QueryExecutor<dyn DocumentStore>>, sim: &mut Simulation) -> OQLResult<()> {
    // Verify that business rules are maintained
    let total_query = parse_oql(r#"accounts.select { total: sum(.balance) }"#)?;
    let initial_total = executor.execute_query(&total_query)?;
    
    // Perform multiple transactions
    for i in 0..5 {
        let transfer_query = parse_oql(&format!(r#"
            accounts.where {{ .id == "acc1" }}.update {{ balance: .balance - {} }}
        "#, i * 10))?;
        
        let _ = executor.execute_query(&transfer_query);
        
        let receive_query = parse_oql(&format!(r#"
            accounts.where {{ .id == "acc2" }}.update {{ balance: .balance + {} }}
        "#, i * 10))?;
        
        let _ = executor.execute_query(&receive_query);
    }
    
    // Verify total balance remains consistent
    let final_total = executor.execute_query(&total_query)?;
    
    if let (Some(Value::Integer(initial)), Some(Value::Integer(final_val))) = (
        initial_total[0].get_field("total"),
        final_total[0].get_field("total")
    ) {
        assert_eq!(initial, final_val, "Total balance should remain consistent");
    }
    
    Ok(())
}

/// Test Isolation property
fn test_isolation(executor: &Arc<QueryExecutor<dyn DocumentStore>>, sim: &mut Simulation) -> OQLResult<()> {
    let concurrency_manager = Arc::new(ConcurrencyManager::new(sim.clock()));
    let results = Arc::new(Mutex::new(Vec::new()));
    
    // Two concurrent transactions reading and updating the same data
    for i in 0..2 {
        let executor_clone = executor.clone();
        let manager = concurrency_manager.clone();
        let results_clone = results.clone();
        
        sim.scheduler().spawn_fn(move || {
            let txn_id = format!("isolation_txn_{}", i);
            manager.register_transaction(&txn_id).unwrap();
            
            // Read current balance
            let read_query = parse_oql(r#"accounts.where { .id == "acc1" }"#).unwrap();
            if let Ok(accounts) = executor_clone.execute_query(&read_query) {
                if let Some(account) = accounts.first() {
                    if let Some(Value::Integer(balance)) = account.get_field("balance") {
                        // Small processing delay
                        sim.clock().sleep(Duration::from_millis(20));
                        
                        // Update based on read value
                        let new_balance = balance + (i as i64 * 50);
                        let update_query = parse_oql(&format!(r#"
                            accounts.where {{ .id == "acc1" }}.update {{ balance: {} }}
                        "#, new_balance)).unwrap();
                        
                        let success = executor_clone.execute_query(&update_query).is_ok();
                        results_clone.lock().unwrap().push((txn_id, *balance, new_balance, success));
                    }
                }
            }
            
            manager.unregister_transaction(&txn_id).unwrap();
        });
    }
    
    sim.scheduler().run_tasks(None);
    
    // Verify isolation - transactions should see consistent snapshots
    let final_results = results.lock().unwrap();
    assert_eq!(final_results.len(), 2);
    
    // At least one transaction should succeed
    let success_count = final_results.iter().filter(|(_, _, _, success)| *success).count();
    assert!(success_count >= 1, "At least one transaction should succeed");
    
    Ok(())
}

/// Test Durability property
fn test_durability(executor: &Arc<QueryExecutor<dyn DocumentStore>>, sim: &mut Simulation) -> OQLResult<()> {
    // Perform a transaction and commit
    let commit_query = parse_oql(r#"accounts.where { .id == "acc1" }.update { 
        balance: 2000, 
        last_update: "durability_test" 
    }"#)?;
    
    executor.execute_query(&commit_query)?;
    
    // Sync to ensure durability
    sim.filesystem().sync_all()?;
    
    // Simulate crash
    sim.filesystem().crash();
    sim.filesystem().recover();
    
    // Verify data survived crash
    let verify_query = parse_oql(r#"accounts.where { .id == "acc1" }"#)?;
    let recovered_data = executor.execute_query(&verify_query)?;
    
    assert!(!recovered_data.is_empty(), "Data should survive crash");
    
    let account = &recovered_data[0];
    assert_eq!(account.get_field("balance"), Some(&Value::Integer(2000)));
    assert_eq!(account.get_field("last_update"), Some(&Value::String("durability_test".to_string())));
    
    Ok(())
}

/// Helper function to set up shared test data
fn setup_shared_test_data(executor: &QueryExecutor<dyn DocumentStore>) -> OQLResult<()> {
    // Create users
    let user_queries = vec![
        r#"users.insert { id: "user1", name: "Alice", balance: 1000, status: "active" }"#,
        r#"users.insert { id: "user2", name: "Bob", balance: 500, status: "active" }"#,
    ];
    
    for query_str in user_queries {
        let query = parse_oql(query_str)?;
        executor.execute_query(&query)?;
    }
    
    // Create initial orders
    let order_queries = vec![
        r#"orders.insert { id: "order1", user_id: "user1", amount: 150, status: "completed" }"#,
        r#"orders.insert { id: "order2", user_id: "user2", amount: 75, status: "pending" }"#,
    ];
    
    for query_str in order_queries {
        let query = parse_oql(query_str)?;
        executor.execute_query(&query)?;
    }
    
    Ok(())
}

/// Helper function to set up larger test dataset
fn setup_large_test_dataset(executor: &QueryExecutor<dyn DocumentStore>) -> OQLResult<()> {
    // Create multiple users
    for i in 1..=10 {
        let query = parse_oql(&format!(r#"users.insert {{ 
            id: "user{}", 
            name: "User{}", 
            balance: {}, 
            access_count: 0,
            status: "{}" 
        }}"#, i, i, 500 + i * 100, if i % 2 == 0 { "premium" } else { "standard" }))?;
        
        executor.execute_query(&query)?;
    }
    
    // Create multiple orders
    for i in 1..=20 {
        let user_id = ((i - 1) % 10) + 1;
        let query = parse_oql(&format!(r#"orders.insert {{ 
            id: "order{}", 
            user_id: "user{}", 
            amount: {}, 
            status: "{}" 
        }}"#, i, user_id, 50 + i * 25, if i % 3 == 0 { "completed" } else { "pending" }))?;
        
        executor.execute_query(&query)?;
    }
    
    Ok(())
}

/// Helper function to set up ACID test data
fn setup_acid_test_data(executor: &QueryExecutor<dyn DocumentStore>) -> OQLResult<()> {
    // Create accounts for ACID testing
    let account_queries = vec![
        r#"accounts.insert { id: "acc1", name: "Account 1", balance: 1000, type: "checking" }"#,
        r#"accounts.insert { id: "acc2", name: "Account 2", balance: 1000, type: "savings" }"#,
    ];
    
    for query_str in account_queries {
        let query = parse_oql(query_str)?;
        executor.execute_query(&query)?;
    }
    
    Ok(())
}