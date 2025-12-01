// Tests for IndexManager module

use owl_db::index_manager::IndexManager;
use owl_storage_zig::ObjRef;
use std::collections::HashMap;

#[test]
fn test_index_manager_creation() {
    let manager = IndexManager::new();
    let indexes = manager.get_indexes("users").unwrap();
    assert_eq!(indexes.len(), 0);
}

#[test]
fn test_create_index() {
    let manager = IndexManager::new();
    
    let index = manager.create_index(
        "users",
        vec!["name".to_string()],
        false,
    );
    
    assert!(index.is_ok());
    let index = index.unwrap();
    assert_eq!(index.schema, "users");
    assert_eq!(index.fields, vec!["name".to_string()]);
    assert!(!index.unique);
    
    // Verify index is registered
    let indexes = manager.get_indexes("users").unwrap();
    assert_eq!(indexes.len(), 1);
}

#[test]
fn test_create_duplicate_index() {
    let manager = IndexManager::new();
    
    let _ = manager.create_index(
        "users",
        vec!["name".to_string()],
        false,
    ).unwrap();
    
    // Try to create duplicate index
    let result = manager.create_index(
        "users",
        vec!["name".to_string()],
        false,
    );
    
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already exists"));
}

#[test]
fn test_index_insert() {
    let manager = IndexManager::new();
    let _ = manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    let mut field_values = HashMap::new();
    field_values.insert("id".to_string(), 42);
    
    let objref = ObjRef::new(1000);
    let result = manager.on_insert("users", objref, &field_values);
    
    assert!(result.is_ok());
    
    // Verify statistics
    let stats = manager.get_stats().unwrap();
    assert_eq!(stats.total_updates, 1);
    assert_eq!(stats.successful_inserts, 1);
}

#[test]
fn test_unique_constraint() {
    let manager = IndexManager::new();
    let _ = manager.create_index(
        "users",
        vec!["email".to_string()],
        true, // unique index
    ).unwrap();
    
    let mut field_values = HashMap::new();
    field_values.insert("email".to_string(), 12345);
    
    // First insert should succeed
    let objref1 = ObjRef::new(1000);
    let result = manager.on_insert("users", objref1, &field_values);
    assert!(result.is_ok());
    
    // Second insert with same key should fail
    let objref2 = ObjRef::new(2000);
    let result = manager.on_insert("users", objref2, &field_values);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unique constraint violation"));
}

#[test]
fn test_index_update() {
    let manager = IndexManager::new();
    let _ = manager.create_index(
        "users",
        vec!["age".to_string()],
        false,
    ).unwrap();
    
    let mut old_values = HashMap::new();
    old_values.insert("age".to_string(), 25);
    
    let mut new_values = HashMap::new();
    new_values.insert("age".to_string(), 26);
    
    let old_objref = ObjRef::new(1000);
    let new_objref = ObjRef::new(1000); // Same object, updated
    
    let result = manager.on_update(
        "users",
        old_objref,
        new_objref,
        &old_values,
        &new_values,
    );
    
    assert!(result.is_ok());
    
    // Verify statistics
    let stats = manager.get_stats().unwrap();
    assert_eq!(stats.total_updates, 1);
}

#[test]
fn test_index_delete() {
    let manager = IndexManager::new();
    let _ = manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    // Insert first
    let mut field_values = HashMap::new();
    field_values.insert("id".to_string(), 42);
    let objref = ObjRef::new(1000);
    let _ = manager.on_insert("users", objref, &field_values).unwrap();
    
    // Then delete
    let result = manager.on_delete("users", &field_values);
    assert!(result.is_ok());
    
    // Verify statistics
    let stats = manager.get_stats().unwrap();
    assert_eq!(stats.total_updates, 2); // 1 insert + 1 delete
    assert_eq!(stats.successful_deletes, 1);
}

#[test]
fn test_composite_index() {
    let manager = IndexManager::new();
    let _ = manager.create_index(
        "users",
        vec!["first_name".to_string(), "last_name".to_string()],
        false,
    ).unwrap();
    
    let mut field_values = HashMap::new();
    field_values.insert("first_name".to_string(), 100);
    field_values.insert("last_name".to_string(), 200);
    
    let objref = ObjRef::new(1000);
    let result = manager.on_insert("users", objref, &field_values);
    
    assert!(result.is_ok());
}

#[test]
fn test_drop_index() {
    let manager = IndexManager::new();
    let index = manager.create_index(
        "users",
        vec!["name".to_string()],
        false,
    ).unwrap();
    
    // Verify index exists
    let indexes = manager.get_indexes("users").unwrap();
    assert_eq!(indexes.len(), 1);
    
    // Drop index
    let dropped = manager.drop_index("users", &index.name).unwrap();
    assert!(dropped);
    
    // Verify index is gone
    let indexes = manager.get_indexes("users").unwrap();
    assert_eq!(indexes.len(), 0);
}

#[test]
fn test_performance_overhead() {
    let manager = IndexManager::new();
    let _ = manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    // Perform multiple inserts
    for i in 0..100 {
        let mut field_values = HashMap::new();
        field_values.insert("id".to_string(), i);
        let objref = ObjRef::new(i * 100);
        let _ = manager.on_insert("users", objref, &field_values).unwrap();
    }
    
    // Check overhead
    let stats = manager.get_stats().unwrap();
    let base_operation_time_us = 100.0; // Assume 100us base operation
    let overhead = stats.overhead_percentage(base_operation_time_us);
    
    // Overhead should be less than 10%
    assert!(overhead < 10.0, "Index overhead is {}%, expected <10%", overhead);
}
