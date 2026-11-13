// Integration tests for index maintenance
// Tests automatic index updates on insert/update/delete operations

use owl_db::index_manager::IndexManager;
use owl_db::document::ObjRef;
use std::collections::HashMap;

#[test]
fn test_index_maintenance_on_insert() {
    let manager = IndexManager::new();
    
    // Create index on 'id' field
    let index = manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    // Insert document
    let mut field_values = HashMap::new();
    field_values.insert("id".to_string(), 42);
    let objref = ObjRef::new(1000);
    
    let result = manager.on_insert("users", objref, &field_values);
    assert!(result.is_ok());
    
    // Verify index was updated
    let found = index.search(42).unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().offset(), 1000);
}

#[test]
fn test_index_maintenance_on_update() {
    let manager = IndexManager::new();
    
    // Create index
    let index = manager.create_index(
        "users",
        vec!["age".to_string()],
        false,
    ).unwrap();
    
    // Insert initial document
    let mut old_values = HashMap::new();
    old_values.insert("age".to_string(), 25);
    let old_objref = ObjRef::new(1000);
    manager.on_insert("users", old_objref, &old_values).unwrap();
    
    // Update document
    let mut new_values = HashMap::new();
    new_values.insert("age".to_string(), 26);
    let new_objref = ObjRef::new(1000);
    
    let result = manager.on_update(
        "users",
        old_objref,
        new_objref,
        &old_values,
        &new_values,
    );
    assert!(result.is_ok());
    
    // Verify old key is removed
    let old_found = index.search(25).unwrap();
    assert!(old_found.is_none());
    
    // Verify new key exists
    let new_found = index.search(26).unwrap();
    assert!(new_found.is_some());
    assert_eq!(new_found.unwrap().offset(), 1000);
}

#[test]
fn test_index_maintenance_on_delete() {
    let manager = IndexManager::new();
    
    // Create index
    let index = manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    // Insert document
    let mut field_values = HashMap::new();
    field_values.insert("id".to_string(), 42);
    let objref = ObjRef::new(1000);
    manager.on_insert("users", objref, &field_values).unwrap();
    
    // Verify it exists
    assert!(index.search(42).unwrap().is_some());
    
    // Delete document
    let result = manager.on_delete("users", &field_values);
    assert!(result.is_ok());
    
    // Verify index entry is removed
    let found = index.search(42).unwrap();
    assert!(found.is_none());
}

#[test]
fn test_unique_index_constraint() {
    let manager = IndexManager::new();
    
    // Create unique index
    manager.create_index(
        "users",
        vec!["email".to_string()],
        true,
    ).unwrap();
    
    // Insert first document
    let mut field_values = HashMap::new();
    field_values.insert("email".to_string(), 12345);
    let objref1 = ObjRef::new(1000);
    
    let result = manager.on_insert("users", objref1, &field_values);
    assert!(result.is_ok());
    
    // Try to insert duplicate
    let objref2 = ObjRef::new(2000);
    let result = manager.on_insert("users", objref2, &field_values);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unique constraint violation"));
}

#[test]
fn test_composite_index_maintenance() {
    let manager = IndexManager::new();
    
    // Create composite index
    let index = manager.create_index(
        "users",
        vec!["first_name".to_string(), "last_name".to_string()],
        false,
    ).unwrap();
    
    // Insert document
    let mut field_values = HashMap::new();
    field_values.insert("first_name".to_string(), 100);
    field_values.insert("last_name".to_string(), 200);
    let objref = ObjRef::new(1000);
    
    let result = manager.on_insert("users", objref, &field_values);
    assert!(result.is_ok());
    
    // Composite key should be searchable
    // (In this simplified implementation, composite keys are hashed)
    let composite_key = 100u64.wrapping_mul(31).wrapping_add(200);
    let found = index.search(composite_key).unwrap();
    assert!(found.is_some());
}

#[test]
fn test_multiple_indexes_per_schema() {
    let manager = IndexManager::new();
    
    // Create multiple indexes
    manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    manager.create_index(
        "users",
        vec!["email".to_string()],
        true,
    ).unwrap();
    
    manager.create_index(
        "users",
        vec!["age".to_string()],
        false,
    ).unwrap();
    
    // Insert document with all fields
    let mut field_values = HashMap::new();
    field_values.insert("id".to_string(), 1);
    field_values.insert("email".to_string(), 12345);
    field_values.insert("age".to_string(), 25);
    let objref = ObjRef::new(1000);
    
    let result = manager.on_insert("users", objref, &field_values);
    assert!(result.is_ok());
    
    // Verify all indexes were updated
    let indexes = manager.get_indexes("users").unwrap();
    assert_eq!(indexes.len(), 3);
    
    // Check each index
    assert!(indexes[0].search(1).unwrap().is_some());
    assert!(indexes[1].search(12345).unwrap().is_some());
    assert!(indexes[2].search(25).unwrap().is_some());
}

