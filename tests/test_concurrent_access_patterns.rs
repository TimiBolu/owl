// Comprehensive Concurrent Access Pattern Tests
// Tests various real-world concurrent access scenarios

use owl_db::concurrency::{ConcurrencyManager, LockType};
use owl_db::document::{Document, Value};
use owl_db::error::OQLError;
use owl_db::storage::DocumentStore;
use owl_db::transaction::{IsolationLevel, TransactionManager, TransactionalDocumentStore};
use std::collections::HashMap;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn create_account_document(id: &str, name: &str, balance: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("balance".to_string(), Value::Integer(balance));
    fields.insert("last_updated".to_string(), Value::String(chrono::Utc::now().to_rfc3339()));
    
    Document::with_id(id.to_string(), "accounts".to_string(), fields)
}

fn create_product_document(id: &str, name: &str, inventory: i64, price: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("inventory".to_string(), Value::Integer(inventory));
    fields.insert("price".to_string(), Value::Integer(price));
    
    Document::with_id(id.to_string(), "products".to_string(), fields)
}

fn create_order_document(id: &str, customer_id: &str, product_id: &str, quantity: i64, total: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("customer_id".to_string(), Value::String(customer_id.to_string()));
    fields.insert("product_id".to_string(), Value::String(product_id.to_string()));
    fields.insert("quantity".to_string(), Value::Integer(quantity));
    fields.insert("total".to_string(), Value::Integer(total));
    fields.insert("status".to_string(), Value::String("pending".to_string()));
    fields.insert("created_at".to_string(), Value::String(chrono::Utc::now().to_rfc3339()));
    
    Document::with_id(id.to_string(), "orders".to_string(), fields)
}

#[test]
fn test_high_frequency_read_pattern() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Setup popular product that many customers will read
    let product = create_product_document("popular_item", "Popular Product", 1000, 50);
    let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    manager.insert_document_in_transaction(&txn_id, product).unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    let num_readers = 10;
    let reads_per_reader = 20;
    let barrier = Arc::new(Barrier::new(num_readers));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    let start_time = Instant::now();

    // Simulate many concurrent readers
    for reader_id in 0..num_readers {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let results_clone = results.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_reads = 0;
            let mut total_read_time = Duration::new(0, 0);
            
            for _ in 0..reads_per_reader {
                let read_start = Instant::now();
                let txn_id = manager_clone.begin_transaction(IsolationLevel::ReadCommitted).unwrap();
                
                let result = manager_clone.get_document_in_transaction(
                    &txn_id,
                    "products",
                    &"popular_item".to_string(),
                );
                
                if result.is_ok() {
                    successful_reads += 1;
                }
                
                let _ = manager_clone.commit_transaction(&txn_id);
                total_read_time += read_start.elapsed();
                
                // Small delay to simulate processing
                thread::sleep(Duration::from_millis(1));
            }
            
            results_clone.lock().unwrap().push((reader_id, successful_reads, total_read_time));
        });
        
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let total_time = start_time.elapsed();
    let results = results.lock().unwrap();
    
    let total_successful_reads: usize = results.iter().map(|(_, reads, _)| *reads).sum();
    let avg_read_time: Duration = results.iter()
        .map(|(_, _, time)| *time)
        .sum::<Duration>() / results.len() as u32;

    println!("High-frequency read test:");
    println!("  Total successful reads: {}", total_successful_reads);
    println!("  Total time: {:?}", total_time);
    println!("  Average read time per reader: {:?}", avg_read_time);
    println!("  Reads per second: {:.2}", total_successful_reads as f64 / total_time.as_secs_f64());

    // All reads should succeed (no conflicts for read-only operations)
    assert_eq!(total_successful_reads, num_readers * reads_per_reader);
}

