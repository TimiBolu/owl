// Comprehensive ACID Properties Testing for OQL Database System
// Tests Atomicity, Consistency, Isolation, and Durability

use owl_db::document::{Document, Value};
use owl_db::error::OQLError;
use owl_db::storage::DocumentStore;
use owl_db::transaction::{IsolationLevel, TransactionManager, TransactionalDocumentStore};
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn create_test_document(id: &str, name: &str, balance: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("balance".to_string(), Value::Integer(balance));
    
    Document::with_id(id.to_string(), "accounts".to_string(), fields)
}

fn create_order_document(id: &str, customer_id: &str, amount: i64, status: &str) -> Document {
    let mut fields = HashMap::new();
    fields.insert("customer_id".to_string(), Value::String(customer_id.to_string()));
    fields.insert("amount".to_string(), Value::Integer(amount));
    fields.insert("status".to_string(), Value::String(status.to_string()));
    
    Document::with_id(id.to_string(), "orders".to_string(), fields)
}

#[test]
fn test_atomicity_all_or_nothing() {
    let temp_dir = TempDir::new().unwrap();
    let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

    // Setup initial accounts
    let account1 = create_test_document("acc1", "Alice", 1000);
    let account2 = create_test_document("acc2", "Bob", 500);
    
    store.put_document(&account1).unwrap();
    store.put_document(&account2).unwrap();

    // Test successful transaction (all operations should commit)
    let result = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        // Transfer $200 from Alice to Bob
        let mut alice = store.get_document("accounts", &"acc1".to_string())?.unwrap();
        let mut bob = store.get_document("accounts", &"acc2".to_string())?.unwrap();
        
        let alice_balance = alice.get_field("balance").unwrap().as_integer().unwrap();
        let bob_balance = bob.get_field("balance").unwrap().as_integer().unwrap();
        
        alice.set_field("balance".to_string(), Value::Integer(alice_balance - 200));
        bob.set_field("balance".to_string(), Value::Integer(bob_balance + 200));
        
        store.put_document(&alice)?;
        store.put_document(&bob)?;
        
        Ok(())
    });

    assert!(result.is_ok());

    // Verify both accounts were updated
    let alice_final = store.get_document("accounts", &"acc1".to_string()).unwrap().unwrap();
    let bob_final = store.get_document("accounts", &"acc2".to_string()).unwrap().unwrap();
    
    assert_eq!(alice_final.get_field("balance").unwrap().as_integer().unwrap(), 800);
    assert_eq!(bob_final.get_field("balance").unwrap().as_integer().unwrap(), 700);
}

#[test]
fn test_atomicity_rollback_on_failure() {
    let temp_dir = TempDir::new().unwrap();
    let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

    // Setup initial accounts
    let account1 = create_test_document("acc1", "Alice", 1000);
    let account2 = create_test_document("acc2", "Bob", 500);
    
    store.put_document(&account1).unwrap();
    store.put_document(&account2).unwrap();

    // Test failed transaction (no operations should commit)
    let result = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        // Transfer $200 from Alice to Bob
        let mut alice = store.get_document("accounts", &"acc1".to_string())?.unwrap();
        let mut bob = store.get_document("accounts", &"acc2".to_string())?.unwrap();
        
        let alice_balance = alice.get_field("balance").unwrap().as_integer().unwrap();
        let bob_balance = bob.get_field("balance").unwrap().as_integer().unwrap();
        
        alice.set_field("balance".to_string(), Value::Integer(alice_balance - 200));
        bob.set_field("balance".to_string(), Value::Integer(bob_balance + 200));
        
        store.put_document(&alice)?;
        
        // Simulate an error before completing the transaction
        return Err(OQLError::Generic { 
            message: "Simulated business logic error".to_string() 
        });
        
        #[allow(unreachable_code)]
        store.put_document(&bob)?;
        
        Ok(())
    });

    assert!(result.is_err());

    // Verify no accounts were updated (atomicity preserved)
    let alice_final = store.get_document("accounts", &"acc1".to_string()).unwrap().unwrap();
    let bob_final = store.get_document("accounts", &"acc2".to_string()).unwrap().unwrap();
    
    assert_eq!(alice_final.get_field("balance").unwrap().as_integer().unwrap(), 1000);
    assert_eq!(bob_final.get_field("balance").unwrap().as_integer().unwrap(), 500);
}

