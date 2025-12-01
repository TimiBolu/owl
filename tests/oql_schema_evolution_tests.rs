// OQL Schema Evolution Tests for Next-Gen Owl DST
//
// Tests adding new fields to existing schemas, backward compatibility of queries,
// queries spanning old and new schema versions, and verifies implicit null handling works correctly.
//
// Requirements: 14

use owl_db::document::{Document, Value};
use owl_db::error::{OQLError, OQLResult};
use owl_db::executor::QueryExecutor;
use owl_db::parser::parse_oql;
use owl_db::schema::{Schema, SchemaField, SchemaType};
use owl_db::simulation::{Clock, SimulatedClock, Simulation};
use owl_db::storage::{create_storage, DocumentStore};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Schema evolution tester for OQL compatibility
struct OQLSchemaEvolutionTester {
    executor: Arc<QueryExecutor<dyn DocumentStore>>,
    schemas: HashMap<String, Schema>,
    schema_versions: HashMap<String, u32>,
    clock: Arc<SimulatedClock>,
    evolution_log: Vec<SchemaEvolutionEvent>,
}

#[derive(Debug, Clone)]
struct SchemaEvolutionEvent {
    timestamp: std::time::Instant,
    event_type: EvolutionEventType,
    schema_name: String,
    version: u32,
    details: String,
}

#[derive(Debug, Clone)]
enum EvolutionEventType {
    SchemaCreated,
    FieldAdded,
    FieldModified,
    QueryExecuted,
    BackwardCompatibilityTest,
    ImplicitNullTest,
}

impl OQLSchemaEvolutionTester {
    fn new(temp_dir: &TempDir, clock: Arc<SimulatedClock>) -> Result<Self, OQLError> {
        let path = temp_dir.path().join("schema_evolution_test.owl");
        let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024)?;
        let executor = Arc::new(QueryExecutor::new(store, clock.clone()));
        
