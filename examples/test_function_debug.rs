// Debug function calls in OQL

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
    println!("=== Function Call Debug ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create test document
    let mut fields = HashMap::new();
    fields.insert("text".to_string(), Value::String("Hello World".to_string()));
    fields.insert("number".to_string(), Value::Integer(42));
    
    let doc = Document::with_id("test1".to_string(), "test_docs".to_string(), fields);
    store.put_document(&doc)?;
    
    println!("Created test document: {:?}", doc);
    
    // Test 1: Simple field access (should work)
    println!("\n1. Testing simple field access:");
    let query_str = r#"test_docs.select { text }"#;
    println!("Query: {}", query_str);
    
    match parse_oql(query_str) {
        Ok(query) => {
            println!("Parsed successfully: {:?}", query);
            match executor.execute_query(&query) {
                Ok(results) => {
                    println!("Results: {:?}", results);
                }
                Err(e) => {
                    println!("Execution error: {:?}", e);
                }
            }
        }
        Err(e) => {
            println!("Parse error: {:?}", e);
        }
    }
    
    // Test 2: Function call in SELECT
    println!("\n2. Testing function call in SELECT:");
    let query_str = r#"test_docs.select { upper_text: upper(.text) }"#;
    println!("Query: {}", query_str);
    
    match parse_oql(query_str) {
        Ok(query) => {
            println!("Parsed successfully: {:?}", query);
            match executor.execute_query(&query) {
                Ok(results) => {
                    println!("Results: {:?}", results);
                }
                Err(e) => {
                    println!("Execution error: {:?}", e);
                }
            }
        }
        Err(e) => {
            println!("Parse error: {:?}", e);
        }
    }
    
    // Test 3: Function call without alias
    println!("\n3. Testing function call without alias:");
    let query_str = r#"test_docs.select { upper(.text) }"#;
    println!("Query: {}", query_str);
    
    match parse_oql(query_str) {
        Ok(query) => {
            println!("Parsed successfully: {:?}", query);
            match executor.execute_query(&query) {
                Ok(results) => {
                    println!("Results: {:?}", results);
                }
                Err(e) => {
                    println!("Execution error: {:?}", e);
                }
            }
        }
        Err(e) => {
            println!("Parse error: {:?}", e);
        }
    }
    
    // Test 4: Function call in WHERE (should work based on previous test)
    println!("\n4. Testing function call in WHERE:");
    let query_str = r#"test_docs.where { length(.text) > 5 }.select { text }"#;
    println!("Query: {}", query_str);
    
    match parse_oql(query_str) {
        Ok(query) => {
            println!("Parsed successfully: {:?}", query);
            match executor.execute_query(&query) {
                Ok(results) => {
                    println!("Results: {:?}", results);
                }
                Err(e) => {
                    println!("Execution error: {:?}", e);
                }
            }
        }
        Err(e) => {
            println!("Parse error: {:?}", e);
        }
    }
    
    Ok(())
}