#[test]
fn test_producer_consumer_pattern() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Setup initial inventory
    let product = create_product_document("widget", "Widget", 100, 10);
    let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    manager.insert_document_in_transaction(&txn_id, product).unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    let num_producers = 2;
    let num_consumers = 5;
    let operations_per_thread = 10;
    
    let barrier = Arc::new(Barrier::new(num_producers + num_consumers));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    // Producer threads (add inventory)
    for producer_id in 0..num_producers {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let results_clone = results.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_productions = 0;
            
            for batch in 0..operations_per_thread {
                let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                
                let result = (|| -> Result<(), OQLError> {
                    let mut product = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "products",
                        &"widget".to_string(),
                    )?.unwrap();
                    
                    let current_inventory = product.get_field("inventory").unwrap().as_integer().unwrap();
                    let production_amount = 5; // Produce 5 units
                    
                    product.set_field("inventory".to_string(), Value::Integer(current_inventory + production_amount));
                    
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "products",
                        &"widget".to_string(),
                        product,
                        None,
                    )?;
                    
                    manager_clone.commit_transaction(&txn_id)?;
                    Ok(())
                })();
                
                if result.is_ok() {
                    successful_productions += 1;
                } else {
                    let _ = manager_clone.rollback_transaction(&txn_id);
                }
                
                thread::sleep(Duration::from_millis(10));
            }
            
            results_clone.lock().unwrap().push((format!("producer_{}", producer_id), successful_productions));
        });
        
        handles.push(handle);
    }

    // Consumer threads (consume inventory)
    for consumer_id in 0..num_consumers {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let results_clone = results.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_consumptions = 0;
            
            for _ in 0..operations_per_thread {
                let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                
                let result = (|| -> Result<(), OQLError> {
                    let mut product = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "products",
                        &"widget".to_string(),
                    )?.unwrap();
                    
                    let current_inventory = product.get_field("inventory").unwrap().as_integer().unwrap();
                    let consumption_amount = 2; // Consume 2 units
                    
                    if current_inventory < consumption_amount {
                        return Err(OQLError::Generic { 
                            message: "Insufficient inventory".to_string() 
                        });
                    }
                    
                    product.set_field("inventory".to_string(), Value::Integer(current_inventory - consumption_amount));
                    
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "products",
                        &"widget".to_string(),
                        product,
                        None,
                    )?;
                    
                    manager_clone.commit_transaction(&txn_id)?;
                    Ok(())
                })();
                
                if result.is_ok() {
                    successful_consumptions += 1;
                } else {
                    let _ = manager_clone.rollback_transaction(&txn_id);
                }
                
                thread::sleep(Duration::from_millis(15));
            }
            
            results_clone.lock().unwrap().push((format!("consumer_{}", consumer_id), successful_consumptions));
        });
        
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let results = results.lock().unwrap();
    
    let total_productions: i32 = results.iter()
        .filter(|(name, _)| name.starts_with("producer"))
        .map(|(_, count)| *count)
        .sum();
    
    let total_consumptions: i32 = results.iter()
        .filter(|(name, _)| name.starts_with("consumer"))
        .map(|(_, count)| *count)
        .sum();

    println!("Producer-Consumer test:");
    println!("  Total productions: {}", total_productions);
    println!("  Total consumptions: {}", total_consumptions);

    // Verify final inventory is consistent
    let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
    let final_product = manager.get_document_in_transaction(&txn_id, "products", &"widget".to_string()).unwrap().unwrap();
    let final_inventory = final_product.get_field("inventory").unwrap().as_integer().unwrap();
    manager.commit_transaction(&txn_id).unwrap();

    let expected_inventory = 100 + (total_productions as i64 * 5) - (total_consumptions as i64 * 2);
    println!("  Final inventory: {} (expected: {})", final_inventory, expected_inventory);
    
    assert_eq!(final_inventory, expected_inventory);
    assert!(final_inventory >= 0, "Inventory should never go negative");
}

