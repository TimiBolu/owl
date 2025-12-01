// Simple storage test to debug the issue

use owl_db::document::{Document, Value};
use owl_db::storage::create_storage;
use std::collections::HashMap;

fn create_test_document(schema: &str, id: &str, value: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), Value::Integer(value));
    
    Document::with_id(
        id.to_string(),
        schema.to_string(),
        fields,
    )
}

#[test]
fn test_simple_storage_write_read() {
    // Clean up any existing file
    std::fs::remove_file("test_simple.owl").ok();
    
    let storage = create_storage("test_simple.owl", 10 * 1024 * 1024).unwrap();
    
    // Create and store a single document
    let doc = create_test_document("items", "item1", 42);
    println!("Storing document: {:?}", doc);
    storage.put_document(&doc).unwrap();
    
    // Try to get the document directly
    println!("Getting document by ID...");
    let retrieved = storage.get_document("items", &"item1".to_string()).unwrap();
    println!("Retrieved: {:?}", retrieved.is_some());
    
    if let Some(retrieved_doc) = retrieved {
        assert_eq!(retrieved_doc.id, "item1");
        println!("Direct get works!");
    } else {
        panic!("Failed to retrieve document by ID");
    }
    
    // Try to scan
    println!("Scanning documents...");
    let docs = storage.scan_documents("items").unwrap();
    println!("Found {} documents", docs.len());
    
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].id, "item1");
    
    std::fs::remove_file("test_simple.owl").ok();
}

#[test]
fn test_simple_storage_multiple_docs() {
    // Clean up any existing file
    std::fs::remove_file("test_simple_multi.owl").ok();
    
    let storage = create_storage("test_simple_multi.owl", 10 * 1024 * 1024).unwrap();
    
    // Create and store multiple documents
    for i in 0..10 {
        let doc = create_test_document("items", &format!("item{}", i), i);
        println!("Storing document {}: {:?}", i, doc.id);
        storage.put_document(&doc).unwrap();
    }
    
    // Try to scan
    println!("Scanning documents...");
    let docs = storage.scan_documents("items").unwrap();
    println!("Found {} documents", docs.len());
    
    assert_eq!(docs.len(), 10);
    
    std::fs::remove_file("test_simple_multi.owl").ok();
}
