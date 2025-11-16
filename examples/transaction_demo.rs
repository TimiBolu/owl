// Transaction Demo - Demonstrates ACID transaction support in OQL

use owl_db::{
    document::{Document, Value},
    transaction::{TransactionalDocumentStore, IsolationLevel},
    storage::DocumentStore,
};
use std::collections::HashMap;
use tempfile::TempDir;

fn create_customer(id: &str, name: &str, balance: f64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("balance".to_string(), Value::Float(balance));
    Document::with_id(id.to_string(), "customers".to_string(), fields)
}

fn create_order(id: &str, customer_id: &str, amount: f64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("customer_id".to_string(), Value::String(customer_id.to_string()));
    fields.insert("amount".to_string(), Value::Float(amount));
    fields.insert("status".to_string(), Value::String("pending".to_string()));
    Document::with_id(id.to_string(), "orders".to_string(), fields)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OQL Transaction Demo ===\n");

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap())?;

    // Demo 1: Basic Transaction - Successful Commit
    println!("Demo 1: Successful Transaction");
    println!("Creating customer and order in a single transaction...");
    
    store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        // Create customer
        let customer = create_customer("cust_1", "Alice Johnson", 1000.0);
        store.put_document(&customer)?;
        println!("  ✓ Created customer: {}", customer.id);
        
        // Create order
        let order = create_order("ord_1", "cust_1", 150.0);
        store.put_document(&order)?;
        println!("  ✓ Created order: {} for ${}", order.id, 150.0);
        
        // Update customer balance
        let mut updated_customer = customer.clone();
        if let Some(Value::Float(balance)) = updated_customer.fields.get_mut("balance") {
            *balance -= 150.0;
        }
        store.put_document(&updated_customer)?;
        println!("  ✓ Updated customer balance to ${}", 850.0);
        
        Ok(())
    })?;
    
    println!("Transaction committed successfully!\n");
    
    // Verify data persisted
    let customer = store.get_document("customers", &"cust_1".to_string())?.unwrap();
    let order = store.get_document("orders", &"ord_1".to_string())?.unwrap();
    println!("Verification:");
    if let Some(Value::Float(balance)) = customer.fields.get("balance") {
        println!("  Customer balance: ${}", balance);
    }
    println!("  Order exists: {}\n", order.id);

    // Demo 2: Transaction Rollback
    println!("Demo 2: Transaction Rollback");
    println!("Attempting transaction that will fail...");
    
    let result = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        // Create another order
        let order2 = create_order("ord_2", "cust_1", 200.0);
        store.put_document(&order2)?;
        println!("  ✓ Created order: {} for ${}", order2.id, 200.0);
        
        // Try to update customer balance (this would succeed)
        let mut customer = store.get_document("customers", &"cust_1".to_string())?.unwrap();
        if let Some(Value::Float(balance)) = customer.fields.get_mut("balance") {
            *balance -= 200.0;
        }
        store.put_document(&customer)?;
        println!("  ✓ Updated customer balance to ${}", 650.0);
        
        // Simulate a business logic error
        if true { // Simulate insufficient inventory or other business rule
            return Err(owl_db::error::OQLError::Generic {
                message: "Insufficient inventory for order".to_string(),
            });
        }
        
        Ok(())
    });
    
    match result {
        Ok(_) => println!("Transaction succeeded (unexpected)"),
        Err(e) => println!("  ✗ Transaction failed: {}", e),
    }
    
    println!("Transaction rolled back automatically!\n");
    
    // Verify rollback - order should not exist, customer balance unchanged
    let customer = store.get_document("customers", &"cust_1".to_string())?.unwrap();
    let order2 = store.get_document("orders", &"ord_2".to_string())?;
    println!("Verification after rollback:");
    if let Some(Value::Float(balance)) = customer.fields.get("balance") {
        println!("  Customer balance: ${} (unchanged)", balance);
    }
    println!("  Order ord_2 exists: {}\n", order2.is_some());

    // Demo 3: Manual Transaction Control
    println!("Demo 3: Manual Transaction Control");
    println!("Using explicit begin/commit/rollback...");
    
    let txn_id = store.begin_transaction(IsolationLevel::RepeatableRead)?;
    println!("  ✓ Started transaction: {}", txn_id);
    
    // Create a product
    let mut product_fields = HashMap::new();
    product_fields.insert("name".to_string(), Value::String("Laptop".to_string()));
    product_fields.insert("price".to_string(), Value::Float(999.99));
    product_fields.insert("inventory".to_string(), Value::Integer(10));
    let product = Document::with_id("prod_1".to_string(), "products".to_string(), product_fields);
    
    store.put_document(&product)?;
    println!("  ✓ Created product: {} (${:.2})", "Laptop", 999.99);
    
    // Check if we can see it within the transaction
    let retrieved = store.get_document("products", &"prod_1".to_string())?;
    println!("  ✓ Product visible within transaction: {}", retrieved.is_some());
    
    store.commit_transaction()?;
    println!("  ✓ Transaction committed manually");
    
    // Verify persistence
    let product = store.get_document("products", &"prod_1".to_string())?.unwrap();
    if let Some(Value::String(name)) = product.fields.get("name") {
        println!("  ✓ Product persisted: {}\n", name);
    }

    // Demo 4: Read Your Own Writes
    println!("Demo 4: Read Your Own Writes");
    println!("Demonstrating transaction isolation...");
    
    store.with_transaction(IsolationLevel::RepeatableRead, |store| {
        // Create a document
        let doc = create_customer("cust_2", "Bob Smith", 500.0);
        store.put_document(&doc)?;
        println!("  ✓ Created customer: {}", doc.id);
        
        // Immediately read it back (should work due to read-your-own-writes)
        let retrieved = store.get_document("customers", &"cust_2".to_string())?;
        println!("  ✓ Can read own write: {}", retrieved.is_some());
        
        // Update the document
        let mut updated_doc = retrieved.unwrap();
        updated_doc.fields.insert("email".to_string(), Value::String("bob@example.com".to_string()));
        store.put_document(&updated_doc)?;
        println!("  ✓ Updated customer with email");
        
        // Read the updated version
        let final_doc = store.get_document("customers", &"cust_2".to_string())?.unwrap();
        println!("  ✓ Can read updated version: {}", 
                final_doc.fields.contains_key("email"));
        
        Ok(())
    })?;
    
    println!("Transaction completed successfully!\n");

    println!("=== Transaction Demo Complete ===");
    println!("Key features demonstrated:");
    println!("  ✓ Atomic operations (all-or-nothing)");
    println!("  ✓ Automatic rollback on errors");
    println!("  ✓ Manual transaction control");
    println!("  ✓ Read-your-own-writes isolation");
    println!("  ✓ Data persistence after commit");

    Ok(())
}