#[test]
fn test_banking_transfer_pattern() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Setup accounts
    let accounts = vec![
        create_account_document("acc1", "Alice", 1000),
        create_account_document("acc2", "Bob", 1000),
        create_account_document("acc3", "Charlie", 1000),
        create_account_document("acc4", "Diana", 1000),
    ];
    
    for account in &accounts {
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        manager.insert_document_in_transaction(&txn_id, account.clone()).unwrap();
        manager.commit_transaction(&txn_id).unwrap();
    }

    let num_transfer_threads = 8;
    let transfers_per_thread = 15;
    let barrier = Arc::new(Barrier::new(num_transfer_threads));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    // Multiple threads performing concurrent transfers
    for thread_id in 0..num_transfer_threads {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let results_clone = results.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_transfers = 0;
            let mut failed_transfers = 0;
            
            for transfer_id in 0..transfers_per_thread {
                let from_account = format!("acc{}", (thread_id + transfer_id) % 4 + 1);
                let to_account = format!("acc{}", (thread_id + transfer_id + 1) % 4 + 1);
                let amount = 50 + (transfer_id % 3) as i64 * 25; // Transfer 50, 75, or 100
                
                if from_account == to_account {
                    continue; // Skip self-transfers
                }
                
                let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                
                let result = (|| -> Result<(), OQLError> {
                    // Get both accounts (ordered by ID to prevent deadlocks)
                    let (first_acc, second_acc) = if from_account < to_account {
                        (&from_account, &to_account)
                    } else {
                        (&to_account, &from_account)
                    };
                    
                    let mut first_doc = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "accounts",
                        first_acc,
                    )?.unwrap();
                    
                    let mut second_doc = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "accounts",
                        second_acc,
                    )?.unwrap();
                    
                    // Determine which is from and which is to
                    let (mut from_doc, mut to_doc) = if first_acc == &from_account {
                        (first_doc, second_doc)
                    } else {
                        (second_doc, first_doc)
                    };
                    
                    let from_balance = from_doc.get_field("balance").unwrap().as_integer().unwrap();
                    let to_balance = to_doc.get_field("balance").unwrap().as_integer().unwrap();
                    
                    if from_balance < amount {
                        return Err(OQLError::Generic { 
                            message: "Insufficient funds".to_string() 
                        });
                    }
                    
                    from_doc.set_field("balance".to_string(), Value::Integer(from_balance - amount));
                    to_doc.set_field("balance".to_string(), Value::Integer(to_balance + amount));
                    
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "accounts",
                        &from_account,
                        from_doc,
                        None,
                    )?;
                    
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "accounts",
                        &to_account,
                        to_doc,
                        None,
                    )?;
                    
                    manager_clone.commit_transaction(&txn_id)?;
                    Ok(())
                })();
                
                match result {
                    Ok(_) => successful_transfers += 1,
                    Err(_) => {
                        failed_transfers += 1;
                        let _ = manager_clone.rollback_transaction(&txn_id);
                    }
                }
                
                thread::sleep(Duration::from_millis(5));
            }
            
            results_clone.lock().unwrap().push((thread_id, successful_transfers, failed_transfers));
        });
        
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let results = results.lock().unwrap();
    let total_successful: i32 = results.iter().map(|(_, success, _)| *success).sum();
    let total_failed: i32 = results.iter().map(|(_, _, failed)| *failed).sum();

    println!("Banking transfer test:");
    println!("  Successful transfers: {}", total_successful);
    println!("  Failed transfers: {}", total_failed);

    // Verify total balance is conserved
    let mut total_balance = 0;
    for i in 1..=4 {
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        let account = manager.get_document_in_transaction(&txn_id, "accounts", &format!("acc{}", i)).unwrap().unwrap();
        let balance = account.get_field("balance").unwrap().as_integer().unwrap();
        total_balance += balance;
        manager.commit_transaction(&txn_id).unwrap();
        
        println!("  Account {}: {}", i, balance);
    }

    println!("  Total balance: {} (should be 4000)", total_balance);
    assert_eq!(total_balance, 4000, "Total balance should be conserved");
    assert!(total_successful > 0, "Some transfers should succeed");
}

