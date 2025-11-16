// Query Executor Demo - Demonstrates basic CRUD operations

use owl_db::{parse_oql, QueryExecutor, Schema, FieldType, DocumentStore};
use owl_db::storage::RocksDBStore;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OQL Query Executor Demo ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create a schema for users
    let mut user_fields = HashMap::new();
    user_fields.insert("name".to_string(), FieldType::String);
    user_fields.insert("age".to_string(), FieldType::Integer);
    user_fields.insert("active".to_string(), FieldType::Boolean);
    user_fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
    
    let user_schema = Schema::new("users".to_string(), user_fields);
    store.put_schema(&user_schema)?;
    
    println!("1. INSERT Operations");
    println!("===================");
    
    // Insert some users
    let insert_queries = vec![
        r#"users.insert { name: "Alice", age: 30, active: true, email: "alice@example.com" }"#,
        r#"users.insert { name: "Bob", age: 25, active: false }"#,
        r#"users.insert { name: "Charlie", age: 35, active: true, email: "charlie@example.com" }"#,
    ];
    
    for query_str in insert_queries {
        println!("Query: {}", query_str);
        let query = parse_oql(query_str)?;
        let results = executor.execute_query(&query)?;
        println!("Inserted: {} document(s)", results.len());
        if let Some(doc) = results.first() {
            println!("  ID: {}, Name: {:?}", doc.id, doc.get_field("name"));
        }
        println!();
    }
    
    println!("2. SELECT Operations");
    println!("===================");
    
    // Select all users
    println!("Query: users.select {{ name, age, active }}");
    let query = parse_oql("users.select { name, age, active }")?;
    let results = executor.execute_query(&query)?;
    println!("Found {} users:", results.len());
    for doc in &results {
        println!("  - Name: {:?}, Age: {:?}, Active: {:?}", 
                 doc.get_field("name"), 
                 doc.get_field("age"), 
                 doc.get_field("active"));
    }
    println!();
    
    // Select with alias
    println!("Query: users.select {{ full_name: .name, years: .age }}");
    let query = parse_oql("users.select { full_name: .name, years: .age }")?;
    let results = executor.execute_query(&query)?;
    println!("Found {} users with aliases:", results.len());
    for doc in &results {
        println!("  - Full Name: {:?}, Years: {:?}", 
                 doc.get_field("full_name"), 
                 doc.get_field("years"));
    }
    println!();
    
    println!("3. WHERE Filtering");
    println!("=================");
    
    // Filter by age
    println!("Query: users.where {{ .age >= 30 }}");
    let query = parse_oql("users.where { .age >= 30 }")?;
    let results = executor.execute_query(&query)?;
    println!("Found {} users aged 30+:", results.len());
    for doc in &results {
        println!("  - Name: {:?}, Age: {:?}", 
                 doc.get_field("name"), 
                 doc.get_field("age"));
    }
    println!();
    
    // Filter by boolean
    println!("Query: users.where {{ .active == true }}");
    let query = parse_oql("users.where { .active == true }")?;
    let results = executor.execute_query(&query)?;
    println!("Found {} active users:", results.len());
    for doc in &results {
        println!("  - Name: {:?}, Active: {:?}", 
                 doc.get_field("name"), 
                 doc.get_field("active"));
    }
    println!();
    
    println!("4. Combined Operations");
    println!("=====================");
    
    // WHERE + SELECT
    println!("Query: users.where {{ .age >= 30 }}.select {{ name }}");
    let query = parse_oql("users.where { .age >= 30 }.select { name }")?;
    let results = executor.execute_query(&query)?;
    println!("Found {} users aged 30+ (names only):", results.len());
    for doc in &results {
        println!("  - Name: {:?}", doc.get_field("name"));
    }
    println!();
    
    println!("5. UPDATE Operations");
    println!("===================");
    
    // Update specific user
    println!("Query: users.where {{ .name == \"Alice\" }}.update {{ age: 31 }}");
    let query = parse_oql(r#"users.where { .name == "Alice" }.update { age: 31 }"#)?;
    let results = executor.execute_query(&query)?;
    println!("Updated {} user(s):", results.len());
    for doc in &results {
        println!("  - Name: {:?}, New Age: {:?}", 
                 doc.get_field("name"), 
                 doc.get_field("age"));
    }
    println!();
    
    // Update multiple users
    println!("Query: users.where {{ .active == true }}.update {{ age: 99 }}");
    let query = parse_oql("users.where { .active == true }.update { age: 99 }")?;
    let results = executor.execute_query(&query)?;
    println!("Updated {} active user(s) to age 99:", results.len());
    for doc in &results {
        println!("  - Name: {:?}, New Age: {:?}", 
                 doc.get_field("name"), 
                 doc.get_field("age"));
    }
    println!();
    
    println!("6. DELETE Operations");
    println!("===================");
    
    // Delete specific user
    println!("Query: users.where {{ .name == \"Bob\" }}.delete");
    let query = parse_oql(r#"users.where { .name == "Bob" }.delete"#)?;
    let results = executor.execute_query(&query)?;
    println!("Deleted {} user(s):", results.len());
    for doc in &results {
        println!("  - Deleted: {:?}", doc.get_field("name"));
    }
    println!();
    
    // Verify remaining users
    println!("Remaining users:");
    let query = parse_oql("users.select { name, age }")?;
    let results = executor.execute_query(&query)?;
    println!("Found {} remaining users:", results.len());
    for doc in &results {
        println!("  - Name: {:?}, Age: {:?}", 
                 doc.get_field("name"), 
                 doc.get_field("age"));
    }
    
    println!("\n=== Demo Complete ===");
    Ok(())
}