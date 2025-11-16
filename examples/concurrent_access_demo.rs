// Demonstration of concurrent access control features
// Shows optimistic locking, deadlock detection, and multi-client operations

use owl_db::concurrency::{ConcurrencyManager, LockType};
use owl_db::document::{Document, Value};
use owl_db::transaction::{IsolationLevel, TransactionManager};
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn create_sample_document(id: &str, name: &str, balance: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("balance".to_string(), Value::Integer(balance));
    
    Document::with_id(id.to_string(), "accounts".to_string(), fields)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OQL Database Concurrent Access Control Demo ===\n");

    // Demo 1: Basic Optimistic Locking
    demo_optimistic_locking()?;
    
    // Demo 2: Lock Compatibility and Conflicts
    demo_lock_compatibility()?;
    
    // Demo 3: Deadlock Detection
    demo_deadlock_detection()?;
    
    // Demo 4: Multi-Client Bank Transfer Simulation
    demo_bank_transfers()?;
    
    // Demo 5: Concurrency Statistics
    demo_concurrency_statistics()?;

    println!("\n=== Demo Complete ===");
    Ok(())
}

fn demo_optimistic_locking() -> Result<(), Box<dyn std::error::Error>> {
    println!("1. Optimistic Locking Demo");
    println!("   Testing version-based conflict detection...\n");

    let manager = ConcurrencyManager::new();
    let doc_key = "account_123";
    let txn_id = "transaction_1";

    manager.register_transaction(txn_id)?;

    // Simulate successful version validation
    println!("   ✓ Validating version 5 against document version 5...");
    manager.validate_version(doc_key, 5, 5, txn_id)?;
    println!("     Success: Versions match");

    // Simulate version conflict
    println!("   ✗ Validating version 5 against document version 7...");
    match manager.validate_version(doc_key, 5, 7, txn_id) {
        Ok(_) => println!("     Unexpected success"),
        Err(e) => println!("     Expected conflict: {}", e.user_message()),
    }

    manager.unregister_transaction(txn_id)?;
    println!("   Optimistic locking validation complete.\n");
    Ok(())
}

fn demo_lock_compatibility() -> Result<(), Box<dyn std::error::Error>> {
    println!("2. Lock Compatibility Demo");
    println!("   Testing read/write lock interactions...\n");

    let manager = ConcurrencyManager::new();
    let doc_key = "shared_document";

    // Register multiple transactions
    manager.register_transaction("reader1")?;
    manager.register_transaction("reader2")?;
    manager.register_transaction("writer1")?;

    // Multiple readers can coexist
    println!("   ✓ Acquiring read lock for reader1...");
    manager.acquire_lock("reader1", doc_key, LockType::Read, None)?;
    println!("     Success: Read lock acquired");

    println!("   ✓ Acquiring read lock for reader2...");
    manager.acquire_lock("reader2", doc_key, LockType::Read, None)?;
    println!("     Success: Multiple read locks allowed");

    // Writer should conflict with existing readers
    println!("   ✗ Attempting write lock while readers active...");
    let start_time = std::time::Instant::now();
    match manager.acquire_lock("writer1", doc_key, LockType::Write, None) {
        Ok(_) => println!("     Unexpected success"),
        Err(e) => {
            let elapsed = start_time.elapsed();
            println!("     Expected conflict after {:?}: {}", elapsed, e.user_message());
        }
    }

    // Release read locks
    println!("   ✓ Releasing read locks...");
    manager.release_lock("reader1", doc_key)?;
    manager.release_lock("reader2", doc_key)?;
    println!("     Read locks released");

    // Now writer should succeed
    println!("   ✓ Acquiring write lock after readers released...");
    manager.acquire_lock("writer1", doc_key, LockType::Write, None)?;
    println!("     Success: Write lock acquired");

    // Cleanup
    manager.unregister_transaction("reader1")?;
    manager.unregister_transaction("reader2")?;
    manager.unregister_transaction("writer1")?;
    
    println!("   Lock compatibility test complete.\n");
    Ok(())
}

