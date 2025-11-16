use owl_db::*;
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

mod performance_test_utils;
use performance_test_utils::TestDatabase;

fn setup_load_test_db() -> (TestDatabase, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db = TestDatabase::new(temp_dir.path()).unwrap();
    
    // Create schemas optimized for load testing
    let user_schema = Schema {
        name: "users".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("username".to_string(), FieldType::String);
            fields.insert("email".to_string(), FieldType::String);
            fields.insert("status".to_string(), FieldType::String);
            fields.insert("created_at".to_string(), FieldType::DateTime);
            fields.insert("last_login".to_string(), FieldType::DateTime);
            fields.insert("login_count".to_string(), FieldType::Integer);
            fields
        },
    };
    
    let session_schema = Schema {
        name: "sessions".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("user_id".to_string(), FieldType::Reference(ReferenceType { target_schema: "users".to_string(), required: false, on_delete: CascadeBehavior::Restrict }));
            fields.insert("token".to_string(), FieldType::String);
            fields.insert("created_at".to_string(), FieldType::DateTime);
            fields.insert("expires_at".to_string(), FieldType::DateTime);
            fields.insert("active".to_string(), FieldType::Boolean);
            fields
        },
    };
    
    let event_schema = Schema {
        name: "events".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("user_id".to_string(), FieldType::Reference(ReferenceType { target_schema: "users".to_string(), required: false, on_delete: CascadeBehavior::Restrict }));
            fields.insert("event_type".to_string(), FieldType::String);
            fields.insert("data".to_string(), FieldType::String);
            fields.insert("timestamp".to_string(), FieldType::DateTime);
            fields
        },
    };
    
    db.create_schema(user_schema).unwrap();
    db.create_schema(session_schema).unwrap();
    db.create_schema(event_schema).unwrap();
    
    // Create indexes for load testing
    db.create_index("users", "username").unwrap();
    db.create_index("users", "email").unwrap();
    db.create_index("sessions", "token").unwrap();
    db.create_index("events", "event_type").unwrap();
    
    (db, temp_dir)
}

fn populate_base_data(db: &TestDatabase, user_count: usize) {
    println!("Populating {} users for load testing...", user_count);
    
    for i in 0..user_count {
        let user = Document {
            id: format!("user_{:08}", i),
            schema: "users".to_string(),
            fields: {
                let mut fields = HashMap::new();
                fields.insert("id".to_string(), Value::String(format!("user_{:08}", i)));
                fields.insert("username".to_string(), Value::String(format!("user{}", i)));
                fields.insert("email".to_string(), Value::String(format!("user{}@loadtest.com", i)));
                fields.insert("status".to_string(), Value::String(
                    match i % 4 {
                        0 => "active",
                        1 => "inactive", 
                        2 => "suspended",
                        _ => "pending",
                    }.to_string()
                ));
                fields.insert("created_at".to_string(), Value::DateTime(
                    chrono::Utc::now() - chrono::Duration::days((i % 365) as i64)
                ));
                fields.insert("last_login".to_string(), Value::DateTime(
                    chrono::Utc::now() - chrono::Duration::hours((i % 24) as i64)
                ));
                fields.insert("login_count".to_string(), Value::Integer((i % 1000) as i64));
                fields
            },
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        };
        db.insert_document(&user).unwrap();
        
        if i % 1000 == 0 && i > 0 {
            println!("  Populated {} users", i);
        }
    }
    
    println!("Base data population completed!");
}

