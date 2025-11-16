// Conditional Logic and Pattern Matching Demo

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
    println!("=== OQL Conditional Logic and Pattern Matching Demo ===\n");

    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let mut executor = QueryExecutor::new(store.clone());

    // Create test data
    setup_test_data(&store)?;

    // Demo 1: IF/THEN/ELSE expressions
    println!("1. IF/THEN/ELSE Expressions");
    println!("----------------------------");
    
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

    // Demo 2: Logical operators in expressions
    println!("2. Logical Operators (AND, OR, NOT)");
    println!("------------------------------------");
    
    let query = r#"
        customers.select {
            name,
            eligible: .premium and .active,
            needs_review: not .verified
        }
    "#;
    
    println!("Query: {}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: eligible={:?}, needs_review={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("eligible").unwrap_or(&Value::Null),
                            doc.get_field("needs_review").unwrap_or(&Value::Null)
                        );
                    }
                }
                Err(e) => println!("Execution error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }
    
    println!();

    // Demo 3: MATCH expressions
    println!("3. MATCH Expressions");
    println!("--------------------");
    
    let query = r#"
        customers.select {
            name,
            tier: match .premium {
                true -> "premium",
                false -> "standard",
                _ -> "unknown"
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

    // Demo 4: Complex conditional logic
    println!("4. Complex Conditional Logic");
    println!("-----------------------------");
    
    let query = r#"
        customers.select {
            name,
            recommendation: if .premium == true then
                "VIP treatment"
            else if .active == true then
                "Standard service"
            else
                "Basic service"
        }
    "#;
    
    println!("Query: {}", query);
    
    match parse_oql(query) {
        Ok(parsed_query) => {
            match executor.execute_query(&parsed_query) {
                Ok(results) => {
                    println!("Results:");
                    for doc in results {
                        println!("  - {:?}: recommendation={:?}", 
                            doc.get_field("name").unwrap_or(&Value::Null),
                            doc.get_field("recommendation").unwrap_or(&Value::Null)
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
    // Create test customers
    let customers = vec![
        Document::with_id("cust_1".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Alice".to_string()));
            fields.insert("age".to_string(), Value::Integer(25));
            fields.insert("premium".to_string(), Value::Boolean(true));
            fields.insert("active".to_string(), Value::Boolean(true));
            fields.insert("verified".to_string(), Value::Boolean(true));
            fields
        }),
        Document::with_id("cust_2".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Bob".to_string()));
            fields.insert("age".to_string(), Value::Integer(17));
            fields.insert("premium".to_string(), Value::Boolean(false));
            fields.insert("active".to_string(), Value::Boolean(true));
            fields.insert("verified".to_string(), Value::Boolean(false));
            fields
        }),
        Document::with_id("cust_3".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Carol".to_string()));
            fields.insert("age".to_string(), Value::Integer(70));
            fields.insert("premium".to_string(), Value::Boolean(false));
            fields.insert("active".to_string(), Value::Boolean(false));
            fields.insert("verified".to_string(), Value::Boolean(true));
            fields
        }),
        Document::with_id("cust_4".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("David".to_string()));
            fields.insert("age".to_string(), Value::Integer(30));
            fields.insert("premium".to_string(), Value::Boolean(true));
            fields.insert("active".to_string(), Value::Boolean(true));
            fields.insert("verified".to_string(), Value::Boolean(true));
            fields
        }),
    ];

    for customer in customers {
        store.put_document(&customer)?;
    }

    Ok(())
}