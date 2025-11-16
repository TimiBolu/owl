// Built-in Functions Demo for OQL
// Demonstrates string manipulation, math, and date/time functions

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::parse_oql,
    storage::{DocumentStore, RocksDBStore},
};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OQL Built-in Functions Demo ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create sample data
    create_sample_data(&store)?;
    
    println!("📊 Sample Data Created");
    println!("- Products with names, prices, descriptions");
    println!("- Users with names, ages, emails");
    println!("- Orders with amounts and dates\n");
    
    // Demonstrate string functions
    demonstrate_string_functions(&executor)?;
    
    // Demonstrate math functions
    demonstrate_math_functions(&executor)?;
    
    // Demonstrate date/time functions
    demonstrate_datetime_functions(&executor)?;
    
    // Demonstrate utility functions
    demonstrate_utility_functions(&executor)?;
    
    // Demonstrate complex combinations
    demonstrate_complex_combinations(&executor)?;
    
    println!("✅ Built-in Functions Demo Complete!");
    println!("\nOQL now supports 40+ built-in functions across:");
    println!("  📝 String manipulation (trim, upper, lower, concat, replace, etc.)");
    println!("  🔢 Math operations (abs, round, sqrt, pow, min, max, etc.)");
    println!("  📅 Date/time handling (now, format, parse, extract, etc.)");
    println!("  🔧 Utility functions (coalesce, type conversion, etc.)");
    
    Ok(())
}

fn create_sample_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    // Products
    let products = vec![
        ("prod1", "  Wireless Headphones  ", 99.99, "Premium wireless headphones with noise cancellation"),
        ("prod2", "GAMING LAPTOP", 1299.50, "High-performance gaming laptop"),
        ("prod3", "smartphone case", 19.99, "Protective case for smartphones"),
    ];
    
    for (id, name, price, description) in products {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        fields.insert("price".to_string(), Value::Float(price));
        fields.insert("description".to_string(), Value::String(description.to_string()));
        
        let doc = Document::with_id(id.to_string(), "products".to_string(), fields);
        store.put_document(&doc)?;
    }
    
    // Users
    let users = vec![
        ("user1", "Alice Johnson", 28, "alice@example.com"),
        ("user2", "Bob Smith", 35, "bob@company.com"),
        ("user3", "Charlie Brown", 42, "charlie.brown@email.org"),
    ];
    
    for (id, name, age, email) in users {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        fields.insert("age".to_string(), Value::Integer(age));
        fields.insert("email".to_string(), Value::String(email.to_string()));
        
        let doc = Document::with_id(id.to_string(), "users".to_string(), fields);
        store.put_document(&doc)?;
    }
    
    // Orders
    let orders = vec![
        ("order1", 150.75, "user1"),
        ("order2", 89.99, "user2"),
        ("order3", 299.00, "user3"),
    ];
    
    for (id, amount, user_id) in orders {
        let mut fields = HashMap::new();
        fields.insert("amount".to_string(), Value::Float(amount));
        fields.insert("user_id".to_string(), Value::String(user_id.to_string()));
        fields.insert("created_at".to_string(), Value::DateTime(Utc::now()));
        
        let doc = Document::with_id(id.to_string(), "orders".to_string(), fields);
        store.put_document(&doc)?;
    }
    
    Ok(())
}

fn demonstrate_string_functions(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔤 STRING FUNCTIONS");
    println!("==================");
    
    // String cleaning and formatting
    println!("\n1. String Cleaning & Formatting:");
    let query = parse_oql(r#"products.select { 
        original: .name,
        cleaned: trim(.name),
        uppercase: upper(trim(.name)),
        lowercase: lower(trim(.name)),
        length: length(trim(.name))
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  Original: '{}'", result.get_field("original").unwrap().to_string_repr());
        println!("  Cleaned:  '{}'", result.get_field("cleaned").unwrap().to_string_repr());
        println!("  Upper:    '{}'", result.get_field("uppercase").unwrap().to_string_repr());
        println!("  Lower:    '{}'", result.get_field("lowercase").unwrap().to_string_repr());
        println!("  Length:   {}", result.get_field("length").unwrap().to_string_repr());
        println!();
    }
    
    // String manipulation
    println!("2. String Manipulation:");
    let query = parse_oql(r#"products.where { .id == "prod1" }.select {
        name: .name,
        substring: substring(trim(.name), 0, 8),
        replaced: replace(.description, "wireless", "Bluetooth"),
        concatenated: concat("Product: ", trim(.name))
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    if let Some(result) = results.first() {
        println!("  Name: {}", result.get_field("name").unwrap().to_string_repr());
        println!("  Substring: {}", result.get_field("substring").unwrap().to_string_repr());
        println!("  Replaced: {}", result.get_field("replaced").unwrap().to_string_repr());
        println!("  Concatenated: {}", result.get_field("concatenated").unwrap().to_string_repr());
    }
    
    // String testing
    println!("\n3. String Testing:");
    let query = parse_oql(r#"products.select {
        name: trim(.name),
        startswithw: starts_with(trim(.name), "W"),
        endswiths: ends_with(trim(.name), "s"),
        containsgaming: contains(upper(.name), "GAMING")
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  '{}' -> starts with 'W': {}, ends with 's': {}, contains 'GAMING': {}",
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("startswithw").unwrap().to_string_repr(),
            result.get_field("endswiths").unwrap().to_string_repr(),
            result.get_field("containsgaming").unwrap().to_string_repr()
        );
    }
    
    Ok(())
}