#[test]
fn test_concurrent_read_load() {
    println!("\n=== Concurrent Read Load Test ===");
    let (db, _temp_dir) = setup_load_test_db();
    populate_base_data(&db, 5000);
    
    let thread_count = 16;
    let queries_per_thread = 100;
    let barrier = Arc::new(Barrier::new(thread_count));
    
    println!("Running {} threads, {} queries each ({} total queries)", 
             thread_count, queries_per_thread, thread_count * queries_per_thread);
    
    let start_time = Instant::now();
    
    let handles: Vec<_> = (0..thread_count).map(|thread_id| {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        
        thread::spawn(move || {
            // Wait for all threads to be ready
            barrier_clone.wait();
            
            let thread_start = Instant::now();
            let mut successful_queries = 0;
            let mut total_results = 0;
            
            for i in 0..queries_per_thread {
                let query = match i % 4 {
                    0 => "users.where { .status == \"active\" }".to_string(),
                    1 => format!("users.where {{ .username == \"user{}\" }}", (thread_id * 1000 + i) % 5000),
                    2 => "users.where { .login_count > 500 }".to_string(),
                    _ => "users.where { .created_at >= 30.days.ago }".to_string(),
                };
                
                match db_clone.execute_query(&query) {
                    Ok(results) => {
                        successful_queries += 1;
                        total_results += results.len();
                    }
                    Err(e) => {
                        eprintln!("Thread {} query {} failed: {}", thread_id, i, e);
                    }
                }
            }
            
            let thread_duration = thread_start.elapsed();
            (thread_id, successful_queries, total_results, thread_duration)
        })
    }).collect();
    
    let mut total_successful = 0;
    let mut total_results = 0;
    let mut max_thread_time = Duration::from_millis(0);
    
    for handle in handles {
        let (thread_id, successful, results, duration) = handle.join().unwrap();
        total_successful += successful;
        total_results += results;
        max_thread_time = max_thread_time.max(duration);
        
        println!("  Thread {}: {}/{} queries successful, {} results, {:?}", 
                 thread_id, successful, queries_per_thread, results, duration);
    }
    
    let total_duration = start_time.elapsed();
    let queries_per_second = total_successful as f64 / total_duration.as_secs_f64();
    
    println!("\nConcurrent Read Load Test Results:");
    println!("  Total successful queries: {}/{}", total_successful, thread_count * queries_per_thread);
    println!("  Total results returned: {}", total_results);
    println!("  Total time: {:?}", total_duration);
    println!("  Max thread time: {:?}", max_thread_time);
    println!("  Queries per second: {:.2}", queries_per_second);
    
    // Assertions for performance expectations
    assert!(total_successful >= (thread_count * queries_per_thread * 95) / 100, 
            "Should have >95% success rate");
    assert!(queries_per_second > 50.0, "Should handle >50 queries/second");
    assert!(total_duration < Duration::from_secs(30), "Should complete within 30 seconds");
}