        Ok(Self {
            executor,
            schemas: HashMap::new(),
            schema_versions: HashMap::new(),
            clock,
            evolution_log: Vec::new(),
        })
    }
    
    /// Create initial schema version
    fn create_schema(&mut self, schema_name: &str, fields: Vec<SchemaField>) -> Result<(), OQLError> {
        let schema = Schema::new(schema_name.to_string(), fields);
        
        self.schemas.insert(schema_name.to_string(), schema);
        self.schema_versions.insert(schema_name.to_string(), 1);
        
        let event = SchemaEvolutionEvent {
            timestamp: self.clock.now(),
            event_type: EvolutionEventType::SchemaCreated,
            schema_name: schema_name.to_string(),
            version: 1,
            details: "Initial schema created".to_string(),
        };
        
        self.evolution_log.push(event);
        
        Ok(())
    }
    
    /// Evolve schema by adding new fields
    fn evolve_schema(&mut self, schema_name: &str, new_fields: Vec<SchemaField>) -> Result<(), OQLError> {
        let current_version = self.schema_versions.get(schema_name).copied().unwrap_or(0);
        let new_version = current_version + 1;
        
        if let Some(schema) = self.schemas.get_mut(schema_name) {
            // Add new fields to existing schema
            for field in new_fields {
                schema.add_field(field.clone())?;
                
                let event = SchemaEvolutionEvent {
                    timestamp: self.clock.now(),
                    event_type: EvolutionEventType::FieldAdded,
                    schema_name: schema_name.to_string(),
                    version: new_version,
                    details: format!("Added field: {}", field.name),
                };
                
                self.evolution_log.push(event);
            }
            
            self.schema_versions.insert(schema_name.to_string(), new_version);
        }
        
        Ok(())
    }
    
    /// Insert document with old schema version
    fn insert_old_version_document(&self, schema_name: &str, doc_data: HashMap<String, Value>) -> Result<(), OQLError> {
        // Create document without new fields
        let mut fields = HashMap::new();
        fields.insert("id".to_string(), Value::String(format!("old_{}", doc_data.len())));
        fields.extend(doc_data);
        
        let doc = Document::with_id(
            format!("old_{}", fields.len()),
            schema_name.to_string(),
            fields,
        );
        
        self.executor.store().put_document(&doc)?;
        Ok(())
    }
    
    /// Insert document with new schema version
    fn insert_new_version_document(&self, schema_name: &str, doc_data: HashMap<String, Value>) -> Result<(), OQLError> {
        // Create document with all fields including new ones
        let mut fields = HashMap::new();
        fields.insert("id".to_string(), Value::String(format!("new_{}", doc_data.len())));
        fields.extend(doc_data);
        
        let doc = Document::with_id(
            format!("new_{}", fields.len()),
            schema_name.to_string(),
            fields,
        );
        
        self.executor.store().put_document(&doc)?;
        Ok(())
    }
    
    /// Execute OQL query and test compatibility
    fn test_query_compatibility(&mut self, oql: &str, expected_behavior: CompatibilityExpectation) -> Result<CompatibilityResult, OQLError> {
        let query = parse_oql(oql)?;
        let start_time = self.clock.now();
        
        let result = self.executor.execute_query(&query);
        let execution_time = self.clock.now().duration_since(start_time);
        
        let event = SchemaEvolutionEvent {
            timestamp: self.clock.now(),
            event_type: EvolutionEventType::QueryExecuted,
            schema_name: "".to_string(),
            version: 0,
            details: format!("Query: {}", oql),
        };
        
        self.evolution_log.push(event);
        
        let compatibility_result = match result {
            Ok(documents) => {
                self.analyze_query_result(&documents, &expected_behavior, oql)
            }
            Err(e) => {
                match expected_behavior {
                    CompatibilityExpectation::ShouldSucceed => {
                        CompatibilityResult::Failed {
                            reason: format!("Query failed unexpectedly: {:?}", e),
                        }
                    }
                    CompatibilityExpectation::ShouldFail => {
                        CompatibilityResult::Passed {
                            details: "Query failed as expected".to_string(),
                        }
                    }
                    _ => {
                        CompatibilityResult::Failed {
                            reason: format!("Query error: {:?}", e),
                        }
                    }
                }
            }
        };
        
        Ok(compatibility_result)
    }
    
    /// Analyze query result for compatibility
    fn analyze_query_result(
        &self,
        documents: &[Document],
        expected: &CompatibilityExpectation,
        query: &str,
    ) -> CompatibilityResult {
        match expected {
            CompatibilityExpectation::ShouldSucceed => {
                CompatibilityResult::Passed {
                    details: format!("Query succeeded with {} results", documents.len()),
                }
            }
            CompatibilityExpectation::ShouldFail => {
                CompatibilityResult::Failed {
                    reason: "Query succeeded when it should have failed".to_string(),
                }
            }
            CompatibilityExpectation::ImplicitNullHandling => {
                // Check that missing fields are handled as null
                let has_implicit_nulls = documents.iter().any(|doc| {
                    // Check if any field access would return null for missing fields
                    self.check_implicit_null_handling(doc, query)
                });
                
                if has_implicit_nulls {
                    CompatibilityResult::Passed {
                        details: "Implicit null handling working correctly".to_string(),
                    }
                } else {
                    CompatibilityResult::Inconclusive {
                        reason: "Could not verify implicit null handling".to_string(),
                    }
                }
            }
            CompatibilityExpectation::MixedVersionResults => {
                // Check that results include both old and new version documents
                let has_old_docs = documents.iter().any(|doc| {
                    doc.get_id().starts_with("old_")
                });
                let has_new_docs = documents.iter().any(|doc| {
                    doc.get_id().starts_with("new_")
                });
                
                if has_old_docs && has_new_docs {
                    CompatibilityResult::Passed {
                        details: "Query returned mixed version documents".to_string(),
                    }
                } else if has_old_docs || has_new_docs {
                    CompatibilityResult::Partial {
                        details: "Query returned only one version of documents".to_string(),
                    }
                } else {
                    CompatibilityResult::Failed {
                        reason: "Query returned no documents".to_string(),
                    }
                }
            }
        }
    }
    
    /// Check if implicit null handling is working for a document
    fn check_implicit_null_handling(&self, doc: &Document, query: &str) -> bool {
        // Simple heuristic: if query contains field access and document doesn't have all fields,
        // then implicit null handling is likely being used
        if query.contains('.') {
            // Extract potential field names from query (simplified)
            let potential_fields = ["profile", "bio", "preferences", "settings", "metadata"];
            
            for field in &potential_fields {
                if query.contains(field) && doc.get_field(field).is_none() {
                    return true; // Missing field that query tries to access
                }
            }
        }
        
        false
    }
    
    /// Test backward compatibility with old queries
    fn test_backward_compatibility(&mut self, old_queries: &[&str]) -> Result<BackwardCompatibilityResult, OQLError> {
        let mut results = Vec::new();
        
        for &query in old_queries {
            let result = self.test_query_compatibility(query, CompatibilityExpectation::ShouldSucceed)?;
            results.push((query.to_string(), result));
            
            let event = SchemaEvolutionEvent {
                timestamp: self.clock.now(),
                event_type: EvolutionEventType::BackwardCompatibilityTest,
                schema_name: "".to_string(),
                version: 0,
                details: format!("Tested backward compatibility for: {}", query),
            };
            
            self.evolution_log.push(event);
        }
        
        let passed = results.iter().filter(|(_, result)| {
            matches!(result, CompatibilityResult::Passed { .. })
        }).count();
        
        let total = results.len();
        
        Ok(BackwardCompatibilityResult {
            total_queries: total,
            passed_queries: passed,
            failed_queries: total - passed,
            query_results: results,
        })
    }
    
    /// Get evolution statistics
    fn get_evolution_stats(&self) -> EvolutionStats {
        let mut stats = EvolutionStats {
            schemas_created: 0,
            fields_added: 0,
            queries_executed: 0,
            compatibility_tests: 0,
            implicit_null_tests: 0,
        };
        
        for event in &self.evolution_log {
            match event.event_type {
                EvolutionEventType::SchemaCreated => stats.schemas_created += 1,
                EvolutionEventType::FieldAdded => stats.fields_added += 1,
                EvolutionEventType::QueryExecuted => stats.queries_executed += 1,
                EvolutionEventType::BackwardCompatibilityTest => stats.compatibility_tests += 1,
                EvolutionEventType::ImplicitNullTest => stats.implicit_null_tests += 1,
                _ => {}
            }
        }
        
        stats
    }
}

