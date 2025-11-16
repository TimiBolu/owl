// Verification example for Task 4.2: Create single-level reference resolution
// This example demonstrates that all requirements are met

use owl_db::{
    parse_oql, QueryExecutor, Schema, FieldType, ReferenceType, DocumentStore, Document, Value,
};
use owl_db::reference::{ReferenceResolver, create_reference_id};
use owl_db::storage::RocksDBStore;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Task 4.2 Verification: Single-Level Reference Resolution ===\n");
    
    // Setup test environment
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create schemas
    create_test_schemas(&store)?;
    
    // Create test data
    create_test_data(&store)?;
    
    println!("✅ Requirement 1: Implement automatic document lookup for references");
    println!("==================================================================");
    
    // Test automatic document lookup
    let query_str = r#"orders.where { .id == "ord_123" }.select { customer_name: .customer.name }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    assert_eq!(results.len(), 1);
    let customer_name = results[0].get_field("customer_name");
    println!("Result: Customer name = {:?}", customer_name);
    assert_eq!(customer_name, Some(&Value::String("Alice Johnson".to_string())));
    println!("✓ Automatic document lookup working correctly\n");
    
    println!("✅ Requirement 2: Support dot notation for reference field access");
    println!("================================================================");
    
    // Test dot notation access
    let query_str = r#"orders.select { 
        order_id: .id,
        customer_name: .customer.name,
        customer_email: .customer.email,
        customer_active: .customer.active
    }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    println!("Results:");
    for doc in &results {
        println!("  Order: {}, Customer: {}, Email: {}, Active: {}",
                 doc.get_field("order_id").unwrap().to_string_repr(),
                 doc.get_field("customer_name").unwrap().to_string_repr(),
                 doc.get_field("customer_email").unwrap().to_string_repr(),
                 doc.get_field("customer_active").unwrap().to_string_repr());
    }
    println!("✓ Dot notation reference access working correctly\n");
    
    println!("✅ Requirement 3: Handle missing reference documents gracefully");
    println!("==============================================================");
    
    // Test graceful handling of missing references
    let query_str = r#"orders.where { .id == "ord_invalid" }.select { customer_name: .customer.name }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    assert_eq!(results.len(), 1);
    let customer_name = results[0].get_field("customer_name");
    println!("Result for missing reference: {:?}", customer_name);
    assert_eq!(customer_name, Some(&Value::Null));
    println!("✓ Missing references handled gracefully (returns null instead of error)\n");
    
    println!("✅ Requirement 4: Add reference caching for performance");
    println!("======================================================");
    
    // Test reference caching by creating a resolver directly
    let mut resolver = ReferenceResolver::new(store.clone());
    
    // Get an order document
    let order_doc = store.get_document("orders", &"ord_123".to_string())?.unwrap();
    
    // First resolution should cache the customer document
    let path = vec!["customer".to_string(), "name".to_string()];
    let result1 = resolver.resolve_field_path(&order_doc, &path)?;
    let (cache_size_after_first, _) = resolver.cache_stats();
    
    // Second resolution should use cache
    let result2 = resolver.resolve_field_path(&order_doc, &path)?;
    let (cache_size_after_second, _) = resolver.cache_stats();
    
    println!("First resolution result: {:?}", result1);
    println!("Second resolution result: {:?}", result2);
    println!("Cache size after first resolution: {}", cache_size_after_first);
    println!("Cache size after second resolution: {}", cache_size_after_second);
    
    assert_eq!(result1, result2);
    assert!(cache_size_after_first > 0);
    assert_eq!(cache_size_after_first, cache_size_after_second);
    println!("✓ Reference caching working correctly\n");
    
    println!("🎉 All Task 4.2 requirements successfully implemented and verified!");
    println!("===================================================================");
    println!("✓ Automatic document lookup for references");
    println!("✓ Dot notation for reference field access (.customer.name)");
    println!("✓ Graceful handling of missing reference documents");
    println!("✓ Reference caching for performance optimization");
    
    Ok(())
}

fn create_test_schemas(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    // Orders schema
    let mut order_fields = HashMap::new();
    order_fields.insert("id".to_string(), FieldType::String);
    order_fields.insert("amount".to_string(), FieldType::Float);
    order_fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
    
    let order_schema = Schema::new("orders".to_string(), order_fields);
    store.put_schema(&order_schema)?;
    
    // Customers schema
    let mut customer_fields = HashMap::new();
    customer_fields.insert("name".to_string(), FieldType::String);
    customer_fields.insert("email".to_string(), FieldType::String);
    customer_fields.insert("active".to_string(), FieldType::Boolean);
    
    let customer_schema = Schema::new("customers".to_string(), customer_fields);
    store.put_schema(&customer_schema)?;
    
    Ok(())
}

fn create_test_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    // Create customer
    let mut customer_fields = HashMap::new();
    customer_fields.insert("name".to_string(), Value::String("Alice Johnson".to_string()));
    customer_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    customer_fields.insert("active".to_string(), Value::Boolean(true));
    
    let customer_doc = Document::with_id("cust_456".to_string(), "customers".to_string(), customer_fields);
    store.put_document(&customer_doc)?;
    
    // Create valid order
    let mut order_fields = HashMap::new();
    order_fields.insert("id".to_string(), Value::String("ord_123".to_string()));
    order_fields.insert("amount".to_string(), Value::Float(150.0));
    order_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "cust_456")));
    
    let order_doc = Document::with_id("ord_123".to_string(), "orders".to_string(), order_fields);
    store.put_document(&order_doc)?;
    
    // Create order with invalid reference for testing graceful handling
    let mut invalid_order_fields = HashMap::new();
    invalid_order_fields.insert("id".to_string(), Value::String("ord_invalid".to_string()));
    invalid_order_fields.insert("amount".to_string(), Value::Float(100.0));
    invalid_order_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "nonexistent")));
    
    let invalid_order_doc = Document::with_id("ord_invalid".to_string(), "orders".to_string(), invalid_order_fields);
    store.put_document(&invalid_order_doc)?;
    
    Ok(())
}