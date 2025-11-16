// Multi-Expression Blocks Demo

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
    println!("=== OQL Multi-Expression Blocks Demo ===\n");

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());

    // Create test data
    setup_test_data(&store)?;

    // Demo: Multi-expression blocks
    println!("Multi-Expression Blocks");
    println!("-----------------------");
    
    // For now, let's test with single expressions in blocks
    // In the future, we could extend this to support multiple statements
    let query = r#"
        customers.select {
            name,
            complex_logic: if .age >= 21 then {
                if .premium == true then "VIP Adult" else "Regular Adult"
            } else {
                if .active == true then "Active Minor" else "Inactive Minor"
            }
        }
    "#;
    
    println!("Query: {}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: complex_logic={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("complex_logic").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}

fn setup_test_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    let customers = vec![
        Document::with_id("cust_1".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Alice".to_string()));
            fields.insert("age".to_string(), Value::Integer(25));
            fields.insert("premium".to_string(), Value::Boolean(true));
            fields.insert("active".to_string(), Value::Boolean(true));
            fields
        }),
        Document::with_id("cust_2".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Bob".to_string()));
            fields.insert("age".to_string(), Value::Integer(17));
            fields.insert("premium".to_string(), Value::Boolean(false));
            fields.insert("active".to_string(), Value::Boolean(true));
            fields
        }),
        Document::with_id("cust_3".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Carol".to_string()));
            fields.insert("age".to_string(), Value::Integer(19));
            fields.insert("premium".to_string(), Value::Boolean(false));
            fields.insert("active".to_string(), Value::Boolean(false));
            fields
        }),
    ];

    for customer in customers {
        store.put_document(&customer)?;
    }

    Ok(())
}