fn demo_deadlock_detection() -> Result<(), Box<dyn std::error::Error>> {
    println!("3. Deadlock Detection Demo");
    println!("   Simulating circular wait condition...\n");

    let manager = Arc::new(ConcurrencyManager::new());
    
    manager.register_transaction("txn_A")?;
    manager.register_transaction("txn_B")?;

    // Create initial locks to set up deadlock scenario
    println!("   ✓ Transaction A locks Document 1");
    manager.acquire_lock("txn_A", "document_1", LockType::Write, None)?;
    
    println!("   ✓ Transaction B locks Document 2");
    manager.acquire_lock("txn_B", "document_2", LockType::Write, None)?;

    let manager_a = manager.clone();
    let manager_b = manager.clone();
    let barrier = Arc::new(Barrier::new(2));
    let barrier_a = barrier.clone();
    let barrier_b = barrier.clone();

    println!("   ⚠ Creating circular dependency...");

    // Spawn threads to create deadlock
    let handle_a = thread::spawn(move || {
        barrier_a.wait();
        println!("     Transaction A attempting to lock Document 2...");
        manager_a.acquire_lock("txn_A", "document_2", LockType::Write, None)
    });

    let handle_b = thread::spawn(move || {
        barrier_b.wait();
        println!("     Transaction B attempting to lock Document 1...");
        manager_b.acquire_lock("txn_B", "document_1", LockType::Write, None)
    });

    // Wait for results
    let result_a = handle_a.join().unwrap();
    let result_b = handle_b.join().unwrap();

    // At least one should fail due to deadlock detection or timeout
    let deadlock_detected = result_a.is_err() || result_b.is_err();
    
    if deadlock_detected {
        println!("   ✓ Deadlock detected and resolved");
        if let Err(e) = result_a {
            println!("     Transaction A aborted: {}", e.user_message());
        }
        if let Err(e) = result_b {
            println!("     Transaction B aborted: {}", e.user_message());
        }
    } else {
        println!("   ⚠ No deadlock detected (unexpected)");
    }

    manager.unregister_transaction("txn_A")?;
    manager.unregister_transaction("txn_B")?;
    
    println!("   Deadlock detection test complete.\n");
    Ok(())
}

fn demo_bank_transfers() -> Result<(), Box<dyn std::error::Error>> {
    println!("4. Multi-Client Bank Transfer Simulation");
    println!("   Simulating concurrent money transfers with ACID properties...\n");

    let temp_dir = TempDir::new()?;
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap())?);

    // Create initial accounts
    let accounts = vec![
        ("alice", "Alice Smith", 1000),
        ("bob", "Bob Johnson", 1500),
        ("charlie", "Charlie Brown", 800),
    ];

    println!("   Setting up initial accounts:");
    for (id, name, balance) in &accounts {
        let doc = create_sample_document(id, name, *balance);
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead)?;
        manager.insert_document_in_transaction(&txn_id, doc)?;
        manager.commit_transaction(&txn_id)?;
        println!("     {} ({}): ${}", name, id, balance);
    }

    let num_clients = 3;
    let transfers_per_client = 5;
    let barrier = Arc::new(Barrier::new(num_clients));
    let mut handles = Vec::new();

    println!("\n   Starting {} concurrent clients, {} transfers each...", num_clients, transfers_per_client);

    // Spawn client threads
    for client_id in 0..num_clients {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let accounts_clone = accounts.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_transfers = 0;
            let mut failed_transfers = 0;
            
            for transfer_id in 0..transfers_per_client {
                // Pick random accounts for transfer
                let from_idx = (client_id + transfer_id) % accounts_clone.len();
                let to_idx = (from_idx + 1) % accounts_clone.len();
                let from_account = &accounts_clone[from_idx].0;
                let to_account = &accounts_clone[to_idx].0;
                let amount = 50; // Transfer $50

                let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                
                let result = (|| -> Result<(), owl_db::error::OQLError> {
                    // Read both accounts
                    let from_doc = manager_clone.get_document_in_transaction(
                        &txn_id, "accounts", &from_account.to_string()
                    )?.ok_or_else(|| owl_db::error::OQLError::Generic {
                        message: "From account not found".to_string()
                    })?;
                    
                    let to_doc = manager_clone.get_document_in_transaction(
                        &txn_id, "accounts", &to_account.to_string()
                    )?.ok_or_else(|| owl_db::error::OQLError::Generic {
                        message: "To account not found".to_string()
                    })?;

                    // Check balance
                    let from_balance = match from_doc.get_field("balance") {
                        Some(Value::Integer(b)) => *b,
                        _ => return Err(owl_db::error::OQLError::Generic {
                            message: "Invalid balance".to_string()
                        }),
                    };

                    if from_balance < amount {
                        return Err(owl_db::error::OQLError::Generic {
                            message: "Insufficient funds".to_string()
                        });
                    }

                    let to_balance = match to_doc.get_field("balance") {
                        Some(Value::Integer(b)) => *b,
                        _ => return Err(owl_db::error::OQLError::Generic {
                            message: "Invalid balance".to_string()
                        }),
                    };

                    // Update balances
                    let mut updated_from = from_doc.clone();
                    updated_from.set_field("balance".to_string(), Value::Integer(from_balance - amount));
                    
                    let mut updated_to = to_doc.clone();
                    updated_to.set_field("balance".to_string(), Value::Integer(to_balance + amount));

                    // Perform updates with optimistic locking
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "accounts",
                        &from_account.to_string(),
                        updated_from,
                        Some(from_doc.version),
                    )?;

                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "accounts", 
                        &to_account.to_string(),
                        updated_to,
                        Some(to_doc.version),
                    )?;

                    manager_clone.commit_transaction(&txn_id)?;
                    Ok(())
                })();

                match result {
                    Ok(_) => {
                        successful_transfers += 1;
                        println!("     Client {} transfer {}: ${} from {} to {} ✓", 
                                client_id, transfer_id, amount, from_account, to_account);
                    }
                    Err(e) => {
                        failed_transfers += 1;
                        let _ = manager_clone.rollback_transaction(&txn_id);
                        println!("     Client {} transfer {}: Failed - {}", 
                                client_id, transfer_id, e.user_message());
                    }
                }

                // Small delay between transfers
                thread::sleep(Duration::from_millis(50));
            }
            
            (successful_transfers, failed_transfers)
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

    println!("\n   Transfer Summary:");
    println!("     Successful: {}", total_successful);
    println!("     Failed: {}", total_failed);
    println!("     Total: {}", total_successful + total_failed);

    // Check final balances
    println!("\n   Final account balances:");
    let mut total_balance = 0;
    for (id, name, _) in &accounts {
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead)?;
        if let Some(doc) = manager.get_document_in_transaction(&txn_id, "accounts", &id.to_string())? {
            if let Some(Value::Integer(balance)) = doc.get_field("balance") {
                println!("     {} ({}): ${}", name, id, balance);
                total_balance += balance;
            }
        }
        manager.commit_transaction(&txn_id)?;
    }
    
    let initial_total: i64 = accounts.iter().map(|(_, _, balance)| balance).sum();
    println!("     Total balance: ${} (initial: ${})", total_balance, initial_total);
    
    if total_balance == initial_total {
        println!("   ✓ Money conservation verified - no funds lost or created");
    } else {
        println!("   ✗ Money conservation violated!");
    }

    println!("   Bank transfer simulation complete.\n");
    Ok(())
}

