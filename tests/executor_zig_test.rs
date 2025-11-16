// Integration tests for QueryExecutor with Zig storage backend
//
// These tests verify that the QueryExecutor works correctly with ZigStore
// and achieves the expected performance improvements through O(1) navigation.

use owl_db::document::{Document, Value, ObjRef};
use owl_db::error::OQLResult;
use owl_db::executor::QueryExecutor;
use owl_db::parser::parse_oql;
use owl_db::schema::{Schema, FieldType};
use owl_db::storage::{create_storage, DocumentStore};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to create a test executor with Zig storage
fn create_test_executor() -> (QueryExecutor<owl_db::storage::zig_adapter::ZigStore>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test.owl");
    let storage = owl_db::storage::zig_adapter::ZigStore::new(path, 100 * 1024 * 1024).unwrap();
    let executor = QueryExecutor::new(Arc::new(storage));
    (executor, temp_dir)
}

// Note: Reference navigation tests are in tests/test_o1_navigation.rs
// This file focuses on basic executor functionality with Zig storage

#[test]
fn test_basic_query_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create test documents
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    fields.insert("age".to_string(), Value::Integer(30));
    let doc = Document::new("users".to_string(), fields);
    executor.store().put_document(&doc).unwrap();

    // Execute query
    let query = parse_oql(r#"users.where { .name == "Alice" }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("name"), Some(&Value::String("Alice".to_string())));
}

#[test]
fn test_insert_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Execute INSERT query
    let query = parse_oql(r#"users.insert { name: "Bob", age: 25 }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 1);
    let inserted_doc = &results[0];
    assert_eq!(inserted_doc.schema, "users");
    assert_eq!(inserted_doc.get_field("name"), Some(&Value::String("Bob".to_string())));
    assert_eq!(inserted_doc.get_field("age"), Some(&Value::Integer(25)));

    // Verify document was stored
    let stored = executor.store().get_document("users", &inserted_doc.id).unwrap();
    assert!(stored.is_some());
}

#[test]
fn test_update_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create initial document
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Charlie".to_string()));
    fields.insert("age".to_string(), Value::Integer(35));
    let doc = Document::new("users".to_string(), fields);
    executor.store().put_document(&doc).unwrap();

    // Execute UPDATE query
    let query = parse_oql(r#"users.where { .name == "Charlie" }.update { age: 36 }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("age"), Some(&Value::Integer(36)));

    // Verify update persisted
    let stored = executor.store().get_document("users", &doc.id).unwrap().unwrap();
    assert_eq!(stored.get_field("age"), Some(&Value::Integer(36)));
}

#[test]
fn test_delete_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create initial document
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("David".to_string()));
    let doc = Document::new("users".to_string(), fields);
    let doc_id = doc.id.clone();
    executor.store().put_document(&doc).unwrap();

    // Execute DELETE query
    let query = parse_oql(r#"users.where { .name == "David" }.delete"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 1);

    // Verify document was deleted
    let stored = executor.store().get_document("users", &doc_id).unwrap();
    assert!(stored.is_none());
}

#[test]
fn test_select_projection_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create test documents
    for i in 0..3 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User{}", i)));
        fields.insert("age".to_string(), Value::Integer(20 + i));
        fields.insert("email".to_string(), Value::String(format!("user{}@example.com", i)));
        let doc = Document::new("users".to_string(), fields);
        executor.store().put_document(&doc).unwrap();
    }

    // Execute SELECT query
    let query = parse_oql(r#"users.select { name, age }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 3);
    for doc in &results {
        assert!(doc.get_field("name").is_some());
        assert!(doc.get_field("age").is_some());
        assert!(doc.get_field("email").is_none()); // Not selected
    }
}

#[test]
fn test_aggregation_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create test documents
    for i in 0..5 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User{}", i)));
        fields.insert("score".to_string(), Value::Integer(10 * (i + 1)));
        let doc = Document::new("users".to_string(), fields);
        executor.store().put_document(&doc).unwrap();
    }

    // Test COUNT
    let query = parse_oql(r#"users.select { count: count() }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("count"), Some(&Value::Integer(5)));

    // Test SUM
    let query = parse_oql(r#"users.select { total: sum(.score) }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 1);
    // Sum: 10 + 20 + 30 + 40 + 50 = 150
    assert_eq!(results[0].get_field("total"), Some(&Value::Integer(150)));

    // Test AVG
    let query = parse_oql(r#"users.select { average: avg(.score) }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 1);
    // Avg: 150 / 5 = 30.0
    assert_eq!(results[0].get_field("average"), Some(&Value::Float(30.0)));
}

#[test]
fn test_sorting_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create test documents in random order
    let ages = vec![35, 25, 30, 40, 20];
    for (i, age) in ages.iter().enumerate() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User{}", i)));
        fields.insert("age".to_string(), Value::Integer(*age));
        let doc = Document::new("users".to_string(), fields);
        executor.store().put_document(&doc).unwrap();
    }

    // Execute SORT query (ascending)
    let query = parse_oql(r#"users.sort { .age asc }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 5);
    // Verify sorted order: 20, 25, 30, 35, 40
    assert_eq!(results[0].get_field("age"), Some(&Value::Integer(20)));
    assert_eq!(results[1].get_field("age"), Some(&Value::Integer(25)));
    assert_eq!(results[2].get_field("age"), Some(&Value::Integer(30)));
    assert_eq!(results[3].get_field("age"), Some(&Value::Integer(35)));
    assert_eq!(results[4].get_field("age"), Some(&Value::Integer(40)));
}

#[test]
fn test_limit_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create test documents
    for i in 0..10 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User{}", i)));
        let doc = Document::new("users".to_string(), fields);
        executor.store().put_document(&doc).unwrap();
    }

    // Execute LIMIT query
    let query = parse_oql(r#"users.limit 3"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 3);
}