fn demonstrate_math_functions(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🔢 MATH FUNCTIONS");
    println!("=================");
    
    // Basic math operations
    println!("\n1. Basic Math Operations:");
    let query = parse_oql(r#"products.select {
        name: trim(.name),
        price: .price,
        rounded: round(.price),
        floored: floor(.price),
        ceiled: ceil(.price),
        absolute: abs(.price)
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {} -> Price: ${}, Rounded: {}, Floor: {}, Ceil: {}, Abs: {}",
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr(),
            result.get_field("rounded").unwrap().to_string_repr(),
            result.get_field("floored").unwrap().to_string_repr(),
            result.get_field("ceiled").unwrap().to_string_repr(),
            result.get_field("absolute").unwrap().to_string_repr()
        );
    }
    
    // Advanced math operations
    println!("\n2. Advanced Math Operations:");
    let query = parse_oql(r#"products.where { .id == "prod2" }.select {
        price: .price,
        sqrtprice: sqrt(.price),
        squared: pow(.price, 2),
        logprice: log(.price),
        sinprice: sin(.price)
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    if let Some(result) = results.first() {
        println!("  Price: ${}", result.get_field("price").unwrap().to_string_repr());
        println!("  Square Root: {:.2}", match result.get_field("sqrtprice").unwrap() {
            Value::Float(f) => *f,
            _ => 0.0,
        });
        println!("  Squared: {:.2}", match result.get_field("squared").unwrap() {
            Value::Float(f) => *f,
            _ => 0.0,
        });
        println!("  Log10: {:.2}", match result.get_field("logprice").unwrap() {
            Value::Float(f) => *f,
            _ => 0.0,
        });
        println!("  Sin: {:.4}", match result.get_field("sinprice").unwrap() {
            Value::Float(f) => *f,
            _ => 0.0,
        });
    }
    
    // Comparison functions
    println!("\n3. Comparison Functions:");
    let query = parse_oql(r#"products.select {
        name: trim(.name),
        price: .price,
        minwith100: min(.price, 100),
        maxwith100: max(.price, 100)
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {} -> Price: ${}, Min(price,100): {}, Max(price,100): {}",
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr(),
            result.get_field("minwith100").unwrap().to_string_repr(),
            result.get_field("maxwith100").unwrap().to_string_repr()
        );
    }
    
    Ok(())
}