#[test]
fn test_concurrent_write_load() {
    println!("\n=== Concurrent Write Load Test ===");
    let (db, _temp_dir) = setup_load_test_db();
    populate_base_data(&db, 1000); // Smaller base for write test
    
    let thread_count = 8;
    let writes_per_thread = 50;
    let barrier = Arc::new(Barrier::new(thread_count));
    
    println!("Running {} threads, {} writes each ({} total writes)", 
             thread_count, writes_per_thread, thread_count * writes_per_thread);
    
    let start_time = Instant::now();
    
    let handles: Vec<_> = (0..thread_count).map(|thread_id| {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        
        thread::spawn(move || {
            barrier_clone.wait();
            
            let thread_start = Instant::now();
            let mut successful_writes = 0;
            
            for i in 0..writes_per_thread {
                // Create session documents
                let session = Document {
                    id: format!("session_{}_{:04}", thread_id, i),
                    schema: "sessions".to_string(),
                    fields: {
                        let mut fields = HashMap::new();
                        fields.insert("id".to_string(), Value::String(format!("session_{}_{:04}", thread_id, i)));
                        fields.insert("user_id".to_string(), Value::Reference(format!("user_{:08}", (thread_id * 100 + i) % 1000)));
                        fields.insert("token".to_string(), Value::String(format!("token_{}_{}", thread_id, i)));
                        fields.insert("created_at".to_string(), Value::DateTime(chrono::Utc::now()));
                        fields.insert("expires_at".to_string(), Value::DateTime(
                            chrono::Utc::now() + chrono::Duration::hours(24)
                        ));
                        fields.insert("active".to_string(), Value::Boolean(true));
                        fields
                    },
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    version: 1,
                };
                
                match db_clone.insert_document(&session) {
                    Ok(_) => successful_writes += 1,
                    Err(e) => eprintln!("Thread {} write {} failed: {}", thread_id, i, e),
                }
                
                // Also create an event document
                let event = Document {
                    id: format!("event_{}_{:04}", thread_id, i),
                    schema: "events".to_string(),
                    fields: {
                        let mut fields = HashMap::new();
                        fields.insert("id".to_string(), Value::String(format!("event_{}_{:04}", thread_id, i)));
                        fields.insert("user_id".to_string(), Value::Reference(format!("user_{:08}", (thread_id * 100 + i) % 1000)));
                        fields.insert("event_type".to_string(), Value::String("login".to_string()));
                        fields.insert("data".to_string(), Value::String(format!("{{\"thread\": {}, \"sequence\": {}}}", thread_id, i)));
                        fields.insert("timestamp".to_string(), Value::DateTime(chrono::Utc::now()));
                        fields
                    },
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    version: 1,
                };
                
                match db_clone.insert_document(&event) {
                    Ok(_) => successful_writes += 1,
                    Err(e) => eprintln!("Thread {} event write {} failed: {}", thread_id, i, e),
                }
            }
            
            let thread_duration = thread_start.elapsed();
            (thread_id, successful_writes, thread_duration)
        })
    }).collect();
    
    let mut total_successful = 0;
    let mut max_thread_time = Duration::from_millis(0);
    
    for handle in handles {
        let (thread_id, successful, duration) = handle.join().unwrap();
        total_successful += successful;
        max_thread_time = max_thread_time.max(duration);
        
        println!("  Thread {}: {}/{} writes successful, {:?}", 
                 thread_id, successful, writes_per_thread * 2, duration);
    }
    
    let total_duration = start_time.elapsed();
    let writes_per_second = total_successful as f64 / total_duration.as_secs_f64();
    
    println!("\nConcurrent Write Load Test Results:");
    println!("  Total successful writes: {}/{}", total_successful, thread_count * writes_per_thread * 2);
    println!("  Total time: {:?}", total_duration);
    println!("  Max thread time: {:?}", max_thread_time);
    println!("  Writes per second: {:.2}", writes_per_second);
    
    // Verify data was written correctly
    let session_count = db.execute_query("sessions.count").unwrap();
    let event_count = db.execute_query("events.count").unwrap();
    
    println!("  Sessions created: {}", session_count.len());
    println!("  Events created: {}", event_count.len());
    
    assert!(total_successful >= (thread_count * writes_per_thread * 2 * 90) / 100, 
            "Should have >90% write success rate");
    assert!(writes_per_second > 20.0, "Should handle >20 writes/second");
}