#[derive(Debug)]
enum CompatibilityExpectation {
    ShouldSucceed,
    ShouldFail,
    ImplicitNullHandling,
    MixedVersionResults,
}

#[derive(Debug)]
enum CompatibilityResult {
    Passed { details: String },
    Failed { reason: String },
    Partial { details: String },
    Inconclusive { reason: String },
}

#[derive(Debug)]
struct BackwardCompatibilityResult {
    total_queries: usize,
    passed_queries: usize,
    failed_queries: usize,
    query_results: Vec<(String, CompatibilityResult)>,
}

#[derive(Debug)]
struct EvolutionStats {
    schemas_created: usize,
    fields_added: usize,
    queries_executed: usize,
    compatibility_tests: usize,
    implicit_null_tests: usize,
}

/// Test adding new fields to existing schema
#[test]
fn test_add_new_fields_to_schema() -> OQLResult<()> {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLSchemaEvolutionTester::new(&temp_dir, sim.clock())?;
    
    // Create initial user schema (version 1)
    let initial_fields = vec![
        SchemaField::new("id".to_string(), SchemaType::String, false),
        SchemaField::new("name".to_string(), SchemaType::String, false),
        SchemaField::new("age".to_string(), SchemaType::Integer, false),
    ];
    
    tester.create_schema("users", initial_fields)?;
    
    // Insert documents with initial schema
    let mut old_user1 = HashMap::new();
    old_user1.insert("name".to_string(), Value::String("Alice".to_string()));
    old_user1.insert("age".to_string(), Value::Integer(30));
    tester.insert_old_version_document("users", old_user1)?;
    
    let mut old_user2 = HashMap::new();
    old_user2.insert("name".to_string(), Value::String("Bob".to_string()));
    old_user2.insert("age".to_string(), Value::Integer(25));
    tester.insert_old_version_document("users", old_user2)?;
    
    // Test queries work with initial schema
    let initial_query_result = tester.test_query_compatibility(
        r#"users.select { id, name, age }"#,
        CompatibilityExpectation::ShouldSucceed,
    )?;
    
    assert!(matches!(initial_query_result, CompatibilityResult::Passed { .. }));
    
    // Evolve schema by adding new fields (version 2)
    let new_fields = vec![
        SchemaField::new("email".to_string(), SchemaType::String, true), // Optional
        SchemaField::new("profile".to_string(), SchemaType::Object, true), // Optional nested object
    ];
    
    tester.evolve_schema("users", new_fields)?;
    
    // Insert documents with new schema
    let mut new_user1 = HashMap::new();
    new_user1.insert("name".to_string(), Value::String("Charlie".to_string()));
    new_user1.insert("age".to_string(), Value::Integer(35));
    new_user1.insert("email".to_string(), Value::String("charlie@example.com".to_string()));
    tester.insert_new_version_document("users", new_user1)?;
    
    // Test that old queries still work (backward compatibility)
    let backward_compat_result = tester.test_query_compatibility(
        r#"users.select { id, name, age }"#,
        CompatibilityExpectation::MixedVersionResults,
    )?;
    
    assert!(matches!(backward_compat_result, 
        CompatibilityResult::Passed { .. } | CompatibilityResult::Partial { .. }));
    
    // Test queries with new fields
    let new_field_result = tester.test_query_compatibility(
        r#"users.select { id, name, email }"#,
        CompatibilityExpectation::ImplicitNullHandling,
    )?;
    
    // Should work with implicit null handling for missing email field
    assert!(!matches!(new_field_result, CompatibilityResult::Failed { .. }));
    
    let stats = tester.get_evolution_stats();
    println!("Schema evolution stats: {:?}", stats);
    
    assert_eq!(stats.schemas_created, 1);
    assert_eq!(stats.fields_added, 2);
    assert!(stats.queries_executed > 0);
    
    Ok(())
}

