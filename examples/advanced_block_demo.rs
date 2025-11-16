// Advanced Block Expressions Demo

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
    println!("=== Advanced Block Expressions Demo ===\n");

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());

    // Create test data
    setup_test_data(&store)?;

    // Demo 1: Single expression blocks (current implementation)
    println!("1. Single Expression Blocks");
    println!("---------------------------");
    
    let query = r#"
        customers.select {
            name,
            tier: if .premium == true then {
                "Premium Tier"
            } else {
                "Standard Tier"
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
                        println!("  - {:?}: tier={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("tier").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }
    
    println!();

    // Demo 2: Deeply nested conditional blocks
    println!("2. Deeply Nested Conditional Blocks");
    println!("-----------------------------------");
    
    let query = r#"
        customers.select {
            name,
            classification: if .age >= 21 then {
                if .premium == true then {
                    if .active == true then "Premium Active Adult" else "Premium Inactive Adult"
                } else {
                    if .active == true then "Standard Active Adult" else "Standard Inactive Adult"
                }
            } else {
                if .active == true then {
                    "Active Minor"
                } else {
                    "Inactive Minor"
                }
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
                        println!("  - {:?}: classification={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("classification").unwrap_or(&Value::Null)
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
            fields.insert("age".to_string(), Value::Integer(30));
            fields.insert("premium".to_string(), Value::Boolean(false));
            fields.insert("active".to_string(), Value::Boolean(false));
            fields
        }),
        Document::with_id("cust_4".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("David".to_string()));
            fields.insert("age".to_string(), Value::Integer(19));
            fields.insert("premium".to_string(), Value::Boolean(true));
            fields.insert("active".to_string(), Value::Boolean(false));
            fields
        }),
    ];

    for customer in customers {
        store.put_document(&customer)?;
    }

    Ok(())
}