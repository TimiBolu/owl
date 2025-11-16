// Test for ObjRef-based reference migration
// Ensures backward compatibility and proper handling of ObjRef references

use oql_db::document::{Document, Value, ObjRef};
use oql_db::storage::{create_storage, Storage};
use std::collections::HashMap;
use tempfile::TempDir;

fn create_test_storage() -> (Storage, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test.owl");
    let storage = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024).unwrap();
    (storage, temp_dir)
}

#[test]
fn test_objref_creation_and_null_check() {
    // Test ObjRef creation
    let objref = ObjRef::new(1024);
    assert!(!objref.is_null());
    assert_eq!(objref.offset(), 1024);
    
    // Test null reference
    let null_ref = ObjRef::null();
    assert!(null_ref.is_null());
    assert_eq!(null_ref.offset(), 0);
}

#[test]
fn test_objref_from_string_conversion() {
    // Test conversion from string to ObjRef
    let ref_str = "customers#cust_123";
    let objref = ObjRef::from_string(ref_str);
    assert!(objref.is_some());
    assert!(!objref.unwrap().is_null());
    
    // Test empty string
    let empty_ref = ObjRef::from_string("");
    assert!(empty_ref.is_some());
    assert!(empty_ref.unwrap().is_null());
}

#[test]
fn test_objref_to_string_conversion() {
    // Test conversion from ObjRef to string
    let objref = ObjRef::new(1024);
    let ref_str = objref.to_string_id();
    assert!(!ref_str.is_empty());
    assert!(ref_str.starts_with("ref_"));
    
    // Test null reference
    let null_ref = ObjRef::null();
    let null_str = null_ref.to_string_id();
    assert!(null_str.is_empty());
}

#[test]
fn test_value_reference_with_objref() {
    // Test creating a reference value with ObjRef
    let objref = ObjRef::new(4096);
    let value = Value::Reference(objref);
    
    match value {
        Value::Reference(r) => {
            assert_eq!(r.offset(), 4096);
            assert!(!r.is_null());
        }
        _ => panic!("Expected Reference value"),
    }
}

#[test]
fn test_value_reference_from_string_helper() {
    // Test the backward compatibility helper
    let ref_str = "customers#cust_123".to_string();
    let value = Value::reference_from_string(ref_str);
    
    match value {
        Value::Reference(objref) => {
            assert!(!objref.is_null());
        }
        _ => panic!("Expected Reference value"),
    }
}

#[test]
fn test_value_reference_to_string_helper() {
    // Test the backward compatibility helper
    let objref = ObjRef::new(1024);
    let value = Value::Reference(objref);
    
    let ref_str = value.reference_to_string();
    assert!(ref_str.is_some());
    assert!(!ref_str.unwrap().is_empty());
}

#[test]
fn test_document_with_objref_reference() {
    let (storage, _temp_dir) = create_test_storage();
    
    // Create a customer document
    let mut customer_fields = HashMap::new();
    customer_fields.insert("name".to_string(), Value::String("Alice".to_string()));
    customer_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    
    let customer_doc = Document::new("customers".to_string(), customer_fields);
    storage.put_document(&customer_doc).unwrap();
    
    // Create an order document with ObjRef reference
    let customer_objref = ObjRef::from_string(&format!("customers#{}", customer_doc.id)).unwrap();
    let mut order_fields = HashMap::new();
    order_fields.insert("amount".to_string(), Value::Float(150.0));
    order_fields.insert("customer".to_string(), Value::Reference(customer_objref));
    
    let order_doc = Document::new("orders".to_string(), order_fields);
    storage.put_document(&order_doc).unwrap();
    
    // Retrieve and verify
    let retrieved_order = storage.get_document("orders", &order_doc.id).unwrap().unwrap();
    match retrieved_order.get_field("customer") {
        Some(Value::Reference(objref)) => {
            assert!(!objref.is_null());
        }
        _ => panic!("Expected Reference field"),
    }
}

