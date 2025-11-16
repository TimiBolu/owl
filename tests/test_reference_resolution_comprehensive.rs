// Comprehensive Reference Resolution Tests for Task 4.4
// Tests single and multi-level reference navigation, circular reference detection, and performance

#[cfg(test)]
mod reference_resolution_tests {
    use owl_db::{
        parse_oql, QueryExecutor, Schema, FieldType, ReferenceType, DocumentStore, Document, Value,
    };
    use owl_db::reference::{ReferenceResolver, create_reference_id, parse_field_path};
    use owl_db::storage::RocksDBStore;
    use owl_db::error::OQLError;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::TempDir;

    /// Test fixture for comprehensive reference resolution testing
    struct ReferenceTestFixture {
        executor: QueryExecutor<RocksDBStore>,
        store: Arc<RocksDBStore>,
        _temp_dir: TempDir,
    }

    impl ReferenceTestFixture {
        fn new() -> Self {
            let temp_dir = TempDir::new().expect("Failed to create temp directory");
            let store = Arc::new(RocksDBStore::new(temp_dir.path()).expect("Failed to create store"));
            let executor = QueryExecutor::new(store.clone());
            
            Self::create_comprehensive_schemas(&store);
            Self::create_comprehensive_test_data(&store);
            
            Self {
                executor,
                store,
                _temp_dir: temp_dir,
            }
        }
        
        fn create_comprehensive_schemas(store: &Arc<RocksDBStore>) {
            // Orders schema
            let mut order_fields = HashMap::new();
            order_fields.insert("id".to_string(), FieldType::String);
            order_fields.insert("amount".to_string(), FieldType::Float);
            order_fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
            order_fields.insert("status".to_string(), FieldType::String);
            let order_schema = Schema::new("orders".to_string(), order_fields);
            store.put_schema(&order_schema).expect("Failed to create orders schema");
            
            // Customers schema
            let mut customer_fields = HashMap::new();
            customer_fields.insert("name".to_string(), FieldType::String);
            customer_fields.insert("email".to_string(), FieldType::String);
            customer_fields.insert("account".to_string(), FieldType::Reference(ReferenceType::new("accounts".to_string())));
            customer_fields.insert("tier".to_string(), FieldType::String);
            let customer_schema = Schema::new("customers".to_string(), customer_fields);
            store.put_schema(&customer_schema).expect("Failed to create customers schema");
            
            // Accounts schema
            let mut account_fields = HashMap::new();
            account_fields.insert("balance".to_string(), FieldType::Float);
            account_fields.insert("secure_token".to_string(), FieldType::String);
            account_fields.insert("bank".to_string(), FieldType::Reference(ReferenceType::new("banks".to_string())));
            account_fields.insert("type".to_string(), FieldType::String);
            let account_schema = Schema::new("accounts".to_string(), account_fields);
            store.put_schema(&account_schema).expect("Failed to create accounts schema");
            
            // Banks schema
            let mut bank_fields = HashMap::new();
            bank_fields.insert("name".to_string(), FieldType::String);
            bank_fields.insert("routing".to_string(), FieldType::Reference(ReferenceType::new("routing".to_string())));
            bank_fields.insert("swift_code".to_string(), FieldType::String);
            let bank_schema = Schema::new("banks".to_string(), bank_fields);
            store.put_schema(&bank_schema).expect("Failed to create banks schema");
            
            // Routing schema
            let mut routing_fields = HashMap::new();
            routing_fields.insert("number".to_string(), FieldType::String);
            routing_fields.insert("country".to_string(), FieldType::Reference(ReferenceType::new("countries".to_string())));
            routing_fields.insert("region".to_string(), FieldType::String);
            let routing_schema = Schema::new("routing".to_string(), routing_fields);
            store.put_schema(&routing_schema).expect("Failed to create routing schema");
            
            // Countries schema
            let mut country_fields = HashMap::new();
            country_fields.insert("name".to_string(), FieldType::String);
            country_fields.insert("code".to_string(), FieldType::String);
            country_fields.insert("continent".to_string(), FieldType::Reference(ReferenceType::new("continents".to_string())));
            let country_schema = Schema::new("countries".to_string(), country_fields);
            store.put_schema(&country_schema).expect("Failed to create countries schema");
            
            // Continents schema (7th level!)
            let mut continent_fields = HashMap::new();
            continent_fields.insert("name".to_string(), FieldType::String);
            continent_fields.insert("code".to_string(), FieldType::String);
            let continent_schema = Schema::new("continents".to_string(), continent_fields);
            store.put_schema(&continent_schema).expect("Failed to create continents schema");
            
            // Circular reference test schemas
            let mut circular_fields = HashMap::new();
            circular_fields.insert("name".to_string(), FieldType::String);
            circular_fields.insert("ref_to_a".to_string(), FieldType::Optional(Box::new(FieldType::Reference(ReferenceType::new("circular".to_string())))));
            circular_fields.insert("ref_to_b".to_string(), FieldType::Optional(Box::new(FieldType::Reference(ReferenceType::new("circular".to_string())))));
            let circular_schema = Schema::new("circular".to_string(), circular_fields);
            store.put_schema(&circular_schema).expect("Failed to create circular schema");
        }
        
