// Dual Syntax Demo for IF/THEN/ELSE

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
    println!("=== OQL Dual Syntax Demo ===\n");

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());

    // Create test data
    setup_test_data(&store)?;

    // Demo 1: Single expression syntax (with then/else)
    println!("1. Single Expression Syntax (with then/else)");
    println!("--------------------------------------------");
    
    let query = r#"
        customers.select {
            name,
            status: if .age >= 18 then "adult" else "minor",
            discount: if .premium == true then 0.2 else 0.1
        }
    "#;
    
    println!("Query: {}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: status={:?}, discount={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("status").unwrap_or(&Value::Null),
                            doc.get_field("discount").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }
    
    println!();

    // Demo 2: Block syntax (with braces, no then/else)
    println!("2. Block Syntax (with braces, no then/else)");
    println!("-------------------------------------------");
    
    let query = r#"
        customers.select {
            name,
            category: if .age >= 21 {
                "full_adult"
            } else {
                "young_adult_or_minor"
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
                        println!("  - {:?}: category={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("category").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }
    
    println!();

    // Demo 3: Complex nested block syntax
    println!("3. Complex Nested Block Syntax");
    println!("------------------------------");
    
    let query = r#"
        customers.select {
            name,
            tier: if .premium == true {
                if .age >= 25 {
                    "premium_adult"
                } else {
                    "premium_young"
                }
            } else {
                if .active == true {
                    "standard_active"
                } else {
                    "standard_inactive"
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
            fields.insert("age".to_string(), Value::Integer(22));
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