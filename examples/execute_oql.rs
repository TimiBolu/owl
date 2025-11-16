// Execute OQL Example
// Simple example showing how to execute OQL using existing utilities

use owl_db::{parse_oql, QueryExecutor, Document, Value, DocumentStore};
use owl_db::storage::RocksDBStore;
use std::sync::Arc;
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦉 Execute OQL Example");
    println!("======================");
    
    // Create database and executor using existing utilities
    let store = Arc::new(RocksDBStore::new("./demo_db")?);
    let executor = QueryExecutor::new(store.clone());
    
    // Add some sample data using existing Document API
    add_sample_data(&store)?;
    
    // Execute OQL queries using existing parse_oql
    let queries = vec![
        "users",
        "users.select { id, name, email }",
        "users.where { .age > 25 }",
        "users.where { .age > 25 }.select { id, name, age }",
        "users.sort { .name }",
        "users.limit(3)",
    ];
    
    for query_str in queries {
        println!("\n🔍 Executing: {}", query_str);
        
        // Use existing parse_oql function
        match parse_oql(query_str) {
            Ok(query) => {
                // Use existing executor
                match executor.execute_query(&query) {
                    Ok(documents) => {
                        println!("   ✅ Found {} documents", documents.len());
                        for doc in documents.iter().take(2) {
                            println!("      - {}: {}", doc.id, 
                                doc.get_field("name")
                                    .map(|v| v.to_string_repr())
                                    .unwrap_or_else(|| "No name".to_string())
                            );
                        }
                    }
                    Err(e) => println!("   ❌ Execution error: {}", e),
                }
            }
            Err(e) => println!("   ❌ Parse error: {}", e),
        }
    }
    
    // Cleanup
    std::fs::remove_dir_all("./demo_db").ok();
    
    Ok(())
}

fn add_sample_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("📦 Adding sample data...");
    
    let users = vec![
        ("user_001", "Alice Smith", "alice@example.com", 28),
        ("user_002", "Bob Jones", "bob@example.com", 35),
        ("user_003", "Carol White", "carol@example.com", 22),
        ("user_004", "David Brown", "david@example.com", 41),
    ];
    
    for (id, name, email, age) in users {
        let mut fields = HashMap::new();
        fields.insert("id".to_string(), Value::String(id.to_string()));
        fields.insert("name".to_string(), Value::String(name.to_string()));
        fields.insert("email".to_string(), Value::String(email.to_string()));
        fields.insert("age".to_string(), Value::Integer(age));
        fields.insert("created_at".to_string(), Value::DateTime(chrono::Utc::now()));
        
        let document = Document::with_id(id.to_string(), "users".to_string(), fields);
        store.put_document(&document)?;
    }
    
    println!("   Added {} users", 4);
    Ok(())
}