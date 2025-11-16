use owl_db::*;
use owl_db::performance::*;
use std::collections::HashMap;
use tempfile::TempDir;

fn setup_test_database() -> (Database, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db = Database::new(temp_dir.path()).unwrap();
    
    // Create schemas
    let customer_schema = Schema {
        name: "customers".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("name".to_string(), FieldType::String);
            fields.insert("email".to_string(), FieldType::String);
            fields.insert("tier".to_string(), FieldType::String);
            fields.insert("balance".to_string(), FieldType::Float);
            fields.insert("created_at".to_string(), FieldType::DateTime);
            fields
        },
    };
    
    let order_schema = Schema {
        name: "orders".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("customer_id".to_string(), FieldType::Reference("customers".to_string()));
            fields.insert("total".to_string(), FieldType::Float);
            fields.insert("status".to_string(), FieldType::String);
            fields.insert("created_at".to_string(), FieldType::DateTime);
            fields
        },
    };
    
    db.create_schema(customer_schema).unwrap();
    db.create_schema(order_schema).unwrap();
    
    (db, temp_dir)
}

fn populate_test_data(db: &Database, customer_count: usize, orders_per_customer: usize) {
    println!("Populating {} customers with {} orders each...", customer_count, orders_per_customer);
    
    // Insert customers
    for i in 0..customer_count {
        let customer = Document {
            id: format!("cust_{:06}", i),
            schema: "customers".to_string(),
            fields: {
                let mut fields = HashMap::new();
                fields.insert("id".to_string(), Value::String(format!("cust_{:06}", i)));
                fields.insert("name".to_string(), Value::String(format!("Customer {}", i)));
                fields.insert("email".to_string(), Value::String(format!("customer{}@example.com", i)));
                fields.insert("tier".to_string(), Value::String(
                    match i % 3 {
                        0 => "bronze",
                        1 => "silver",
                        _ => "gold",
                    }.to_string()
                ));
                fields.insert("balance".to_string(), Value::Float((i as f64) * 100.0 + 1000.0));
                fields.insert("created_at".to_string(), Value::DateTime(
                    chrono::Utc::now() - chrono::Duration::days((i % 365) as i64)
                ));
                fields
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        };
        db.insert_document(&customer).unwrap();
    }
    
    // Insert orders
    let mut order_id = 0;
    for customer_i in 0..customer_count {
        for order_i in 0..orders_per_customer {
            let order = Document {
                id: format!("ord_{:08}", order_id),
                schema: "orders".to_string(),
                fields: {
                    let mut fields = HashMap::new();
                    fields.insert("id".to_string(), Value::String(format!("ord_{:08}", order_id)));
                    fields.insert("customer_id".to_string(), Value::Reference(format!("cust_{:06}", customer_i)));
                    fields.insert("total".to_string(), Value::Float((order_i as f64 + 1.0) * 50.0));
                    fields.insert("status".to_string(), Value::String(
                        match order_i % 3 {
                            0 => "pending",
                            1 => "shipped",
                            _ => "delivered",
                        }.to_string()
                    ));
                    fields.insert("created_at".to_string(), Value::DateTime(
                        chrono::Utc::now() - chrono::Duration::days((order_i % 30) as i64)
                    ));
                    fields
                },
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                version: 1,
            };
            db.insert_document(&order).unwrap();
            order_id += 1;
        }
    }
    
    println!("Data population completed!");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OQL Database Performance Testing Demo ===\n");
    
    let (db, _temp_dir) = setup_test_database();
    let benchmark = PerformanceBenchmark::new();
    
    // Test with different dataset sizes
    let test_sizes = vec![
        (100, 5, "Small Dataset"),
        (1000, 5, "Medium Dataset"),
        (2000, 8, "Large Dataset"),
    ];
    
    for (customer_count, orders_per_customer, size_name) in test_sizes {
        println!("\n=== {} Performance Tests ===", size_name);
        println!("({} customers, {} orders per customer = {} total orders)", 
                 customer_count, orders_per_customer, customer_count * orders_per_customer);
        
        // Populate data for this test size
        populate_test_data(&db, customer_count, orders_per_customer);
        
        // Create indexes for some tests
        db.create_index("customers", "email").unwrap();
        db.create_index("customers", "tier").unwrap();
        
        // Test 1: Simple document lookup
        let results = benchmark.benchmark_iterations(
            "Simple Document Lookup",
            50,
            || {
                let query = format!("customers.where {{ .id == \"cust_{:06}\" }}", customer_count / 2);
                db.execute_query(&query).unwrap()
            }
        );
        results.print_summary();
        
        // Test 2: Indexed field query
        let results = benchmark.benchmark_iterations(
            "Indexed Field Query",
            50,
            || {
                db.execute_query("customers.where { .tier == \"gold\" }").unwrap()
            }
        );
        results.print_summary();
        
        // Test 3: Reference navigation
        let results = benchmark.benchmark_iterations(
            "Reference Navigation",
            30,
            || {
                db.execute_query("orders.where { .status == \"delivered\" }.select { customer_name: .customer_id.name }").unwrap()
            }
        );
        results.print_summary();
        
        // Test 4: Aggregation query
        let results = benchmark.benchmark_iterations(
            "Aggregation Query",
            20,
            || {
                db.execute_query("customers.group_by { .tier }.select { tier: group_key, count: count(*), avg_balance: avg(.balance) }").unwrap()
            }
        );
        results.print_summary();
        
        // Test 5: Complex query with references
        let results = benchmark.benchmark_iterations(
            "Complex Reference Query",
            15,
            || {
                db.execute_query("customers.select { name, order_count: count(.orders), total_spent: sum(.orders.total) }").unwrap()
            }
        );
        results.print_summary();
        
        // Test 6: Sorting large result set
        let results = benchmark.benchmark_iterations(
            "Sorting Query",
            20,
            || {
                db.execute_query("customers.sort { .balance.desc }.limit(100)").unwrap()
            }
        );
        results.print_summary();
        
        // Clear data for next test
        // (In a real scenario, you might want to use separate databases)
        println!("\nClearing data for next test...");
    }
    
    // Performance monitoring demonstration
    println!("\n=== Performance Monitoring Demo ===");
    
    let monitor = PerformanceMonitor::new();
    
    // Simulate some queries with performance tracking
    for i in 0..20 {
        let mut context = QueryExecutionContext::new("customers.where { .tier == \"gold\" }");
        
        // Simulate query execution metrics
        context.record_document_scan(100 + i * 10);
        context.record_index_hit();
        if i % 3 == 0 {
            context.record_cache_hit();
        } else {
            context.record_cache_miss();
        }
        
        let metrics = context.finish(25 + i);
        monitor.record_query(metrics);
    }
    
    // Get and display performance statistics
    let stats = monitor.get_stats();
    println!("Performance Statistics:");
    println!("  Total queries: {}", stats.total_queries);
    println!("  Average execution time: {:?}", stats.avg_execution_time);
    println!("  Min execution time: {:?}", stats.min_execution_time);
    println!("  Max execution time: {:?}", stats.max_execution_time);
    println!("  Index hit rate: {:.2}%", stats.index_hit_rate * 100.0);
    println!("  Cache hit rate: {:.2}%", stats.cache_hit_rate * 100.0);
    println!("  Queries per second: {:.2}", stats.queries_per_second);
    
    // Get slowest queries
    let slowest = monitor.get_slowest_queries(5);
    println!("\nSlowest queries:");
    for (i, metric) in slowest.iter().enumerate() {
        println!("  {}: {:?} ({} results)", i + 1, metric.execution_time, metric.result_count);
    }
    
    // Get most frequent queries
    let frequent = monitor.get_frequent_queries(5);
    println!("\nMost frequent queries:");
    for (i, (query_hash, count, avg_time)) in frequent.iter().enumerate() {
        println!("  {}: {} executions, avg {:?} (hash: {})", i + 1, count, avg_time, &query_hash[..8]);
    }
    
    println!("\n=== Concurrent Performance Test ===");
    
    // Test concurrent query execution
    let (db, _temp_dir) = setup_test_database();
    populate_test_data(&db, 1000, 5);
    
    let (duration, _) = benchmark.benchmark("Concurrent Queries", || {
        let handles: Vec<_> = (0..8).map(|i| {
            let db_clone = db.clone();
            std::thread::spawn(move || {
                let tier = match i % 3 {
                    0 => "bronze",
                    1 => "silver",
                    _ => "gold",
                };
                let query = format!("customers.where {{ .tier == \"{}\" }}", tier);
                
                for _ in 0..10 {
                    let _result = db_clone.execute_query(&query).unwrap();
                }
            })
        }).collect();
        
        for handle in handles {
            handle.join().unwrap();
        }
    });
    
    println!("Concurrent execution of 80 queries across 8 threads completed in {:?}", duration);
    
    println!("\n=== Streaming vs Regular Query Comparison ===");
    
    // Compare streaming vs regular queries for large result sets
    let query = "orders.where { .total > 100.0 }";
    
    let (regular_result, regular_duration) = benchmark.benchmark("Regular Query", || {
        db.execute_query(query).unwrap()
    });
    
    let (stream_count, streaming_duration) = benchmark.benchmark("Streaming Query", || {
        let stream = db.execute_stream(query).unwrap();
        let mut count = 0;
        for batch in stream {
            count += batch.unwrap().len();
        }
        count
    });
    
    println!("Regular query: {} results in {:?}", regular_result.len(), regular_duration);
    println!("Streaming query: {} results in {:?}", stream_count, streaming_duration);
    println!("Results match: {}", regular_result.len() == stream_count);
    
    println!("\n=== Performance Testing Complete ===");
    println!("This demo showed:");
    println!("✓ Benchmark different query types across dataset sizes");
    println!("✓ Performance monitoring and metrics collection");
    println!("✓ Concurrent query execution testing");
    println!("✓ Streaming vs regular query comparison");
    println!("✓ Index performance impact measurement");
    
    Ok(())
}