#[test]
fn test_index_consistency_on_update() {
    let manager = IndexManager::new();
    
    // Create multiple indexes
    manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    manager.create_index(
        "users",
        vec!["age".to_string()],
        false,
    ).unwrap();
    
    // Insert document
    let mut old_values = HashMap::new();
    old_values.insert("id".to_string(), 1);
    old_values.insert("age".to_string(), 25);
    let old_objref = ObjRef::new(1000);
    manager.on_insert("users", old_objref, &old_values).unwrap();
    
    // Update only age
    let mut new_values = HashMap::new();
    new_values.insert("id".to_string(), 1); // Same ID
    new_values.insert("age".to_string(), 26); // New age
    let new_objref = ObjRef::new(1000);
    
    let result = manager.on_update(
        "users",
        old_objref,
        new_objref,
        &old_values,
        &new_values,
    );
    assert!(result.is_ok());
    
    // Verify both indexes are consistent
    let indexes = manager.get_indexes("users").unwrap();
    
    // ID index should still have the entry
    let id_found = indexes[0].search(1).unwrap();
    assert!(id_found.is_some());
    
    // Age index should have new value, not old
    let old_age_found = indexes[1].search(25).unwrap();
    assert!(old_age_found.is_none());
    
    let new_age_found = indexes[1].search(26).unwrap();
    assert!(new_age_found.is_some());
}

#[test]
fn test_index_performance_overhead() {
    let manager = IndexManager::new();
    
    // Create index
    manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    // Perform many inserts
    let num_inserts = 1000;
    for i in 0..num_inserts {
        let mut field_values = HashMap::new();
        field_values.insert("id".to_string(), i);
        let objref = ObjRef::new(i * 100);
        manager.on_insert("users", objref, &field_values).unwrap();
    }
    
    // Check statistics
    let stats = manager.get_stats().unwrap();
    assert_eq!(stats.total_updates, num_inserts);
    assert_eq!(stats.successful_inserts, num_inserts);
    
    // Calculate overhead (assuming 100us base operation time)
    let base_operation_time_us = 100.0;
    let overhead = stats.overhead_percentage(base_operation_time_us);
    
    println!("Index maintenance overhead: {:.2}%", overhead);
    println!("Average update time: {:.2}us", stats.avg_update_time_us());
    
    // Overhead should be less than 10%
    assert!(
        overhead < 10.0,
        "Index overhead is {:.2}%, expected <10%",
        overhead
    );
}

#[test]
fn test_transactional_consistency() {
    let manager = IndexManager::new();
    
    // Create unique index
    manager.create_index(
        "users",
        vec!["email".to_string()],
        true,
    ).unwrap();
    
    // Insert first document
    let mut field_values1 = HashMap::new();
    field_values1.insert("email".to_string(), 12345);
    let objref1 = ObjRef::new(1000);
    manager.on_insert("users", objref1, &field_values1).unwrap();
    
    // Try to insert duplicate (should fail)
    let mut field_values2 = HashMap::new();
    field_values2.insert("email".to_string(), 12345);
    let objref2 = ObjRef::new(2000);
    let result = manager.on_insert("users", objref2, &field_values2);
    assert!(result.is_err());
    
    // Verify first document is still there
    let indexes = manager.get_indexes("users").unwrap();
    let found = indexes[0].search(12345).unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().offset(), 1000);
    
    // Verify second document was not inserted
    let stats = manager.get_stats().unwrap();
    assert_eq!(stats.successful_inserts, 1);
}

#[test]
fn test_index_drop() {
    let manager = IndexManager::new();
    
    // Create index
    let index = manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    // Insert some data
    let mut field_values = HashMap::new();
    field_values.insert("id".to_string(), 42);
    let objref = ObjRef::new(1000);
    manager.on_insert("users", objref, &field_values).unwrap();
    
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
fn test_missing_field_values() {
    let manager = IndexManager::new();
    
    // Create composite index
    manager.create_index(
        "users",
        vec!["first_name".to_string(), "last_name".to_string()],
        false,
    ).unwrap();
    
    // Try to insert with missing field
    let mut field_values = HashMap::new();
    field_values.insert("first_name".to_string(), 100);
    // Missing last_name
    let objref = ObjRef::new(1000);
    
    // Should succeed but not create index entry (graceful degradation)
    let result = manager.on_insert("users", objref, &field_values);
    assert!(result.is_ok());
}

#[test]
fn test_concurrent_index_access() {
    use std::sync::Arc;
    use std::thread;
    
    let manager = Arc::new(IndexManager::new());
    
    // Create index
    manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    ).unwrap();
    
    // Spawn multiple threads to insert concurrently
    let mut handles = vec![];
    for i in 0..10 {
        let manager_clone = Arc::clone(&manager);
        let handle = thread::spawn(move || {
            for j in 0..10 {
                let mut field_values = HashMap::new();
                field_values.insert("id".to_string(), (i * 10 + j) as u64);
                let objref = ObjRef::new((i * 10 + j) * 100);
                let _ = manager_clone.on_insert("users", objref, &field_values);
            }
        });
        handles.push(handle);
    }
    
    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Verify all inserts succeeded
    let stats = manager.get_stats().unwrap();
    assert_eq!(stats.successful_inserts, 100);
}
