// Comprehensive Observability System
// Integrates Prometheus metrics, OpenTelemetry tracing, and structured logging

use crate::error::{OQLError, OQLResult};
use crate::monitoring::{MonitoringSystem, QueryMetrics};
use lazy_static::lazy_static;
use prometheus::{Counter, CounterVec, Gauge, HistogramOpts, HistogramVec, Opts, Registry};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, span, Level, Span};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// Prometheus metrics registry.
lazy_static! {
    pub static ref METRICS_REGISTRY: Registry = Registry::new();

    // Query metrics
    pub static ref QUERY_COUNTER: CounterVec = CounterVec::new(
        Opts::new("oql_queries_total", "Total number of queries executed"),
        &["schema", "operation", "status"]
    ).unwrap();

    pub static ref QUERY_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("oql_query_duration_seconds", "Query execution duration")
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
        &["schema", "operation"]
    ).unwrap();

    pub static ref DOCUMENTS_SCANNED: HistogramVec = HistogramVec::new(
        HistogramOpts::new("oql_documents_scanned", "Number of documents scanned per query")
            .buckets(vec![1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0]),
        &["schema"]
    ).unwrap();

    // Storage metrics (from Zig layer)
    pub static ref STORAGE_ALLOCATIONS: Counter = Counter::new(
        "oql_storage_allocations_total",
        "Total number of storage allocations"
    ).unwrap();

    pub static ref STORAGE_BYTES_ALLOCATED: Counter = Counter::new(
        "oql_storage_bytes_allocated_total",
        "Total bytes allocated in storage"
    ).unwrap();

    pub static ref STORAGE_NAVIGATIONS: CounterVec = CounterVec::new(
        Opts::new("oql_storage_navigations_total", "Total number of navigations"),
        &["type"]
    ).unwrap();

    pub static ref NAVIGATION_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("oql_navigation_duration_nanoseconds", "Navigation duration in nanoseconds")
            .buckets(vec![10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0]),
        &["type"]
    ).unwrap();

    pub static ref STORAGE_IO_OPS: CounterVec = CounterVec::new(
        Opts::new("oql_storage_io_operations_total", "Total I/O operations"),
        &["operation"]
    ).unwrap();

    pub static ref STORAGE_IO_BYTES: CounterVec = CounterVec::new(
        Opts::new("oql_storage_io_bytes_total", "Total I/O bytes"),
        &["operation"]
    ).unwrap();

    pub static ref WAL_OPERATIONS: CounterVec = CounterVec::new(
        Opts::new("oql_wal_operations_total", "Total WAL operations"),
        &["operation"]
    ).unwrap();

    pub static ref BTREE_OPERATIONS: CounterVec = CounterVec::new(
        Opts::new("oql_btree_operations_total", "Total B-tree operations"),
        &["operation"]
    ).unwrap();

    pub static ref COMPRESSION_OPERATIONS: CounterVec = CounterVec::new(
        Opts::new("oql_compression_operations_total", "Total compression operations"),
        &["operation"]
    ).unwrap();

    pub static ref COMPRESSION_BYTES_SAVED: Counter = Counter::new(
        "oql_compression_bytes_saved_total",
        "Total bytes saved by compression"
    ).unwrap();

    // System metrics
    pub static ref ARENA_SIZE: Gauge = Gauge::new(
        "oql_arena_size_bytes",
        "Current arena size in bytes"
    ).unwrap();

    pub static ref ARENA_USED: Gauge = Gauge::new(
        "oql_arena_used_bytes",
        "Current arena used bytes"
    ).unwrap();

    pub static ref ARENA_FRAGMENTATION: Gauge = Gauge::new(
        "oql_arena_fragmentation_percent",
        "Arena fragmentation percentage"
    ).unwrap();

    pub static ref CACHE_OPERATIONS: CounterVec = CounterVec::new(
        Opts::new("oql_cache_operations_total", "Total cache operations"),
        &["result"]
    ).unwrap();

    pub static ref CACHE_HIT_RATE: Gauge = Gauge::new(
        "oql_cache_hit_rate",
        "Cache hit rate"
    ).unwrap();

    pub static ref ERROR_COUNTER: CounterVec = CounterVec::new(
        Opts::new("oql_errors_total", "Total errors"),
        &["type"]
    ).unwrap();
}

