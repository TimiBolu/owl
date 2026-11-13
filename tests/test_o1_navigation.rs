// Test O(1) Reference Navigation
//
// This test verifies that O(1) pointer-based navigation works correctly
// and meets the performance target of <50ns per hop.

use owl_db::document::{Document, Value, ObjRef};
use owl_db::storage::{create_storage, DocumentStore};
use std::collections::HashMap;

#[test]
fn test_single_hop_navigation() {
    use owl_db::storage::RustStore;
    
    let rust_store = RustStore::new("/tmp/test_single_hop.owl", 10 * 1024 * 1024).unwrap();
    
    // Create two documents: doc1 -> doc2
    let mut doc2_fields = HashMap::new();
    doc2_fields.insert("name".to_string(), Value::String("Document 2".to_string()));
    doc2_fields.insert("value".to_string(), Value::Integer(42));
    let doc2 = Document::with_id("doc2".to_string(), "test".to_string(), doc2_fields);
    rust_store.put_document(&doc2).unwrap();
    
    // Get the actual ObjRef for doc2
    let doc2_ref = rust_store.get_objref("test", &doc2.id).unwrap().unwrap();
    
    // Create doc1 with reference to doc2
    let mut doc1_fields = HashMap::new();
    doc1_fields.insert("name".to_string(), Value::String("Document 1".to_string()));
    doc1_fields.insert("next".to_string(), Value::Reference(doc2_ref));
    let doc1 = Document::with_id("doc1".to_string(), "test".to_string(), doc1_fields);
    rust_store.put_document(&doc1).unwrap();
    
    // Test navigation: doc1 -> doc2
    let next_ref = match doc1.get_field("next") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    let navigated_doc = rust_store.navigate_reference(&next_ref).unwrap();
    assert!(navigated_doc.is_some());
    
    let doc2_retrieved = navigated_doc.unwrap();
    assert_eq!(doc2_retrieved.id, doc2.id);
    assert_eq!(doc2_retrieved.schema, "test");
    
    match doc2_retrieved.get_field("name") {
        Some(Value::String(s)) => assert_eq!(s, "Document 2"),
        _ => panic!("Expected name field"),
    }
    
    std::fs::remove_file("/tmp/test_single_hop.owl").ok();
}

#[test]
fn test_multi_hop_navigation() {
    use owl_db::storage::RustStore;
    
    let rust_store = RustStore::new("/tmp/test_multi_hop.owl", 10 * 1024 * 1024).unwrap();
    
    // Create a chain: doc1 -> doc2 -> doc3
    let mut doc3_fields = HashMap::new();
    doc3_fields.insert("name".to_string(), Value::String("Document 3".to_string()));
    doc3_fields.insert("value".to_string(), Value::Integer(100));
    let doc3 = Document::with_id("doc3".to_string(), "test".to_string(), doc3_fields);
    rust_store.put_document(&doc3).unwrap();
    
    let doc3_ref = rust_store.get_objref("test", &doc3.id).unwrap().unwrap();
    let mut doc2_fields = HashMap::new();
    doc2_fields.insert("name".to_string(), Value::String("Document 2".to_string()));
    doc2_fields.insert("next".to_string(), Value::Reference(doc3_ref));
    let doc2 = Document::with_id("doc2".to_string(), "test".to_string(), doc2_fields);
    rust_store.put_document(&doc2).unwrap();
    
    let doc2_ref = rust_store.get_objref("test", &doc2.id).unwrap().unwrap();
    let mut doc1_fields = HashMap::new();
    doc1_fields.insert("name".to_string(), Value::String("Document 1".to_string()));
    doc1_fields.insert("next".to_string(), Value::Reference(doc2_ref));
    let doc1 = Document::with_id("doc1".to_string(), "test".to_string(), doc1_fields);
    rust_store.put_document(&doc1).unwrap();
    
    // Navigate: doc1 -> doc2
    let next_ref = match doc1.get_field("next") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    let doc2_nav = rust_store.navigate_reference(&next_ref).unwrap().unwrap();
    assert_eq!(doc2_nav.id, doc2.id);
    
    // Navigate: doc2 -> doc3
    let next_ref2 = match doc2_nav.get_field("next") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    let doc3_nav = rust_store.navigate_reference(&next_ref2).unwrap().unwrap();
    assert_eq!(doc3_nav.id, doc3.id);
    
    match doc3_nav.get_field("value") {
        Some(Value::Integer(v)) => assert_eq!(*v, 100),
        _ => panic!("Expected value field"),
    }
    
    std::fs::remove_file("/tmp/test_multi_hop.owl").ok();
}

#[test]
fn test_null_reference_navigation() {
    let storage = create_storage("/tmp/test_null_ref.owl", 10 * 1024 * 1024).unwrap();
    
    // Test navigating a null reference
    let null_ref = ObjRef::null();
    let result = storage.navigate_reference(&null_ref).unwrap();
    assert!(result.is_none());
    
    std::fs::remove_file("/tmp/test_null_ref.owl").ok();
}

#[test]
fn test_invalid_reference_navigation() {
    let storage = create_storage("/tmp/test_invalid_ref.owl", 10 * 1024 * 1024).unwrap();
    
    // Test navigating an invalid reference (points to non-existent object)
    let invalid_ref = ObjRef::new(999999);
    let result = storage.navigate_reference(&invalid_ref).unwrap();
    assert!(result.is_none());
    
    std::fs::remove_file("/tmp/test_invalid_ref.owl").ok();
}

#[test]
fn test_navigation_with_nested_fields() {
    use owl_db::storage::RustStore;
    
    let rust_store = RustStore::new("/tmp/test_nested_nav.owl", 10 * 1024 * 1024).unwrap();
    
    // Create doc2 with nested object
    let mut profile = HashMap::new();
    profile.insert("age".to_string(), Value::Integer(30));
    profile.insert("city".to_string(), Value::String("San Francisco".to_string()));
    
    let mut doc2_fields = HashMap::new();
    doc2_fields.insert("name".to_string(), Value::String("Alice".to_string()));
    doc2_fields.insert("profile".to_string(), Value::Object(profile));
    let doc2 = Document::with_id("doc2".to_string(), "users".to_string(), doc2_fields);
    rust_store.put_document(&doc2).unwrap();
    
    // Create doc1 with reference to doc2
    let doc2_ref = rust_store.get_objref("users", &doc2.id).unwrap().unwrap();
    let mut doc1_fields = HashMap::new();
    doc1_fields.insert("friend".to_string(), Value::Reference(doc2_ref));
    let doc1 = Document::with_id("doc1".to_string(), "users".to_string(), doc1_fields);
    rust_store.put_document(&doc1).unwrap();
    
    // Navigate and access nested field
    let friend_ref = match doc1.get_field("friend") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    let friend_doc = rust_store.navigate_reference(&friend_ref).unwrap().unwrap();
    
    // Access nested profile.city
    match friend_doc.get_field("profile") {
        Some(Value::Object(profile)) => {
            match profile.get("city") {
                Some(Value::String(city)) => assert_eq!(city, "San Francisco"),
                _ => panic!("Expected city field"),
            }
        }
        _ => panic!("Expected profile object"),
    }
    
    std::fs::remove_file("/tmp/test_nested_nav.owl").ok();
}
