use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use owl_db::*;
use std::collections::HashMap;
use tempfile::TempDir;
use std::time::Duration;

fn setup_test_db() -> (Database, TempDir) {
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
            fields.insert("created_at".to_string(), FieldType::DateTime);
            fields.insert("balance".to_string(), FieldType::Float);
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

fn populate_customers(db: &Database, count: usize) {
    for i in 0..count {
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
                fields.insert("created_at".to_string(), Value::DateTime(
                    chrono::Utc::now() - chrono::Duration::days((i % 365) as i64)
                ));
                fields.insert("balance".to_string(), Value::Float((i as f64) * 100.0 + 1000.0));
                fields
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        };
        db.insert_document(&customer).unwrap();
    }
}

fn populate_orders(db: &Database, customer_count: usize, orders_per_customer: usize) {
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
                        match order_i % 4 {
                            0 => "pending",
                            1 => "processing", 
                            2 => "shipped",
                            _ => "completed",
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
}

// Benchmark basic document operations
fn bench_document_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("document_operations");
    
    for size in [100, 1000, 10000].iter() {
        let (db, _temp_dir) = setup_test_db();
        populate_customers(&db, *size);
        
        group.bench_with_input(
            BenchmarkId::new("single_document_lookup", size),
            size,
            |b, &size| {
                let query = format!("customers.where {{ .id == \"cust_{:06}\" }}", size / 2);
                b.iter(|| {
                    let result = db.execute_query(black_box(&query)).unwrap();
                    black_box(result);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("full_table_scan", size),
            size,
            |b, _| {
                let query = "customers.where { .tier == \"gold\" }";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
    }
    
    group.finish();
}

// Benchmark reference resolution performance
fn bench_reference_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("reference_resolution");
    
    for customer_count in [100, 500, 1000].iter() {
        let (db, _temp_dir) = setup_test_db();
        populate_customers(&db, *customer_count);
        populate_orders(&db, *customer_count, 5); // 5 orders per customer
        
        group.bench_with_input(
            BenchmarkId::new("single_level_reference", customer_count),
            customer_count,
            |b, _| {
                let query = "orders.where { .status == \"completed\" }.select { customer_name: .customer_id.name }";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("batch_reference_resolution", customer_count),
            customer_count,
            |b, _| {
                let query = "orders.select { id, customer_name: .customer_id.name, customer_email: .customer_id.email }";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
    }
    
    group.finish();
}

// Benchmark aggregation operations
fn bench_aggregation_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("aggregation_operations");
    
    for size in [1000, 5000, 10000].iter() {
        let (db, _temp_dir) = setup_test_db();
        populate_customers(&db, *size);
        populate_orders(&db, *size, 3); // 3 orders per customer
        
        group.bench_with_input(
            BenchmarkId::new("count_aggregation", size),
            size,
            |b, _| {
                let query = "customers.group_by { .tier }.select { tier: group_key, count: count(*) }";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("sum_aggregation", size),
            size,
            |b, _| {
                let query = "orders.group_by { .status }.select { status: group_key, total_revenue: sum(.total) }";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("complex_aggregation_with_references", size),
            size,
            |b, _| {
                let query = "customers.select { name, total_orders: count(.orders), avg_order_value: avg(.orders.total) }";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
    }
    
    group.finish();
}

// Benchmark sorting and pagination
fn bench_sorting_pagination(c: &mut Criterion) {
    let mut group = c.benchmark_group("sorting_pagination");
    
    for size in [1000, 5000, 10000].iter() {
        let (db, _temp_dir) = setup_test_db();
        populate_customers(&db, *size);
        
        group.bench_with_input(
            BenchmarkId::new("sort_by_single_field", size),
            size,
            |b, _| {
                let query = "customers.sort { .balance.desc }";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("sort_with_limit", size),
            size,
            |b, _| {
                let query = "customers.sort { .balance.desc }.limit(100)";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("pagination", size),
            size,
            |b, _| {
                let query = "customers.sort { .created_at.desc }.limit(50).offset(100)";
                b.iter(|| {
                    let result = db.execute_query(black_box(query)).unwrap();
                    black_box(result);
                });
            },
        );
    }
    
    group.finish();
}

// Benchmark concurrent operations
fn bench_concurrent_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_operations");
    group.measurement_time(Duration::from_secs(10));
    
    let (db, _temp_dir) = setup_test_db();
    populate_customers(&db, 1000);
    
    group.bench_function("concurrent_reads", |b| {
        b.iter(|| {
            let handles: Vec<_> = (0..4).map(|i| {
                let db_clone = db.clone();
                std::thread::spawn(move || {
                    let query = format!("customers.where {{ .tier == \"{}\" }}", 
                        match i % 3 { 0 => "bronze", 1 => "silver", _ => "gold" });
                    db_clone.execute_query(&query).unwrap()
                })
            }).collect();
            
            for handle in handles {
                black_box(handle.join().unwrap());
            }
        });
    });
    
    group.bench_function("concurrent_writes", |b| {
        b.iter(|| {
            let handles: Vec<_> = (0..4).map(|i| {
                let db_clone = db.clone();
                std::thread::spawn(move || {
                    let customer = Document {
                        id: format!("temp_cust_{}", i),
                        schema: "customers".to_string(),
                        fields: {
                            let mut fields = HashMap::new();
                            fields.insert("id".to_string(), Value::String(format!("temp_cust_{}", i)));
                            fields.insert("name".to_string(), Value::String(format!("Temp Customer {}", i)));
                            fields.insert("email".to_string(), Value::String(format!("temp{}@example.com", i)));
                            fields.insert("tier".to_string(), Value::String("bronze".to_string()));
                            fields.insert("created_at".to_string(), Value::DateTime(chrono::Utc::now()));
                            fields.insert("balance".to_string(), Value::Float(1000.0));
                            fields
                        },
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                        version: 1,
                    };
                    db_clone.insert_document(&customer).unwrap()
                })
            }).collect();
            
            for handle in handles {
                black_box(handle.join().unwrap());
            }
        });
    });
    
    group.finish();
}

// Benchmark index performance
fn bench_index_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_performance");
    
    for size in [1000, 5000, 10000].iter() {
        let (db, _temp_dir) = setup_test_db();
        populate_customers(&db, *size);
        
        // Create index on email field
        db.create_index("customers", "email").unwrap();
        
        group.bench_with_input(
            BenchmarkId::new("indexed_lookup", size),
            size,
            |b, &size| {
                let email = format!("customer{}@example.com", size / 2);
                let query = format!("customers.where {{ .email == \"{}\" }}", email);
                b.iter(|| {
                    let result = db.execute_query(black_box(&query)).unwrap();
                    black_box(result);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("non_indexed_lookup", size),
            size,
            |b, &size| {
                let name = format!("Customer {}", size / 2);
                let query = format!("customers.where {{ .name == \"{}\" }}", name);
                b.iter(|| {
                    let result = db.execute_query(black_box(&query)).unwrap();
                    black_box(result);
                });
            },
        );
    }
    
    group.finish();
}

// Benchmark streaming operations
fn bench_streaming_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("streaming_operations");
    group.measurement_time(Duration::from_secs(15));
    
    for size in [5000, 10000, 20000].iter() {
        let (db, _temp_dir) = setup_test_db();
        populate_customers(&db, *size);
        
        group.bench_with_input(
            BenchmarkId::new("stream_large_dataset", size),
            size,
            |b, _| {
                b.iter(|| {
                    let stream = db.execute_stream("customers.where { .balance > 5000.0 }").unwrap();
                    let mut count = 0;
                    for batch in stream {
                        count += batch.unwrap().len();
                    }
                    black_box(count);
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("paginated_access", size),
            size,
            |b, _| {
                b.iter(|| {
                    let mut total_processed = 0;
                    let mut offset = 0;
                    let limit = 100;
                    
                    loop {
                        let query = format!("customers.sort {{ .id }}.limit({}).offset({})", limit, offset);
                        let result = db.execute_query(black_box(&query)).unwrap();
                        let batch_size = result.len();
                        total_processed += batch_size;
                        
                        if batch_size < limit {
                            break;
                        }
                        offset += limit;
                    }
                    black_box(total_processed);
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_document_operations,
    bench_reference_resolution,
    bench_aggregation_operations,
    bench_sorting_pagination,
    bench_concurrent_operations,
    bench_index_performance,
    bench_streaming_operations
);

criterion_main!(benches);