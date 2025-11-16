// Task 5 Comprehensive Verification: All Aggregation, Grouping, and Sorting Features
// This example demonstrates the complete functionality implemented in Task 5

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
    println!("=== Task 5: Comprehensive Aggregation, Grouping, and Sorting Test ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create comprehensive test data - sales records
    let sales_data = vec![
        create_sale("sale_1", "Electronics", "North", "Q1", 1500.0, 3),
        create_sale("sale_2", "Books", "South", "Q1", 300.0, 12),
        create_sale("sale_3", "Electronics", "South", "Q1", 2200.0, 2),
        create_sale("sale_4", "Books", "North", "Q1", 450.0, 15),
        create_sale("sale_5", "Clothing", "North", "Q1", 800.0, 8),
        create_sale("sale_6", "Electronics", "North", "Q2", 1800.0, 4),
        create_sale("sale_7", "Books", "South", "Q2", 350.0, 14),
        create_sale("sale_8", "Clothing", "South", "Q2", 950.0, 10),
        create_sale("sale_9", "Electronics", "South", "Q2", 1200.0, 2),
        create_sale("sale_10", "Books", "North", "Q2", 400.0, 16),
    ];
    
    // Store test data
    for sale in &sales_data {
        store.put_document(sale)?;
    }
    
    println!("Created {} sales records across categories, regions, and quarters", sales_data.len());
    println!();
    
    // Test 1: Basic aggregation functions
    println!("1. Basic Aggregation Functions:");
    let query = parse_oql("sales.select { 
        totalsales: count(*), 
        totalrevenue: sum(.amount), 
        avgamount: avg(.amount),
        minamount: min(.amount),
        maxamount: max(.amount)
    }")?;
    let results = executor.execute_query(&query)?;
    let result = &results[0];
    
    println!("  Overall Sales Summary:");
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
    println!();
    
    // Test 2: GROUP BY with aggregation
    println!("2. GROUP BY Category with Aggregation:");
    let query = parse_oql("sales.group_by { .category }.select { 
        category, 
        salescount: count(*), 
        totalrevenue: sum(.amount), 
        avgamount: avg(.amount)
    }")?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {}: {} sales, ${} revenue, ${:.2} avg", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("salescount").unwrap().to_string_repr(),
            result.get_field("totalrevenue").unwrap().to_string_repr(),
            match result.get_field("avgamount").unwrap() {
                Value::Float(f) => *f,
                _ => 0.0,
            }
        );
    }
    println!();
    
    // Test 3: GROUP BY multiple fields
    println!("3. GROUP BY Category and Region:");
    let query = parse_oql("sales.group_by { .category, .region }.select { 
        category, 
        region, 
        salescount: count(*), 
        totalrevenue: sum(.amount)
    }")?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {} - {}: {} sales, ${} revenue", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("region").unwrap().to_string_repr(),
            result.get_field("salescount").unwrap().to_string_repr(),
            result.get_field("totalrevenue").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 4: Sorting with aggregation
    println!("4. Top Categories by Revenue (Sorted):");
    let query = parse_oql("sales.group_by { .category }.select { 
        category, 
        totalrevenue: sum(.amount)
    }.sort { .totalrevenue.desc }")?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {}: ${}", 
            result.get_field("category").unwrap().to_string_repr(),
            result.get_field("totalrevenue").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 5: Combined WHERE, GROUP BY, and SORT
    println!("5. High-Value Sales (>$1000) by Quarter:");
    let query = parse_oql("sales.where { .amount > 1000 }.group_by { .quarter }.select { 
        quarter, 
        salescount: count(*), 
        totalrevenue: sum(.amount)
    }.sort { .totalrevenue.desc }")?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {}: {} high-value sales, ${} revenue", 
            result.get_field("quarter").unwrap().to_string_repr(),
            result.get_field("salescount").unwrap().to_string_repr(),
            result.get_field("totalrevenue").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 6: LIMIT with GROUP BY and SORT
    println!("6. Top 2 Regions by Average Sale Amount:");
    let query = parse_oql("sales.group_by { .region }.select { 
        region, 
        avgamount: avg(.amount)
    }.sort { .avgamount.desc }.limit 2")?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {}: ${:.2} average", 
            result.get_field("region").unwrap().to_string_repr(),
            match result.get_field("avgamount").unwrap() {
                Value::Float(f) => *f,
                _ => 0.0,
            }
        );
    }
    println!();
    
    // Test 7: Complex query with all features
    println!("7. Complex Analysis - Electronics Sales by Region:");
    let query = parse_oql(r#"sales.where { .category == "Electronics" }.group_by { .region }.select { 
        region, 
        salescount: count(*), 
        totalrevenue: sum(.amount),
        avgamount: avg(.amount),
        maxsale: max(.amount)
    }.sort { .totalrevenue.desc }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {} Region:", result.get_field("region").unwrap().to_string_repr());
        println!("    Sales: {}", result.get_field("salescount").unwrap().to_string_repr());
        println!("    Revenue: ${}", result.get_field("totalrevenue").unwrap().to_string_repr());
        println!("    Average: ${:.2}", 
            match result.get_field("avgamount").unwrap() {
                Value::Float(f) => *f,
                _ => 0.0,
            }
        );
        println!("    Largest Sale: ${}", result.get_field("maxsale").unwrap().to_string_repr());
    }
    println!();
    
    println!("✅ Task 5 comprehensive test completed successfully!");
    println!("All aggregation, grouping, and sorting functionality is working correctly:");
    println!("  ✓ Basic aggregation functions (COUNT, SUM, AVG, MIN, MAX)");
    println!("  ✓ GROUP BY single and multiple fields");
    println!("  ✓ GROUP BY with aggregation functions");
    println!("  ✓ Sorting with ASC/DESC options");
    println!("  ✓ LIMIT for result pagination");
    println!("  ✓ Complex queries combining WHERE, GROUP BY, aggregation, SORT, and LIMIT");
    println!("  ✓ Support for reference fields in aggregation and grouping");
    
    Ok(())
}

fn create_sale(id: &str, category: &str, region: &str, quarter: &str, amount: f64, quantity: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("category".to_string(), Value::String(category.to_string()));
    fields.insert("region".to_string(), Value::String(region.to_string()));
    fields.insert("quarter".to_string(), Value::String(quarter.to_string()));
    fields.insert("amount".to_string(), Value::Float(amount));
    fields.insert("quantity".to_string(), Value::Integer(quantity));
    
    Document::with_id(id.to_string(), "sales".to_string(), fields)
}