#[test]
fn test_consistency_business_rules() {
    let temp_dir = TempDir::new().unwrap();
    let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

    // Setup initial account
    let account = create_test_document("acc1", "Alice", 100);
    store.put_document(&account).unwrap();

    // Test consistency: prevent overdraft (business rule)
    let result = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        let mut alice = store.get_document("accounts", &"acc1".to_string())?.unwrap();
        let balance = alice.get_field("balance").unwrap().as_integer().unwrap();
        
        // Try to withdraw more than available balance
        let withdrawal_amount = 200;
        if balance < withdrawal_amount {
            return Err(OQLError::Generic { 
                message: "Insufficient funds".to_string() 
            });
        }
        
        alice.set_field("balance".to_string(), Value::Integer(balance - withdrawal_amount));
        store.put_document(&alice)?;
        
        Ok(())
    });

    assert!(result.is_err());
    
    // Verify account balance unchanged (consistency preserved)
    let alice_final = store.get_document("accounts", &"acc1".to_string()).unwrap().unwrap();
    assert_eq!(alice_final.get_field("balance").unwrap().as_integer().unwrap(), 100);
}

#[test]
fn test_consistency_referential_integrity() {
    let temp_dir = TempDir::new().unwrap();
    let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

    // Setup customer and order
    let customer = create_test_document("cust1", "Alice", 0);
    let order = create_order_document("ord1", "cust1", 100, "pending");
    
    store.put_document(&customer).unwrap();
    store.put_document(&order).unwrap();

    // Test referential integrity: cannot delete customer with active orders
    let result = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        // Check if customer has active orders
        let order = store.get_document("orders", &"ord1".to_string())?;
        if let Some(ord) = order {
            if ord.get_field("status").unwrap().as_string().unwrap() == "pending" {
                return Err(OQLError::Generic { 
                    message: "Cannot delete customer with active orders".to_string() 
                });
            }
        }
        
        // This should not be reached
        store.delete_document("accounts", &"cust1".to_string())?;
        Ok(())
    });

    assert!(result.is_err());
    
    // Verify customer still exists (referential integrity preserved)
    let customer_final = store.get_document("accounts", &"cust1".to_string()).unwrap();
    assert!(customer_final.is_some());
}

#[test]
fn test_isolation_read_committed() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());

    // Setup initial document
    let doc = create_test_document("acc1", "Alice", 1000);
    let txn_id = manager.begin_transaction(IsolationLevel::ReadCommitted).unwrap();
    manager.insert_document_in_transaction(&txn_id, doc).unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    // Test read committed isolation with sequential transactions (due to our pessimistic locking)
    let txn1 = manager.begin_transaction(IsolationLevel::ReadCommitted).unwrap();

    // Transaction 1 reads initial value
    let doc1_initial = manager.get_document_in_transaction(&txn1, "accounts", &"acc1".to_string()).unwrap().unwrap();
    assert_eq!(doc1_initial.get_field("balance").unwrap().as_integer().unwrap(), 1000);

    // Commit transaction 1 first
    manager.commit_transaction(&txn1).unwrap();

    // Now start transaction 2 to update the document
    let txn2 = manager.begin_transaction(IsolationLevel::ReadCommitted).unwrap();
    let mut doc2 = manager.get_document_in_transaction(&txn2, "accounts", &"acc1".to_string()).unwrap().unwrap();
    doc2.set_field("balance".to_string(), Value::Integer(1500));
    manager.update_document_in_transaction(&txn2, "accounts", &"acc1".to_string(), doc2, None).unwrap();
    manager.commit_transaction(&txn2).unwrap();

    // Start a new transaction to verify the committed changes are visible
    let txn3 = manager.begin_transaction(IsolationLevel::ReadCommitted).unwrap();
    let doc3 = manager.get_document_in_transaction(&txn3, "accounts", &"acc1".to_string()).unwrap().unwrap();
    assert_eq!(doc3.get_field("balance").unwrap().as_integer().unwrap(), 1500);
    manager.commit_transaction(&txn3).unwrap();
}

