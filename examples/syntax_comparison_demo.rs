// Syntax Comparison Demo - Single vs Block Expressions

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
    println!("=== OQL Syntax Comparison Demo ===\n");

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());

    // Create test data
    setup_test_data(&store)?;

    println!("Demonstrating the dual syntax approach:");
    println!("- Single expressions: use 'then' and 'else' keywords");
    println!("- Block expressions: use braces without 'then'/'else'");
    println!();

    // Demo 1: Single expression syntax - best for simple conditions
    println!("1. Single Expression Syntax (Simple Conditions)");
    println!("===============================================");
    
    let query = r#"
        customers.select {
            name,
            age_group: if .age >= 18 then "adult" else "minor",
            membership: if .premium then "premium" else "standard",
            status: if .active then "active" else "inactive"
        }
    "#;
    
    println!("Query:");
    println!("{}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: age_group={:?}, membership={:?}, status={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("age_group").unwrap_or(&Value::Null),
                            doc.get_field("membership").unwrap_or(&Value::Null),
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

    // Demo 2: Block expression syntax - best for complex nested logic
    println!("2. Block Expression Syntax (Complex Nested Logic)");
    println!("=================================================");
    
    let query = r#"
        customers.select {
            name,
            detailed_category: if .premium {
                if .age >= 25 {
                    if .active {
                        "Premium Active Adult"
                    } else {
                        "Premium Inactive Adult"
                    }
                } else {
                    if .active {
                        "Premium Active Young"
                    } else {
                        "Premium Inactive Young"
                    }
                }
            } else {
                if .age >= 25 {
                    if .active {
                        "Standard Active Adult"
                    } else {
                        "Standard Inactive Adult"
                    }
                } else {
                    if .active {
                        "Standard Active Young"
                    } else {
                        "Standard Inactive Young"
                    }
                }
            }
        }
    "#;
    
    println!("Query:");
    println!("{}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: detailed_category={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("detailed_category").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }

    println!();
    println!("Key Benefits:");
    println!("- Single syntax: Concise for simple conditions");
    println!("- Block syntax: Clean structure for complex logic, no repetitive keywords");
    println!("- Both syntaxes can be mixed in the same query as needed");

    println!("\n=== Demo Complete ===");
    Ok(())
}

fn setup_test_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    let customers = vec![
        Document::with_id("cust_1".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Alice".to_string()));
            fields.insert("age".to_string(), Value::Integer(28));
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