#[test]
fn test_complex_query_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create test documents
    for i in 0..10 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User{}", i)));
        fields.insert("age".to_string(), Value::Integer(20 + i));
        fields.insert("active".to_string(), Value::Boolean(i % 2 == 0));
        let doc = Document::new("users".to_string(), fields);
        executor.store().put_document(&doc).unwrap();
    }

    // Execute complex query: filter, sort, limit
    let query = parse_oql(r#"users.where { .active == true }.sort { .age desc }.limit 3"#).unwrap();
    let results = executor.execute_query(&query).unwrap();

    assert_eq!(results.len(), 3);
    // Should get active users (0, 2, 4, 6, 8) sorted by age desc, limited to 3
    // Ages: 28, 26, 24
    assert_eq!(results[0].get_field("age"), Some(&Value::Integer(28)));
    assert_eq!(results[1].get_field("age"), Some(&Value::Integer(26)));
    assert_eq!(results[2].get_field("age"), Some(&Value::Integer(24)));
}

#[test]
fn test_schema_validation_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create schema
    let mut field_types = HashMap::new();
    field_types.insert("name".to_string(), FieldType::String);
    field_types.insert("age".to_string(), FieldType::Integer);
    let schema = Schema::new("users".to_string(), field_types);
    executor.store().put_schema(&schema).unwrap();

    // Insert valid document
    let query = parse_oql(r#"users.insert { name: "Alice", age: 30 }"#).unwrap();
    let result = executor.execute_query(&query);
    assert!(result.is_ok());

    // Insert invalid document (wrong type)
    let query = parse_oql(r#"users.insert { name: "Bob", age: "thirty" }"#).unwrap();
    let result = executor.execute_query(&query);
    assert!(result.is_err()); // Should fail validation
}

#[test]
fn test_concurrent_queries_with_zig_storage() {
    use std::thread;

    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test.owl");
    let storage = Arc::new(owl_db::storage::zig_adapter::ZigStore::new(path, 100 * 1024 * 1024).unwrap());

    // Pre-populate with documents
    for i in 0..100 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User{}", i)));
        fields.insert("value".to_string(), Value::Integer(i));
        let doc = Document::new("users".to_string(), fields);
        storage.put_document(&doc).unwrap();
    }

    // Spawn multiple threads to query concurrently
    let mut handles = vec![];
    for thread_id in 0..4 {
        let storage_clone = Arc::clone(&storage);
        let handle = thread::spawn(move || {
            let executor = QueryExecutor::new(storage_clone);
            let query = parse_oql(r#"users.where { .value >= 50 }"#).unwrap();
            let results = executor.execute_query(&query).unwrap();
            assert!(results.len() >= 50);
            thread_id
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_memory_efficiency_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create many documents to test memory efficiency
    for i in 0..1000 {
        let mut fields = HashMap::new();
        fields.insert("id".to_string(), Value::Integer(i));
        fields.insert("data".to_string(), Value::String(format!("Data{}", i)));
        let doc = Document::new("items".to_string(), fields);
        executor.store().put_document(&doc).unwrap();
    }

    // Query all documents
    let query = parse_oql(r#"items"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 1000);

    // Memory should be efficiently managed by Zig arena
    // This test mainly ensures no crashes or memory issues
}

#[test]
fn test_null_handling_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Create document with null field
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    fields.insert("email".to_string(), Value::Null);
    let doc = Document::new("users".to_string(), fields);
    executor.store().put_document(&doc).unwrap();

    // Query for null field
    let query = parse_oql(r#"users.where { .email == null }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 1);

    // Query for missing field (implicit null)
    let query = parse_oql(r#"users.where { .phone == null }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn test_error_handling_with_zig_storage() {
    let (executor, _temp_dir) = create_test_executor();

    // Test query on non-existent collection
    let query = parse_oql(r#"nonexistent.where { .id == 1 }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 0); // Should return empty, not error

    // Test invalid field access
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    let doc = Document::new("users".to_string(), fields);
    executor.store().put_document(&doc).unwrap();

    let query = parse_oql(r#"users.where { .nonexistent == "value" }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 0); // Implicit null handling
}

#[test]
fn test_performance_baseline_with_zig_storage() {
    use std::time::Instant;

    let (executor, _temp_dir) = create_test_executor();

    // Create test dataset
    for i in 0..1000 {
        let mut fields = HashMap::new();
        fields.insert("id".to_string(), Value::Integer(i));
        fields.insert("value".to_string(), Value::Integer(i * 2));
        let doc = Document::new("items".to_string(), fields);
        executor.store().put_document(&doc).unwrap();
    }

    // Measure query performance
    let start = Instant::now();
    let query = parse_oql(r#"items.where { .value >= 1000 }"#).unwrap();
    let results = executor.execute_query(&query).unwrap();
    let duration = start.elapsed();

    assert!(results.len() > 0);
    println!("Query executed in {:?} for {} results", duration, results.len());

    // Performance should be reasonable (< 100ms for 1000 documents)
    assert!(duration.as_millis() < 100);
}
