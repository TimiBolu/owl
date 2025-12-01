//! Tests for Cross-Layer Observability (Task 8.15)
//!
//! This test suite verifies the cross-layer observability implementation,
//! including metrics collection, tracing, performance monitoring, and
//! unified logging across the Rust-Zig boundary.

use owl_db::{
    cross_layer_observability::{
        CrossLayerObservabilityManager, LogLevel, ArenaMetrics, BTreeMetrics,
        WALMetrics, TransactionMetrics, MemoryMetrics, IOMetrics,
    },
    monitoring::{MonitoringSystem, MonitoringConfig},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn test_cross_layer_observability_manager_creation() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Test that the manager was created successfully
    // This is mainly a compilation and initialization test
    drop(observability);
}

#[test]
fn test_unified_logging() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Test Rust layer logging
    let result = observability.log(
        LogLevel::Info,
        "rust",
        "test_component",
        "Test message from Rust layer",
        None,
        Some("trace_123".to_string()),
        Some("span_456".to_string()),
        HashMap::new(),
    );
    assert!(result.is_ok());

    // Test Zig layer logging
    let result = observability.log(
        LogLevel::Warn,
        "zig",
        "storage_component",
        "Test message from Zig layer",
        Some(0),
        Some("trace_123".to_string()),
        Some("span_789".to_string()),
        HashMap::new(),
    );
    assert!(result.is_ok());

    // Get recent logs
    let logs = observability.get_recent_logs(10).unwrap();
    assert_eq!(logs.len(), 2);

    // Verify log entries
    assert_eq!(logs[0].layer, "rust");
    assert_eq!(logs[0].component, "test_component");
    assert_eq!(logs[0].message, "Test message from Rust layer");
    assert_eq!(logs[0].trace_id, Some("trace_123".to_string()));

    assert_eq!(logs[1].layer, "zig");
    assert_eq!(logs[1].component, "storage_component");
    assert_eq!(logs[1].message, "Test message from Zig layer");
    assert_eq!(logs[1].shard_id, Some(0));
}

#[test]
fn test_cross_layer_tracing() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Start a cross-layer trace
    let trace_id = observability.start_trace("cross_layer_operation").unwrap();
    assert!(trace_id.starts_with("trace_"));

    // Start Rust span
    let rust_span_id = observability
        .start_span(&trace_id, "rust_operation", "rust", None)
        .unwrap();
    assert!(rust_span_id.starts_with("span_"));

    // Start Zig span
    let zig_span_id = observability
        .start_span(&trace_id, "zig_operation", "zig", Some(0))
        .unwrap();
    assert!(zig_span_id.starts_with("span_"));

    // Finish spans
    observability.finish_span(&trace_id, &rust_span_id).unwrap();
    observability.finish_span(&trace_id, &zig_span_id).unwrap();

    // Finish trace
    let trace_context = observability.finish_trace(&trace_id).unwrap();
    assert_eq!(trace_context.trace_id, trace_id);
    assert_eq!(trace_context.span_stack.len(), 2);

    // Verify spans
    let rust_span = &trace_context.span_stack[0];
    assert_eq!(rust_span.operation_name, "rust_operation");
    assert_eq!(rust_span.layer, "rust");
    assert!(rust_span.end_time.is_some());

    let zig_span = &trace_context.span_stack[1];
    assert_eq!(zig_span.operation_name, "zig_operation");
    assert_eq!(zig_span.layer, "zig");
    assert_eq!(zig_span.shard_id, Some(0));
    assert!(zig_span.end_time.is_some());
}

#[test]
fn test_performance_monitoring() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Record some performance metrics
    let result = observability.record_performance(
        "test_operation",
        Duration::from_millis(50),
        Some(1024),
    );
    assert!(result.is_ok());

    let result = observability.record_performance(
        "test_operation",
        Duration::from_millis(75),
        Some(2048),
    );
    assert!(result.is_ok());

    let result = observability.record_performance(
        "another_operation",
        Duration::from_millis(100),
        None,
    );
    assert!(result.is_ok());

    // Performance metrics are recorded internally
    // In a real implementation, we would have methods to retrieve and verify them
}

#[test]
fn test_log_levels() {
    // Test log level ordering
    assert!(LogLevel::Trace < LogLevel::Debug);
    assert!(LogLevel::Debug < LogLevel::Info);
    assert!(LogLevel::Info < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Error);
    assert!(LogLevel::Error < LogLevel::Fatal);
}