fn demo_concurrency_statistics() -> Result<(), Box<dyn std::error::Error>> {
    println!("5. Concurrency Statistics Demo");
    println!("   Monitoring system concurrency metrics...\n");

    let manager = ConcurrencyManager::new();

    // Initial state
    let stats = manager.get_statistics()?;
    println!("   Initial state:");
    println!("     Active transactions: {}", stats.active_transactions);
    println!("     Total locks: {}", stats.total_locks);
    println!("     Locked documents: {}", stats.locked_documents);
    println!("     Waiting transactions: {}", stats.waiting_transactions);

    // Register transactions and acquire locks
    println!("\n   Registering transactions and acquiring locks...");
    
    manager.register_transaction("stats_txn_1")?;
    manager.register_transaction("stats_txn_2")?;
    manager.register_transaction("stats_txn_3")?;

    manager.acquire_lock("stats_txn_1", "doc_A", LockType::Read, None)?;
    manager.acquire_lock("stats_txn_1", "doc_B", LockType::Read, None)?;
    manager.acquire_lock("stats_txn_2", "doc_C", LockType::Write, None)?;
    manager.acquire_lock("stats_txn_3", "doc_A", LockType::Read, None)?; // Shared read lock

    let stats = manager.get_statistics()?;
    println!("   After acquiring locks:");
    println!("     Active transactions: {}", stats.active_transactions);
    println!("     Total locks: {}", stats.total_locks);
    println!("     Locked documents: {}", stats.locked_documents);
    println!("     Waiting transactions: {}", stats.waiting_transactions);

    // Show lock details
    println!("\n   Lock details:");
    for doc in ["doc_A", "doc_B", "doc_C"] {
        let lock_info = manager.get_lock_info(doc)?;
        if !lock_info.is_empty() {
            println!("     {}: {} lock(s)", doc, lock_info.len());
            for lock in &lock_info {
                println!("       - {} ({:?})", lock.transaction_id, lock.lock_type);
            }
        }
    }

    // Cleanup
    println!("\n   Cleaning up transactions...");
    manager.unregister_transaction("stats_txn_1")?;
    manager.unregister_transaction("stats_txn_2")?;
    manager.unregister_transaction("stats_txn_3")?;

    let stats = manager.get_statistics()?;
    println!("   After cleanup:");
    println!("     Active transactions: {}", stats.active_transactions);
    println!("     Total locks: {}", stats.total_locks);
    println!("     Locked documents: {}", stats.locked_documents);
    println!("     Waiting transactions: {}", stats.waiting_transactions);

    println!("   Concurrency statistics demo complete.\n");
    Ok(())
}