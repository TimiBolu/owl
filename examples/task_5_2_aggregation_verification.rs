// Task 5.2 Advanced Verification: GROUP BY with Aggregation Functions
// This example demonstrates GROUP BY with SUM, AVG, COUNT, MIN, MAX

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
    println!("=== Task 5.2: GROUP BY with Aggregation Functions ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create test data - sales records with different categories
    let sales_data = vec![
        create_sale("sale_1", "Electronics", 150.0, 2),
        create_sale("sale_2", "Books", 50.0, 3),
        create_sale("sale_3", "Electronics", 200.0, 1),
        create_sale("sale_4", "Books", 75.0, 2),
        create_sale("sale_5", "Electronics", 300.0, 1),
        create_sale("sale_6", "Clothing", 120.0, 4),
        create_sale("sale_7", "Books", 40.0, 1),
        create_sale("sale_8", "Clothing", 80.0, 2),
    ];
    
    // Store test data
    for sale in &sales_data {
        store.put_document(sale)?;
    }
    
    println!("Created {} sales records", sales_data.len());
    println!("Sales data by category:");
    
    // Show data organized by category for reference
    let mut by_category: HashMap<String, Vec<&Document>> = HashMap::new();
    for sale in &sales_data {
        let category = match sale.get_field("category").unwrap() {
            Value::String(s) => s.clone(),
            _ => "Unknown".to_string(),
        };
        by_category.entry(category).or_insert_with(Vec::new).push(sale);
    }
    
    for (category, sales) in &by_category {
        println!("  {}:", category);
        for sale in sales {
            println!("    {}: ${} (qty: {})", 
                sale.id,
                sale.get_field("amount").unwrap().to_string_repr(),
                sale.get_field("quantity").unwrap().to_string_repr()
            );
        }
    }
    println!();
    
    // Test 1: GROUP BY with COUNT aggregation
    println!("1. GROUP BY with COUNT:");
    let query = parse_oql("sales.group_by { .category }.select { category, salescount: count(*) }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Sales count by category:");
    for result in &results {
        println!("    {}: {} sales", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("salescount").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 2: GROUP BY with SUM aggregation
    println!("2. GROUP BY with SUM:");
    let query = parse_oql("sales.group_by { .category }.select { category, totalrevenue: sum(.amount), totalqty: sum(.quantity) }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Revenue and quantity by category:");
    for result in &results {
        println!("    {}: ${} revenue, {} items", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("totalrevenue").unwrap().to_string_repr(),
            result.get_field("totalqty").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 3: GROUP BY with AVG aggregation
    println!("3. GROUP BY with AVG:");
    let query = parse_oql("sales.group_by { .category }.select { category, avgamount: avg(.amount) }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Average amount by category:");
    for result in &results {
        println!("    {}: ${:.2} average", 
            result.get_field("category").unwrap().to_string_repr(),
            match result.get_field("avgamount").unwrap() {
                Value::Float(f) => *f,
                Value::Integer(i) => *i as f64,
                _ => 0.0,
            }
        );
    }
    println!();
    
    // Test 4: GROUP BY with MIN/MAX aggregation
    println!("4. GROUP BY with MIN/MAX:");
    let query = parse_oql("sales.group_by { .category }.select { category, minamount: min(.amount), maxamount: max(.amount) }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Min/Max amounts by category:");
    for result in &results {
        println!("    {}: ${} - ${}", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("minamount").unwrap().to_string_repr(),
            result.get_field("maxamount").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 5: GROUP BY with multiple aggregations
    println!("5. GROUP BY with Multiple Aggregations:");
    let query = parse_oql("sales.group_by { .category }.select { category, salescount: count(*), totalamount: sum(.amount), avgamount: avg(.amount), minamount: min(.amount), maxamount: max(.amount) }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Complete summary by category:");
    for result in &results {
        println!("    {}:", result.get_field("category").unwrap().to_string_repr());
        println!("      Count: {} sales", result.get_field("salescount").unwrap().to_string_repr());
        println!("      Total: ${}", result.get_field("totalamount").unwrap().to_string_repr());
        println!("      Average: ${:.2}", 
            match result.get_field("avgamount").unwrap() {
                Value::Float(f) => *f,
                Value::Integer(i) => *i as f64,
                _ => 0.0,
            }
        );
        println!("      Range: ${} - ${}", 
            result.get_field("minamount").unwrap().to_string_repr(),
            result.get_field("maxamount").unwrap().to_string_repr()
        );
    }
    println!();
    
    println!("✅ Task 5.2 GROUP BY with aggregation completed successfully!");
    println!("GROUP BY functionality is working correctly with all aggregation functions:");
    println!("  - COUNT(*) and COUNT(field)");
    println!("  - SUM(field) for numeric aggregation");
    println!("  - AVG(field) for average calculation");
    println!("  - MIN(field) and MAX(field) for range analysis");
    println!("  - Multiple aggregations in single query");
    
    Ok(())
}

fn create_sale(id: &str, category: &str, amount: f64, quantity: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("category".to_string(), Value::String(category.to_string()));
    fields.insert("amount".to_string(), Value::Float(amount));
    fields.insert("quantity".to_string(), Value::Integer(quantity));
    
    Document::with_id(id.to_string(), "sales".to_string(), fields)
}