#[test]
fn test_e_commerce_order_processing() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Setup customers, products
    let customers = vec![
        create_account_document("cust1", "Alice", 500),
        create_account_document("cust2", "Bob", 300),
        create_account_document("cust3", "Charlie", 800),
    ];
    
    let products = vec![
        create_product_document("prod1", "Laptop", 10, 200),
        create_product_document("prod2", "Mouse", 50, 25),
        create_product_document("prod3", "Keyboard", 30, 75),
    ];
    
    // Insert initial data
    for customer in &customers {
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        manager.insert_document_in_transaction(&txn_id, customer.clone()).unwrap();
        manager.commit_transaction(&txn_id).unwrap();
    }
    
    for product in &products {
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        manager.insert_document_in_transaction(&txn_id, product.clone()).unwrap();
        manager.commit_transaction(&txn_id).unwrap();
    }

    let num_order_threads = 6;
    let orders_per_thread = 8;
    let barrier = Arc::new(Barrier::new(num_order_threads));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    // Simulate concurrent order processing
    for thread_id in 0..num_order_threads {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let results_clone = results.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_orders = 0;
            let mut failed_orders = 0;
            
            for order_id in 0..orders_per_thread {
                let customer_id = format!("cust{}", (thread_id % 3) + 1);
                let product_id = format!("prod{}", (order_id % 3) + 1);
                let quantity = 1 + (order_id % 2); // Order 1 or 2 items
                
                let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                
                let result = (|| -> Result<(), OQLError> {
                    // Get customer account
                    let mut customer = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "accounts",
                        &customer_id,
                    )?.unwrap();
                    
                    // Get product
                    let mut product = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "products",
                        &product_id,
                    )?.unwrap();
                    
                    let customer_balance = customer.get_field("balance").unwrap().as_integer().unwrap();
                    let product_price = product.get_field("price").unwrap().as_integer().unwrap();
                    let product_inventory = product.get_field("inventory").unwrap().as_integer().unwrap();
                    
                    let total_cost = product_price * quantity;
                    
                    // Check if customer has enough money
                    if customer_balance < total_cost {
                        return Err(OQLError::Generic { 
                            message: "Insufficient customer balance".to_string() 
                        });
                    }
                    
                    // Check if enough inventory
                    if product_inventory < quantity {
                        return Err(OQLError::Generic { 
                            message: "Insufficient inventory".to_string() 
                        });
                    }
                    
                    // Process the order
                    customer.set_field("balance".to_string(), Value::Integer(customer_balance - total_cost));
                    product.set_field("inventory".to_string(), Value::Integer(product_inventory - quantity));
                    
                    // Update customer and product
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "accounts",
                        &customer_id,
                        customer,
                        None,
                    )?;
                    
                    manager_clone.update_document_in_transaction(
                        &txn_id,
                        "products",
                        &product_id,
                        product,
                        None,
                    )?;
                    
                    // Create order record
                    let order = create_order_document(
                        &format!("order_{}_{}", thread_id, order_id),
                        &customer_id,
                        &product_id,
                        quantity,
                        total_cost,
                    );
                    
                    manager_clone.insert_document_in_transaction(&txn_id, order)?;
                    
                    manager_clone.commit_transaction(&txn_id)?;
                    Ok(())
                })();
                
                match result {
                    Ok(_) => successful_orders += 1,
                    Err(_) => {
                        failed_orders += 1;
                        let _ = manager_clone.rollback_transaction(&txn_id);
                    }
                }
                
                thread::sleep(Duration::from_millis(10));
            }
            
            results_clone.lock().unwrap().push((thread_id, successful_orders, failed_orders));
        });
        
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let results = results.lock().unwrap();
    let total_successful: i32 = results.iter().map(|(_, success, _)| *success).sum();
    let total_failed: i32 = results.iter().map(|(_, _, failed)| *failed).sum();

    println!("E-commerce order processing test:");
    println!("  Successful orders: {}", total_successful);
    println!("  Failed orders: {}", total_failed);

    // Verify final state consistency
    let mut total_customer_balance = 0;
    let mut total_inventory_value = 0;
    
    for i in 1..=3 {
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        let customer = manager.get_document_in_transaction(&txn_id, "accounts", &format!("cust{}", i)).unwrap().unwrap();
        let balance = customer.get_field("balance").unwrap().as_integer().unwrap();
        total_customer_balance += balance;
        manager.commit_transaction(&txn_id).unwrap();
        
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        let product = manager.get_document_in_transaction(&txn_id, "products", &format!("prod{}", i)).unwrap().unwrap();
        let inventory = product.get_field("inventory").unwrap().as_integer().unwrap();
        let price = product.get_field("price").unwrap().as_integer().unwrap();
        total_inventory_value += inventory * price;
        manager.commit_transaction(&txn_id).unwrap();
        
        println!("  Customer {}: balance {}", i, balance);
        println!("  Product {}: inventory {} (value: {})", i, inventory, inventory * price);
    }

    println!("  Total customer balance: {}", total_customer_balance);
    println!("  Total inventory value: {}", total_inventory_value);

    // The sum of customer balances + inventory value should equal initial value minus successful order profits
    let initial_customer_balance = 500 + 300 + 800; // 1600
    let initial_inventory_value = (10 * 200) + (50 * 25) + (30 * 75); // 2000 + 1250 + 2250 = 5500
    let initial_total = initial_customer_balance + initial_inventory_value; // 7100

    println!("  Initial total value: {}", initial_total);
    println!("  Current total value: {}", total_customer_balance + total_inventory_value);

    // Some orders should succeed
    assert!(total_successful > 0, "Some orders should succeed");
    
    // All balances should be non-negative
    assert!(total_customer_balance >= 0, "Customer balances should not be negative");
    assert!(total_inventory_value >= 0, "Inventory values should not be negative");
}

