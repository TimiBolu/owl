// Monitoring and Observability Demo
// Demonstrates query execution metrics, system monitoring, alerts, and distributed tracing

use owl_db::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn main() -> OQLResult<()> {
    println!("=== OQL Monitoring and Observability Demo ===\n");

    // Create monitoring system with custom configuration
    let mut config = MonitoringConfig::default();
    config.alert_thresholds.max_query_time_ms = 100; // Lower threshold for demo
    config.enable_distributed_tracing = true;
    
    let monitoring = Arc::new(MonitoringSystem::new(config));

    // Create a temporary database
    let temp_dir = TempDir::new().unwrap();
    let store = Arc::new(storage::RocksDBStore::new(temp_dir.path())?);
    let change_store = ChangeTrackingStore::new(store.clone());

    // Setup schemas and test data
    setup_test_data(&change_store)?;

    // Demo 1: Query execution metrics
    demo_query_metrics(&change_store, &monitoring)?;

    // Demo 2: System metrics and alerts
    demo_system_monitoring(&monitoring)?;

    // Demo 3: Distributed tracing
    demo_distributed_tracing(&monitoring)?;

    // Demo 4: Performance dashboard
    demo_performance_dashboard(&monitoring)?;

    // Demo 5: Prometheus metrics export
    demo_prometheus_export(&monitoring)?;

    println!("\n=== Monitoring and Observability Demo Complete ===");
    Ok(())
}

fn setup_test_data(store: &ChangeTrackingStore<storage::RocksDBStore>) -> OQLResult<()> {
    println!("Setting up test data...");

    // Create users schema
    let mut user_fields = HashMap::new();
    user_fields.insert("name".to_string(), FieldType::String);
    user_fields.insert("email".to_string(), FieldType::String);
    user_fields.insert("age".to_string(), FieldType::Integer);
    user_fields.insert("tier".to_string(), FieldType::String);
    let users_schema = Schema::new("users".to_string(), user_fields);
    store.put_schema(&users_schema)?;

    // Create test users
    for i in 1..=10 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User {}", i)));
        fields.insert("email".to_string(), Value::String(format!("user{}@example.com", i)));
        fields.insert("age".to_string(), Value::Integer(20 + (i % 40) as i64));
        fields.insert("tier".to_string(), Value::String(
            if i % 3 == 0 { "premium" } else { "standard" }.to_string()
        ));
        let user = Document::with_id(format!("user_{}", i), "users".to_string(), fields);
        store.put_document(&user)?;
    }

    println!("✓ Test data created\n");
    Ok(())
}

fn demo_query_metrics(
    store: &ChangeTrackingStore<storage::RocksDBStore>,
    monitoring: &Arc<MonitoringSystem>,
) -> OQLResult<()> {
    println!("--- Demo 1: Query Execution Metrics ---");

    let executor = executor::QueryExecutor::new(store.inner().clone());

    // Simulate various queries with different performance characteristics
    let queries = vec![
        ("Fast query", "users.where { .tier == \"premium\" }"),
        ("Medium query", "users.where { .age > 25 }"),
        ("Slow query", "users.where { .name == \"User 5\" }"), // Will be artificially slowed
    ];

    for (description, query_text) in queries {
        println!("Executing {}: {}", description, query_text);
        
        // Parse and execute query
        let query = parser::parse_oql(query_text).map_err(|e| {
            OQLError::Generic {
                message: format!("Parse error: {:?}", e),
            }
        })?;
        
        // Start timing
        let start_time = std::time::Instant::now();
        
        // Create a trace span for this query
        let mut span = monitoring.start_trace(format!("execute_query: {}", description))?;
        span = span.tag("query".to_string(), query_text.to_string());
        span = span.tag("schema".to_string(), query.collection.clone());
        
        // Execute query
        let result = executor.execute_query(&query);
        let execution_time = start_time.elapsed().as_millis() as u64;
        
        // Artificially slow down the "slow query" for demo purposes
        if description.contains("Slow") {
            thread::sleep(Duration::from_millis(150));
        }
        
        // Create query metrics
        let metrics = match result {
            Ok(documents) => {
                span.log("info".to_string(), format!("Query returned {} documents", documents.len()));
                QueryMetrics::new(&query, execution_time, 10, documents.len())
                    .with_cache_hit(description.contains("Fast"))
            }
            Err(e) => {
                span.log("error".to_string(), format!("Query failed: {}", e));
                QueryMetrics::error(&query, execution_time, &e.to_string())
            }
        };
        
        // Record metrics
        monitoring.record_query_metrics(metrics)?;
        
        // Finish trace span
        monitoring.finish_span(span)?;
        
        println!("  ✓ Executed in {}ms", execution_time);
        thread::sleep(Duration::from_millis(50)); // Small delay between queries
    }

    // Show query statistics
    let stats = monitoring.get_query_stats()?;
    println!("\nQuery Statistics:");
    println!("  Total queries: {}", stats.total_queries);
    println!("  Successful queries: {}", stats.successful_queries);
    println!("  Average execution time: {:.2}ms", stats.avg_execution_time_ms);
    println!("  Success rate: {:.2}%", stats.success_rate * 100.0);
    println!("  Cache hit rate: {:.2}%", stats.cache_hit_rate * 100.0);
    println!("  Queries per second: {:.2}", stats.queries_per_second);

    println!("✓ Query metrics demo complete\n");
    Ok(())
}

