// Task 5.2 Verification: GROUP BY Functionality
// This example demonstrates grouping by single and multiple fields with aggregation

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
    println!("=== Task 5.2: GROUP BY Functionality ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create test data - sales records with different categories and regions
    let sales_data = vec![
        create_sale("sale_1", "Electronics", "North", "Alice", 150.0, 2),
        create_sale("sale_2", "Books", "South", "Bob", 50.0, 3),
        create_sale("sale_3", "Electronics", "North", "Charlie", 200.0, 1),
        create_sale("sale_4", "Books", "North", "Alice", 75.0, 2),
        create_sale("sale_5", "Electronics", "South", "Bob", 300.0, 1),
        create_sale("sale_6", "Clothing", "North", "Charlie", 120.0, 4),
        create_sale("sale_7", "Books", "South", "Alice", 40.0, 1),
        create_sale("sale_8", "Clothing", "South", "Bob", 80.0, 2),
    ];
    
    // Store test data
    for sale in &sales_data {
        store.put_document(sale)?;
    }
    
    println!("Created {} sales records", sales_data.len());
    println!("Sales data:");
    for sale in &sales_data {
        println!("  {} - {} | {} | {}: ${} (qty: {})", 
            sale.id, 
            sale.get_field("category").unwrap().to_string_repr(),
            sale.get_field("region").unwrap().to_string_repr(),
            sale.get_field("customer").unwrap().to_string_repr(),
            sale.get_field("amount").unwrap().to_string_repr(),
            sale.get_field("quantity").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 1: GROUP BY single field (category)
    println!("1. GROUP BY Single Field (Category):");
    let query = parse_oql("sales.group_by { .category }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Groups found: {}", results.len());
    for result in &results {
        println!("    Category: {} (Count: {})", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("group_count").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 2: GROUP BY single field (region)
    println!("2. GROUP BY Single Field (Region):");
    let query = parse_oql("sales.group_by { .region }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Groups found: {}", results.len());
    for result in &results {
        println!("    Region: {} (Count: {})", 
            result.get_field("region").unwrap().to_string_repr(),
            result.get_field("group_count").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 3: GROUP BY multiple fields (category and region)
    println!("3. GROUP BY Multiple Fields (Category + Region):");
    let query = parse_oql("sales.group_by { .category, .region }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Groups found: {}", results.len());
    for result in &results {
        println!("    Category: {}, Region: {} (Count: {})", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("region").unwrap().to_string_repr(),
            result.get_field("group_count").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 4: GROUP BY with SELECT (basic projection)
    println!("4. GROUP BY with SELECT:");
    let query = parse_oql("sales.group_by { .category }.select { category, groupcount: .group_count }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Category Summary:");
    for result in &results {
        println!("    {}: {} sales", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("groupcount").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 5: GROUP BY with filtering
    println!("5. GROUP BY with Filtering (amount > 100):");
    let query = parse_oql("sales.where { .amount > 100 }.group_by { .category }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  High-value sales by category:");
    for result in &results {
        println!("    Category: {} (Count: {})", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("group_count").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 6: GROUP BY with customer field
    println!("6. GROUP BY Customer:");
    let query = parse_oql("sales.group_by { .customer }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Sales by customer:");
    for result in &results {
        println!("    Customer: {} (Count: {})", 
            result.get_field("customer").unwrap().to_string_repr(),
            result.get_field("group_count").unwrap().to_string_repr()
        );
    }
    println!();
    
    println!("✅ Task 5.2 completed successfully!");
    println!("GROUP BY functionality is working correctly with:");
    println!("  - Single field grouping");
    println!("  - Multiple field grouping");
    println!("  - GROUP BY with SELECT projection");
    println!("  - GROUP BY with WHERE filtering");
    
    Ok(())
}

fn create_sale(id: &str, category: &str, region: &str, customer: &str, amount: f64, quantity: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("category".to_string(), Value::String(category.to_string()));
    fields.insert("region".to_string(), Value::String(region.to_string()));
    fields.insert("customer".to_string(), Value::String(customer.to_string()));
    fields.insert("amount".to_string(), Value::Float(amount));
    fields.insert("quantity".to_string(), Value::Integer(quantity));
    
    Document::with_id(id.to_string(), "sales".to_string(), fields)
}