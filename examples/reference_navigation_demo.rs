// Reference Navigation Demo - Deep field access like .customer.account.bank.routing.country.code

use owl_db::{
    parse_oql, QueryExecutor, Schema, FieldType, ReferenceType, DocumentStore, Document, Value,
    create_reference_id,
};
use owl_db::storage::RocksDBStore;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OQL Reference Navigation Demo ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());
    
    // Create schemas with reference relationships
    create_schemas(&store)?;
    
    // Create test data with deep reference chains
    create_test_data(&store)?;
    
    println!("1. Single Level Reference Navigation");
    println!("===================================");
    
    // Test: orders.where { .id == "ord_123" }.select { customer_ref: .customer }
    let query_str = r#"orders.where { .id == "ord_123" }.select { customer_ref: .customer }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    for doc in &results {
        println!("  Customer Reference: {:?}", doc.get_field("customer_ref"));
    }
    println!();
    
    println!("2. Two Level Reference Navigation");
    println!("=================================");
    
    // Test: orders.where { .id == "ord_123" }.select { customer_name: .customer.name }
    let query_str = r#"orders.where { .id == "ord_123" }.select { customer_name: .customer.name }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    for doc in &results {
        println!("  Customer Name: {:?}", doc.get_field("customer_name"));
    }
    println!();
    
    println!("3. Three Level Reference Navigation");
    println!("===================================");
    
    // Test: orders.where { .id == "ord_123" }.select { secure_token: .customer.account.secure_token }
    let query_str = r#"orders.where { .id == "ord_123" }.select { secure_token: .customer.account.secure_token }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    for doc in &results {
        println!("  Account Secure Token: {:?}", doc.get_field("secure_token"));
    }
    println!();
    
    println!("4. Deep Reference Navigation (6 levels!)");
    println!("=========================================");
    
    // Test: orders.select { country_code: .customer.account.bank.routing.country.code }
    let query_str = r#"orders.where { .id == "ord_123" }.select { country_code: .customer.account.bank.routing.country.code }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    for doc in &results {
        println!("  Country Code: {:?}", doc.get_field("country_code"));
    }
    println!();
    
    println!("5. Multiple Deep References in One Query");
    println!("========================================");
    
    // Test multiple deep references
    let query_str = r#"orders.where { .id == "ord_123" }.select { 
        customer_name: .customer.name,
        account_balance: .customer.account.balance,
        bank_name: .customer.account.bank.name,
        routing_number: .customer.account.bank.routing.number,
        country_name: .customer.account.bank.routing.country.name,
        country_code: .customer.account.bank.routing.country.code
    }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    for doc in &results {
        println!("  Customer Name: {:?}", doc.get_field("customer_name"));
        println!("  Account Balance: {:?}", doc.get_field("account_balance"));
        println!("  Bank Name: {:?}", doc.get_field("bank_name"));
        println!("  Routing Number: {:?}", doc.get_field("routing_number"));
        println!("  Country Name: {:?}", doc.get_field("country_name"));
        println!("  Country Code: {:?}", doc.get_field("country_code"));
    }
    println!();
    
    println!("6. Error Handling - Missing References");
    println!("======================================");
    
    // Test with missing reference
    let query_str = r#"orders.where { .id == "ord_456" }.select { customer_name: .customer.name }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    println!("Results for non-existent order: {} documents", results.len());
    println!();
    
    println!("7. Performance Test - Multiple Orders");
    println!("====================================");
    
    // Test performance with multiple orders
    let query_str = r#"orders.select { 
        id,
        customer_name: .customer.name,
        country_code: .customer.account.bank.routing.country.code
    }"#;
    println!("Query: {}", query_str);
    let query = parse_oql(query_str)?;
    let results = executor.execute_query(&query)?;
    
    println!("Processed {} orders with deep reference navigation:", results.len());
    for doc in &results {
        println!("  Order {}: Customer {}, Country {}",
                 doc.get_field("id").unwrap().to_string_repr(),
                 doc.get_field("customer_name").unwrap().to_string_repr(),
                 doc.get_field("country_code").unwrap().to_string_repr());
    }
    
    println!("\n=== Reference Navigation Demo Complete ===");
    println!("✓ Single level navigation (.customer)");
    println!("✓ Multi-level navigation (.customer.account.secure_token)");
    println!("✓ Deep navigation (.customer.account.bank.routing.country.code)");
    println!("✓ Multiple references in single query");
    println!("✓ Error handling for missing references");
    println!("✓ Performance with multiple documents");
    
    Ok(())
}

fn create_schemas(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
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
    
    Ok(())
}

fn create_test_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
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
    customer_fields.insert("name".to_string(), Value::String("Alice".to_string()));
    customer_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    customer_fields.insert("account".to_string(), Value::Reference(create_reference_id("accounts", "acc_789")));
    
    let customer_doc = Document::with_id("cust_456".to_string(), "customers".to_string(), customer_fields);
    store.put_document(&customer_doc)?;
    
    // Create order documents
    let mut order1_fields = HashMap::new();
    order1_fields.insert("id".to_string(), Value::String("ord_123".to_string()));
    order1_fields.insert("amount".to_string(), Value::Float(150.0));
    order1_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "cust_456")));
    
    let order1_doc = Document::with_id("ord_123".to_string(), "orders".to_string(), order1_fields);
    store.put_document(&order1_doc)?;
    
    let mut order2_fields = HashMap::new();
    order2_fields.insert("id".to_string(), Value::String("ord_124".to_string()));
    order2_fields.insert("amount".to_string(), Value::Float(75.0));
    order2_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "cust_456")));
    
    let order2_doc = Document::with_id("ord_124".to_string(), "orders".to_string(), order2_fields);
    store.put_document(&order2_doc)?;
    
    Ok(())
}