// Integration test for single-level reference resolution (Task 4.2)

#[cfg(test)]
mod single_level_reference_tests {
    use owl_db::{
        parse_oql, QueryExecutor, Schema, FieldType, ReferenceType, DocumentStore, Document, Value,
    };
    use owl_db::reference::create_reference_id;
    use owl_db::storage::RocksDBStore;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    struct SingleLevelReferenceFixture {
        executor: QueryExecutor<RocksDBStore>,
        store: Arc<RocksDBStore>,
        _temp_dir: TempDir,
    }

    impl SingleLevelReferenceFixture {
        fn new() -> Self {
            let temp_dir = TempDir::new().expect("Failed to create temp directory");
            let store = Arc::new(RocksDBStore::new(temp_dir.path()).expect("Failed to create store"));
            let executor = QueryExecutor::new(store.clone());
            
            Self::create_schemas(&store);
            Self::create_test_data(&store);
            
            Self {
                executor,
                store,
                _temp_dir: temp_dir,
            }
        }
        
        fn create_schemas(store: &Arc<RocksDBStore>) {
            // Orders schema with customer reference
            let mut order_fields = HashMap::new();
            order_fields.insert("id".to_string(), FieldType::String);
            order_fields.insert("amount".to_string(), FieldType::Float);
            order_fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
            
            let order_schema = Schema::new("orders".to_string(), order_fields);
            store.put_schema(&order_schema).expect("Failed to create orders schema");
            
            // Customers schema
            let mut customer_fields = HashMap::new();
            customer_fields.insert("name".to_string(), FieldType::String);
            customer_fields.insert("email".to_string(), FieldType::String);
            customer_fields.insert("phone".to_string(), FieldType::Optional(Box::new(FieldType::String)));
            customer_fields.insert("active".to_string(), FieldType::Boolean);
            
            let customer_schema = Schema::new("customers".to_string(), customer_fields);
            store.put_schema(&customer_schema).expect("Failed to create customers schema");
        }
        
        fn create_test_data(store: &Arc<RocksDBStore>) {
            // Create customers
            let customers = vec![
                ("cust_001", "Alice Johnson", "alice@example.com", Some("555-0001"), true),
                ("cust_002", "Bob Smith", "bob@example.com", Some("555-0002"), false),
                ("cust_003", "Charlie Brown", "charlie@example.com", None, true),
            ];
            
            for (id, name, email, phone, active) in customers {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), Value::String(name.to_string()));
                fields.insert("email".to_string(), Value::String(email.to_string()));
                if let Some(phone_num) = phone {
                    fields.insert("phone".to_string(), Value::String(phone_num.to_string()));
                }
                fields.insert("active".to_string(), Value::Boolean(active));
                
                let customer_doc = Document::with_id(id.to_string(), "customers".to_string(), fields);
                store.put_document(&customer_doc).expect("Failed to create customer");
            }
            
            // Create orders
            let orders = vec![
                ("ord_001", 150.0, "cust_001"),
                ("ord_002", 75.0, "cust_001"),
                ("ord_003", 200.0, "cust_002"),
                ("ord_004", 50.0, "cust_003"),
            ];
            
            for (id, amount, customer_id) in orders {
                let mut fields = HashMap::new();
                fields.insert("id".to_string(), Value::String(id.to_string()));
                fields.insert("amount".to_string(), Value::Float(amount));
                fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", customer_id)));
                
                let order_doc = Document::with_id(id.to_string(), "orders".to_string(), fields);
                store.put_document(&order_doc).expect("Failed to create order");
            }
            
