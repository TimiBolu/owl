use owl_db::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tempfile::TempDir;

mod performance_test_utils;
use performance_test_utils::{TestDatabase, setup_test_db};

fn populate_large_dataset(db: &TestDatabase, customer_count: usize, orders_per_customer: usize, product_count: usize) {
    println!("Populating dataset: {} customers, {} orders per customer, {} products", 
             customer_count, orders_per_customer, product_count);
    
    let start_time = Instant::now();
    
    // Populate customers
    for i in 0..customer_count {
        let customer = Document {
            id: format!("cust_{:08}", i),
            schema: "customers".to_string(),
            fields: {
                let mut fields = HashMap::new();
                fields.insert("id".to_string(), Value::String(format!("cust_{:08}", i)));
                fields.insert("name".to_string(), Value::String(format!("Customer {}", i)));
                fields.insert("email".to_string(), Value::String(format!("customer{}@example.com", i)));
                fields.insert("tier".to_string(), Value::String(
                    match i % 4 {
                        0 => "bronze",
                        1 => "silver", 
                        2 => "gold",
                        _ => "platinum",
                    }.to_string()
                ));
                fields.insert("created_at".to_string(), Value::DateTime(
                    chrono::Utc::now() - chrono::Duration::days((i % 1000) as i64)
                ));
                fields.insert("balance".to_string(), Value::Float((i as f64) * 50.0 + 1000.0));
                fields.insert("age".to_string(), Value::Integer(25 + (i % 50) as i64));
                fields
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        };
        db.insert_document(&customer).unwrap();
        
        if i % 1000 == 0 && i > 0 {
            println!("  Inserted {} customers", i);
        }
    }
    
    // Populate products
    for i in 0..product_count {
        let product = Document {
            id: format!("prod_{:06}", i),
            schema: "products".to_string(),
            fields: {
                let mut fields = HashMap::new();
                fields.insert("id".to_string(), Value::String(format!("prod_{:06}", i)));
                fields.insert("name".to_string(), Value::String(format!("Product {}", i)));
                fields.insert("price".to_string(), Value::Float((i as f64) * 10.0 + 50.0));
                fields.insert("category".to_string(), Value::String(
                    match i % 5 {
                        0 => "electronics",
                        1 => "clothing",
                        2 => "books",
                        3 => "home",
                        _ => "sports",
                    }.to_string()
                ));
                fields.insert("in_stock".to_string(), Value::Boolean(i % 10 != 0));
                fields
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        };
        db.insert_document(&product).unwrap();
    }
    
    // Populate orders
    let mut order_id = 0;
    for customer_i in 0..customer_count {
        for order_i in 0..orders_per_customer {
            let order = Document {
                id: format!("ord_{:010}", order_id),
                schema: "orders".to_string(),
                fields: {
                    let mut fields = HashMap::new();
                    fields.insert("id".to_string(), Value::String(format!("ord_{:010}", order_id)));
                    fields.insert("customer_id".to_string(), Value::Reference(format!("cust_{:08}", customer_i)));
                    fields.insert("total".to_string(), Value::Float((order_i as f64 + 1.0) * 75.0 + 25.0));
                    fields.insert("status".to_string(), Value::String(
                        match order_i % 5 {
                            0 => "pending",
                            1 => "processing",
                            2 => "shipped", 
                            3 => "delivered",
                            _ => "cancelled",
                        }.to_string()
                    ));
                    fields.insert("created_at".to_string(), Value::DateTime(
                        chrono::Utc::now() - chrono::Duration::days((order_i % 90) as i64)
                    ));
                    fields.insert("item_count".to_string(), Value::Integer((order_i % 10 + 1) as i64));
                    fields
                },
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                version: 1,
            };
            db.insert_document(&order).unwrap();
            order_id += 1;
        }
        
        if customer_i % 500 == 0 && customer_i > 0 {
            println!("  Inserted orders for {} customers", customer_i);
        }
    }
    
    let duration = start_time.elapsed();
    println!("Dataset population completed in {:?}", duration);
}

#[test]
fn test_scalability_small_dataset() {
    println!("\n=== Small Dataset Scalability Test (1K customers, 5K orders) ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 1000, 5, 500);
    
    // Test various query patterns
    let queries = vec![
        ("Simple filter", "customers.where { .tier == \"gold\" }"),
        ("Range query", "customers.where { .balance > 5000.0 }"),
        ("Reference navigation", "orders.where { .status == \"delivered\" }.select { customer_name: .customer_id.name }"),
        ("Aggregation", "customers.group_by { .tier }.select { tier: group_key, count: count(*), avg_balance: avg(.balance) }"),
        ("Complex aggregation", "customers.select { name, order_count: count(.orders), total_spent: sum(.orders.total) }"),
        ("Sorting", "customers.sort { .balance.desc }.limit(100)"),
    ];
    
    for (name, query) in queries {
        let start_time = Instant::now();
        let result = db.execute_query(query).unwrap();
        let duration = start_time.elapsed();
        
        println!("  {}: {} results in {:?}", name, result.len(), duration);
        assert!(duration < Duration::from_millis(500), "{} took too long: {:?}", name, duration);
    }
}

#[test]
fn test_scalability_medium_dataset() {
    println!("\n=== Medium Dataset Scalability Test (5K customers, 25K orders) ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 5000, 5, 1000);
    
    // Create indexes for better performance
    db.create_index("customers", "email").unwrap();
    db.create_index("customers", "tier").unwrap();
    db.create_index("orders", "status").unwrap();
    
    let queries = vec![
        ("Indexed lookup", "customers.where { .tier == \"platinum\" }"),
        ("Email lookup", "customers.where { .email == \"customer2500@example.com\" }"),
        ("Complex filter", "orders.where { .total > 200.0 && .status == \"delivered\" }"),
        ("Reference with aggregation", "customers.where { .tier == \"gold\" }.select { name, recent_orders: count(.orders where .created_at >= 30.days.ago) }"),
        ("Multi-level aggregation", "customers.group_by { .tier }.select { tier: group_key, customers: count(*), total_orders: sum(count(.orders)), avg_order_value: avg(avg(.orders.total)) }"),
        ("Large result sorting", "orders.sort { .total.desc }.limit(1000)"),
    ];
    
    for (name, query) in queries {
        let start_time = Instant::now();
        let result = db.execute_query(query).unwrap();
        let duration = start_time.elapsed();
        
        println!("  {}: {} results in {:?}", name, result.len(), duration);
        assert!(duration < Duration::from_secs(2), "{} took too long: {:?}", name, duration);
    }
}

#[test]
fn test_scalability_large_dataset() {
    println!("\n=== Large Dataset Scalability Test (10K customers, 50K orders) ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 10000, 5, 2000);
    
    // Create comprehensive indexes
    db.create_index("customers", "email").unwrap();
    db.create_index("customers", "tier").unwrap();
    db.create_index("customers", "balance").unwrap();
    db.create_index("orders", "status").unwrap();
    db.create_index("orders", "total").unwrap();
    db.create_index("products", "category").unwrap();
    
    let queries = vec![
        ("Selective filter", "customers.where { .balance > 10000.0 }"),
        ("Multi-condition filter", "orders.where { .total > 300.0 && .status == \"delivered\" && .item_count > 5 }"),
        ("Complex reference query", "customers.where { .tier == \"platinum\" }.select { name, email, high_value_orders: count(.orders where .total > 500.0) }"),
        ("Cross-schema aggregation", "orders.group_by { .status }.select { status: group_key, order_count: count(*), total_revenue: sum(.total), avg_items: avg(.item_count) }"),
        ("Top customers analysis", "customers.select { name, tier, lifetime_value: sum(.orders.total), order_frequency: count(.orders) }.sort { lifetime_value.desc }.limit(50)"),
    ];
    
    for (name, query) in queries {
        let start_time = Instant::now();
        let result = db.execute_query(query).unwrap();
        let duration = start_time.elapsed();
        
        println!("  {}: {} results in {:?}", name, result.len(), duration);
        assert!(duration < Duration::from_secs(5), "{} took too long: {:?}", name, duration);
    }
}

#[test]
fn test_streaming_performance_large_dataset() {
    println!("\n=== Streaming Performance Test ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 5000, 10, 1000);
    
    // Test streaming vs regular query for large result sets
    let query = "orders.where { .total > 100.0 }";
    
    // Regular query
    let start_time = Instant::now();
    let regular_result = db.execute_query(query).unwrap();
    let regular_duration = start_time.elapsed();
    
    // Streaming query
    let start_time = Instant::now();
    let stream = db.execute_stream(query).unwrap();
    let mut streamed_count = 0;
    for batch in stream {
        streamed_count += batch.unwrap().len();
    }
    let streaming_duration = start_time.elapsed();
    
    println!("  Regular query: {} results in {:?}", regular_result.len(), regular_duration);
    println!("  Streaming query: {} results in {:?}", streamed_count, streaming_duration);
    
    assert_eq!(regular_result.len(), streamed_count, "Result counts should match");
    
    // For large datasets, streaming should be more memory efficient
    // (we can't easily test memory usage, but we can test that it completes)
    assert!(streaming_duration < Duration::from_secs(10), "Streaming took too long");
}

#[test]
fn test_concurrent_performance() {
    println!("\n=== Concurrent Performance Test ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 2000, 5, 500);
    db.create_index("customers", "tier").unwrap();
    
    // Test concurrent reads
    let start_time = Instant::now();
    let handles: Vec<_> = (0..8).map(|i| {
        let db_clone = db.clone();
        std::thread::spawn(move || {
            let tier = match i % 4 {
                0 => "bronze",
                1 => "silver", 
                2 => "gold",
                _ => "platinum",
            };
            let query = format!("customers.where {{ .tier == \"{}\" }}", tier);
            
            let mut total_results = 0;
            for _ in 0..10 {
                let result = db_clone.execute_query(&query).unwrap();
                total_results += result.len();
            }
            total_results
        })
    }).collect();
    
    let mut total_processed = 0;
    for handle in handles {
        total_processed += handle.join().unwrap();
    }
    let concurrent_duration = start_time.elapsed();
    
    println!("  Concurrent reads: {} total results processed in {:?}", total_processed, concurrent_duration);
    assert!(concurrent_duration < Duration::from_secs(10), "Concurrent reads took too long");
    
    // Test mixed read/write workload
    let start_time = Instant::now();
    let handles: Vec<_> = (0..4).map(|i| {
        let db_clone = db.clone();
        std::thread::spawn(move || {
            if i % 2 == 0 {
                // Reader thread
                let query = "customers.where { .balance > 5000.0 }";
                for _ in 0..5 {
                    let _result = db_clone.execute_query(query).unwrap();
                }
            } else {
                // Writer thread
                for j in 0..5 {
                    let customer = Document {
                        id: format!("temp_cust_{}_{}", i, j),
                        schema: "customers".to_string(),
                        fields: {
                            let mut fields = HashMap::new();
                            fields.insert("id".to_string(), Value::String(format!("temp_cust_{}_{}", i, j)));
                            fields.insert("name".to_string(), Value::String(format!("Temp Customer {} {}", i, j)));
                            fields.insert("email".to_string(), Value::String(format!("temp{}{}@example.com", i, j)));
                            fields.insert("tier".to_string(), Value::String("bronze".to_string()));
                            fields.insert("created_at".to_string(), Value::DateTime(chrono::Utc::now()));
                            fields.insert("balance".to_string(), Value::Float(1000.0));
                            fields.insert("age".to_string(), Value::Integer(30));
                            fields
                        },
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                        version: 1,
                    };
                    db_clone.insert_document(&customer).unwrap();
                }
            }
        })
    }).collect();
    
    for handle in handles {
        handle.join().unwrap();
    }
    let mixed_duration = start_time.elapsed();
    
    println!("  Mixed read/write workload completed in {:?}", mixed_duration);
    assert!(mixed_duration < Duration::from_secs(15), "Mixed workload took too long");
}

#[test]
fn test_index_performance_comparison() {
    println!("\n=== Index Performance Comparison ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 5000, 3, 1000);
    
    // Test query performance without index
    let query = "customers.where { .email == \"customer2500@example.com\" }";
    
    let start_time = Instant::now();
    let result_no_index = db.execute_query(query).unwrap();
    let duration_no_index = start_time.elapsed();
    
    // Create index and test again
    db.create_index("customers", "email").unwrap();
    
    let start_time = Instant::now();
    let result_with_index = db.execute_query(query).unwrap();
    let duration_with_index = start_time.elapsed();
    
    println!("  Without index: {} results in {:?}", result_no_index.len(), duration_no_index);
    println!("  With index: {} results in {:?}", result_with_index.len(), duration_with_index);
    
    assert_eq!(result_no_index.len(), result_with_index.len(), "Results should be identical");
    
    // Index should provide significant speedup for large datasets
    if result_no_index.len() > 0 {
        let speedup_ratio = duration_no_index.as_nanos() as f64 / duration_with_index.as_nanos() as f64;
        println!("  Speedup ratio: {:.2}x", speedup_ratio);
        
        // For exact matches on large datasets, we expect significant speedup
        assert!(speedup_ratio > 2.0 || duration_with_index < Duration::from_millis(10), 
                "Index should provide significant speedup or be very fast");
    }
}

#[test]
fn test_memory_efficiency_large_results() {
    println!("\n=== Memory Efficiency Test ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 3000, 8, 1000);
    
    // Test pagination vs full result loading
    let base_query = "orders.sort { .created_at.desc }";
    
    // Full result loading
    let start_time = Instant::now();
    let full_result = db.execute_query(base_query).unwrap();
    let full_duration = start_time.elapsed();
    
    // Paginated access
    let start_time = Instant::now();
    let mut paginated_count = 0;
    let mut offset = 0;
    let limit = 100;
    
    loop {
        let query = format!("{}.limit({}).offset({})", base_query, limit, offset);
        let batch = db.execute_query(&query).unwrap();
        let batch_size = batch.len();
        paginated_count += batch_size;
        
        if batch_size < limit {
            break;
        }
        offset += limit;
    }
    let paginated_duration = start_time.elapsed();
    
    println!("  Full result: {} records in {:?}", full_result.len(), full_duration);
    println!("  Paginated: {} records in {:?}", paginated_count, paginated_duration);
    
    assert_eq!(full_result.len(), paginated_count, "Should process same number of records");
    
    // Both approaches should complete in reasonable time
    assert!(full_duration < Duration::from_secs(10), "Full result loading took too long");
    assert!(paginated_duration < Duration::from_secs(15), "Paginated access took too long");
}

#[test]
fn test_query_complexity_scaling() {
    println!("\n=== Query Complexity Scaling Test ===");
    let (db, _temp_dir) = setup_test_db();
    
    populate_large_dataset(&db, 2000, 6, 800);
    
    // Create indexes for complex queries
    db.create_index("customers", "tier").unwrap();
    db.create_index("orders", "status").unwrap();
    
    let queries = vec![
        ("Simple", "customers.count"),
        ("Single filter", "customers.where { .tier == \"gold\" }.count"),
        ("Multiple filters", "customers.where { .tier == \"gold\" && .balance > 5000.0 }.count"),
        ("Reference navigation", "orders.where { .customer_id.tier == \"platinum\" }.count"),
        ("Aggregation", "customers.group_by { .tier }.select { tier: group_key, count: count(*) }"),
        ("Complex aggregation", "customers.select { tier, order_count: count(.orders), avg_order_value: avg(.orders.total) }.where { order_count > 3 }"),
        ("Multi-level analysis", "customers.where { .tier == \"gold\" }.select { name, recent_high_value_orders: count(.orders where .total > 300.0 && .created_at >= 30.days.ago) }"),
    ];
    
    for (name, query) in queries {
        let start_time = Instant::now();
        let result = db.execute_query(query).unwrap();
        let duration = start_time.elapsed();
        
        println!("  {}: {} results in {:?}", name, result.len(), duration);
        
        // Even complex queries should complete in reasonable time
        assert!(duration < Duration::from_secs(8), "{} query took too long: {:?}", name, duration);
    }
}