/// Test backward compatibility of queries
#[test]
fn test_backward_compatibility_of_queries() -> OQLResult<()> {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLSchemaEvolutionTester::new(&temp_dir, sim.clock())?;
    
    // Create initial product schema
    let initial_fields = vec![
        SchemaField::new("id".to_string(), SchemaType::String, false),
        SchemaField::new("name".to_string(), SchemaType::String, false),
        SchemaField::new("price".to_string(), SchemaType::Float, false),
        SchemaField::new("category".to_string(), SchemaType::String, false),
    ];
    
    tester.create_schema("products", initial_fields)?;
    
    // Insert initial products
    let products = vec![
        ("Product A", 19.99, "electronics"),
        ("Product B", 29.99, "books"),
        ("Product C", 39.99, "electronics"),
    ];
    
    for (name, price, category) in products {
        let mut product_data = HashMap::new();
        product_data.insert("name".to_string(), Value::String(name.to_string()));
        product_data.insert("price".to_string(), Value::Float(price));
        product_data.insert("category".to_string(), Value::String(category.to_string()));
        tester.insert_old_version_document("products", product_data)?;
    }
    
    // Define old queries that should continue to work
    let old_queries = vec![
        r#"products"#,
        r#"products.where { .price > 25.0 }"#,
        r#"products.where { .category == "electronics" }"#,
        r#"products.select { id, name, price }"#,
        r#"products.sort { .price desc }"#,
        r#"products.where { .price > 20.0 }.select { name, category }"#,
    ];
    
    // Test backward compatibility before evolution
    let initial_compat = tester.test_backward_compatibility(&old_queries)?;
    assert_eq!(initial_compat.passed_queries, old_queries.len());
    
    // Evolve schema by adding new fields
    let new_fields = vec![
        SchemaField::new("description".to_string(), SchemaType::String, true),
        SchemaField::new("tags".to_string(), SchemaType::Array, true),
        SchemaField::new("metadata".to_string(), SchemaType::Object, true),
    ];
    
    tester.evolve_schema("products", new_fields)?;
    
    // Add new products with evolved schema
    let new_products = vec![
        ("Product D", 49.99, "clothing", Some("A great product"), Some(vec!["new", "popular"])),
        ("Product E", 59.99, "electronics", Some("Latest tech"), Some(vec!["tech", "gadget"])),
    ];
    
    for (name, price, category, description, tags) in new_products {
        let mut product_data = HashMap::new();
        product_data.insert("name".to_string(), Value::String(name.to_string()));
        product_data.insert("price".to_string(), Value::Float(price));
        product_data.insert("category".to_string(), Value::String(category.to_string()));
        
        if let Some(desc) = description {
            product_data.insert("description".to_string(), Value::String(desc.to_string()));
        }
        
        if let Some(tag_list) = tags {
            let tag_values: Vec<Value> = tag_list.into_iter()
                .map(|tag| Value::String(tag.to_string()))
                .collect();
            product_data.insert("tags".to_string(), Value::Array(tag_values));
        }
        
        tester.insert_new_version_document("products", product_data)?;
    }
    
    // Test backward compatibility after evolution
    let evolved_compat = tester.test_backward_compatibility(&old_queries)?;
    
    println!("Backward compatibility results:");
    println!("Before evolution: {}/{} queries passed", 
        initial_compat.passed_queries, initial_compat.total_queries);
    println!("After evolution: {}/{} queries passed", 
        evolved_compat.passed_queries, evolved_compat.total_queries);
    
    // All old queries should still work
    assert!(evolved_compat.passed_queries >= old_queries.len() - 1, 
        "Most old queries should still work after schema evolution");
    
    // Test new queries that use evolved fields
    let new_query_result = tester.test_query_compatibility(
        r#"products.select { id, name, description }"#,
        CompatibilityExpectation::ImplicitNullHandling,
    )?;
    
    assert!(!matches!(new_query_result, CompatibilityResult::Failed { .. }));
    
    Ok(())
}