fn demo_system_monitoring(monitoring: &Arc<MonitoringSystem>) -> OQLResult<()> {
    println!("--- Demo 2: System Metrics and Alerts ---");

    // Simulate system metrics updates
    let mut system_metrics = SystemMetrics::new();
    system_metrics.active_connections = 25;
    system_metrics.queries_per_second = 150.0;
    system_metrics.avg_query_time_ms = 45.0;
    system_metrics.storage_size_bytes = 1024 * 1024 * 100; // 100MB
    system_metrics.memory_usage_bytes = 1024 * 1024 * 256; // 256MB
    system_metrics.cache_hit_rate = 0.85;
    system_metrics.error_rate = 0.02; // 2%

    monitoring.update_system_metrics(system_metrics.clone())?;
    println!("✓ Updated system metrics");

    // Simulate high error rate to trigger alert
    system_metrics.error_rate = 0.08; // 8% - above threshold
    monitoring.update_system_metrics(system_metrics.clone())?;
    println!("✓ Simulated high error rate");

    // Simulate low cache hit rate to trigger alert
    system_metrics.cache_hit_rate = 0.70; // 70% - below threshold
    monitoring.update_system_metrics(system_metrics)?;
    println!("✓ Simulated low cache hit rate");

    // Create a manual alert
    let manual_alert = Alert::new(
        AlertSeverity::Info,
        "Maintenance Window".to_string(),
        "Scheduled maintenance will begin in 30 minutes".to_string(),
    )
    .with_metadata("maintenance_type".to_string(), "database_upgrade".to_string())
    .with_metadata("duration_minutes".to_string(), "60".to_string());

    monitoring.create_alert(manual_alert)?;
    println!("✓ Created manual maintenance alert");

    // Show active alerts
    let alerts = monitoring.get_active_alerts()?;
    println!("\nActive Alerts ({}):", alerts.len());
    for alert in &alerts {
        println!("  🚨 {} - {} ({})", 
            format!("{:?}", alert.severity),
            alert.title,
            alert.timestamp.format("%H:%M:%S")
        );
        println!("     {}", alert.message);
        if !alert.metadata.is_empty() {
            println!("     Metadata: {:?}", alert.metadata);
        }
    }

    // Resolve some alerts
    if let Some(alert) = alerts.first() {
        monitoring.resolve_alert(&alert.id)?;
        println!("\n✓ Resolved alert: {}", alert.title);
    }

    let remaining_alerts = monitoring.get_active_alerts()?;
    println!("✓ {} alerts remaining active", remaining_alerts.len());

    println!("✓ System monitoring demo complete\n");
    Ok(())
}

fn demo_distributed_tracing(monitoring: &Arc<MonitoringSystem>) -> OQLResult<()> {
    println!("--- Demo 3: Distributed Tracing ---");

    // Simulate a complex operation with nested spans
    let mut root_span = monitoring.start_trace("process_user_request".to_string())?;
    root_span = root_span.tag("user_id".to_string(), "user_123".to_string());
    root_span = root_span.tag("request_type".to_string(), "user_profile_update".to_string());
    
    root_span.log("info".to_string(), "Starting user profile update".to_string());

    // Simulate database lookup
    let mut db_span = root_span.child("database_lookup".to_string());
    db_span = db_span.tag("operation".to_string(), "select".to_string());
    db_span = db_span.tag("table".to_string(), "users".to_string());
    
    thread::sleep(Duration::from_millis(25));
    db_span.log("info".to_string(), "User found in database".to_string());
    monitoring.finish_span(db_span)?;

    // Simulate validation
    let mut validation_span = root_span.child("validate_input".to_string());
    validation_span = validation_span.tag("validator".to_string(), "email_validator".to_string());
    
    thread::sleep(Duration::from_millis(10));
    validation_span.log("info".to_string(), "Email validation passed".to_string());
    monitoring.finish_span(validation_span)?;

    // Simulate update operation
    let mut update_span = root_span.child("update_user".to_string());
    update_span = update_span.tag("operation".to_string(), "update".to_string());
    update_span = update_span.tag("fields_updated".to_string(), "email,name".to_string());
    
    thread::sleep(Duration::from_millis(30));
    update_span.log("info".to_string(), "User profile updated successfully".to_string());
    monitoring.finish_span(update_span)?;

    // Simulate cache invalidation
    let mut cache_span = root_span.child("invalidate_cache".to_string());
    cache_span = cache_span.tag("cache_type".to_string(), "user_profile".to_string());
    
    thread::sleep(Duration::from_millis(5));
    cache_span.log("info".to_string(), "Cache invalidated".to_string());
    monitoring.finish_span(cache_span)?;

    root_span.log("info".to_string(), "User profile update completed".to_string());
    let trace_id = root_span.trace_id.clone();
    monitoring.finish_span(root_span)?;

    // Retrieve and display the trace
    let trace_spans = monitoring.get_trace(&trace_id)?;
    println!("Trace {} ({} spans):", trace_id, trace_spans.len());
    
    for span in &trace_spans {
        let indent = if span.parent_span_id.is_some() { "  " } else { "" };
        println!("{}📍 {} ({}ms)", 
            indent,
            span.operation_name,
            span.duration_ms.unwrap_or(0)
        );
        
        if !span.tags.is_empty() {
            println!("{}   Tags: {:?}", indent, span.tags);
        }
        
        for log in &span.logs {
            println!("{}   {} [{}]: {}", 
                indent,
                log.timestamp.format("%H:%M:%S%.3f"),
                log.level,
                log.message
            );
        }
    }

    println!("✓ Distributed tracing demo complete\n");
    Ok(())
}

