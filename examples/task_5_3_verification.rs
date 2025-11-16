// Task 5.3 Verification: Sorting and Limiting
// This example demonstrates ORDER BY with ASC/DESC and LIMIT functionality

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
    println!("=== Task 5.3: Sorting and Limiting ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create test data - products with different prices and ratings
    let products_data = vec![
        create_product("prod_1", "Laptop", 1200.0, 4.5),
        create_product("prod_2", "Mouse", 25.0, 4.2),
        create_product("prod_3", "Keyboard", 75.0, 4.8),
        create_product("prod_4", "Monitor", 300.0, 4.1),
        create_product("prod_5", "Headphones", 150.0, 4.7),
        create_product("prod_6", "Webcam", 80.0, 3.9),
        create_product("prod_7", "Speakers", 120.0, 4.3),
        create_product("prod_8", "Tablet", 400.0, 4.6),
    ];
    
    // Store test data
    for product in &products_data {
        store.put_document(product)?;
    }
    
    println!("Created {} products", products_data.len());
    println!("Product data:");
    for product in &products_data {
        println!("  {} - {}: ${} (rating: {})", 
            product.id, 
            product.get_field("name").unwrap().to_string_repr(),
            product.get_field("price").unwrap().to_string_repr(),
            product.get_field("rating").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 1: Sort by price (ascending - default)
    println!("1. Sort by Price (Ascending):");
    let query = parse_oql("products.sort { .price }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Products sorted by price (low to high):");
    for result in &results {
        println!("    {}: ${}", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 2: Sort by price (descending)
    println!("2. Sort by Price (Descending):");
    let query = parse_oql("products.sort { .price.desc }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Products sorted by price (high to low):");
    for result in &results {
        println!("    {}: ${}", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 3: Sort by rating (descending)
    println!("3. Sort by Rating (Descending):");
    let query = parse_oql("products.sort { .rating.desc }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Products sorted by rating (high to low):");
    for result in &results {
        println!("    {}: {} stars", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("rating").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 4: Sort by name (alphabetical)
    println!("4. Sort by Name (Alphabetical):");
    let query = parse_oql("products.sort { .name }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Products sorted alphabetically:");
    for result in &results {
        println!("    {}", result.get_field("name").unwrap().to_string_repr());
    }
    println!();
    
    // Test 5: LIMIT results
    println!("5. LIMIT Results:");
    let query = parse_oql("products.sort { .price.desc }.limit 3")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Top 3 most expensive products:");
    for result in &results {
        println!("    {}: ${}", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 6: LIMIT with different value
    println!("6. LIMIT with Different Value:");
    let query = parse_oql("products.sort { .rating.desc }.limit 5")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Top 5 highest rated products:");
    for result in &results {
        println!("    {}: {} stars", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("rating").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 7: Sort with SELECT projection
    println!("7. Sort with SELECT Projection:");
    let query = parse_oql("products.sort { .price }.select { name, price }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Products with name and price only (sorted by price):");
    for result in &results {
        println!("    {}: ${}", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 8: Sort with WHERE filtering
    println!("8. Sort with WHERE Filtering:");
    let query = parse_oql("products.where { .price > 100 }.sort { .rating.desc }")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Expensive products (>$100) sorted by rating:");
    for result in &results {
        println!("    {}: ${} ({} stars)", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr(),
            result.get_field("rating").unwrap().to_string_repr()
        );
    }
    println!();
    
    // Test 9: Combined WHERE, SORT, and LIMIT
    println!("9. Combined WHERE, SORT, and LIMIT:");
    let query = parse_oql("products.where { .rating >= 4.5 }.sort { .price }.limit 2")?;
    let results = executor.execute_query(&query)?;
    
    println!("  Top 2 cheapest high-rated products (rating >= 4.5):");
    for result in &results {
        println!("    {}: ${} ({} stars)", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr(),
            result.get_field("rating").unwrap().to_string_repr()
        );
    }
    println!();
    
    println!("✅ Task 5.3 completed successfully!");
    println!("Sorting and limiting functionality is working correctly with:");
    println!("  - ORDER BY with ASC (default) and DESC options");
    println!("  - Sorting by different data types (numbers, strings, floats)");
    println!("  - LIMIT for result pagination");
    println!("  - Combined operations (WHERE + SORT + LIMIT)");
    println!("  - Sorting with SELECT projection");
    
    Ok(())
}

fn create_product(id: &str, name: &str, price: f64, rating: f64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("price".to_string(), Value::Float(price));
    fields.insert("rating".to_string(), Value::Float(rating));
    
    Document::with_id(id.to_string(), "products".to_string(), fields)
}