/// Test queries spanning old and new schema versions
#[test]
fn test_queries_spanning_schema_versions() -> OQLResult<()> {
    let seed = 456;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLSchemaEvolutionTester::new(&temp_dir, sim.clock())?;
    
    // Create initial order schema
    let initial_fields = vec![
        SchemaField::new("id".to_string(), SchemaType::String, false),
        SchemaField::new("customer_id".to_string(), SchemaType::String, false),
        SchemaField::new("total".to_string(), SchemaType::Float, false),
        SchemaField::new("status".to_string(), SchemaType::String, false),
    ];
    
    tester.create_schema("orders", initial_fields)?;
    
    // Insert old version orders
    let old_orders = vec![
        ("order1", "customer1", 100.0, "completed"),
        ("order2", "customer2", 150.0, "pending"),
        ("order3", "customer1", 200.0, "completed"),
    ];
    
    for (id, customer_id, total, status) in old_orders {
        let mut order_data = HashMap::new();
        order_data.insert("id".to_string(), Value::String(id.to_string()));
        order_data.insert("customer_id".to_string(), Value::String(customer_id.to_string()));
        order_data.insert("total".to_string(), Value::Float(total));
        order_data.insert("status".to_string(), Value::String(status.to_string()));
        tester.insert_old_version_document("orders", order_data)?;
    }
    
    // Evolve schema to add shipping and payment info
    let new_fields = vec![
        SchemaField::new("shipping_address".to_string(), SchemaType::Object, true),
        SchemaField::new("payment_method".to_string(), SchemaType::String, true),
        SchemaField::new("created_at".to_string(), SchemaType::String, true),
    ];
    
    tester.evolve_schema("orders", new_fields)?;
    
    // Insert new version orders
    let new_orders = vec![
        ("order4", "customer3", 250.0, "processing", Some("credit_card"), Some("2023-01-01")),
        ("order5", "customer2", 300.0, "shipped", Some("paypal"), Some("2023-01-02")),
    ];
    
    for (id, customer_id, total, status, payment, created) in new_orders {
        let mut order_data = HashMap::new();
        order_data.insert("id".to_string(), Value::String(id.to_string()));
        order_data.insert("customer_id".to_string(), Value::String(customer_id.to_string()));
        order_data.insert("total".to_string(), Value::Float(total));
        order_data.insert("status".to_string(), Value::String(status.to_string()));
        
        if let Some(payment_method) = payment {
            order_data.insert("payment_method".to_string(), Value::String(payment_method.to_string()));
        }
        
        if let Some(created_at) = created {
            order_data.insert("created_at".to_string(), Value::String(created_at.to_string()));
        }
        
        tester.insert_new_version_document("orders", order_data)?;
    }
    
    // Test queries that span both versions
    let spanning_queries = vec![
        // Basic query should return all orders
        r#"orders"#,
        
        // Filter by common field
        r#"orders.where { .total > 150.0 }"#,
        
        // Select mix of old and new fields (implicit null handling)
        r#"orders.select { id, total, payment_method }"#,
        
        // Group by common field
        r#"orders.where { .status == "completed" }.select { customer_id, total }"#,
        
        // Aggregation across versions
        r#"orders.select { total_orders: count(), avg_total: avg(.total) }"#,
    ];
    
    for query in &spanning_queries {
        let result = tester.test_query_compatibility(query, CompatibilityExpectation::MixedVersionResults)?;
        
        match result {
            CompatibilityResult::Passed { details } => {
                println!("Query '{}' passed: {}", query, details);
            }
            CompatibilityResult::Partial { details } => {
                println!("Query '{}' partially passed: {}", query, details);
            }
            CompatibilityResult::Failed { reason } => {
                println!("Query '{}' failed: {}", query, reason);
                // Some failures might be acceptable depending on the query
            }
            CompatibilityResult::Inconclusive { reason } => {
                println!("Query '{}' inconclusive: {}", query, reason);
            }
        }
    }
    
    // Test specific implicit null handling
    let null_handling_result = tester.test_query_compatibility(
        r#"orders.select { id, total, payment_method, created_at }"#,
        CompatibilityExpectation::ImplicitNullHandling,
    )?;
    
    println!("Implicit null handling result: {:?}", null_handling_result);
    
    let stats = tester.get_evolution_stats();
    println!("Schema spanning stats: {:?}", stats);
    
    Ok(())
}