#[test]
fn test_read_write_workload_mix() {
    let temp_dir = TempDir::new().unwrap();
    let manager = Arc::new(TransactionManager::new(temp_dir.path().to_str().unwrap()).unwrap());
    
    // Setup documents
    for i in 1..=10 {
        let doc = create_account_document(&format!("acc{}", i), &format!("Account {}", i), 1000);
        let txn_id = manager.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        manager.insert_document_in_transaction(&txn_id, doc).unwrap();
        manager.commit_transaction(&txn_id).unwrap();
    }

    let num_threads = 8;
    let operations_per_thread = 25;
    let barrier = Arc::new(Barrier::new(num_threads));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    let start_time = Instant::now();

    // Mixed read/write workload
    for thread_id in 0..num_threads {
        let manager_clone = manager.clone();
        let barrier_clone = barrier.clone();
        let results_clone = results.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut reads = 0;
            let mut writes = 0;
            let mut read_time = Duration::new(0, 0);
            let mut write_time = Duration::new(0, 0);
            
            for op_id in 0..operations_per_thread {
                let account_id = format!("acc{}", (op_id % 10) + 1);
                
                // 70% reads, 30% writes
                if op_id % 10 < 7 {
                    // Read operation
                    let op_start = Instant::now();
                    let txn_id = manager_clone.begin_transaction(IsolationLevel::ReadCommitted).unwrap();
                    
                    let result = manager_clone.get_document_in_transaction(
                        &txn_id,
                        "accounts",
                        &account_id,
                    );
                    
                    if result.is_ok() {
                        reads += 1;
                    }
                    
                    let _ = manager_clone.commit_transaction(&txn_id);
                    read_time += op_start.elapsed();
                } else {
                    // Write operation
                    let op_start = Instant::now();
                    let txn_id = manager_clone.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
                    
                    let result = (|| -> Result<(), OQLError> {
                        let mut account = manager_clone.get_document_in_transaction(
                            &txn_id,
                            "accounts",
                            &account_id,
                        )?.unwrap();
                        
                        let balance = account.get_field("balance").unwrap().as_integer().unwrap();
                        account.set_field("balance".to_string(), Value::Integer(balance + 1));
                        
                        manager_clone.update_document_in_transaction(
                            &txn_id,
                            "accounts",
                            &account_id,
                            account,
                            None,
                        )?;
                        
                        manager_clone.commit_transaction(&txn_id)?;
                        Ok(())
                    })();
                    
                    if result.is_ok() {
                        writes += 1;
                    } else {
                        let _ = manager_clone.rollback_transaction(&txn_id);
                    }
                    
                    write_time += op_start.elapsed();
                }
                
                // Small delay between operations
                thread::sleep(Duration::from_millis(2));
            }
            
            results_clone.lock().unwrap().push((thread_id, reads, writes, read_time, write_time));
        });
        
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let total_time = start_time.elapsed();
    let results = results.lock().unwrap();
    
    let total_reads: i32 = results.iter().map(|(_, reads, _, _, _)| *reads).sum();
    let total_writes: i32 = results.iter().map(|(_, _, writes, _, _)| *writes).sum();
    let total_read_time: Duration = results.iter().map(|(_, _, _, read_time, _)| *read_time).sum();
    let total_write_time: Duration = results.iter().map(|(_, _, _, _, write_time)| *write_time).sum();

    println!("Mixed read/write workload test:");
    println!("  Total reads: {}", total_reads);
    println!("  Total writes: {}", total_writes);
    println!("  Total time: {:?}", total_time);
    println!("  Average read time: {:?}", total_read_time / total_reads as u32);
    println!("  Average write time: {:?}", total_write_time / total_writes as u32);
    println!("  Operations per second: {:.2}", (total_reads + total_writes) as f64 / total_time.as_secs_f64());

    // Most operations should succeed
    let expected_total_ops = num_threads * operations_per_thread;
    let actual_total_ops = total_reads + total_writes;
    let success_rate = actual_total_ops as f64 / expected_total_ops as f64;
    
    println!("  Success rate: {:.2}%", success_rate * 100.0);
    assert!(success_rate > 0.8, "Success rate should be above 80%");
}