#[test]
fn test_mixed_workload_stress() {
    println!("\n=== Mixed Workload Stress Test ===");
    let (db, _temp_dir) = setup_load_test_db();
    populate_base_data(&db, 3000);
    
    let reader_threads = 6;
    let writer_threads = 2;
    let updater_threads = 2;
    let total_threads = reader_threads + writer_threads + updater_threads;
    
    let operations_per_thread = 100;
    let barrier = Arc::new(Barrier::new(total_threads));
    
    println!("Running mixed workload: {} readers, {} writers, {} updaters", 
             reader_threads, writer_threads, updater_threads);
    
    let start_time = Instant::now();
    let mut handles = Vec::new();
    
    // Reader threads
    for thread_id in 0..reader_threads {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_ops = 0;
            for i in 0..operations_per_thread {
                let query = match i % 5 {
                    0 => "users.where { .status == \"active\" }.count",
                    1 => "sessions.where { .active == true }",
                    2 => "events.where { .event_type == \"login\" }.limit(10)",
                    3 => "users.select { username, login_count, last_session: .sessions.where { .active == true }.first }",
                    _ => "users.group_by { .status }.select { status: group_key, count: count(*) }",
                };
                
                if db_clone.execute_query(query).is_ok() {
                    successful_ops += 1;
                }
            }
            
            ("reader", thread_id, successful_ops)
        });
        handles.push(handle);
    }
    
    // Writer threads
    for thread_id in 0..writer_threads {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_ops = 0;
            for i in 0..operations_per_thread {
                let session = Document {
                    id: format!("stress_session_{}_{}", thread_id, i),
                    schema: "sessions".to_string(),
                    fields: {
                        let mut fields = HashMap::new();
                        fields.insert("id".to_string(), Value::String(format!("stress_session_{}_{}", thread_id, i)));
                        fields.insert("user_id".to_string(), Value::Reference(format!("user_{:08}", (thread_id * 100 + i) % 3000)));
                        fields.insert("token".to_string(), Value::String(format!("stress_token_{}_{}", thread_id, i)));
                        fields.insert("created_at".to_string(), Value::DateTime(chrono::Utc::now()));
                        fields.insert("expires_at".to_string(), Value::DateTime(
                            chrono::Utc::now() + chrono::Duration::hours(1)
                        ));
                        fields.insert("active".to_string(), Value::Boolean(true));
                        fields
                    },
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    version: 1,
                };
                
                if db_clone.insert_document(&session).is_ok() {
                    successful_ops += 1;
                }
            }
            
            ("writer", thread_id, successful_ops)
        });
        handles.push(handle);
    }
    
    // Updater threads
    for thread_id in 0..updater_threads {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            
            let mut successful_ops = 0;
            for i in 0..operations_per_thread {
                let user_id = format!("user_{:08}", (thread_id * 500 + i) % 3000);
                let update_query = format!(
                    "users.where {{ .id == \"{}\" }}.update {{ login_count: .login_count + 1, last_login: now() }}", 
                    user_id
                );
                
                if db_clone.execute_query(&update_query).is_ok() {
                    successful_ops += 1;
                }
            }
            
            ("updater", thread_id, successful_ops)
        });
        handles.push(handle);
    }
    
    // Collect results
    let mut results_by_type: HashMap<String, (usize, usize)> = HashMap::new();
    
    for handle in handles {
        let (thread_type, _thread_id, successful_ops) = handle.join().unwrap();
        let entry = results_by_type.entry(thread_type.to_string()).or_insert((0, 0));
        entry.0 += successful_ops;
        entry.1 += operations_per_thread;
    }
    
    let total_duration = start_time.elapsed();
    
    println!("\nMixed Workload Stress Test Results:");
    println!("  Total time: {:?}", total_duration);
    
    for (thread_type, (successful, total)) in results_by_type {
        let success_rate = (successful as f64 / total as f64) * 100.0;
        let ops_per_second = successful as f64 / total_duration.as_secs_f64();
        println!("  {}: {}/{} operations ({:.1}% success), {:.2} ops/sec", 
                 thread_type, successful, total, success_rate, ops_per_second);
    }
    
    // Verify database consistency
    let user_count = db.execute_query("users.count").unwrap();
    let session_count = db.execute_query("sessions.count").unwrap();
    let event_count = db.execute_query("events.count").unwrap();
    
    println!("  Final counts - Users: {}, Sessions: {}, Events: {}", 
             user_count.len(), session_count.len(), event_count.len());
    
    assert!(total_duration < Duration::from_secs(60), "Mixed workload should complete within 60 seconds");
}