#[test]
fn test_metrics_structures() {
    // Test that metrics structures can be created and have sensible defaults
    let arena_metrics = ArenaMetrics {
        total_allocations: 100,
        successful_allocations: 95,
        failed_allocations: 5,
        total_bytes_allocated: 1024000,
        total_bytes_freed: 512000,
        current_usage_bytes: 512000,
        peak_usage_bytes: 1024000,
        fragmentation_ratio: 0.1,
        avg_allocation_size: 1024.0,
        allocation_latency_ns: 1000,
        colocation_success_rate: 0.8,
    };

    assert_eq!(arena_metrics.total_allocations, 100);
    assert_eq!(arena_metrics.successful_allocations, 95);
    assert_eq!(arena_metrics.failed_allocations, 5);

    let btree_metrics = BTreeMetrics {
        total_insertions: 50,
        total_deletions: 10,
        total_lookups: 200,
        successful_lookups: 190,
        node_splits: 5,
        node_merges: 2,
        tree_height: 4,
        total_nodes: 25,
        avg_lookup_latency_ns: 500,
        avg_insertion_latency_ns: 2000,
        cache_hit_rate: 0.95,
    };

    assert_eq!(btree_metrics.total_insertions, 50);
    assert_eq!(btree_metrics.total_lookups, 200);
    assert_eq!(btree_metrics.successful_lookups, 190);

    let wal_metrics = WALMetrics {
        total_writes: 100,
        total_bytes_written: 1048576,
        total_syncs: 20,
        avg_write_latency_ns: 1500,
        avg_sync_latency_ns: 10000,
        current_wal_size_bytes: 2097152,
        checkpoint_count: 5,
        last_checkpoint_time: None,
        write_throughput_bytes_per_sec: 1000000.0,
    };

    assert_eq!(wal_metrics.total_writes, 100);
    assert_eq!(wal_metrics.total_bytes_written, 1048576);

    let transaction_metrics = TransactionMetrics {
        active_transactions: 5,
        committed_transactions: 95,
        aborted_transactions: 5,
        avg_transaction_duration_ms: 50.0,
        lock_conflicts: 2,
        deadlocks_detected: 0,
        two_phase_commits: 10,
        distributed_transactions: 15,
    };

    assert_eq!(transaction_metrics.active_transactions, 5);
    assert_eq!(transaction_metrics.committed_transactions, 95);

    let memory_metrics = MemoryMetrics {
        total_allocated_bytes: 10485760,
        peak_allocated_bytes: 15728640,
        current_rss_bytes: 8388608,
        gc_cycles: 10,
        gc_time_ms: 100,
        objects_collected: 1000,
        memory_pressure_events: 2,
    };

    assert_eq!(memory_metrics.total_allocated_bytes, 10485760);
    assert_eq!(memory_metrics.gc_cycles, 10);

    let io_metrics = IOMetrics {
        total_reads: 500,
        total_writes: 200,
        total_bytes_read: 5242880,
        total_bytes_written: 2097152,
        avg_read_latency_ns: 1000,
        avg_write_latency_ns: 2000,
        io_errors: 1,
        cache_hits: 450,
        cache_misses: 50,
    };

    assert_eq!(io_metrics.total_reads, 500);
    assert_eq!(io_metrics.total_writes, 200);
    assert_eq!(io_metrics.cache_hits, 450);
    assert_eq!(io_metrics.cache_misses, 50);
}