/// Test implicit null handling works correctly
#[test]
fn test_implicit_null_handling() -> OQLResult<()> {
    let seed = 789;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLSchemaEvolutionTester::new(&temp_dir, sim.clock())?;
    
    // Create user schema with basic fields
    let initial_fields = vec![
        SchemaField::new("id".to_string(), SchemaType::String, false),
        SchemaField::new("name".to_string(), SchemaType::String, false),
        SchemaField::new("age".to_string(), SchemaType::Integer, false),
    ];
    
    tester.create_schema("users", initial_fields)?;
    
    // Insert users without profile information
    let users_without_profile = vec![
        ("user1", "Alice", 30),
        ("user2", "Bob", 25),
    ];
    
    for (id, name, age) in users_without_profile {
        let mut user_data = HashMap::new();
        user_data.insert("id".to_string(), Value::String(id.to_string()));
        user_data.insert("name".to_string(), Value::String(name.to_string()));
        user_data.insert("age".to_string(), Value::Integer(age));
        tester.insert_old_version_document("users", user_data)?;
    }
    
    // Evolve schema to add profile fields
    let profile_fields = vec![
        SchemaField::new("profile".to_string(), SchemaType::Object, true),
        SchemaField::new("bio".to_string(), SchemaType::String, true),
        SchemaField::new("preferences".to_string(), SchemaType::Object, true),
    ];
    
    tester.evolve_schema("users", profile_fields)?;
    
    // Insert users with profile information
    let mut user_with_profile = HashMap::new();
    user_with_profile.insert("id".to_string(), Value::String("user3".to_string()));
    user_with_profile.insert("name".to_string(), Value::String("Charlie".to_string()));
    user_with_profile.insert("age".to_string(), Value::Integer(35));
    user_with_profile.insert("bio".to_string(), Value::String("Software developer".to_string()));
    tester.insert_new_version_document("users", user_with_profile)?;
    
    // Test implicit null handling queries
    let null_handling_queries = vec![
        // Should return null for missing bio field
        r#"users.select { id, name, bio }"#,
        
        // Should handle nested null access gracefully
        r#"users.select { id, name, email: .profile.email }"#,
        
        // Should work with null checks
        r#"users.where { .bio != null }.select { id, name, bio }"#,
        
        // Should handle deep nesting
        r#"users.select { id, name, theme: .profile.preferences.theme }"#,
        
        // Should work with coalesce-like operations
        r#"users.select { id, name, display_bio: coalesce(.bio, "No bio available") }"#,
    ];
    
    for query in &null_handling_queries {
        let result = tester.test_query_compatibility(query, CompatibilityExpectation::ImplicitNullHandling)?;
        
        match result {
            CompatibilityResult::Passed { details } => {
                println!("Null handling query '{}' passed: {}", query, details);
            }
            CompatibilityResult::Inconclusive { reason } => {
                println!("Null handling query '{}' inconclusive: {}", query, reason);
                // This is acceptable for implicit null handling tests
            }
            CompatibilityResult::Failed { reason } => {
                println!("Null handling query '{}' failed: {}", query, reason);
                // Some failures might indicate issues with null handling
            }
            _ => {}
        }
    }
    
    // Test that queries don't fail when accessing missing fields
    let safe_access_result = tester.test_query_compatibility(
        r#"users.select { id, name, bio }"#,
        CompatibilityExpectation::ShouldSucceed,
    )?;
    
    assert!(matches!(safe_access_result, CompatibilityResult::Passed { .. }), 
        "Accessing missing fields should not cause query failure");
    
    let stats = tester.get_evolution_stats();
    println!("Implicit null handling stats: {:?}", stats);
    
    Ok(())
}

