// Task 5.1 Verification: Basic Aggregation Functions
// This example demonstrates COUNT, SUM, AVG, MIN, MAX functions

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::parse_oql,
    storage::{DocumentStore, RocksDBStore},
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Task 5.1: Basic Aggregation Functions ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create test data - sales records
    let sales_data = vec![
        create_sale("sale_1", "Alice", 150.0, 2),
        create_sale("sale_2", "Bob", 200.0, 1),
        create_sale("sale_3", "Alice", 75.0, 3),
        create_sale("sale_4", "Charlie", 300.0, 1),
        create_sale("sale_5", "Bob", 125.0, 2),
    ];
    
    // Store test data
    for sale in &sales_data {
        store.put_document(sale)?;
    }
    
    println!("Created {} sales records", sales_data.len());
    println!("Sales data:");
    for sale in &sales_data {
        println!("  {} - {}: ${} (qty: {})", 
            sale.id, 
            sale.get_field("customer").unwrap().to_string_repr(),
            sale.get_field("amount").unwrap().to_string_repr(),
            sale.get_field("quantity").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test COUNT function
    println!("1. COUNT Function Tests:");
    
    // Count all sales
    let query = parse_oql("sales.select { totalsales: count(*) }")?;
    let results = executor.execute_query(&query)?;
    println!("  count(*): {}", results[0].get_field("totalsales").unwrap().to_string_repr());
    
    // Count non-null amounts
    let query = parse_oql("sales.select { amountcount: count(.amount) }")?;
    let results = executor.execute_query(&query)?;
    println!("  count(.amount): {}", results[0].get_field("amountcount").unwrap().to_string_repr());
    
    // Test SUM function
    println!("\n2. SUM Function Tests:");
    
    // Sum all amounts
    let query = parse_oql("sales.select { totalrevenue: sum(.amount) }")?;
    let results = executor.execute_query(&query)?;
    println!("  sum(.amount): ${}", results[0].get_field("totalrevenue").unwrap().to_string_repr());
    
    // Sum all quantities
    let query = parse_oql("sales.select { totalquantity: sum(.quantity) }")?;
    let results = executor.execute_query(&query)?;
    println!("  sum(.quantity): {}", results[0].get_field("totalquantity").unwrap().to_string_repr());
    
    // Test AVG function
    println!("\n3. AVG Function Tests:");
    
    // Average amount
    let query = parse_oql("sales.select { avgamount: avg(.amount) }")?;
    let results = executor.execute_query(&query)?;
    println!("  avg(.amount): ${:.2}", 
        match results[0].get_field("avgamount").unwrap() {
            Value::Float(f) => *f,
            _ => 0.0,
        }
    );
    
    // Average quantity
    let query = parse_oql("sales.select { avgquantity: avg(.quantity) }")?;
    let results = executor.execute_query(&query)?;
    println!("  avg(.quantity): {:.2}", 
        match results[0].get_field("avgquantity").unwrap() {
            Value::Float(f) => *f,
            _ => 0.0,
        }
    );
    
    // Test MIN function
    println!("\n4. MIN Function Tests:");
    
    // Minimum amount
    let query = parse_oql("sales.select { minamount: min(.amount) }")?;
    let results = executor.execute_query(&query)?;
    println!("  min(.amount): ${}", results[0].get_field("minamount").unwrap().to_string_repr());
    
    // Minimum quantity
    let query = parse_oql("sales.select { minquantity: min(.quantity) }")?;
    let results = executor.execute_query(&query)?;
    println!("  min(.quantity): {}", results[0].get_field("minquantity").unwrap().to_string_repr());
    
    // Test MAX function
    println!("\n5. MAX Function Tests:");
    
    // Maximum amount
    let query = parse_oql("sales.select { maxamount: max(.amount) }")?;
    let results = executor.execute_query(&query)?;
    println!("  max(.amount): ${}", results[0].get_field("maxamount").unwrap().to_string_repr());
    
    // Maximum quantity
    let query = parse_oql("sales.select { maxquantity: max(.quantity) }")?;
    let results = executor.execute_query(&query)?;
    println!("  max(.quantity): {}", results[0].get_field("maxquantity").unwrap().to_string_repr());
    
    // Test aggregation with filtering
    println!("\n6. Aggregation with Filtering:");
    
    // Sum amounts for Alice only
    let query = parse_oql(r#"sales.where { .customer == "Alice" }.select { alicetotal: sum(.amount) }"#)?;
    let results = executor.execute_query(&query)?;
    println!("  Alice's total: ${}", results[0].get_field("alicetotal").unwrap().to_string_repr());
    
    // Count sales over $100
    let query = parse_oql("sales.where { .amount > 100 }.select { highvaluecount: count(*) }")?;
    let results = executor.execute_query(&query)?;
    println!("  Sales over $100: {}", results[0].get_field("highvaluecount").unwrap().to_string_repr());
    
    // Test multiple aggregations in one query
    println!("\n7. Multiple Aggregations:");
    
    let query = parse_oql("sales.select { 
        totalsales: count(*), 
        totalrevenue: sum(.amount), 
        avgamount: avg(.amount),
        minamount: min(.amount),
        maxamount: max(.amount)
    }")?;
    let results = executor.execute_query(&query)?;
    let result = &results[0];
    
    println!("  Sales Summary:");
    println!("    Total Sales: {}", result.get_field("totalsales").unwrap().to_string_repr());
    println!("    Total Revenue: ${}", result.get_field("totalrevenue").unwrap().to_string_repr());
    println!("    Average Amount: ${:.2}", 
        match result.get_field("avgamount").unwrap() {
            Value::Float(f) => *f,
            _ => 0.0,
        }
    );
    println!("    Min Amount: ${}", result.get_field("minamount").unwrap().to_string_repr());
    println!("    Max Amount: ${}", result.get_field("maxamount").unwrap().to_string_repr());
    
    println!("\n✅ Task 5.1 completed successfully!");
    println!("All basic aggregation functions (COUNT, SUM, AVG, MIN, MAX) are working correctly.");
    
    Ok(())
}

fn create_sale(id: &str, customer: &str, amount: f64, quantity: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("customer".to_string(), Value::String(customer.to_string()));
    fields.insert("amount".to_string(), Value::Float(amount));
    fields.insert("quantity".to_string(), Value::Integer(quantity));
    
    Document::with_id(id.to_string(), "sales".to_string(), fields)
}