            // Create order with invalid reference for testing graceful handling
            let mut invalid_fields = HashMap::new();
            invalid_fields.insert("id".to_string(), Value::String("ord_invalid".to_string()));
            invalid_fields.insert("amount".to_string(), Value::Float(100.0));
            invalid_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "nonexistent")));
            
            let invalid_order = Document::with_id("ord_invalid".to_string(), "orders".to_string(), invalid_fields);
            store.put_document(&invalid_order).expect("Failed to create invalid order");
        }
    }

    #[test]
    fn test_single_level_reference_access() {
        let fixture = SingleLevelReferenceFixture::new();
        
        // Test accessing reference field directly (should return reference ID)
        let query_str = r#"orders.where { .id == "ord_001" }.select { customer_ref: .customer }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 1);
        let doc = &results[0];
        
        // Should return the reference ID
        assert!(matches!(doc.get_field("customer_ref"), Some(Value::Reference(_))));
        if let Some(Value::Reference(ref_id)) = doc.get_field("customer_ref") {
            assert_eq!(ref_id, "customers#cust_001");
        }
    }

    #[test]
    fn test_single_level_reference_navigation() {
        let fixture = SingleLevelReferenceFixture::new();
        
        // Test single-level navigation to customer name
        let query_str = r#"orders.where { .id == "ord_001" }.select { customer_name: .customer.name }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 1);
        let doc = &results[0];
        
        // Should return the actual customer name
        assert_eq!(doc.get_field("customer_name"), Some(&Value::String("Alice Johnson".to_string())));
    }

    #[test]
    fn test_multiple_single_level_references() {
        let fixture = SingleLevelReferenceFixture::new();
        
        // Test multiple single-level references in one query
        let query_str = r#"orders.select { 
            order_id: .id,
            customer_name: .customer.name,
            customer_email: .customer.email,
            customer_active: .customer.active
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        // Should return all orders including the invalid one
        assert_eq!(results.len(), 5);
        
        // Check specific orders
        let alice_orders: Vec<_> = results.iter()
            .filter(|doc| doc.get_field("customer_name") == Some(&Value::String("Alice Johnson".to_string())))
            .collect();
        assert_eq!(alice_orders.len(), 2); // ord_001 and ord_002
        
        let bob_orders: Vec<_> = results.iter()
            .filter(|doc| doc.get_field("customer_name") == Some(&Value::String("Bob Smith".to_string())))
            .collect();
        assert_eq!(bob_orders.len(), 1); // ord_003
        
        let charlie_orders: Vec<_> = results.iter()
            .filter(|doc| doc.get_field("customer_name") == Some(&Value::String("Charlie Brown".to_string())))
            .collect();
        assert_eq!(charlie_orders.len(), 1); // ord_004
        
        // Check invalid order returns null for customer fields
        let invalid_orders: Vec<_> = results.iter()
            .filter(|doc| doc.get_field("order_id") == Some(&Value::String("ord_invalid".to_string())))
            .collect();
        assert_eq!(invalid_orders.len(), 1);
        let invalid_order = invalid_orders[0];
        assert_eq!(invalid_order.get_field("customer_name"), Some(&Value::Null));
        assert_eq!(invalid_order.get_field("customer_email"), Some(&Value::Null));
        assert_eq!(invalid_order.get_field("customer_active"), Some(&Value::Null));
    }

    #[test]
    fn test_graceful_missing_reference_handling() {
        let fixture = SingleLevelReferenceFixture::new();
        
        // Test query with missing reference
        let query_str = r#"orders.where { .id == "ord_invalid" }.select { customer_name: .customer.name }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Query should execute gracefully");
        
        assert_eq!(results.len(), 1);
        let doc = &results[0];
        
        // Should return null for missing reference, not error
        assert_eq!(doc.get_field("customer_name"), Some(&Value::Null));
    }

    #[test]
    fn test_reference_navigation_with_filtering() {
        let fixture = SingleLevelReferenceFixture::new();
        
        // Test filtering based on referenced field
        let query_str = r#"orders.where { .customer.active == true }.select { 
            order_id: .id,
            customer_name: .customer.name
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        // Should return orders from active customers (Alice and Charlie)
        // Note: This test assumes the WHERE clause can navigate references, which may not be implemented yet
        // If not implemented, this test will help identify that functionality gap
        
        for doc in &results {
            let customer_name = doc.get_field("customer_name");
            // Should only have orders from active customers
            assert!(
                customer_name == Some(&Value::String("Alice Johnson".to_string())) ||
                customer_name == Some(&Value::String("Charlie Brown".to_string()))
            );
        }
    }

    #[test]
    fn test_optional_reference_fields() {
        let fixture = SingleLevelReferenceFixture::new();
        
        // Test accessing optional fields through references
        let query_str = r#"orders.select { 
            order_id: .id,
            customer_phone: .customer.phone
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        // Check that some customers have phone numbers and some don't
        let with_phone: Vec<_> = results.iter()
            .filter(|doc| !matches!(doc.get_field("customer_phone"), Some(Value::Null)))
            .collect();
        
        let without_phone: Vec<_> = results.iter()
            .filter(|doc| matches!(doc.get_field("customer_phone"), Some(Value::Null)))
            .collect();
        
        // Alice and Bob have phones, Charlie and invalid order don't
        assert!(with_phone.len() >= 2);
        assert!(without_phone.len() >= 2);
    }

    #[test]
    fn test_reference_caching_performance() {
        let fixture = SingleLevelReferenceFixture::new();
        
        // Execute multiple queries that should benefit from caching
        let queries = vec![
            r#"orders.where { .id == "ord_001" }.select { customer_name: .customer.name }"#,
            r#"orders.where { .id == "ord_002" }.select { customer_name: .customer.name }"#,
            r#"orders.where { .id == "ord_001" }.select { customer_email: .customer.email }"#,
        ];
        
        for query_str in queries {
            let query = parse_oql(query_str).expect("Failed to parse query");
            let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
            
            assert_eq!(results.len(), 1);
            // Both ord_001 and ord_002 reference the same customer (Alice)
            // The reference resolver should cache Alice's document after the first lookup
        }
        
        // This test mainly verifies that caching doesn't break functionality
        // Performance benefits would be measured in a benchmark test
    }
}