#[test]
fn test_null_reference_handling() {
    let (storage, _temp_dir) = create_test_storage();
    
    // Create a document with null reference
    let mut doc_fields = HashMap::new();
    doc_fields.insert("name".to_string(), Value::String("Test".to_string()));
    doc_fields.insert("optional_ref".to_string(), Value::Reference(ObjRef::null()));
    
    let doc = Document::new("test".to_string(), doc_fields);
    storage.put_document(&doc).unwrap();
    
    // Retrieve and verify
    let retrieved_doc = storage.get_document("test", &doc.id).unwrap().unwrap();
    match retrieved_doc.get_field("optional_ref") {
        Some(Value::Reference(objref)) => {
            assert!(objref.is_null());
        }
        _ => panic!("Expected Reference field"),
    }
}

#[test]
fn test_reference_serialization() {
    // Test that ObjRef can be serialized and deserialized
    let objref = ObjRef::new(4096);
    let value = Value::Reference(objref);
    
    // Serialize to JSON
    let json = serde_json::to_string(&value).unwrap();
    assert!(!json.is_empty());
    
    // Deserialize from JSON
    let deserialized: Value = serde_json::from_str(&json).unwrap();
    match deserialized {
        Value::Reference(r) => {
            assert_eq!(r.offset(), 4096);
        }
        _ => panic!("Expected Reference value after deserialization"),
    }
}

#[test]
fn test_reference_equality() {
    let objref1 = ObjRef::new(1024);
    let objref2 = ObjRef::new(1024);
    let objref3 = ObjRef::new(2048);
    
    let value1 = Value::Reference(objref1);
    let value2 = Value::Reference(objref2);
    let value3 = Value::Reference(objref3);
    
    assert_eq!(value1, value2);
    assert_ne!(value1, value3);
}

#[test]
fn test_reference_in_nested_object() {
    let objref = ObjRef::new(4096);
    
    let mut nested_obj = HashMap::new();
    nested_obj.insert("ref_field".to_string(), Value::Reference(objref));
    nested_obj.insert("name".to_string(), Value::String("Nested".to_string()));
    
    let value = Value::Object(nested_obj);
    
    // Verify nested reference
    if let Value::Object(obj) = value {
        match obj.get("ref_field") {
            Some(Value::Reference(r)) => {
                assert_eq!(r.offset(), 4096);
            }
            _ => panic!("Expected Reference in nested object"),
        }
    }
}

#[test]
fn test_reference_in_list() {
    let objref1 = ObjRef::new(1024);
    let objref2 = ObjRef::new(2048);
    
    let list = vec![
        Value::Reference(objref1),
        Value::Reference(objref2),
        Value::String("test".to_string()),
    ];
    
    let value = Value::List(list);
    
    // Verify references in list
    if let Value::List(items) = value {
        assert_eq!(items.len(), 3);
        
        match &items[0] {
            Value::Reference(r) => assert_eq!(r.offset(), 1024),
            _ => panic!("Expected Reference at index 0"),
        }
        
        match &items[1] {
            Value::Reference(r) => assert_eq!(r.offset(), 2048),
            _ => panic!("Expected Reference at index 1"),
        }
    }
}

#[test]
fn test_objref_page_and_slot_calculation() {
    // Test page and slot ID calculation
    let objref = ObjRef::new(4096 + 128); // Page 1, slot 2
    assert_eq!(objref.page_id(), 1);
    assert_eq!(objref.slot_id(), 2);
    
    // Test page 0, slot 0
    let objref0 = ObjRef::new(0);
    assert_eq!(objref0.page_id(), 0);
    assert_eq!(objref0.slot_id(), 0);
    
    // Test last slot of page 0
    let objref_last = ObjRef::new(4096 - 64);
    assert_eq!(objref_last.page_id(), 0);
    assert_eq!(objref_last.slot_id(), 63);
}

#[test]
fn test_objref_from_page_slot() {
    let page_id = 5;
    let slot_id = 10;
    
    let objref = ObjRef::from_page_slot(page_id, slot_id);
    assert_eq!(objref.page_id(), page_id);
    assert_eq!(objref.slot_id(), slot_id);
    
    let expected_offset = (page_id as u64 * 4096) + (slot_id as u64 * 64);
    assert_eq!(objref.offset(), expected_offset);
}
