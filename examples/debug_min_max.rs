// Debug min/max functions

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
    println!("=== Min/Max Function Debug ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create test document
    let mut fields = HashMap::new();
    fields.insert("number".to_string(), Value::Integer(42));
    
    let doc = Document::with_id("doc1".to_string(), "test_docs".to_string(), fields);
    store.put_document(&doc)?;
    
    println!("Created test document with number: 42");
    
    // Test min function
    println!("\n1. Testing min(42, 50):");
    let query_str = r#"test_docs.where { .id == "doc1" }.select { minimum: min(.number, 50) }"#;
    println!("Query: {}", query_str);
    
    match parse_oql(query_str) {
        Ok(query) => {
            println!("Parsed successfully");
            match executor.execute_query(&query) {
                Ok(results) => {
                    println!("Results: {:?}", results);
                    if let Some(min_val) = results[0].get_field("minimum") {
                        println!("Min value: {:?}", min_val);
                    }
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
    
    // Test max function
    println!("\n2. Testing max(42, 50):");
    let query_str = r#"test_docs.where { .id == "doc1" }.select { maximum: max(.number, 50) }"#;
    println!("Query: {}", query_str);
    
    match parse_oql(query_str) {
        Ok(query) => {
            println!("Parsed successfully");
            match executor.execute_query(&query) {
                Ok(results) => {
                    println!("Results: {:?}", results);
                    if let Some(max_val) = results[0].get_field("maximum") {
                        println!("Max value: {:?}", max_val);
                    }
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