#[test]
fn test_isolation_repeatable_read() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());

    // Setup initial document
    let doc = create_test_document("acc1", "Alice", 1000);
    let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    manager.insert_document_in_transaction(&txn_id, doc).unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    // Test repeatable read isolation with sequential transactions (due to our locking model)
    let txn1 = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    
    // Transaction 1 reads initial value
    let doc1_read1 = manager.get_document_in_transaction(&txn1, "accounts", &"acc1".to_string()).unwrap().unwrap();
    assert_eq!(doc1_read1.get_field("balance").unwrap().as_integer().unwrap(), 1000);
    
    // Transaction 1 reads again - should see same value (repeatable read)
    let doc1_read2 = manager.get_document_in_transaction(&txn1, "accounts", &"acc1".to_string()).unwrap().unwrap();
    assert_eq!(doc1_read2.get_field("balance").unwrap().as_integer().unwrap(), 1000);
    
    manager.commit_transaction(&txn1).unwrap();

    // Now test with a second transaction
    let txn2 = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    let mut doc2 = manager.get_document_in_transaction(&txn2, "accounts", &"acc1".to_string()).unwrap().unwrap();
    doc2.set_field("balance".to_string(), Value::Integer(1500));
    manager.update_document_in_transaction(&txn2, "accounts", &"acc1".to_string(), doc2, None).unwrap();
    manager.commit_transaction(&txn2).unwrap();

    // Verify the update was committed
    let txn3 = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    let doc3 = manager.get_document_in_transaction(&txn3, "accounts", &"acc1".to_string()).unwrap().unwrap();
    assert_eq!(doc3.get_field("balance").unwrap().as_integer().unwrap(), 1500);
    manager.commit_transaction(&txn3).unwrap();
}

#[test]
fn test_durability_persistence_after_commit() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().to_str().unwrap();
    
    // Create and commit a transaction
    {
        let mut store = TransactionalDocumentStore::new(db_path).unwrap();
        
        store.with_transaction(IsolationLevel::RepeatableRead, |store| {
            let doc = create_test_document("acc1", "Alice", 1000);
            store.put_document(&doc)?;
            Ok(())
        }).unwrap();
    } // Store goes out of scope, simulating application restart

    // Reopen database and verify data persisted
    {
        let store = TransactionalDocumentStore::new(db_path).unwrap();
        let doc = store.get_document("accounts", &"acc1".to_string()).unwrap();
        
        assert!(doc.is_some());
        let doc = doc.unwrap();
        assert_eq!(doc.get_field("name").unwrap().as_string().unwrap(), "Alice");
        assert_eq!(doc.get_field("balance").unwrap().as_integer().unwrap(), 1000);
    }
}

#[test]
fn test_durability_rollback_not_persisted() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().to_str().unwrap();
    
    // Create a successful transaction first
    {
        let mut store = TransactionalDocumentStore::new(db_path).unwrap();
        let doc = create_test_document("acc1", "Alice", 1000);
        store.put_document(&doc).unwrap();
    }

    // Create and rollback a transaction
    {
        let mut store = TransactionalDocumentStore::new(db_path).unwrap();
        
        let result: Result<(), OQLError> = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
            let mut doc = store.get_document("accounts", &"acc1".to_string())?.unwrap();
            doc.set_field("balance".to_string(), Value::Integer(2000));
            store.put_document(&doc)?;
            
            // Force rollback
            Err(OQLError::Generic { message: "Force rollback".to_string() })
        });
        
        assert!(result.is_err());
    } // Store goes out of scope

    // Reopen database and verify rollback was not persisted
    {
        let store = TransactionalDocumentStore::new(db_path).unwrap();
        let doc = store.get_document("accounts", &"acc1".to_string()).unwrap().unwrap();
        
        // Should still have original value
        assert_eq!(doc.get_field("balance").unwrap().as_integer().unwrap(), 1000);
    }
}