#[test]
fn test_memory_pressure_large_dataset() {
    println!("\n=== Memory Pressure Test with Large Dataset ===");
    let (db, _temp_dir) = setup_load_test_db();
    
    // Create a large dataset to test memory handling
    let large_user_count = 10000;
    populate_base_data(&db, large_user_count);
    
    // Create many sessions and events to increase memory pressure
    println!("Creating large dataset for memory pressure testing...");
    for i in 0..large_user_count {
        // Create 3 sessions per user
        for j in 0..3 {
            let session = Document {
                id: format!("mem_session_{}_{}", i, j),
                schema: "sessions".to_string(),
                fields: {
                    let mut fields = HashMap::new();
                    fields.insert("id".to_string(), Value::String(format!("mem_session_{}_{}", i, j)));
                    fields.insert("user_id".to_string(), Value::Reference(format!("user_{:08}", i)));
                    fields.insert("token".to_string(), Value::String(format!("mem_token_{}_{}_{}", i, j, chrono::Utc::now().timestamp())));
                    fields.insert("created_at".to_string(), Value::DateTime(chrono::Utc::now()));
                    fields.insert("expires_at".to_string(), Value::DateTime(
                        chrono::Utc::now() + chrono::Duration::hours(24)
                    ));
                    fields.insert("active".to_string(), Value::Boolean(j == 0)); // Only first session active
                    fields
                },
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                version: 1,
            };
            db.insert_document(&session).unwrap();
        }
        
        // Create 5 events per user
        for j in 0..5 {
            let event = Document {
                id: format!("mem_event_{}_{}", i, j),
                schema: "events".to_string(),
                fields: {
                    let mut fields = HashMap::new();
                    fields.insert("id".to_string(), Value::String(format!("mem_event_{}_{}", i, j)));
                    fields.insert("user_id".to_string(), Value::Reference(format!("user_{:08}", i)));
                    fields.insert("event_type".to_string(), Value::String(
                        match j % 3 {
                            0 => "login",
                            1 => "logout",
                            _ => "action",
                        }.to_string()
                    ));
                    fields.insert("data".to_string(), Value::String(format!("{{\"user\": {}, \"event\": {}, \"timestamp\": {}}}", i, j, chrono::Utc::now().timestamp())));
                    fields.insert("timestamp".to_string(), Value::DateTime(chrono::Utc::now()));
                    fields
                },
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                version: 1,
            };
            db.insert_document(&event).unwrap();
        }
        
        if i % 1000 == 0 && i > 0 {
            println!("  Created data for {} users", i);
        }
    }
    
    println!("Large dataset created. Testing memory-intensive queries...");
    
    // Test queries that could cause memory pressure
    let memory_intensive_queries = vec![
        ("Full table scan", "users.where { .login_count > 0 }"),
        ("Large aggregation", "users.group_by { .status }.select { status: group_key, users: count(*), total_logins: sum(.login_count) }"),
        ("Complex reference query", "users.select { username, active_sessions: count(.sessions where .active == true), total_events: count(.events) }"),
        ("Large result sorting", "events.sort { .timestamp.desc }.limit(5000)"),
        ("Cross-reference analysis", "sessions.where { .active == true }.select { token, user_email: .user_id.email, recent_events: count(.user_id.events where .timestamp >= 1.hour.ago) }"),
    ];
    
    for (name, query) in memory_intensive_queries {
        println!("  Testing: {}", name);
        let start_time = Instant::now();
        
        match db.execute_query(query) {
            Ok(results) => {
                let duration = start_time.elapsed();
                println!("    ✓ {} results in {:?}", results.len(), duration);
                assert!(duration < Duration::from_secs(30), "{} took too long: {:?}", name, duration);
            }
            Err(e) => {
                panic!("Memory pressure query '{}' failed: {}", name, e);
            }
        }
    }
    
    // Test streaming for large result sets
    println!("  Testing streaming with large results...");
    let start_time = Instant::now();
    let stream = db.execute_stream("events.where { .event_type == \"login\" }").unwrap();
    let mut total_streamed = 0;
    
    for batch in stream {
        total_streamed += batch.unwrap().len();
    }
    
    let streaming_duration = start_time.elapsed();
    println!("    ✓ Streamed {} events in {:?}", total_streamed, streaming_duration);
    
    assert!(streaming_duration < Duration::from_secs(45), "Streaming took too long: {:?}", streaming_duration);
    assert!(total_streamed > 0, "Should have streamed some results");
    
    println!("Memory pressure test completed successfully!");
}