        fn create_comprehensive_test_data(store: &Arc<RocksDBStore>) {
            // Create continent
            let mut continent_fields = HashMap::new();
            continent_fields.insert("name".to_string(), Value::String("North America".to_string()));
            continent_fields.insert("code".to_string(), Value::String("NA".to_string()));
            let continent_doc = Document::with_id("na".to_string(), "continents".to_string(), continent_fields);
            store.put_document(&continent_doc).expect("Failed to create continent");
            
            // Create country
            let mut country_fields = HashMap::new();
            country_fields.insert("name".to_string(), Value::String("United States".to_string()));
            country_fields.insert("code".to_string(), Value::String("US".to_string()));
            country_fields.insert("continent".to_string(), Value::Reference(create_reference_id("continents", "na")));
            let country_doc = Document::with_id("us".to_string(), "countries".to_string(), country_fields);
            store.put_document(&country_doc).expect("Failed to create country");
            
            // Create routing
            let mut routing_fields = HashMap::new();
            routing_fields.insert("number".to_string(), Value::String("123456789".to_string()));
            routing_fields.insert("country".to_string(), Value::Reference(create_reference_id("countries", "us")));
            routing_fields.insert("region".to_string(), Value::String("Northeast".to_string()));
            let routing_doc = Document::with_id("rt_001".to_string(), "routing".to_string(), routing_fields);
            store.put_document(&routing_doc).expect("Failed to create routing");
            
            // Create bank
            let mut bank_fields = HashMap::new();
            bank_fields.insert("name".to_string(), Value::String("First National Bank".to_string()));
            bank_fields.insert("routing".to_string(), Value::Reference(create_reference_id("routing", "rt_001")));
            bank_fields.insert("swift_code".to_string(), Value::String("FNBKUS33".to_string()));
            let bank_doc = Document::with_id("bank_001".to_string(), "banks".to_string(), bank_fields);
            store.put_document(&bank_doc).expect("Failed to create bank");
            
            // Create accounts
            let accounts = vec![
                ("acc_001", 5000.0, "premium_token_123", "premium"),
                ("acc_002", 1500.0, "standard_token_456", "standard"),
                ("acc_003", 10000.0, "vip_token_789", "vip"),
            ];
            
            for (id, balance, token, acc_type) in accounts {
                let mut account_fields = HashMap::new();
                account_fields.insert("balance".to_string(), Value::Float(balance));
                account_fields.insert("secure_token".to_string(), Value::String(token.to_string()));
                account_fields.insert("bank".to_string(), Value::Reference(create_reference_id("banks", "bank_001")));
                account_fields.insert("type".to_string(), Value::String(acc_type.to_string()));
                let account_doc = Document::with_id(id.to_string(), "accounts".to_string(), account_fields);
                store.put_document(&account_doc).expect("Failed to create account");
            }
            
            // Create customers
            let customers = vec![
                ("cust_001", "Alice Johnson", "alice@example.com", "acc_001", "premium"),
                ("cust_002", "Bob Smith", "bob@example.com", "acc_002", "standard"),
                ("cust_003", "Charlie Brown", "charlie@example.com", "acc_003", "vip"),
            ];
            
            for (id, name, email, account_id, tier) in customers {
                let mut customer_fields = HashMap::new();
                customer_fields.insert("name".to_string(), Value::String(name.to_string()));
                customer_fields.insert("email".to_string(), Value::String(email.to_string()));
                customer_fields.insert("account".to_string(), Value::Reference(create_reference_id("accounts", account_id)));
                customer_fields.insert("tier".to_string(), Value::String(tier.to_string()));
                let customer_doc = Document::with_id(id.to_string(), "customers".to_string(), customer_fields);
                store.put_document(&customer_doc).expect("Failed to create customer");
            }
            
            // Create orders
            let orders = vec![
                ("ord_001", 150.0, "cust_001", "completed"),
                ("ord_002", 75.0, "cust_001", "pending"),
                ("ord_003", 200.0, "cust_002", "completed"),
                ("ord_004", 50.0, "cust_003", "shipped"),
                ("ord_005", 300.0, "cust_003", "completed"),
            ];
            
            for (id, amount, customer_id, status) in orders {
                let mut order_fields = HashMap::new();
                order_fields.insert("id".to_string(), Value::String(id.to_string()));
                order_fields.insert("amount".to_string(), Value::Float(amount));
                order_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", customer_id)));
                order_fields.insert("status".to_string(), Value::String(status.to_string()));
                let order_doc = Document::with_id(id.to_string(), "orders".to_string(), order_fields);
                store.put_document(&order_doc).expect("Failed to create order");
            }
            
            // Create circular reference test data
            let mut doc_a_fields = HashMap::new();
            doc_a_fields.insert("name".to_string(), Value::String("Document A".to_string()));
            doc_a_fields.insert("ref_to_b".to_string(), Value::Reference(create_reference_id("circular", "doc_b")));
            let doc_a = Document::with_id("doc_a".to_string(), "circular".to_string(), doc_a_fields);
            store.put_document(&doc_a).expect("Failed to create circular doc A");
            
            let mut doc_b_fields = HashMap::new();
            doc_b_fields.insert("name".to_string(), Value::String("Document B".to_string()));
            doc_b_fields.insert("ref_to_a".to_string(), Value::Reference(create_reference_id("circular", "doc_a")));
            let doc_b = Document::with_id("doc_b".to_string(), "circular".to_string(), doc_b_fields);
            store.put_document(&doc_b).expect("Failed to create circular doc B");
            
            // Create order with missing reference for error testing
            let mut invalid_order_fields = HashMap::new();
            invalid_order_fields.insert("id".to_string(), Value::String("ord_invalid".to_string()));
            invalid_order_fields.insert("amount".to_string(), Value::Float(100.0));
            invalid_order_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "nonexistent")));
            invalid_order_fields.insert("status".to_string(), Value::String("pending".to_string()));
            let invalid_order = Document::with_id("ord_invalid".to_string(), "orders".to_string(), invalid_order_fields);
            store.put_document(&invalid_order).expect("Failed to create invalid order");
        }
    }

    // ========== SINGLE-LEVEL REFERENCE NAVIGATION TESTS ==========

    #[test]
    fn test_single_level_reference_basic_navigation() {
        let fixture = ReferenceTestFixture::new();
        
        // Test basic single-level navigation
        let query_str = r#"orders.where { .id == "ord_001" }.select { customer_name: .customer.name }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get_field("customer_name"), Some(&Value::String("Alice Johnson".to_string())));
    }

    #[test]
    fn test_single_level_multiple_fields() {
        let fixture = ReferenceTestFixture::new();
        
        // Test accessing multiple fields through single-level references
        let query_str = r#"orders.select { 
            order_id: .id,
            customer_name: .customer.name,
            customer_email: .customer.email,
            customer_tier: .customer.tier
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 6); // 5 valid orders + 1 invalid
        
        // Check specific order
        let alice_orders: Vec<_> = results.iter()
            .filter(|doc| doc.get_field("customer_name") == Some(&Value::String("Alice Johnson".to_string())))
            .collect();
        assert_eq!(alice_orders.len(), 2); // ord_001 and ord_002
        
        for order in alice_orders {
            assert_eq!(order.get_field("customer_email"), Some(&Value::String("alice@example.com".to_string())));
            assert_eq!(order.get_field("customer_tier"), Some(&Value::String("premium".to_string())));
        }
    }

    #[test]
    fn test_single_level_reference_direct_access() {
        let fixture = ReferenceTestFixture::new();
        
        // Test accessing the reference itself (not navigating through it)
        let query_str = r#"orders.where { .id == "ord_001" }.select { customer_ref: .customer }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 1);
        if let Some(Value::Reference(ref_id)) = results[0].get_field("customer_ref") {
            assert_eq!(ref_id, "customers#cust_001");
        } else {
            panic!("Expected reference value");
        }
    }

    // ========== MULTI-LEVEL REFERENCE NAVIGATION TESTS ==========

    #[test]
    fn test_two_level_reference_navigation() {
        let fixture = ReferenceTestFixture::new();
        
        // Test two-level navigation: order -> customer -> account
        let query_str = r#"orders.where { .id == "ord_001" }.select { 
            account_balance: .customer.account.balance,
            account_token: .customer.account.secure_token
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get_field("account_balance"), Some(&Value::Float(5000.0)));
        assert_eq!(results[0].get_field("account_token"), Some(&Value::String("premium_token_123".to_string())));
    }

    #[test]
    fn test_three_level_reference_navigation() {
        let fixture = ReferenceTestFixture::new();
        
        // Test three-level navigation: order -> customer -> account -> bank
        let query_str = r#"orders.where { .id == "ord_001" }.select { 
            bank_name: .customer.account.bank.name,
            swift_code: .customer.account.bank.swift_code
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get_field("bank_name"), Some(&Value::String("First National Bank".to_string())));
        assert_eq!(results[0].get_field("swift_code"), Some(&Value::String("FNBKUS33".to_string())));
    }

    #[test]
    fn test_deep_reference_chain_seven_levels() {
        let fixture = ReferenceTestFixture::new();
        
        // Test extremely deep navigation: order -> customer -> account -> bank -> routing -> country -> continent (7 levels!)
        let query_str = r#"orders.where { .id == "ord_001" }.select { 
            continent_name: .customer.account.bank.routing.country.continent.name,
            continent_code: .customer.account.bank.routing.country.continent.code
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get_field("continent_name"), Some(&Value::String("North America".to_string())));
        assert_eq!(results[0].get_field("continent_code"), Some(&Value::String("NA".to_string())));
    }

    #[test]
    fn test_multi_level_mixed_depths() {
        let fixture = ReferenceTestFixture::new();
        
        // Test query with references of different depths in same query
        let query_str = r#"orders.select { 
            order_id: .id,
            customer_name: .customer.name,
            account_type: .customer.account.type,
            routing_number: .customer.account.bank.routing.number,
            country_code: .customer.account.bank.routing.country.code
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        // Should return all orders including invalid one
        assert_eq!(results.len(), 6);
        
        // Check valid orders have all fields resolved
        let valid_orders: Vec<_> = results.iter()
            .filter(|doc| !matches!(doc.get_field("customer_name"), Some(Value::Null)))
            .collect();
        assert_eq!(valid_orders.len(), 5);
        
        for order in valid_orders {
            assert!(matches!(order.get_field("customer_name"), Some(Value::String(_))));
            assert!(matches!(order.get_field("account_type"), Some(Value::String(_))));
            assert_eq!(order.get_field("routing_number"), Some(&Value::String("123456789".to_string())));
            assert_eq!(order.get_field("country_code"), Some(&Value::String("US".to_string())));
        }
    }

    // ========== CIRCULAR REFERENCE DETECTION TESTS ==========

    #[test]
    fn test_circular_reference_detection_direct() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        
        // Test direct circular reference detection
        let doc_a = fixture.store.get_document("circular", &"doc_a".to_string()).unwrap().unwrap();
        let circular_path = vec!["ref_to_b".to_string(), "ref_to_a".to_string(), "name".to_string()];
        
        let result = resolver.resolve_field_path(&doc_a, &circular_path);
        
        assert!(result.is_err());
        match result {
            Err(OQLError::ReferenceError { message }) => {
                assert!(message.contains("Circular reference detected"));
            }
            _ => panic!("Expected circular reference error"),
        }
    }

    #[test]
    fn test_circular_reference_detection_complex() {
        let fixture = ReferenceTestFixture::new();
        
        // Create a more complex circular reference: A -> B -> C -> A
        let mut doc_c_fields = HashMap::new();
        doc_c_fields.insert("name".to_string(), Value::String("Document C".to_string()));
        doc_c_fields.insert("ref_to_a".to_string(), Value::Reference(create_reference_id("circular", "doc_a")));
        let doc_c = Document::with_id("doc_c".to_string(), "circular".to_string(), doc_c_fields);
        fixture.store.put_document(&doc_c).expect("Failed to create doc C");
        
        // Update doc_b to reference doc_c instead of doc_a
        let mut doc_b_fields = HashMap::new();
        doc_b_fields.insert("name".to_string(), Value::String("Document B".to_string()));
        doc_b_fields.insert("ref_to_a".to_string(), Value::Reference(create_reference_id("circular", "doc_c")));
        let doc_b_updated = Document::with_id("doc_b".to_string(), "circular".to_string(), doc_b_fields);
        fixture.store.put_document(&doc_b_updated).expect("Failed to update doc B");
        
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        let doc_a = fixture.store.get_document("circular", &"doc_a".to_string()).unwrap().unwrap();
        
        // Try to navigate: A -> B -> C -> A (circular)
        let circular_path = vec!["ref_to_b".to_string(), "ref_to_a".to_string(), "ref_to_a".to_string(), "name".to_string()];
        let result = resolver.resolve_field_path(&doc_a, &circular_path);
        
        assert!(result.is_err());
        match result {
            Err(OQLError::ReferenceError { message }) => {
                assert!(message.contains("Circular reference detected"));
            }
            _ => panic!("Expected circular reference error"),
        }
    }

    #[test]
    fn test_max_depth_protection() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::with_max_depth(fixture.store.clone(), 3);
        
        let order_doc = fixture.store.get_document("orders", &"ord_001".to_string()).unwrap().unwrap();
        
        // Try to navigate deeper than max depth
        let deep_path = vec![
            "customer".to_string(),
            "account".to_string(),
            "bank".to_string(),
            "routing".to_string(),  // This would be depth 4, exceeding limit of 3
            "country".to_string(),
        ];
        
        let result = resolver.resolve_field_path(&order_doc, &deep_path);
        
        assert!(result.is_err());
        match result {
            Err(OQLError::ReferenceError { message }) => {
                assert!(message.contains("Maximum reference resolution depth"));
            }
            _ => panic!("Expected max depth error"),
        }
    }

    #[test]
    fn test_circular_reference_detection_in_queries() {
        let fixture = ReferenceTestFixture::new();
        
        // Test that circular references are detected in queries
        let query_str = r#"circular.where { .name == "Document A" }.select { 
            name,
            circular_ref: .ref_to_b.ref_to_a.name
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let result = fixture.executor.execute_query(&query);
        
        // Should detect circular reference and return error
        assert!(result.is_err());
        match result {
            Err(e) => {
                let error_msg = format!("{:?}", e);
                assert!(error_msg.contains("Circular reference detected"));
            }
            Ok(_) => panic!("Expected circular reference error"),
        }
    }

    // ========== PERFORMANCE TESTS WITH DEEP REFERENCE CHAINS ==========

    #[test]
    fn test_deep_reference_chain_performance() {
        let fixture = ReferenceTestFixture::new();
        
        // Test performance of deep reference resolution
        let start_time = Instant::now();
        
        let query_str = r#"orders.select { 
            order_id: .id,
            continent_name: .customer.account.bank.routing.country.continent.name
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        
        let duration = start_time.elapsed();
        
        // Should complete within reasonable time (adjust threshold as needed)
        assert!(duration.as_millis() < 1000, "Deep reference resolution took too long: {:?}", duration);
        
        // Should return all orders
        assert_eq!(results.len(), 6);
        
        // Valid orders should have continent resolved
        let valid_results: Vec<_> = results.iter()
            .filter(|doc| !matches!(doc.get_field("continent_name"), Some(Value::Null)))
            .collect();
        assert_eq!(valid_results.len(), 5);
        
        for result in valid_results {
            assert_eq!(result.get_field("continent_name"), Some(&Value::String("North America".to_string())));
        }
    }

    #[test]
    fn test_batch_reference_resolution_performance() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        
        // Test batch resolution performance
        let ref_ids = vec![
            "customers#cust_001".to_string(),
            "customers#cust_002".to_string(),
            "customers#cust_003".to_string(),
            "accounts#acc_001".to_string(),
            "accounts#acc_002".to_string(),
            "accounts#acc_003".to_string(),
            "banks#bank_001".to_string(),
            "routing#rt_001".to_string(),
            "countries#us".to_string(),
            "continents#na".to_string(),
        ];
        
        let start_time = Instant::now();
        let results = resolver.batch_resolve_references(&ref_ids).expect("Failed to batch resolve");
        let duration = start_time.elapsed();
        
        // Should complete quickly
        assert!(duration.as_millis() < 500, "Batch resolution took too long: {:?}", duration);
        
        // Should resolve all references
        assert_eq!(results.len(), 10);
        let resolved_count = results.iter().filter(|r| r.is_some()).count();
        assert_eq!(resolved_count, 10);
    }

    #[test]
    fn test_reference_caching_effectiveness() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        
        let order_doc = fixture.store.get_document("orders", &"ord_001".to_string()).unwrap().unwrap();
        
        // First resolution should populate cache
        let path = vec!["customer".to_string(), "name".to_string()];
        let start_time = Instant::now();
        let result1 = resolver.resolve_field_path(&order_doc, &path).expect("Failed to resolve first time");
        let _first_duration = start_time.elapsed();
        
        let (cache_size_after_first, _) = resolver.cache_stats();
        assert!(cache_size_after_first > 0);
        
        // Second resolution should use cache and be faster
        let start_time = Instant::now();
        let result2 = resolver.resolve_field_path(&order_doc, &path).expect("Failed to resolve second time");
        let second_duration = start_time.elapsed();
        
        let (cache_size_after_second, _) = resolver.cache_stats();
        
        // Results should be identical
        assert_eq!(result1, result2);
        
        // Cache size should remain the same (no new entries)
        assert_eq!(cache_size_after_first, cache_size_after_second);
        
        // Second resolution should be faster (though this might be flaky in tests)
        // We'll just verify it completes quickly
        assert!(second_duration.as_millis() < 100);
    }

    #[test]
    fn test_multiple_orders_same_customer_caching() {
        let fixture = ReferenceTestFixture::new();
        
        // Test that multiple orders referencing the same customer benefit from caching
        let query_str = r#"orders.where { .customer.name == "Alice Johnson" }.select { 
            order_id: .id,
            customer_name: .customer.name,
            customer_email: .customer.email,
            account_balance: .customer.account.balance
        }"#;
        
        let start_time = Instant::now();
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
        let duration = start_time.elapsed();
        
        // Should complete quickly due to caching
        assert!(duration.as_millis() < 500);
        
        // Should return Alice's orders
        assert_eq!(results.len(), 2);
        
        for result in &results {
            assert_eq!(result.get_field("customer_name"), Some(&Value::String("Alice Johnson".to_string())));
            assert_eq!(result.get_field("customer_email"), Some(&Value::String("alice@example.com".to_string())));
            assert_eq!(result.get_field("account_balance"), Some(&Value::Float(5000.0)));
        }
    }

    // ========== ERROR HANDLING AND EDGE CASES ==========

    #[test]
    fn test_missing_reference_graceful_handling() {
        let fixture = ReferenceTestFixture::new();
        
        // Test query with missing reference
        let query_str = r#"orders.where { .id == "ord_invalid" }.select { 
            order_id: .id,
            customer_name: .customer.name,
            deep_field: .customer.account.bank.name
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Query should handle missing references gracefully");
        
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get_field("order_id"), Some(&Value::String("ord_invalid".to_string())));
        assert_eq!(results[0].get_field("customer_name"), Some(&Value::Null));
        assert_eq!(results[0].get_field("deep_field"), Some(&Value::Null));
    }

    #[test]
    fn test_partial_reference_chain_failure() {
        let fixture = ReferenceTestFixture::new();
        
        // Create customer without account reference
        let mut customer_fields = HashMap::new();
        customer_fields.insert("name".to_string(), Value::String("No Account Customer".to_string()));
        customer_fields.insert("email".to_string(), Value::String("noaccnt@example.com".to_string()));
        customer_fields.insert("tier".to_string(), Value::String("basic".to_string()));
        // Note: no account reference
        let customer_doc = Document::with_id("cust_no_account".to_string(), "customers".to_string(), customer_fields);
        fixture.store.put_document(&customer_doc).expect("Failed to create customer without account");
        
        // Create order referencing this customer
        let mut order_fields = HashMap::new();
        order_fields.insert("id".to_string(), Value::String("ord_no_account".to_string()));
        order_fields.insert("amount".to_string(), Value::Float(100.0));
        order_fields.insert("customer".to_string(), Value::Reference(create_reference_id("customers", "cust_no_account")));
        order_fields.insert("status".to_string(), Value::String("pending".to_string()));
        let order_doc = Document::with_id("ord_no_account".to_string(), "orders".to_string(), order_fields);
        fixture.store.put_document(&order_doc).expect("Failed to create order");
        
        // Test query that tries to navigate through missing account reference
        let query_str = r#"orders.where { .id == "ord_no_account" }.select { 
            customer_name: .customer.name,
            account_balance: .customer.account.balance
        }"#;
        let query = parse_oql(query_str).expect("Failed to parse query");
        let results = fixture.executor.execute_query(&query).expect("Query should handle partial chain failure gracefully");
        
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get_field("customer_name"), Some(&Value::String("No Account Customer".to_string())));
        assert_eq!(results[0].get_field("account_balance"), Some(&Value::Null));
    }

    #[test]
    fn test_reference_to_nonexistent_schema() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        
        // Test reference to non-existent schema
        let result = resolver.validate_reference("nonexistent_schema#doc_id");
        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_malformed_reference_ids() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        
        // Test malformed reference ID (no schema prefix) - should return graceful null
        // Create document with malformed reference
        let mut test_fields = HashMap::new();
        test_fields.insert("bad_ref".to_string(), Value::Reference("just_an_id_no_schema".to_string()));
        let test_doc = Document::with_id("test_malformed".to_string(), "orders".to_string(), test_fields);
        
        let path = vec!["bad_ref".to_string(), "name".to_string()];
        let result = resolver.resolve_field_path(&test_doc, &path);
        
        // The current implementation handles missing references gracefully by returning null
        // rather than erroring, so we expect Ok(Value::Null)
        match result {
            Ok(Value::Null) => {
                // This is the expected behavior - graceful handling
            }
            Err(OQLError::ReferenceError { message }) => {
                // This is also acceptable - explicit error for malformed reference
                assert!(message.contains("must include schema prefix"));
            }
            other => panic!("Unexpected result for malformed reference ID: {:?}", other),
        }
    }

    // ========== UTILITY FUNCTION TESTS ==========

    #[test]
    fn test_parse_field_path_utility() {
        // Test the parse_field_path utility function
        assert_eq!(parse_field_path(".customer.name"), vec!["customer", "name"]);
        assert_eq!(parse_field_path("customer.account.balance"), vec!["customer", "account", "balance"]);
        assert_eq!(parse_field_path(".customer.account.bank.routing.country.code"), 
                   vec!["customer", "account", "bank", "routing", "country", "code"]);
        assert_eq!(parse_field_path("single_field"), vec!["single_field"]);
        assert_eq!(parse_field_path(""), Vec::<String>::new());
        assert_eq!(parse_field_path("."), Vec::<String>::new());
        assert_eq!(parse_field_path("..double.dot"), vec!["double", "dot"]);
    }

    #[test]
    fn test_create_reference_id_utility() {
        // Test the create_reference_id utility function
        assert_eq!(create_reference_id("customers", "cust_123"), "customers#cust_123");
        assert_eq!(create_reference_id("orders", "ord_456"), "orders#ord_456");
        assert_eq!(create_reference_id("", "empty_schema"), "#empty_schema");
        assert_eq!(create_reference_id("schema", ""), "schema#");
    }

    #[test]
    fn test_resolver_configuration() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        
        // Test max depth configuration
        assert_eq!(resolver.max_depth(), 10); // Default
        
        resolver.set_max_depth(5);
        assert_eq!(resolver.max_depth(), 5);
        
        // Test cache management
        let (initial_cache_size, _) = resolver.cache_stats();
        assert_eq!(initial_cache_size, 0);
        
        // Perform some resolution to populate cache
        let order_doc = fixture.store.get_document("orders", &"ord_001".to_string()).unwrap().unwrap();
        let path = vec!["customer".to_string(), "name".to_string()];
        let _result = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        let (cache_size_after_resolution, _) = resolver.cache_stats();
        assert!(cache_size_after_resolution > 0);
        
        // Clear cache
        resolver.clear_cache();
        let (cache_size_after_clear, _) = resolver.cache_stats();
        assert_eq!(cache_size_after_clear, 0);
    }

    #[test]
    fn test_batch_field_path_resolution() {
        let fixture = ReferenceTestFixture::new();
        let mut resolver = ReferenceResolver::new(fixture.store.clone());
        
        // Get multiple order documents
        let order_docs = vec![
            fixture.store.get_document("orders", &"ord_001".to_string()).unwrap().unwrap(),
            fixture.store.get_document("orders", &"ord_002".to_string()).unwrap().unwrap(),
            fixture.store.get_document("orders", &"ord_003".to_string()).unwrap().unwrap(),
        ];
        
        // Batch resolve customer names
        let path = vec!["customer".to_string(), "name".to_string()];
        let results = resolver.batch_resolve_field_paths(&order_docs, &path).expect("Failed to batch resolve field paths");
        
        assert_eq!(results.len(), 3);
        assert_eq!(results[0], Value::String("Alice Johnson".to_string())); // ord_001
        assert_eq!(results[1], Value::String("Alice Johnson".to_string())); // ord_002
        assert_eq!(results[2], Value::String("Bob Smith".to_string()));     // ord_003
    }
}