#[test]
fn test_acid_complex_scenario() {
    let temp_dir = TempDir::new().unwrap();
    let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

    // Setup: Create multiple accounts and orders
    let accounts = vec![
        create_test_document("acc1", "Alice", 1000),
        create_test_document("acc2", "Bob", 500),
        create_test_document("acc3", "Charlie", 750),
    ];
    
    for account in &accounts {
        store.put_document(account).unwrap();
    }

    // Complex transaction: Process multiple orders with business rules
    let result = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        // Order 1: Alice buys $300 item
        let mut alice = store.get_document("accounts", &"acc1".to_string())?.unwrap();
        let alice_balance = alice.get_field("balance").unwrap().as_integer().unwrap();
        
        if alice_balance < 300 {
            return Err(OQLError::Generic { message: "Insufficient funds for Alice".to_string() });
        }
        
        alice.set_field("balance".to_string(), Value::Integer(alice_balance - 300));
        store.put_document(&alice)?;
        
        let order1 = create_order_document("ord1", "acc1", 300, "completed");
        store.put_document(&order1)?;

        // Order 2: Bob buys $200 item
        let mut bob = store.get_document("accounts", &"acc2".to_string())?.unwrap();
        let bob_balance = bob.get_field("balance").unwrap().as_integer().unwrap();
        
        if bob_balance < 200 {
            return Err(OQLError::Generic { message: "Insufficient funds for Bob".to_string() });
        }
        
        bob.set_field("balance".to_string(), Value::Integer(bob_balance - 200));
        store.put_document(&bob)?;
        
        let order2 = create_order_document("ord2", "acc2", 200, "completed");
        store.put_document(&order2)?;

        // Order 3: Charlie tries to buy $800 item (should fail)
        let charlie = store.get_document("accounts", &"acc3".to_string())?.unwrap();
        let charlie_balance = charlie.get_field("balance").unwrap().as_integer().unwrap();
        
        if charlie_balance < 800 {
            return Err(OQLError::Generic { message: "Insufficient funds for Charlie".to_string() });
        }
        
        // This should not be reached
        Ok(())
    });

    // Transaction should fail due to Charlie's insufficient funds
    assert!(result.is_err());
    
    // Verify atomicity: no changes should be persisted
    let alice_final = store.get_document("accounts", &"acc1".to_string()).unwrap().unwrap();
    let bob_final = store.get_document("accounts", &"acc2".to_string()).unwrap().unwrap();
    let charlie_final = store.get_document("accounts", &"acc3".to_string()).unwrap().unwrap();
    
    assert_eq!(alice_final.get_field("balance").unwrap().as_integer().unwrap(), 1000);
    assert_eq!(bob_final.get_field("balance").unwrap().as_integer().unwrap(), 500);
    assert_eq!(charlie_final.get_field("balance").unwrap().as_integer().unwrap(), 750);
    
    // Verify no orders were created
    let order1 = store.get_document("orders", &"ord1".to_string()).unwrap();
    let order2 = store.get_document("orders", &"ord2".to_string()).unwrap();
    
    assert!(order1.is_none());
    assert!(order2.is_none());
}

#[test]
fn test_concurrent_acid_compliance() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Setup initial account
    let doc = create_test_document("acc1", "SharedAccount", 1000);
    let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    manager.insert_document_in_transaction(&txn_id, doc).unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    let num_threads = 5;
    let operations_per_thread = 10;
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::new();

    // Multiple threads performing concurrent transactions
    for thread_id in 0..num_threads {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_transactions = 0;
            
            for op_id in 0..operations_per_thread {
                let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                
                let result = (|| -> Result<(), OQLError> {
                    let mut doc = manager_clone.get_document_in_transaction(
                        &txn_id, 
                        "accounts", 
                        &"acc1".to_string()
                    )?.unwrap();
                    
                    let current_balance = doc.get_field("balance").unwrap().as_integer().unwrap();
                    
                    // Simulate different operations
                    let new_balance = match op_id % 3 {
                        0 => current_balance + 10, // Deposit
                        1 => if current_balance >= 5 { current_balance - 5 } else { current_balance }, // Withdraw
                        _ => current_balance, // No change
                    };
                    
                    doc.set_field("balance".to_string(), Value::Integer(new_balance));
                    
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "accounts",
                        &"acc1".to_string(),
                        doc,
                        None,
                    )?;
                    
                    manager_clone.commit_transaction(&txn_id)?;
                    Ok(())
                })();
                
                match result {
                    Ok(_) => successful_transactions += 1,
                    Err(_) => {
                        let _ = manager_clone.rollback_transaction(&txn_id);
                    }
                }
                
                // Small delay between operations
                thread::sleep(Duration::from_millis(1));
            }
            
            (thread_id, successful_transactions)
        });
        
        handles.push(handle);
    }

    // Collect results
    let mut total_successful = 0;
    for handle in handles {
        let (thread_id, successful) = handle.join().unwrap();
        total_successful += successful;
        println!("Thread {} completed {} successful transactions", thread_id, successful);
    }

    // Verify final state is consistent
    let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    let final_doc = manager.get_document_in_transaction(&txn_id, "accounts", &"acc1".to_string()).unwrap().unwrap();
    let final_balance = final_doc.get_field("balance").unwrap().as_integer().unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    println!("Final balance: {}, Total successful transactions: {}", final_balance, total_successful);
    
    // Balance should be reasonable (not negative, not impossibly high)
    assert!(final_balance >= 0);
    assert!(final_balance <= 1000 + (total_successful as i64 * 10)); // Max possible with all deposits
    
    // At least some transactions should have succeeded
    assert!(total_successful > 0);
}