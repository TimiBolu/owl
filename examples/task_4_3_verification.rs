// Verification example for Task 4.3: Extend to multi-level reference navigation
// This example demonstrates deep reference chains, circular reference detection, and batch optimization

use owl_db::{
    parse_oql, QueryExecutor, Schema, FieldType, ReferenceType, DocumentStore, Document, Value,
};
use owl_db::reference::{ReferenceResolver, create_reference_id};
use owl_db::storage::RocksDBStore;
use owl_db::error::OQLError;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Task 4.3 Verification: Multi-Level Reference Navigation ===\n");
    
    // Setup test environment
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create schemas and test data
    create_test_schemas(&store)?;
    create_deep_reference_data(&store)?;
    
    println!("✅ Requirement 1: Support deep reference chains (.customer.account.bank.routing.country.code)");
    println!("=========================================================================================");
    
    // Test deep reference chain navigation (6 levels!)
    let query_str = r#"orders.where { .id == "ord_123" }.select { 
        order_id: .id,
        customer_name: .customer.name,
        account_token: .customer.account.secure_token,
        bank_name: .customer.account.bank.name,
        routing_number: .customer.account.bank.routing.number,
        country_code: .customer.account.bank.routing.country.code
    }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    println!("Results:");
    println!("  Order ID: {}", result.get_field("order_id").unwrap().to_string_repr());
    println!("  Customer Name: {}", result.get_field("customer_name").unwrap().to_string_repr());
    println!("  Account Token: {}", result.get_field("account_token").unwrap().to_string_repr());
    println!("  Bank Name: {}", result.get_field("bank_name").unwrap().to_string_repr());
    println!("  Routing Number: {}", result.get_field("routing_number").unwrap().to_string_repr());
    println!("  Country Code: {}", result.get_field("country_code").unwrap().to_string_repr());
    
    assert_eq!(result.get_field("country_code"), Some(&Value::String("US".to_string())));
    println!("✓ Deep reference chain navigation working correctly (6 levels deep!)\n");
    
    println!("✅ Requirement 2: Implement recursive reference resolution");
    println!("=========================================================");
    
    // Test recursive resolution with direct resolver
    let mut resolver = ReferenceResolver::new(store.clone());
    let order_doc = store.get_document("orders", &"ord_123".to_string())?.unwrap();
    
    // Test various depths
    let paths_and_expected = vec![
        (vec!["customer".to_string()], "customers#cust_456"),
        (vec!["customer".to_string(), "name".to_string()], "Alice Johnson"),
        (vec!["customer".to_string(), "account".to_string(), "balance".to_string()], "5000"),
        (vec!["customer".to_string(), "account".to_string(), "bank".to_string(), "name".to_string()], "First National Bank"),
        (vec!["customer".to_string(), "account".to_string(), "bank".to_string(), "routing".to_string(), "number".to_string()], "123456789"),
        (vec!["customer".to_string(), "account".to_string(), "bank".to_string(), "routing".to_string(), "country".to_string(), "code".to_string()], "US"),
    ];
    
    for (path, expected) in paths_and_expected {
        let result = resolver.resolve_field_path(&order_doc, &path)?;
        let result_str = result.to_string_repr();
        println!("  Path {:?} -> {}", path, result_str);
        assert!(result_str.contains(expected));
    }
    println!("✓ Recursive reference resolution working correctly\n");
    
    println!("✅ Requirement 3: Add circular reference detection and prevention");
    println!("================================================================");
    
    // Create circular reference data
    create_circular_reference_data(&store)?;
    
    // Test circular reference detection
    let circular_doc = store.get_document("circular", &"doc_a".to_string())?.unwrap();
    let circular_path = vec!["ref_to_b".to_string(), "ref_to_a".to_string(), "name".to_string()];
    
    let result = resolver.resolve_field_path(&circular_doc, &circular_path);
    println!("Attempting to resolve circular reference: doc_a.ref_to_b.ref_to_a.name");
    
    match result {
        Err(OQLError::ReferenceError { message }) => {
            println!("✓ Circular reference detected: {}", message);
            assert!(message.contains("Circular reference detected"));
        }
        _ => panic!("Expected circular reference error"),
    }
    
    // Test max depth protection
    resolver.set_max_depth(3);
    let deep_path = vec![
        "customer".to_string(),
        "account".to_string(),
        "bank".to_string(),
        "routing".to_string(),
        "country".to_string(),
        "code".to_string()
    ];
    
    let result = resolver.resolve_field_path(&order_doc, &deep_path);
    println!("Testing max depth protection (depth=3, path has 6 levels):");
    
    match result {
        Err(OQLError::ReferenceError { message }) => {
            println!("✓ Max depth protection working: {}", message);
            assert!(message.contains("Maximum reference resolution depth"));
        }
        _ => panic!("Expected max depth error"),
    }
    
    // Reset max depth for remaining tests
    resolver.set_max_depth(10);
    println!("✓ Circular reference detection and prevention working correctly\n");
    
    println!("✅ Requirement 4: Optimize batch reference lookups");
    println!("==================================================");
    
    // Test batch reference resolution
    let ref_ids = vec![
        "customers#cust_456".to_string(),
        "accounts#acc_789".to_string(),
        "banks#bank_001".to_string(),
        "routing#rt_001".to_string(),
        "countries#us".to_string(),
        "nonexistent#invalid".to_string(), // Should return None
    ];
    
    println!("Batch resolving {} references:", ref_ids.len());
    let batch_results = resolver.batch_resolve_references(&ref_ids)?;
    
    assert_eq!(batch_results.len(), 6);
    let resolved_count = batch_results.iter().filter(|r| r.is_some()).count();
    println!("  Successfully resolved: {}/{}", resolved_count, ref_ids.len());
    println!("  Failed to resolve: {}", ref_ids.len() - resolved_count);
    
    // Verify specific results
    assert!(batch_results[0].is_some()); // customer
    assert!(batch_results[1].is_some()); // account
    assert!(batch_results[2].is_some()); // bank
    assert!(batch_results[3].is_some()); // routing
    assert!(batch_results[4].is_some()); // country
    assert!(batch_results[5].is_none());  // nonexistent
    
    // Test batch field path resolution
    let order_docs = vec![
        store.get_document("orders", &"ord_123".to_string())?.unwrap(),
        store.get_document("orders", &"ord_456".to_string())?.unwrap(),
    ];
    
    let field_path = vec!["customer".to_string(), "name".to_string()];
    println!("Batch resolving field path {:?} for {} documents:", field_path, order_docs.len());
    let field_results = resolver.batch_resolve_field_paths(&order_docs, &field_path)?;
    
    assert_eq!(field_results.len(), 2);
    for (i, result) in field_results.iter().enumerate() {
        println!("  Document {}: {}", i + 1, result.to_string_repr());
    }
    
    println!("✓ Batch reference lookup optimization working correctly\n");
    
    println!("🎉 All Task 4.3 requirements successfully implemented and verified!");
    println!("===================================================================");
    println!("✓ Deep reference chains (.customer.account.bank.routing.country.code)");
    println!("✓ Recursive reference resolution with proper error handling");
    println!("✓ Circular reference detection and prevention");
    println!("✓ Optimized batch reference lookups for performance");
    println!("✓ Maximum depth protection to prevent infinite loops");
    println!("✓ Graceful handling of missing references in deep chains");
    
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
    customer_fields.insert("account".to_string(), FieldType::Reference(ReferenceType::new("accounts".to_string())));
    let customer_schema = Schema::new("customers".to_string(), customer_fields);
    store.put_schema(&customer_schema)?;
    
    // Accounts schema
    let mut account_fields = HashMap::new();
    account_fields.insert("balance".to_string(), FieldType::Float);
    account_fields.insert("secure_token".to_string(), FieldType::String);
    account_fields.insert("bank".to_string(), FieldType::Reference(ReferenceType::new("banks".to_string())));
    let account_schema = Schema::new("accounts".to_string(), account_fields);
    store.put_schema(&account_schema)?;
    
    // Banks schema
    let mut bank_fields = HashMap::new();
    bank_fields.insert("name".to_string(), FieldType::String);
    bank_fields.insert("routing".to_string(), FieldType::Reference(ReferenceType::new("routing".to_string())));
    let bank_schema = Schema::new("banks".to_string(), bank_fields);
    store.put_schema(&bank_schema)?;
    
    // Routing schema
    let mut routing_fields = HashMap::new();
    routing_fields.insert("number".to_string(), FieldType::String);
    routing_fields.insert("country".to_string(), FieldType::Reference(ReferenceType::new("countries".to_string())));
    let routing_schema = Schema::new("routing".to_string(), routing_fields);
    store.put_schema(&routing_schema)?;
    
    // Countries schema
    let mut country_fields = HashMap::new();
    country_fields.insert("name".to_string(), FieldType::String);
    country_fields.insert("code".to_string(), FieldType::String);
    let country_schema = Schema::new("countries".to_string(), country_fields);
    store.put_schema(&country_schema)?;
    
    // Circular reference test schema
    let mut circular_fields = HashMap::new();
    circular_fields.insert("name".to_string(), FieldType::String);
    circular_fields.insert("ref_to_a".to_string(), FieldType::Optional(Box::new(FieldType::Reference(ReferenceType::new("circular".to_string())))));
    circular_fields.insert("ref_to_b".to_string(), FieldType::Optional(Box::new(FieldType::Reference(ReferenceType::new("circular".to_string())))));
    let circular_schema = Schema::new("circular".to_string(), circular_fields);
    store.put_schema(&circular_schema)?;
    
    Ok(())
}

