// Simple Conditional Logic Test

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::{parse_oql},
    storage::{RocksDBStore, DocumentStore},
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Simple Conditional Logic Test ===\n");

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());

    // Create test data
    let customer = Document::with_id("cust_1".to_string(), "customers".to_string(), {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String("Alice".to_string()));
        fields.insert("age".to_string(), Value::Integer(25));
        fields.insert("premium".to_string(), Value::Boolean(true));
        fields.insert("active".to_string(), Value::Boolean(true));
        fields
    });
    store.put_document(&customer)?;

    // Test 1: Simple IF/THEN/ELSE
    println!("1. Simple IF/THEN/ELSE");
    let query = r#"customers.select { name, status: if .age >= 18 then "adult" else "minor" }"#;
    println!("Query: {}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: status={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("status").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }
    
    println!();

    // Test 2: Simple logical AND
    println!("2. Simple Logical AND");
    let query = r#"customers.select { name, eligible: .premium and .active }"#;
    println!("Query: {}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: eligible={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("eligible").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }

    println!("\n=== Test Complete ===");
    Ok(())
}