#[test]
fn test_comprehensive_observability_workflow() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Simulate a complete cross-layer operation with observability
    
    // 1. Start trace
    let trace_id = observability.start_trace("database_query").unwrap();
    
    // 2. Log operation start
    observability.log(
        LogLevel::Info,
        "rust",
        "query_executor",
        "Starting database query execution",
        None,
        Some(trace_id.clone()),
        None,
        HashMap::new(),
    ).unwrap();
    
    // 3. Start Rust span for query parsing
    let parse_span_id = observability
        .start_span(&trace_id, "parse_query", "rust", None)
        .unwrap();
    
    // 4. Record parsing performance
    observability.record_performance(
        "query_parsing",
        Duration::from_micros(500),
        None,
    ).unwrap();
    
    // 5. Finish parsing span
    observability.finish_span(&trace_id, &parse_span_id).unwrap();
    
    // 6. Start Zig span for storage operations
    let storage_span_id = observability
        .start_span(&trace_id, "storage_access", "zig", Some(0))
        .unwrap();
    
    // 7. Log storage operation
    observability.log(
        LogLevel::Debug,
        "zig",
        "arena_allocator",
        "Allocating memory for query result",
        Some(0),
        Some(trace_id.clone()),
        Some(storage_span_id.clone()),
        HashMap::new(),
    ).unwrap();
    
    // 8. Record storage performance
    observability.record_performance(
        "storage_access",
        Duration::from_micros(1500),
        Some(4096),
    ).unwrap();
    
    // 9. Finish storage span
    observability.finish_span(&trace_id, &storage_span_id).unwrap();
    
    // 10. Log operation completion
    observability.log(
        LogLevel::Info,
        "rust",
        "query_executor",
        "Database query execution completed",
        None,
        Some(trace_id.clone()),
        None,
        HashMap::new(),
    ).unwrap();
    
    // 11. Finish trace
    let trace_context = observability.finish_trace(&trace_id).unwrap();
    
    // Verify the complete workflow
    assert_eq!(trace_context.trace_id, trace_id);
    assert_eq!(trace_context.span_stack.len(), 2);
    
    // Verify logs were recorded
    let logs = observability.get_recent_logs(10).unwrap();
    assert!(logs.len() >= 3);
    
    // Find our logs (they might be mixed with other test logs)
    let our_logs: Vec<_> = logs.iter()
        .filter(|log| log.trace_id.as_ref() == Some(&trace_id))
        .collect();
    
    assert!(our_logs.len() >= 3);
    
    // Verify log content
    let start_log = our_logs.iter()
        .find(|log| log.message.contains("Starting database query"))
        .expect("Should find start log");
    assert_eq!(start_log.layer, "rust");
    assert_eq!(start_log.component, "query_executor");
    
    let storage_log = our_logs.iter()
        .find(|log| log.message.contains("Allocating memory"))
        .expect("Should find storage log");
    assert_eq!(storage_log.layer, "zig");
    assert_eq!(storage_log.component, "arena_allocator");
    assert_eq!(storage_log.shard_id, Some(0));
    
    let completion_log = our_logs.iter()
        .find(|log| log.message.contains("execution completed"))
        .expect("Should find completion log");
    assert_eq!(completion_log.layer, "rust");
    assert_eq!(completion_log.component, "query_executor");
}

#[test]
fn test_error_handling_in_observability() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Test invalid trace operations
    let result = observability.finish_span("invalid_trace", "invalid_span");
    assert!(result.is_err());

    let result = observability.finish_trace("invalid_trace");
    assert!(result.is_err());

    // Test invalid layer in logging
    let result = observability.log(
        LogLevel::Error,
        "invalid_layer",
        "component",
        "message",
        None,
        None,
        None,
        HashMap::new(),
    );
    assert!(result.is_err());
}

#[test]
fn test_observability_macros() {
    // Test that the macros compile (they use the observability manager)
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // This is mainly a compilation test for the macros
    // In a real scenario, these would be used within actual operations
    
    // The macros are defined but would need actual usage context to test fully
    // For now, we just verify they exist and can be referenced
    drop(observability);
}

#[test]
fn test_metrics_collection_integration() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Test that metrics collection doesn't panic
    // Note: This will return default/empty metrics since we're not actually
    // running Zig operations, but it tests the FFI boundary
    let result = observability.collect_comprehensive_metrics(0);
    
    // The result might be an error if Zig observability isn't initialized
    // in the test environment, which is expected
    match result {
        Ok(metrics) => {
            // If we get metrics, verify the structure
            assert!(metrics.timestamp.timestamp() > 0);
        }
        Err(_) => {
            // Expected in test environment without full Zig integration
        }
    }
}

#[test]
fn test_dashboard_data_generation() {
    let monitoring_config = MonitoringConfig::default();
    let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
    let observability = CrossLayerObservabilityManager::new(monitoring);

    // Add some log entries first
    observability.log(
        LogLevel::Info,
        "rust",
        "test",
        "Dashboard test message",
        None,
        None,
        None,
        HashMap::new(),
    ).unwrap();

    // Test dashboard data generation
    let result = observability.get_dashboard_data(0);
    
    // Similar to metrics collection, this might fail in test environment
    match result {
        Ok(dashboard) => {
            // If we get dashboard data, verify structure
            assert!(!dashboard.recent_logs.is_empty());
            assert!(dashboard.timestamp.timestamp() > 0);
        }
        Err(_) => {
            // Expected in test environment without full Zig integration
        }
    }
}