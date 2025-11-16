// Tests for conditional logic and pattern matching

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::parse_oql,
    storage::{RocksDBStore, DocumentStore},
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn create_test_executor() -> (QueryExecutor<RocksDBStore>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
    let executor = QueryExecutor::new(store);
    (executor, temp_dir)
}

fn setup_test_data(executor: &QueryExecutor<RocksDBStore>) {
    let customers = vec![
        Document::with_id("cust_1".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Alice".to_string()));
            fields.insert("age".to_string(), Value::Integer(25));
            fields.insert("premium".to_string(), Value::Boolean(true));
            fields.insert("active".to_string(), Value::Boolean(true));
            fields
        }),
        Document::with_id("cust_2".to_string(), "customers".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Bob".to_string()));
            fields.insert("age".to_string(), Value::Integer(17));
            fields.insert("premium".to_string(), Value::Boolean(false));
            fields.insert("active".to_string(), Value::Boolean(false));
            fields
        }),
    ];

    for customer in customers {
        executor.store().put_document(&customer).unwrap();
    }
}

#[test]
fn test_if_then_else_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { name, status: if .age >= 18 then "adult" else "minor" }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (25) should be adult
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("status"), Some(&Value::String("adult".to_string())));
    
    // Bob (17) should be minor
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("status"), Some(&Value::String("minor".to_string())));
}

#[test]
fn test_logical_and_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { name, eligible: .premium and .active }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (premium=true, active=true) should be eligible
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("eligible"), Some(&Value::Boolean(true)));
    
    // Bob (premium=false, active=false) should not be eligible
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("eligible"), Some(&Value::Boolean(false)));
}

#[test]
fn test_logical_or_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { name, qualified: .premium or .active }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (premium=true, active=true) should be qualified
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("qualified"), Some(&Value::Boolean(true)));
    
    // Bob (premium=false, active=false) should not be qualified
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("qualified"), Some(&Value::Boolean(false)));
}

#[test]
fn test_logical_not_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { name, inactive: not .active }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (active=true) should not be inactive
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("inactive"), Some(&Value::Boolean(false)));
    
    // Bob (active=false) should be inactive
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("inactive"), Some(&Value::Boolean(true)));
}

#[test]
fn test_match_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { name, tier: match .premium { true -> "premium", false -> "standard", _ -> "unknown" } }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (premium=true) should be premium tier
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("tier"), Some(&Value::String("premium".to_string())));
    
    // Bob (premium=false) should be standard tier
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("tier"), Some(&Value::String("standard".to_string())));
}

#[test]
fn test_nested_conditional_logic() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { 
        name, 
        category: if .premium == true then 
            "VIP" 
        else if .active == true then 
            "Regular" 
        else 
            "Inactive" 
    }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (premium=true) should be VIP
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("category"), Some(&Value::String("VIP".to_string())));
    
    // Bob (premium=false, active=false) should be Inactive
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("category"), Some(&Value::String("Inactive".to_string())));
}

#[test]
fn test_complex_logical_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { 
        name, 
        complex: .premium and not .active or .active and not .premium
    }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (premium=true, active=true): true and false or true and false = false
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("complex"), Some(&Value::Boolean(false)));
    
    // Bob (premium=false, active=false): false and true or false and true = false
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("complex"), Some(&Value::Boolean(false)));
}

#[test]
fn test_block_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { 
        name, 
        category: if .age >= 18 {
            "adult"
        } else {
            "minor"
        }
    }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (25) should be adult
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("category"), Some(&Value::String("adult".to_string())));
    
    // Bob (17) should be minor
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("category"), Some(&Value::String("minor".to_string())));
}

#[test]
fn test_nested_block_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    let query = r#"customers.select { 
        name, 
        status: if .premium == true {
            if .active == true then "VIP Active" else "VIP Inactive"
        } else {
            if .active == true then "Regular Active" else "Regular Inactive"
        }
    }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (premium=true, active=true) should be "VIP Active"
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("status"), Some(&Value::String("VIP Active".to_string())));
    
    // Bob (premium=false, active=false) should be "Regular Inactive"
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("status"), Some(&Value::String("Regular Inactive".to_string())));
}

#[test]
fn test_dual_syntax_single_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    // Test single expression syntax with then/else
    let query = r#"customers.select { 
        name, 
        category: if .age >= 18 then "adult" else "minor"
    }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (25) should be adult
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("category"), Some(&Value::String("adult".to_string())));
    
    // Bob (17) should be minor
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("category"), Some(&Value::String("minor".to_string())));
}

#[test]
fn test_dual_syntax_block_expressions() {
    let (executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor);

    // Test block syntax without then/else keywords
    let query = r#"customers.select { 
        name, 
        tier: if .premium == true {
            "premium_customer"
        } else {
            "standard_customer"
        }
    }"#;
    let parsed_query = parse_oql(query).unwrap();
    let results = executor.execute_query(&parsed_query).unwrap();

    assert_eq!(results.len(), 2);
    
    // Alice (premium=true) should be premium_customer
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice")
    }).unwrap();
    assert_eq!(alice.get_field("tier"), Some(&Value::String("premium_customer".to_string())));
    
    // Bob (premium=false) should be standard_customer
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob")
    }).unwrap();
    assert_eq!(bob.get_field("tier"), Some(&Value::String("standard_customer".to_string())));
}