fn demo_performance_dashboard(monitoring: &Arc<MonitoringSystem>) -> OQLResult<()> {
    println!("--- Demo 4: Performance Dashboard ---");

    let dashboard_data = monitoring.get_dashboard_data()?;

    println!("📊 System Overview:");
    println!("  Uptime: {}s", dashboard_data.system_metrics.uptime_seconds);
    println!("  Active connections: {}", dashboard_data.system_metrics.active_connections);
    println!("  Queries/sec: {:.2}", dashboard_data.system_metrics.queries_per_second);
    println!("  Avg query time: {:.2}ms", dashboard_data.system_metrics.avg_query_time_ms);
    println!("  Storage size: {:.2}MB", dashboard_data.system_metrics.storage_size_bytes as f64 / 1024.0 / 1024.0);
    println!("  Memory usage: {:.2}MB", dashboard_data.system_metrics.memory_usage_bytes as f64 / 1024.0 / 1024.0);
    println!("  Cache hit rate: {:.2}%", dashboard_data.system_metrics.cache_hit_rate * 100.0);
    println!("  Error rate: {:.2}%", dashboard_data.system_metrics.error_rate * 100.0);

    println!("\n📈 Query Performance:");
    println!("  Total queries: {}", dashboard_data.query_stats.total_queries);
    println!("  Successful queries: {}", dashboard_data.query_stats.successful_queries);
    println!("  Success rate: {:.2}%", dashboard_data.query_stats.success_rate * 100.0);
    println!("  Avg execution time: {:.2}ms", dashboard_data.query_stats.avg_execution_time_ms);

    println!("\n🚨 Active Alerts ({}):", dashboard_data.active_alerts.len());
    for alert in &dashboard_data.active_alerts {
        println!("  {} - {} ({})", 
            match alert.severity {
                AlertSeverity::Info => "ℹ️",
                AlertSeverity::Warning => "⚠️",
                AlertSeverity::Error => "❌",
                AlertSeverity::Critical => "🔥",
            },
            alert.title,
            alert.timestamp.format("%H:%M:%S")
        );
    }

    println!("\n📝 Recent Queries ({}):", dashboard_data.recent_queries.len());
    for query in dashboard_data.recent_queries.iter().take(5) {
        let status = if query.success { "✅" } else { "❌" };
        println!("  {} {} on {} ({}ms)", 
            status,
            query.operation_type,
            query.schema,
            query.execution_time_ms
        );
    }

    println!("✓ Performance dashboard demo complete\n");
    Ok(())
}

fn demo_prometheus_export(monitoring: &Arc<MonitoringSystem>) -> OQLResult<()> {
    println!("--- Demo 5: Prometheus Metrics Export ---");

    let prometheus_metrics = monitoring.export_prometheus_metrics()?;
    
    println!("Prometheus metrics export:");
    println!("{}", prometheus_metrics);

    // Verify key metrics are present
    let required_metrics = vec![
        "oql_active_connections",
        "oql_queries_per_second",
        "oql_avg_query_time_ms",
        "oql_cache_hit_rate",
        "oql_error_rate",
        "oql_storage_size_bytes",
        "oql_memory_usage_bytes",
        "oql_uptime_seconds",
    ];

    println!("Verification:");
    for metric in required_metrics {
        if prometheus_metrics.contains(metric) {
            println!("  ✅ {} - present", metric);
        } else {
            println!("  ❌ {} - missing", metric);
        }
    }

    println!("✓ Prometheus export demo complete\n");
    Ok(())
}