fn demonstrate_datetime_functions(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📅 DATE/TIME FUNCTIONS");
    println!("======================");
    
    // Current date/time functions
    println!("\n1. Current Date/Time:");
    let query = parse_oql(r#"orders.where { .id == "order1" }.select {
        currenttime: now(),
        currentdate: today(),
        onehouraago: hours_ago(1),
        sevendaysago: days_ago(7)
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    if let Some(result) = results.first() {
        println!("  Current Time: {}", result.get_field("currenttime").unwrap().to_string_repr());
        println!("  Current Date: {}", result.get_field("currentdate").unwrap().to_string_repr());
        println!("  1 Hour Ago: {}", result.get_field("onehouraago").unwrap().to_string_repr());
        println!("  7 Days Ago: {}", result.get_field("sevendaysago").unwrap().to_string_repr());
    }
    
    // Date extraction functions
    println!("\n2. Date Extraction:");
    let query = parse_oql(r#"orders.select {
        orderid: .id,
        createdat: .created_at,
        year: year(.created_at),
        month: month(.created_at),
        day: day(.created_at),
        hour: hour(.created_at),
        weekday: weekday(.created_at)
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  Order {} -> Year: {}, Month: {}, Day: {}, Hour: {}, Weekday: {}",
            result.get_field("orderid").unwrap().to_string_repr(),
            result.get_field("year").unwrap().to_string_repr(),
            result.get_field("month").unwrap().to_string_repr(),
            result.get_field("day").unwrap().to_string_repr(),
            result.get_field("hour").unwrap().to_string_repr(),
            result.get_field("weekday").unwrap().to_string_repr()
        );
    }
    
    // Date formatting
    println!("\n3. Date Formatting:");
    let query = parse_oql(r#"orders.where { .id == "order1" }.select {
        createdat: .created_at,
        formatteddate: format_date(.created_at, "%Y-%m-%d"),
        formattedtime: format_date(.created_at, "%H:%M:%S"),
        formattedfull: format_date(.created_at, "%Y-%m-%d %H:%M:%S")
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    if let Some(result) = results.first() {
        println!("  Original: {}", result.get_field("createdat").unwrap().to_string_repr());
        println!("  Date Only: {}", result.get_field("formatteddate").unwrap().to_string_repr());
        println!("  Time Only: {}", result.get_field("formattedtime").unwrap().to_string_repr());
        println!("  Full Format: {}", result.get_field("formattedfull").unwrap().to_string_repr());
    }
    
    Ok(())
}

fn demonstrate_utility_functions(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🔧 UTILITY FUNCTIONS");
    println!("====================");
    
    // Type conversion and inspection
    println!("\n1. Type Conversion & Inspection:");
    let query = parse_oql(r#"users.select {
        name: .name,
        age: .age,
        ageasstring: to_string(.age),
        ageasfloat: to_float(.age),
        nametype: type_of(.name),
        agetype: type_of(.age)
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {} (age {}) -> Age as String: '{}', Age as Float: {}, Name Type: {}, Age Type: {}",
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("age").unwrap().to_string_repr(),
            result.get_field("ageasstring").unwrap().to_string_repr(),
            result.get_field("ageasfloat").unwrap().to_string_repr(),
            result.get_field("nametype").unwrap().to_string_repr(),
            result.get_field("agetype").unwrap().to_string_repr()
        );
    }
    
    // Null handling
    println!("\n2. Null Handling:");
    let query = parse_oql(r#"users.select {
        name: .name,
        email: .email,
        safeemail: coalesce(.email, "No email provided")
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  {} -> Email: {}, Safe Email: {}",
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("email").unwrap().to_string_repr(),
            result.get_field("safeemail").unwrap().to_string_repr()
        );
    }
    
    Ok(())
}

fn demonstrate_complex_combinations(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🎯 COMPLEX COMBINATIONS");
    println!("=======================");
    
    // Nested function calls and complex expressions
    println!("\n1. Product Analysis with Complex Functions:");
    let query = parse_oql(r#"products.select {
        productname: upper(trim(.name)),
        pricecategory: concat(
            "Price: $",
            to_string(round(.price))
        ),
        descriptionsummary: concat(
            substring(.description, 0, 20),
            "..."
        ),
        priceanalysis: concat(
            "Min with $50: $",
            to_string(min(.price, 50)),
            ", Max with $200: $", 
            to_string(max(.price, 200))
        )
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    for result in &results {
        println!("  📦 {}", result.get_field("productname").unwrap().to_string_repr());
        println!("     Category: {}", result.get_field("pricecategory").unwrap().to_string_repr());
        println!("     Summary: {}", result.get_field("descriptionsummary").unwrap().to_string_repr());
        println!("     Analysis: {}", result.get_field("priceanalysis").unwrap().to_string_repr());
        println!();
    }
    
    // Functions in WHERE clauses
    println!("2. Advanced Filtering with Functions:");
    let query = parse_oql(r#"products.where { 
        contains(lower(.description), "gaming") == true 
    }.select {
        name: trim(.name),
        price: .price,
        namelength: length(trim(.name))
    }"#)?;
    let results = executor.execute_query(&query)?;
    
    println!("  Products containing 'gaming' in description:");
    for result in &results {
        println!("    {} (length: {}) - ${}", 
            result.get_field("name").unwrap().to_string_repr(),
            result.get_field("namelength").unwrap().to_string_repr(),
            result.get_field("price").unwrap().to_string_repr()
        );
    }
    
    Ok(())
}