/// Initialize Prometheus metrics
pub fn init_prometheus_metrics() -> OQLResult<()> {
    METRICS_REGISTRY
        .register(Box::new(QUERY_COUNTER.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register query counter: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(QUERY_DURATION.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register query duration: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(DOCUMENTS_SCANNED.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register documents scanned: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(STORAGE_ALLOCATIONS.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register storage allocations: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(STORAGE_BYTES_ALLOCATED.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register storage bytes allocated: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(STORAGE_NAVIGATIONS.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register storage navigations: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(NAVIGATION_DURATION.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register navigation duration: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(STORAGE_IO_OPS.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register storage I/O ops: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(STORAGE_IO_BYTES.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register storage I/O bytes: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(WAL_OPERATIONS.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register WAL operations: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(BTREE_OPERATIONS.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register B-tree operations: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(COMPRESSION_OPERATIONS.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register compression operations: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(COMPRESSION_BYTES_SAVED.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register compression bytes saved: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(ARENA_SIZE.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register arena size: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(ARENA_USED.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register arena used: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(ARENA_FRAGMENTATION.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register arena fragmentation: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(CACHE_OPERATIONS.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register cache operations: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(CACHE_HIT_RATE.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register cache hit rate: {}", e),
        })?;

    METRICS_REGISTRY
        .register(Box::new(ERROR_COUNTER.clone()))
        .map_err(|e| OQLError::Generic {
            message: format!("Failed to register error counter: {}", e),
        })?;

    Ok(())
}

/// Initialize tracing with OpenTelemetry
pub fn init_tracing(service_name: &str) -> OQLResult<()> {
    // Set up tracing subscriber with JSON formatting
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!(service = service_name, "Tracing initialized");

    Ok(())
}

/// Observability system that integrates metrics, tracing, and logging
#[allow(dead_code)]
pub struct ObservabilitySystem {
    monitoring: Arc<MonitoringSystem>,
    service_name: String,
}

impl ObservabilitySystem {
    /// Create a new observability system
    pub fn new(service_name: String, monitoring: Arc<MonitoringSystem>) -> OQLResult<Self> {
        init_prometheus_metrics()?;
        init_tracing(&service_name)?;

        // Initialize Zig metrics
        unsafe {
            owl_storage_zig::owl_metrics_init();
        }

        Ok(Self {
            monitoring,
            service_name,
        })
    }

    /// Record query metrics
    pub fn record_query(&self, metrics: &QueryMetrics) {
        let status = if metrics.success { "success" } else { "error" };

        QUERY_COUNTER
            .with_label_values(&[&metrics.schema, &metrics.operation_type, status])
            .inc();

        QUERY_DURATION
            .with_label_values(&[&metrics.schema, &metrics.operation_type])
            .observe(metrics.execution_time_ms as f64 / 1000.0);

        DOCUMENTS_SCANNED
            .with_label_values(&[&metrics.schema])
            .observe(metrics.documents_scanned as f64);

        // Also record in monitoring system
        let _ = self.monitoring.record_query_metrics(metrics.clone());
    }

    /// Update storage metrics from Zig layer
    pub fn update_storage_metrics(&self) {
        unsafe {
            let mut snapshot = std::mem::MaybeUninit::uninit();
            owl_storage_zig::owl_metrics_get_snapshot(snapshot.as_mut_ptr());
            let snapshot = snapshot.assume_init();

            // Update Prometheus metrics
            STORAGE_ALLOCATIONS.inc_by(snapshot.allocations_total as f64);
            STORAGE_BYTES_ALLOCATED.inc_by(snapshot.bytes_allocated as f64);

            STORAGE_NAVIGATIONS
                .with_label_values(&["local"])
                .inc_by(snapshot.navigations_local as f64);
            STORAGE_NAVIGATIONS
                .with_label_values(&["remote"])
                .inc_by(snapshot.navigations_remote as f64);

            STORAGE_IO_OPS
                .with_label_values(&["read"])
                .inc_by(snapshot.reads_total as f64);
            STORAGE_IO_OPS
                .with_label_values(&["write"])
                .inc_by(snapshot.writes_total as f64);
            STORAGE_IO_OPS
                .with_label_values(&["sync"])
                .inc_by(snapshot.syncs_total as f64);

            STORAGE_IO_BYTES
                .with_label_values(&["read"])
                .inc_by(snapshot.bytes_read as f64);
            STORAGE_IO_BYTES
                .with_label_values(&["write"])
                .inc_by(snapshot.bytes_written as f64);

            WAL_OPERATIONS
                .with_label_values(&["append"])
                .inc_by(snapshot.wal_appends as f64);
            WAL_OPERATIONS
                .with_label_values(&["sync"])
                .inc_by(snapshot.wal_syncs as f64);

            BTREE_OPERATIONS
                .with_label_values(&["insert"])
                .inc_by(snapshot.btree_inserts as f64);
            BTREE_OPERATIONS
                .with_label_values(&["delete"])
                .inc_by(snapshot.btree_deletes as f64);
            BTREE_OPERATIONS
                .with_label_values(&["lookup"])
                .inc_by(snapshot.btree_lookups as f64);

            COMPRESSION_OPERATIONS
                .with_label_values(&["compress"])
                .inc_by(snapshot.compressions_total as f64);
            COMPRESSION_OPERATIONS
                .with_label_values(&["decompress"])
                .inc_by(snapshot.decompressions_total as f64);

            COMPRESSION_BYTES_SAVED.inc_by(snapshot.compression_bytes_saved as f64);

            ARENA_SIZE.set(snapshot.arena_size_bytes as f64);
            ARENA_USED.set(snapshot.arena_used_bytes as f64);
            ARENA_FRAGMENTATION.set(snapshot.arena_fragmentation_percent as f64);

            CACHE_OPERATIONS
                .with_label_values(&["hit"])
                .inc_by(snapshot.cache_hits as f64);
            CACHE_OPERATIONS
                .with_label_values(&["miss"])
                .inc_by(snapshot.cache_misses as f64);

            let total_cache = snapshot.cache_hits + snapshot.cache_misses;
            if total_cache > 0 {
                let hit_rate = snapshot.cache_hits as f64 / total_cache as f64;
                CACHE_HIT_RATE.set(hit_rate);
            }

            ERROR_COUNTER
                .with_label_values(&["storage"])
                .inc_by(snapshot.errors_total as f64);
        }
    }

    /// Export Prometheus metrics
    pub fn export_prometheus(&self) -> OQLResult<String> {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();
        let metric_families = METRICS_REGISTRY.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .map_err(|e| OQLError::Generic {
                message: format!("Failed to encode metrics: {}", e),
            })?;

        String::from_utf8(buffer).map_err(|e| OQLError::Generic {
            message: format!("Failed to convert metrics to string: {}", e),
        })
    }

    /// Create a traced span for an operation
    pub fn trace_operation(&self, operation: &str) -> TracedOperation {
        let span = span!(Level::INFO, "operation", operation = operation);

        TracedOperation {
            span,
            start: Instant::now(),
            operation: operation.to_string(),
        }
    }
}

/// A traced operation that automatically records duration
pub struct TracedOperation {
    span: Span,
    start: Instant,
    operation: String,
}

impl TracedOperation {
    /// Finish the operation and record metrics
    pub fn finish(self) {
        let duration = self.start.elapsed();
        debug!(
            operation = %self.operation,
            duration_ms = duration.as_millis(),
            "Operation completed"
        );
    }

    /// Add a field to the span
    pub fn record(&self, field: &str, value: &dyn std::fmt::Display) {
        self.span.record(field, &tracing::field::display(value));
    }
}

impl Drop for TracedOperation {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        debug!(
            operation = %self.operation,
            duration_ms = duration.as_millis(),
            "Operation completed (auto)"
        );
    }
}

// FFI declarations for Zig metrics
mod owl_storage_zig {

    #[repr(C)]
    pub struct MetricsSnapshot {
        pub allocations_total: u64,
        pub allocations_failed: u64,
        pub bytes_allocated: u64,
        pub bytes_freed: u64,

        pub navigations_local: u64,
        pub navigations_remote: u64,
        pub navigation_time_ns_total: u64,

        pub reads_total: u64,
        pub writes_total: u64,
        pub syncs_total: u64,
        pub bytes_read: u64,
        pub bytes_written: u64,
        pub io_time_ns_total: u64,

        pub wal_appends: u64,
        pub wal_syncs: u64,
        pub wal_bytes_written: u64,
        pub wal_replay_count: u64,

        pub btree_inserts: u64,
        pub btree_deletes: u64,
        pub btree_lookups: u64,
        pub btree_splits: u64,
        pub btree_merges: u64,

        pub compressions_total: u64,
        pub decompressions_total: u64,
        pub compression_bytes_saved: u64,
        pub compression_time_ns_total: u64,

        pub arena_size_bytes: u64,
        pub arena_used_bytes: u64,
        pub arena_fragmentation_percent: u64,

        pub cache_hits: u64,
        pub cache_misses: u64,
        pub cache_evictions: u64,

        pub errors_total: u64,
        pub corruption_detected: u64,
        pub recovery_attempts: u64,
    }

    extern "C" {
        pub fn owl_metrics_init();
        pub fn owl_metrics_get_snapshot(out: *mut MetricsSnapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_metrics_init() {
        let result = init_prometheus_metrics();
        assert!(result.is_ok());
    }

    #[test]
    fn test_tracing_init() {
        let result = init_tracing("test_service");
        assert!(result.is_ok());
    }

    #[test]
    fn test_traced_operation() {
        let _ = init_tracing("test");

        let op = TracedOperation {
            span: span!(Level::INFO, "test_op"),
            start: Instant::now(),
            operation: "test".to_string(),
        };

        std::thread::sleep(std::time::Duration::from_millis(10));
        op.finish();
    }
}