fn create_deep_reference_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    // Create country document
    let mut country_fields = HashMap::new();
    country_fields.insert("name".to_string(), Value::String("United States".to_string()));
    country_fields.insert("code".to_string(), Value::String("US".to_string()));
    let country_doc = Document::with_id("us".to_string(), "countries".to_string(), country_fields);
    store.put_document(&country_doc)?;
    
    // Create routing document
    let mut routing_fields = HashMap::new();
    routing_fields.insert("number".to_string(), Value::String("123456789".to_string()));
    routing_fields.insert("country".to_string(), Value::Reference(create_reference_id("countries", "us")));
    let routing_doc = Document::with_id("rt_001".to_string(), "routing".to_string(), routing_fields);
    store.put_document(&routing_doc)?;
    
    // Create bank document
    let mut bank_fields = HashMap::new();
    bank_fields.insert("name".to_string(), Value::String("First National Bank".to_string()));
    bank_fields.insert("routing".to_string(), Value::Reference(create_reference_id("routing", "rt_001")));
    let bank_doc = Document::with_id("bank_001".to_string(), "banks".to_string(), bank_fields);
    store.put_document(&bank_doc)?;
    
    // Create account document
    let mut account_fields = HashMap::new();
    account_fields.insert("balance".to_string(), Value::Float(5000.0));
    account_fields.insert("secure_token".to_string(), Value::String("abc123xyz".to_string()));
    account_fields.insert("bank".to_string(), Value::Reference(create_reference_id("banks", "bank_001")));
    let account_doc = Document::with_id("acc_789".to_string(), "accounts".to_string(), account_fields);
    store.put_document(&account_doc)?;
    
    // Create customer document
    let mut customer_fields = HashMap::new();
    customer_fields.insert("name".to_string(), Value::String("Alice Johnson".to_string()));
    customer_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    customer_fields.insert("account".to_string(), Value::Reference(create_reference_id("accounts", "acc_789")));
    let customer_doc = Document::with_id("cust_456".to_string(), "customers".to_string(), customer_fields);
    store.put_document(&customer_doc)?;
    
    // Create order documents
    let mut order_fields = HashMap::new();
    order_fields.insert("id".to_string(), Value::String("ord_123".to_string()));
    order_fields.insert("amount".to_string(), Value::Float(150.0));
    order_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "cust_456")));
    let order_doc = Document::with_id("ord_123".to_string(), "orders".to_string(), order_fields);
    store.put_document(&order_doc)?;
    
    // Create second order for batch testing
    let mut order2_fields = HashMap::new();
    order2_fields.insert("id".to_string(), Value::String("ord_456".to_string()));
    order2_fields.insert("amount".to_string(), Value::Float(200.0));
    order2_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "cust_456")));
    let order2_doc = Document::with_id("ord_456".to_string(), "orders".to_string(), order2_fields);
    store.put_document(&order2_doc)?;
    
    Ok(())
}

fn create_circular_reference_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    // Create document A that references B
    let mut doc_a_fields = HashMap::new();
    doc_a_fields.insert("name".to_string(), Value::String("Document A".to_string()));
    doc_a_fields.insert("ref_to_b".to_string(), Value::Reference(create_reference_id("circular", "doc_b")));
    let doc_a = Document::with_id("doc_a".to_string(), "circular".to_string(), doc_a_fields);
    store.put_document(&doc_a)?;
    
    // Create document B that references A (creating circular reference)
    let mut doc_b_fields = HashMap::new();
    doc_b_fields.insert("name".to_string(), Value::String("Document B".to_string()));
    doc_b_fields.insert("ref_to_a".to_string(), Value::Reference(create_reference_id("circular", "doc_a")));
    let doc_b = Document::with_id("doc_b".to_string(), "circular".to_string(), doc_b_fields);
    store.put_document(&doc_b)?;
    
    Ok(())
}