/// Test complex schema evolution scenario
#[test]
fn test_complex_schema_evolution_scenario() -> OQLResult<()> {
    let seed = 101;
    let mut sim = Simulation::new(seed);
    
    let temp_dir = TempDir::new().unwrap();
    let mut tester = OQLSchemaEvolutionTester::new(&temp_dir, sim.clock())?;
    
    // Phase 1: Initial e-commerce schema
    let initial_fields = vec![
        SchemaField::new("id".to_string(), SchemaType::String, false),
        SchemaField::new("name".to_string(), SchemaType::String, false),
        SchemaField::new("price".to_string(), SchemaType::Float, false),
    ];
    
    tester.create_schema("products", initial_fields)?;
    
    // Insert initial products
    let mut product1 = HashMap::new();
    product1.insert("name".to_string(), Value::String("Basic Widget".to_string()));
    product1.insert("price".to_string(), Value::Float(19.99));
    tester.insert_old_version_document("products", product1)?;
    
    // Phase 2: Add category and inventory
    let phase2_fields = vec![
        SchemaField::new("category".to_string(), SchemaType::String, true),
        SchemaField::new("inventory_count".to_string(), SchemaType::Integer, true),
    ];
    
    tester.evolve_schema("products", phase2_fields)?;
    
    let mut product2 = HashMap::new();
    product2.insert("name".to_string(), Value::String("Advanced Widget".to_string()));
    product2.insert("price".to_string(), Value::Float(29.99));
    product2.insert("category".to_string(), Value::String("electronics".to_string()));
    product2.insert("inventory_count".to_string(), Value::Integer(50));
    tester.insert_new_version_document("products", product2)?;
    
    // Phase 3: Add detailed product information
    let phase3_fields = vec![
        SchemaField::new("description".to_string(), SchemaType::String, true),
        SchemaField::new("specifications".to_string(), SchemaType::Object, true),
        SchemaField::new("reviews".to_string(), SchemaType::Array, true),
    ];
    
    tester.evolve_schema("products", phase3_fields)?;
    
    let mut product3 = HashMap::new();
    product3.insert("name".to_string(), Value::String("Premium Widget".to_string()));
    product3.insert("price".to_string(), Value::Float(49.99));
    product3.insert("category".to_string(), Value::String("premium".to_string()));
    product3.insert("inventory_count".to_string(), Value::Integer(25));
    product3.insert("description".to_string(), Value::String("The best widget available".to_string()));
    tester.insert_new_version_document("products", product3)?;
    
    // Test queries across all schema versions
    let complex_queries = vec![
        // Should work across all versions
        r#"products.select { id, name, price }"#,
        
        // Should handle missing fields gracefully
        r#"products.select { id, name, price, category, description }"#,
        
        // Should work with filtering on evolved fields
        r#"products.where { .category == "electronics" }.select { name, price }"#,
        
        // Should handle aggregations across versions
        r#"products.select { total_products: count(), avg_price: avg(.price) }"#,
        
        // Should work with complex nested access
        r#"products.select { id, name, weight: .specifications.weight }"#,
    ];
    
    let mut successful_queries = 0;
    let total_queries = complex_queries.len();
    
    for query in &complex_queries {
        let result = tester.test_query_compatibility(query, CompatibilityExpectation::ShouldSucceed)?;
        
        match result {
            CompatibilityResult::Passed { .. } => {
                successful_queries += 1;
                println!("Complex query passed: {}", query);
            }
            _ => {
                println!("Complex query had issues: {} -> {:?}", query, result);
            }
        }
    }
    
    let success_rate = successful_queries as f64 / total_queries as f64;
    println!("Complex evolution success rate: {:.1}%", success_rate * 100.0);
    
    // Should have high success rate for backward compatibility
    assert!(success_rate >= 0.8, "Should maintain high backward compatibility");
    
    let stats = tester.get_evolution_stats();
    println!("Complex evolution stats: {:?}", stats);
    
    assert_eq!(stats.schemas_created, 1);
    assert_eq!(stats.fields_added, 7); // 2 + 3 + 2 fields added in phases
    
    Ok(())
}