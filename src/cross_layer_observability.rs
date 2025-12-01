//! Cross-Layer Observability (Rust + Zig)
//!
//! Task 8.15: Implement cross-layer observability (Rust + Zig)
//!
//! This module provides comprehensive observability across the Rust-Zig boundary,
//! including metrics collection, distributed tracing, performance monitoring,
//! and unified logging for both Rust and Zig components.

use crate::error::{ErrorContext, OQLError, OQLResult};
use crate::monitoring::{MonitoringSystem, TraceSpan, QueryMetrics, SystemMetrics};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_ulong};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Cross-layer observability manager
pub struct CrossLayerObservabilityManager {
    /// Rust monitoring system
    monitoring: Arc<MonitoringSystem>,
    /// Zig metrics collector
    zig_metrics: Arc<Mutex<ZigMetricsCollector>>,
    /// Trace context manager
    trace_manager: Arc<RwLock<TraceContextManager>>,
    /// Performance monitor
    performance_monitor: Arc<Mutex<PerformanceMonitor>>,
    /// Unified logger
    unified_logger: Arc<Mutex<UnifiedLogger>>,
}

/// Zig-level metrics collected from storage operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZigStorageMetrics {
    /// Arena allocation metrics
    pub arena_metrics: ArenaMetrics,
    /// B-tree operation metrics
    pub btree_metrics: BTreeMetrics,
    /// WAL operation metrics
    pub wal_metrics: WALMetrics,
    /// Transaction metrics
    pub transaction_metrics: TransactionMetrics,
    /// Memory usage metrics
    pub memory_metrics: MemoryMetrics,
    /// I/O operation metrics
    pub io_metrics: IOMetrics,
    /// Timestamp when metrics were collected
    pub timestamp: DateTime<Utc>,
}

/// Arena allocation metrics from Zig layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArenaMetrics {
    pub total_allocations: u64,
    pub successful_allocations: u64,
    pub failed_allocations: u64,
    pub total_bytes_allocated: u64,
    pub total_bytes_freed: u64,
    pub current_usage_bytes: u64,
    pub peak_usage_bytes: u64,
    pub fragmentation_ratio: f64,
    pub avg_allocation_size: f64,
    pub allocation_latency_ns: u64,
    pub colocation_success_rate: f64,
}

/// B-tree operation metrics from Zig layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BTreeMetrics {
    pub total_insertions: u64,
    pub total_deletions: u64,
    pub total_lookups: u64,
    pub successful_lookups: u64,
    pub node_splits: u64,
    pub node_merges: u64,
    pub tree_height: u32,
    pub total_nodes: u64,
    pub avg_lookup_latency_ns: u64,
    pub avg_insertion_latency_ns: u64,
    pub cache_hit_rate: f64,
}

/// WAL operation metrics from Zig layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WALMetrics {
    pub total_writes: u64,
    pub total_bytes_written: u64,
    pub total_syncs: u64,
    pub avg_write_latency_ns: u64,
    pub avg_sync_latency_ns: u64,
    pub current_wal_size_bytes: u64,
    pub checkpoint_count: u64,
    pub last_checkpoint_time: Option<DateTime<Utc>>,
    pub write_throughput_bytes_per_sec: f64,
}

/// Transaction metrics from Zig layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionMetrics {
    pub active_transactions: u64,
    pub committed_transactions: u64,
    pub aborted_transactions: u64,
    pub avg_transaction_duration_ms: f64,
    pub lock_conflicts: u64,
    pub deadlocks_detected: u64,
    pub two_phase_commits: u64,
    pub distributed_transactions: u64,
}

/// Memory usage metrics from Zig layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_allocated_bytes: u64,
    pub peak_allocated_bytes: u64,
    pub current_rss_bytes: u64,
    pub gc_cycles: u64,
    pub gc_time_ms: u64,
    pub objects_collected: u64,
    pub memory_pressure_events: u64,
}

/// I/O operation metrics from Zig layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IOMetrics {
    pub total_reads: u64,
    pub total_writes: u64,
    pub total_bytes_read: u64,
    pub total_bytes_written: u64,
    pub avg_read_latency_ns: u64,
    pub avg_write_latency_ns: u64,
    pub io_errors: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

/// Zig metrics collector
pub struct ZigMetricsCollector {
    /// Last collected metrics
    last_metrics: Option<ZigStorageMetrics>,
    /// Collection interval
    collection_interval: Duration,
    /// Last collection time
    last_collection: Instant,
}

impl ZigMetricsCollector {
    pub fn new(collection_interval: Duration) -> Self {
        Self {
            last_metrics: None,
            collection_interval,
            last_collection: Instant::now(),
        }
    }

    /// Collect metrics from Zig layer
    pub fn collect_metrics(&mut self, shard_id: u32) -> OQLResult<ZigStorageMetrics> {
        let now = Instant::now();
        
        // Check if it's time to collect new metrics
        if now.duration_since(self.last_collection) < self.collection_interval {
            if let Some(ref metrics) = self.last_metrics {
                return Ok(metrics.clone());
            }
        }

        // Collect fresh metrics from Zig layer
        let metrics = self.collect_from_zig(shard_id)?;
        self.last_metrics = Some(metrics.clone());
        self.last_collection = now;

        Ok(metrics)
    }

    /// Collect metrics directly from Zig FFI
    fn collect_from_zig(&self, shard_id: u32) -> OQLResult<ZigStorageMetrics> {
        let mut arena_metrics = ArenaMetrics {
            total_allocations: 0,
            successful_allocations: 0,
            failed_allocations: 0,
            total_bytes_allocated: 0,
            total_bytes_freed: 0,
            current_usage_bytes: 0,
            peak_usage_bytes: 0,
            fragmentation_ratio: 0.0,
            avg_allocation_size: 0.0,
            allocation_latency_ns: 0,
            colocation_success_rate: 0.0,
        };

        let mut btree_metrics = BTreeMetrics {
            total_insertions: 0,
            total_deletions: 0,
            total_lookups: 0,
            successful_lookups: 0,
            node_splits: 0,
            node_merges: 0,
            tree_height: 0,
            total_nodes: 0,
            avg_lookup_latency_ns: 0,
            avg_insertion_latency_ns: 0,
            cache_hit_rate: 0.0,
        };

        let mut wal_metrics = WALMetrics {
            total_writes: 0,
            total_bytes_written: 0,
            total_syncs: 0,
            avg_write_latency_ns: 0,
            avg_sync_latency_ns: 0,
            current_wal_size_bytes: 0,
            checkpoint_count: 0,
            last_checkpoint_time: None,
            write_throughput_bytes_per_sec: 0.0,
        };

        let mut transaction_metrics = TransactionMetrics {
            active_transactions: 0,
            committed_transactions: 0,
            aborted_transactions: 0,
            avg_transaction_duration_ms: 0.0,
            lock_conflicts: 0,
            deadlocks_detected: 0,
            two_phase_commits: 0,
            distributed_transactions: 0,
        };

        let mut memory_metrics = MemoryMetrics {
            total_allocated_bytes: 0,
            peak_allocated_bytes: 0,
            current_rss_bytes: 0,
            gc_cycles: 0,
            gc_time_ms: 0,
            objects_collected: 0,
            memory_pressure_events: 0,
        };

        let mut io_metrics = IOMetrics {
            total_reads: 0,
            total_writes: 0,
            total_bytes_read: 0,
            total_bytes_written: 0,
            avg_read_latency_ns: 0,
            avg_write_latency_ns: 0,
            io_errors: 0,
            cache_hits: 0,
            cache_misses: 0,
        };

        // Collect arena metrics
        unsafe {
            zig_collect_arena_metrics(
                shard_id,
                &mut arena_metrics.total_allocations,
                &mut arena_metrics.successful_allocations,
                &mut arena_metrics.failed_allocations,
                &mut arena_metrics.total_bytes_allocated,
                &mut arena_metrics.total_bytes_freed,
                &mut arena_metrics.current_usage_bytes,
                &mut arena_metrics.peak_usage_bytes,
                &mut arena_metrics.fragmentation_ratio,
                &mut arena_metrics.avg_allocation_size,
                &mut arena_metrics.allocation_latency_ns,
                &mut arena_metrics.colocation_success_rate,
            );
        }

        // Collect B-tree metrics
        unsafe {
            zig_collect_btree_metrics(
                shard_id,
                &mut btree_metrics.total_insertions,
                &mut btree_metrics.total_deletions,
                &mut btree_metrics.total_lookups,
                &mut btree_metrics.successful_lookups,
                &mut btree_metrics.node_splits,
                &mut btree_metrics.node_merges,
                &mut btree_metrics.tree_height,
                &mut btree_metrics.total_nodes,
                &mut btree_metrics.avg_lookup_latency_ns,
                &mut btree_metrics.avg_insertion_latency_ns,
                &mut btree_metrics.cache_hit_rate,
            );
        }

        // Collect WAL metrics
        unsafe {
            let mut last_checkpoint_timestamp: u64 = 0;
            let mut has_checkpoint: bool = false;

            zig_collect_wal_metrics(
                shard_id,
                &mut wal_metrics.total_writes,
                &mut wal_metrics.total_bytes_written,
                &mut wal_metrics.total_syncs,
                &mut wal_metrics.avg_write_latency_ns,
                &mut wal_metrics.avg_sync_latency_ns,
                &mut wal_metrics.current_wal_size_bytes,
                &mut wal_metrics.checkpoint_count,
                &mut last_checkpoint_timestamp,
                &mut has_checkpoint,
                &mut wal_metrics.write_throughput_bytes_per_sec,
            );

            if has_checkpoint {
                wal_metrics.last_checkpoint_time = Some(
                    DateTime::from_timestamp_nanos(last_checkpoint_timestamp as i64)
                );
            }
        }

        // Collect transaction metrics
        unsafe {
            zig_collect_transaction_metrics(
                shard_id,
                &mut transaction_metrics.active_transactions,
                &mut transaction_metrics.committed_transactions,
                &mut transaction_metrics.aborted_transactions,
                &mut transaction_metrics.avg_transaction_duration_ms,
                &mut transaction_metrics.lock_conflicts,
                &mut transaction_metrics.deadlocks_detected,
                &mut transaction_metrics.two_phase_commits,
                &mut transaction_metrics.distributed_transactions,
            );
        }

        // Collect memory metrics
        unsafe {
            zig_collect_memory_metrics(
                shard_id,
                &mut memory_metrics.total_allocated_bytes,
                &mut memory_metrics.peak_allocated_bytes,
                &mut memory_metrics.current_rss_bytes,
                &mut memory_metrics.gc_cycles,
                &mut memory_metrics.gc_time_ms,
                &mut memory_metrics.objects_collected,
                &mut memory_metrics.memory_pressure_events,
            );
        }

        // Collect I/O metrics
        unsafe {
            zig_collect_io_metrics(
                shard_id,
                &mut io_metrics.total_reads,
                &mut io_metrics.total_writes,
                &mut io_metrics.total_bytes_read,
                &mut io_metrics.total_bytes_written,
                &mut io_metrics.avg_read_latency_ns,
                &mut io_metrics.avg_write_latency_ns,
                &mut io_metrics.io_errors,
                &mut io_metrics.cache_hits,
                &mut io_metrics.cache_misses,
            );
        }

        Ok(ZigStorageMetrics {
            arena_metrics,
            btree_metrics,
            wal_metrics,
            transaction_metrics,
            memory_metrics,
            io_metrics,
            timestamp: Utc::now(),
        })
    }
}

/// Trace context manager for cross-layer tracing
pub struct TraceContextManager {
    /// Active trace contexts
    active_traces: HashMap<String, CrossLayerTraceContext>,
    /// Trace ID counter
    trace_id_counter: u64,
}

/// Cross-layer trace context
#[derive(Debug, Clone)]
pub struct CrossLayerTraceContext {
    /// Trace ID
    pub trace_id: String,
    /// Parent span ID (if any)
    pub parent_span_id: Option<String>,
    /// Current span stack
    pub span_stack: Vec<CrossLayerSpan>,
    /// Trace start time
    pub start_time: Instant,
    /// Rust spans
    pub rust_spans: Vec<TraceSpan>,
    /// Zig spans
    pub zig_spans: Vec<ZigTraceSpan>,
}

/// Cross-layer span information
#[derive(Debug, Clone)]
pub struct CrossLayerSpan {
    /// Span ID
    pub span_id: String,
    /// Operation name
    pub operation_name: String,
    /// Layer (rust or zig)
    pub layer: String,
    /// Start time
    pub start_time: Instant,
    /// End time (if finished)
    pub end_time: Option<Instant>,
    /// Tags
    pub tags: HashMap<String, String>,
    /// Shard ID (if applicable)
    pub shard_id: Option<u32>,
}

/// Zig trace span
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZigTraceSpan {
    pub span_id: String,
    pub operation_name: String,
    pub start_time_ns: u64,
    pub end_time_ns: Option<u64>,
    pub duration_ns: Option<u64>,
    pub shard_id: u32,
    pub tags: HashMap<String, String>,
}

impl TraceContextManager {
    pub fn new() -> Self {
        Self {
            active_traces: HashMap::new(),
            trace_id_counter: 0,
        }
    }

    /// Start a new cross-layer trace
    pub fn start_trace(&mut self, operation_name: &str) -> String {
        self.trace_id_counter += 1;
        let trace_id = format!("trace_{}", self.trace_id_counter);

        let context = CrossLayerTraceContext {
            trace_id: trace_id.clone(),
            parent_span_id: None,
            span_stack: Vec::new(),
            start_time: Instant::now(),
            rust_spans: Vec::new(),
            zig_spans: Vec::new(),
        };

        self.active_traces.insert(trace_id.clone(), context);

        // Initialize trace in Zig layer
        unsafe {
            let trace_id_cstr = CString::new(trace_id.clone()).unwrap();
            let operation_cstr = CString::new(operation_name).unwrap();
            zig_start_trace(trace_id_cstr.as_ptr(), operation_cstr.as_ptr());
        }

        trace_id
    }

    /// Start a span within a trace
    pub fn start_span(
        &mut self,
        trace_id: &str,
        operation_name: &str,
        layer: &str,
        shard_id: Option<u32>,
    ) -> OQLResult<String> {
        let span_id = format!("span_{}_{}", trace_id, uuid::Uuid::new_v4());

        if let Some(context) = self.active_traces.get_mut(trace_id) {
            let span = CrossLayerSpan {
                span_id: span_id.clone(),
                operation_name: operation_name.to_string(),
                layer: layer.to_string(),
                start_time: Instant::now(),
                end_time: None,
                tags: HashMap::new(),
                shard_id,
            };

            context.span_stack.push(span);

            // Start span in Zig layer if applicable
            if layer == "zig" {
                unsafe {
                    let trace_id_cstr = CString::new(trace_id).unwrap();
                    let span_id_cstr = CString::new(span_id.clone()).unwrap();
                    let operation_cstr = CString::new(operation_name).unwrap();
                    zig_start_span(
                        trace_id_cstr.as_ptr(),
                        span_id_cstr.as_ptr(),
                        operation_cstr.as_ptr(),
                        shard_id.unwrap_or(0),
                    );
                }
            }

            Ok(span_id)
        } else {
            Err(OQLError::Generic {
                message: format!("Trace not found: {}", trace_id),
            })
        }
    }

    /// Finish a span
    pub fn finish_span(&mut self, trace_id: &str, span_id: &str) -> OQLResult<()> {
        if let Some(context) = self.active_traces.get_mut(trace_id) {
            // Find and finish the span
            for span in &mut context.span_stack {
                if span.span_id == span_id {
                    span.end_time = Some(Instant::now());

                    // Finish span in Zig layer if applicable
                    if span.layer == "zig" {
                        unsafe {
                            let trace_id_cstr = CString::new(trace_id).unwrap();
                            let span_id_cstr = CString::new(span_id).unwrap();
                            zig_finish_span(trace_id_cstr.as_ptr(), span_id_cstr.as_ptr());
                        }
                    }

                    return Ok(());
                }
            }

            Err(OQLError::Generic {
                message: format!("Span not found: {}", span_id),
            })
        } else {
            Err(OQLError::Generic {
                message: format!("Trace not found: {}", trace_id),
            })
        }
    }

    /// Finish a trace and collect all spans
    pub fn finish_trace(&mut self, trace_id: &str) -> OQLResult<CrossLayerTraceContext> {
        if let Some(mut context) = self.active_traces.remove(trace_id) {
            // Collect Zig spans
            context.zig_spans = self.collect_zig_spans(trace_id)?;

            // Finish trace in Zig layer
            unsafe {
                let trace_id_cstr = CString::new(trace_id).unwrap();
                zig_finish_trace(trace_id_cstr.as_ptr());
            }

            Ok(context)
        } else {
            Err(OQLError::Generic {
                message: format!("Trace not found: {}", trace_id),
            })
        }
    }

    /// Collect Zig spans for a trace
    fn collect_zig_spans(&self, trace_id: &str) -> OQLResult<Vec<ZigTraceSpan>> {
        let mut spans = Vec::new();
        let mut span_count: u32 = 0;

        // Get span count
        unsafe {
            let trace_id_cstr = CString::new(trace_id).unwrap();
            span_count = zig_get_span_count(trace_id_cstr.as_ptr());
        }

        // Collect each span
        for i in 0..span_count {
            let mut span_id_buffer = vec![0u8; 256];
            let mut operation_buffer = vec![0u8; 256];
            let mut start_time_ns: u64 = 0;
            let mut end_time_ns: u64 = 0;
            let mut has_end_time: bool = false;
            let mut shard_id: u32 = 0;
            let mut tag_count: u32 = 0;

            unsafe {
                let trace_id_cstr = CString::new(trace_id).unwrap();
                zig_get_span_info(
                    trace_id_cstr.as_ptr(),
                    i,
                    span_id_buffer.as_mut_ptr() as *mut c_char,
                    span_id_buffer.len(),
                    operation_buffer.as_mut_ptr() as *mut c_char,
                    operation_buffer.len(),
                    &mut start_time_ns,
                    &mut end_time_ns,
                    &mut has_end_time,
                    &mut shard_id,
                    &mut tag_count,
                );
            }

            let span_id = unsafe {
                CStr::from_ptr(span_id_buffer.as_ptr() as *const c_char)
                    .to_string_lossy()
                    .into_owned()
            };

            let operation_name = unsafe {
                CStr::from_ptr(operation_buffer.as_ptr() as *const c_char)
                    .to_string_lossy()
                    .into_owned()
            };

            // Collect tags
            let mut tags = HashMap::new();
            for tag_idx in 0..tag_count {
                let mut key_buffer = vec![0u8; 128];
                let mut value_buffer = vec![0u8; 256];

                unsafe {
                    let trace_id_cstr = CString::new(trace_id).unwrap();
                    let span_id_cstr = CString::new(&span_id).unwrap();
                    zig_get_span_tag(
                        trace_id_cstr.as_ptr(),
                        span_id_cstr.as_ptr(),
                        tag_idx,
                        key_buffer.as_mut_ptr() as *mut c_char,
                        key_buffer.len(),
                        value_buffer.as_mut_ptr() as *mut c_char,
                        value_buffer.len(),
                    );
                }

                let key = unsafe {
                    CStr::from_ptr(key_buffer.as_ptr() as *const c_char)
                        .to_string_lossy()
                        .into_owned()
                };

                let value = unsafe {
                    CStr::from_ptr(value_buffer.as_ptr() as *const c_char)
                        .to_string_lossy()
                        .into_owned()
                };

                tags.insert(key, value);
            }

            let span = ZigTraceSpan {
                span_id,
                operation_name,
                start_time_ns,
                end_time_ns: if has_end_time { Some(end_time_ns) } else { None },
                duration_ns: if has_end_time {
                    Some(end_time_ns - start_time_ns)
                } else {
                    None
                },
                shard_id,
                tags,
            };

            spans.push(span);
        }

        Ok(spans)
    }
}

/// Performance monitor for cross-layer operations
pub struct PerformanceMonitor {
    /// Operation timings
    operation_timings: HashMap<String, Vec<Duration>>,
    /// Latency percentiles
    latency_percentiles: HashMap<String, LatencyPercentiles>,
    /// Throughput measurements
    throughput_measurements: HashMap<String, ThroughputMeasurement>,
}

/// Latency percentiles for an operation
#[derive(Debug, Clone)]
pub struct LatencyPercentiles {
    pub p50: Duration,
    pub p90: Duration,
    pub p95: Duration,
    pub p99: Duration,
    pub max: Duration,
    pub min: Duration,
    pub avg: Duration,
}

/// Throughput measurement
#[derive(Debug, Clone)]
pub struct ThroughputMeasurement {
    pub operations_per_second: f64,
    pub bytes_per_second: f64,
    pub last_measurement_time: Instant,
    pub total_operations: u64,
    pub total_bytes: u64,
}

impl PerformanceMonitor {
    pub fn new() -> Self {
        Self {
            operation_timings: HashMap::new(),
            latency_percentiles: HashMap::new(),
            throughput_measurements: HashMap::new(),
        }
    }

    /// Record operation timing
    pub fn record_timing(&mut self, operation: &str, duration: Duration) {
        self.operation_timings
            .entry(operation.to_string())
            .or_insert_with(Vec::new)
            .push(duration);

        // Keep only last 1000 measurements to avoid memory growth
        let timings = self.operation_timings.get_mut(operation).unwrap();
        if timings.len() > 1000 {
            timings.drain(0..timings.len() - 1000);
        }

        // Update percentiles
        self.update_percentiles(operation);
    }

    /// Update latency percentiles for an operation
    fn update_percentiles(&mut self, operation: &str) {
        if let Some(timings) = self.operation_timings.get(operation) {
            if timings.is_empty() {
                return;
            }

            let mut sorted_timings = timings.clone();
            sorted_timings.sort();

            let len = sorted_timings.len();
            let p50_idx = (len as f64 * 0.50) as usize;
            let p90_idx = (len as f64 * 0.90) as usize;
            let p95_idx = (len as f64 * 0.95) as usize;
            let p99_idx = (len as f64 * 0.99) as usize;

            let sum: Duration = sorted_timings.iter().sum();
            let avg = sum / len as u32;

            let percentiles = LatencyPercentiles {
                p50: sorted_timings[p50_idx.min(len - 1)],
                p90: sorted_timings[p90_idx.min(len - 1)],
                p95: sorted_timings[p95_idx.min(len - 1)],
                p99: sorted_timings[p99_idx.min(len - 1)],
                max: sorted_timings[len - 1],
                min: sorted_timings[0],
                avg,
            };

            self.latency_percentiles.insert(operation.to_string(), percentiles);
        }
    }

    /// Record throughput measurement
    pub fn record_throughput(&mut self, operation: &str, operations: u64, bytes: u64) {
        let now = Instant::now();

        if let Some(measurement) = self.throughput_measurements.get_mut(operation) {
            let elapsed = now.duration_since(measurement.last_measurement_time);
            if elapsed.as_secs_f64() >= 1.0 {
                // Update measurements
                measurement.operations_per_second = operations as f64 / elapsed.as_secs_f64();
                measurement.bytes_per_second = bytes as f64 / elapsed.as_secs_f64();
                measurement.last_measurement_time = now;
                measurement.total_operations += operations;
                measurement.total_bytes += bytes;
            }
        } else {
            let measurement = ThroughputMeasurement {
                operations_per_second: 0.0,
                bytes_per_second: 0.0,
                last_measurement_time: now,
                total_operations: operations,
                total_bytes: bytes,
            };
            self.throughput_measurements.insert(operation.to_string(), measurement);
        }
    }

    /// Get performance summary
    pub fn get_performance_summary(&self) -> PerformanceSummary {
        PerformanceSummary {
            latency_percentiles: self.latency_percentiles.clone(),
            throughput_measurements: self.throughput_measurements.clone(),
            timestamp: Utc::now(),
        }
    }
}

/// Performance summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSummary {
    pub latency_percentiles: HashMap<String, LatencyPercentiles>,
    pub throughput_measurements: HashMap<String, ThroughputMeasurement>,
    pub timestamp: DateTime<Utc>,
}

/// Unified logger for both Rust and Zig components
pub struct UnifiedLogger {
    /// Log entries
    log_entries: Vec<UnifiedLogEntry>,
    /// Maximum log entries to keep
    max_entries: usize,
}

/// Unified log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedLogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub layer: String, // "rust" or "zig"
    pub component: String,
    pub message: String,
    pub shard_id: Option<u32>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub metadata: HashMap<String, String>,
}

/// Log level
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl UnifiedLogger {
    pub fn new(max_entries: usize) -> Self {
        Self {
            log_entries: Vec::new(),
            max_entries,
        }
    }

    /// Log a message from Rust layer
    pub fn log_rust(
        &mut self,
        level: LogLevel,
        component: &str,
        message: &str,
        trace_id: Option<String>,
        span_id: Option<String>,
        metadata: HashMap<String, String>,
    ) {
        let entry = UnifiedLogEntry {
            timestamp: Utc::now(),
            level,
            layer: "rust".to_string(),
            component: component.to_string(),
            message: message.to_string(),
            shard_id: None,
            trace_id,
            span_id,
            metadata,
        };

        self.add_entry(entry);
    }

    /// Log a message from Zig layer
    pub fn log_zig(
        &mut self,
        level: LogLevel,
        component: &str,
        message: &str,
        shard_id: u32,
        trace_id: Option<String>,
        span_id: Option<String>,
        metadata: HashMap<String, String>,
    ) {
        let entry = UnifiedLogEntry {
            timestamp: Utc::now(),
            level,
            layer: "zig".to_string(),
            component: component.to_string(),
            message: message.to_string(),
            shard_id: Some(shard_id),
            trace_id,
            span_id,
            metadata,
        };

        self.add_entry(entry);
    }

    /// Add log entry and maintain size limit
    fn add_entry(&mut self, entry: UnifiedLogEntry) {
        self.log_entries.push(entry);

        // Maintain size limit
        if self.log_entries.len() > self.max_entries {
            self.log_entries.drain(0..self.log_entries.len() - self.max_entries);
        }
    }

    /// Get recent log entries
    pub fn get_recent_entries(&self, count: usize) -> Vec<UnifiedLogEntry> {
        let start_idx = if self.log_entries.len() > count {
            self.log_entries.len() - count
        } else {
            0
        };

        self.log_entries[start_idx..].to_vec()
    }

    /// Get log entries by level
    pub fn get_entries_by_level(&self, level: LogLevel) -> Vec<UnifiedLogEntry> {
        self.log_entries
            .iter()
            .filter(|entry| entry.level >= level)
            .cloned()
            .collect()
    }

    /// Get log entries by trace ID
    pub fn get_entries_by_trace(&self, trace_id: &str) -> Vec<UnifiedLogEntry> {
        self.log_entries
            .iter()
            .filter(|entry| {
                entry.trace_id.as_ref().map_or(false, |id| id == trace_id)
            })
            .cloned()
            .collect()
    }
}

impl CrossLayerObservabilityManager {
    /// Create a new cross-layer observability manager
    pub fn new(monitoring: Arc<MonitoringSystem>) -> Self {
        // Initialize Zig observability system
        unsafe {
            zig_observability_init();
        }

        Self {
            monitoring,
            zig_metrics: Arc::new(Mutex::new(ZigMetricsCollector::new(Duration::from_secs(10)))),
            trace_manager: Arc::new(RwLock::new(TraceContextManager::new())),
            performance_monitor: Arc::new(Mutex::new(PerformanceMonitor::new())),
            unified_logger: Arc::new(Mutex::new(UnifiedLogger::new(10000))),
        }
    }

    /// Collect comprehensive metrics from both layers
    pub fn collect_comprehensive_metrics(&self, shard_id: u32) -> OQLResult<ComprehensiveMetrics> {
        // Collect Rust metrics
        let rust_metrics = self.monitoring.get_system_metrics()?;
        let query_stats = self.monitoring.get_query_stats()?;

        // Collect Zig metrics
        let zig_metrics = {
            let mut collector = self.zig_metrics.lock().map_err(|_| {
                OQLError::concurrency_error("Failed to acquire Zig metrics lock", None)
            })?;
            collector.collect_metrics(shard_id)?
        };

        // Collect performance metrics
        let performance_summary = {
            let monitor = self.performance_monitor.lock().map_err(|_| {
                OQLError::concurrency_error("Failed to acquire performance monitor lock", None)
            })?;
            monitor.get_performance_summary()
        };

        Ok(ComprehensiveMetrics {
            rust_system_metrics: rust_metrics,
            rust_query_stats: query_stats,
            zig_storage_metrics: zig_metrics,
            performance_summary,
            timestamp: Utc::now(),
        })
    }

    /// Start a cross-layer trace
    pub fn start_trace(&self, operation_name: &str) -> OQLResult<String> {
        let mut trace_manager = self.trace_manager.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire trace manager lock", None)
        })?;

        let trace_id = trace_manager.start_trace(operation_name);

        // Also start a Rust trace span
        let rust_span = self.monitoring.start_trace(operation_name.to_string())?;
        
        Ok(trace_id)
    }

    /// Start a span within a trace
    pub fn start_span(
        &self,
        trace_id: &str,
        operation_name: &str,
        layer: &str,
        shard_id: Option<u32>,
    ) -> OQLResult<String> {
        let mut trace_manager = self.trace_manager.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire trace manager lock", None)
        })?;

        trace_manager.start_span(trace_id, operation_name, layer, shard_id)
    }

    /// Finish a span
    pub fn finish_span(&self, trace_id: &str, span_id: &str) -> OQLResult<()> {
        let mut trace_manager = self.trace_manager.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire trace manager lock", None)
        })?;

        trace_manager.finish_span(trace_id, span_id)
    }

    /// Finish a trace
    pub fn finish_trace(&self, trace_id: &str) -> OQLResult<CrossLayerTraceContext> {
        let mut trace_manager = self.trace_manager.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire trace manager lock", None)
        })?;

        trace_manager.finish_trace(trace_id)
    }

    /// Record operation performance
    pub fn record_performance(&self, operation: &str, duration: Duration, bytes: Option<u64>) -> OQLResult<()> {
        let mut monitor = self.performance_monitor.lock().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire performance monitor lock", None)
        })?;

        monitor.record_timing(operation, duration);

        if let Some(bytes) = bytes {
            monitor.record_throughput(operation, 1, bytes);
        }

        Ok(())
    }

    /// Log a unified message
    pub fn log(
        &self,
        level: LogLevel,
        layer: &str,
        component: &str,
        message: &str,
        shard_id: Option<u32>,
        trace_id: Option<String>,
        span_id: Option<String>,
        metadata: HashMap<String, String>,
    ) -> OQLResult<()> {
        let mut logger = self.unified_logger.lock().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire unified logger lock", None)
        })?;

        match layer {
            "rust" => logger.log_rust(level, component, message, trace_id, span_id, metadata),
            "zig" => logger.log_zig(level, component, message, shard_id.unwrap_or(0), trace_id, span_id, metadata),
            _ => return Err(OQLError::Generic {
                message: format!("Unknown layer: {}", layer),
            }),
        }

        Ok(())
    }

    /// Get recent log entries
    pub fn get_recent_logs(&self, count: usize) -> OQLResult<Vec<UnifiedLogEntry>> {
        let logger = self.unified_logger.lock().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire unified logger lock", None)
        })?;

        Ok(logger.get_recent_entries(count))
    }

    /// Get observability dashboard data
    pub fn get_dashboard_data(&self, shard_id: u32) -> OQLResult<ObservabilityDashboard> {
        let comprehensive_metrics = self.collect_comprehensive_metrics(shard_id)?;
        let recent_logs = self.get_recent_logs(100)?;

        Ok(ObservabilityDashboard {
            comprehensive_metrics,
            recent_logs,
            timestamp: Utc::now(),
        })
    }

    /// Cleanup observability resources
    pub fn cleanup(&self) {
        unsafe {
            zig_observability_cleanup();
        }
    }
}

impl Drop for CrossLayerObservabilityManager {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Comprehensive metrics from both layers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComprehensiveMetrics {
    pub rust_system_metrics: SystemMetrics,
    pub rust_query_stats: crate::monitoring::QueryStats,
    pub zig_storage_metrics: ZigStorageMetrics,
    pub performance_summary: PerformanceSummary,
    pub timestamp: DateTime<Utc>,
}

/// Observability dashboard data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityDashboard {
    pub comprehensive_metrics: ComprehensiveMetrics,
    pub recent_logs: Vec<UnifiedLogEntry>,
    pub timestamp: DateTime<Utc>,
}

/// FFI functions for Zig observability
extern "C" {
    /// Initialize Zig observability system
    fn zig_observability_init() -> bool;

    /// Cleanup Zig observability system
    fn zig_observability_cleanup();

    /// Collect arena metrics from Zig
    fn zig_collect_arena_metrics(
        shard_id: c_uint,
        total_allocations: *mut c_ulong,
        successful_allocations: *mut c_ulong,
        failed_allocations: *mut c_ulong,
        total_bytes_allocated: *mut c_ulong,
        total_bytes_freed: *mut c_ulong,
        current_usage_bytes: *mut c_ulong,
        peak_usage_bytes: *mut c_ulong,
        fragmentation_ratio: *mut f64,
        avg_allocation_size: *mut f64,
        allocation_latency_ns: *mut c_ulong,
        colocation_success_rate: *mut f64,
    );

    /// Collect B-tree metrics from Zig
    fn zig_collect_btree_metrics(
        shard_id: c_uint,
        total_insertions: *mut c_ulong,
        total_deletions: *mut c_ulong,
        total_lookups: *mut c_ulong,
        successful_lookups: *mut c_ulong,
        node_splits: *mut c_ulong,
        node_merges: *mut c_ulong,
        tree_height: *mut c_uint,
        total_nodes: *mut c_ulong,
        avg_lookup_latency_ns: *mut c_ulong,
        avg_insertion_latency_ns: *mut c_ulong,
        cache_hit_rate: *mut f64,
    );

    /// Collect WAL metrics from Zig
    fn zig_collect_wal_metrics(
        shard_id: c_uint,
        total_writes: *mut c_ulong,
        total_bytes_written: *mut c_ulong,
        total_syncs: *mut c_ulong,
        avg_write_latency_ns: *mut c_ulong,
        avg_sync_latency_ns: *mut c_ulong,
        current_wal_size_bytes: *mut c_ulong,
        checkpoint_count: *mut c_ulong,
        last_checkpoint_timestamp: *mut c_ulong,
        has_checkpoint: *mut bool,
        write_throughput_bytes_per_sec: *mut f64,
    );

    /// Collect transaction metrics from Zig
    fn zig_collect_transaction_metrics(
        shard_id: c_uint,
        active_transactions: *mut c_ulong,
        committed_transactions: *mut c_ulong,
        aborted_transactions: *mut c_ulong,
        avg_transaction_duration_ms: *mut f64,
        lock_conflicts: *mut c_ulong,
        deadlocks_detected: *mut c_ulong,
        two_phase_commits: *mut c_ulong,
        distributed_transactions: *mut c_ulong,
    );

    /// Collect memory metrics from Zig
    fn zig_collect_memory_metrics(
        shard_id: c_uint,
        total_allocated_bytes: *mut c_ulong,
        peak_allocated_bytes: *mut c_ulong,
        current_rss_bytes: *mut c_ulong,
        gc_cycles: *mut c_ulong,
        gc_time_ms: *mut c_ulong,
        objects_collected: *mut c_ulong,
        memory_pressure_events: *mut c_ulong,
    );

    /// Collect I/O metrics from Zig
    fn zig_collect_io_metrics(
        shard_id: c_uint,
        total_reads: *mut c_ulong,
        total_writes: *mut c_ulong,
        total_bytes_read: *mut c_ulong,
        total_bytes_written: *mut c_ulong,
        avg_read_latency_ns: *mut c_ulong,
        avg_write_latency_ns: *mut c_ulong,
        io_errors: *mut c_ulong,
        cache_hits: *mut c_ulong,
        cache_misses: *mut c_ulong,
    );

    /// Start trace in Zig
    fn zig_start_trace(trace_id: *const c_char, operation_name: *const c_char) -> bool;

    /// Start span in Zig
    fn zig_start_span(
        trace_id: *const c_char,
        span_id: *const c_char,
        operation_name: *const c_char,
        shard_id: c_uint,
    ) -> bool;

    /// Finish span in Zig
    fn zig_finish_span(trace_id: *const c_char, span_id: *const c_char) -> bool;

    /// Finish trace in Zig
    fn zig_finish_trace(trace_id: *const c_char) -> bool;

    /// Get span count for trace
    fn zig_get_span_count(trace_id: *const c_char) -> c_uint;

    /// Get span information
    fn zig_get_span_info(
        trace_id: *const c_char,
        span_index: c_uint,
        span_id_buffer: *mut c_char,
        span_id_buffer_size: usize,
        operation_buffer: *mut c_char,
        operation_buffer_size: usize,
        start_time_ns: *mut c_ulong,
        end_time_ns: *mut c_ulong,
        has_end_time: *mut bool,
        shard_id: *mut c_uint,
        tag_count: *mut c_uint,
    ) -> bool;

    /// Get span tag
    fn zig_get_span_tag(
        trace_id: *const c_char,
        span_id: *const c_char,
        tag_index: c_uint,
        key_buffer: *mut c_char,
        key_buffer_size: usize,
        value_buffer: *mut c_char,
        value_buffer_size: usize,
    ) -> bool;
}

/// Macro for timing cross-layer operations
#[macro_export]
macro_rules! time_cross_layer_operation {
    ($observability:expr, $operation:expr, $layer:expr, $shard_id:expr, $code:block) => {{
        let start_time = std::time::Instant::now();
        let trace_id = $observability.start_trace($operation)?;
        let span_id = $observability.start_span(&trace_id, $operation, $layer, $shard_id)?;
        
        let result = $code;
        
        let duration = start_time.elapsed();
        $observability.record_performance($operation, duration, None)?;
        $observability.finish_span(&trace_id, &span_id)?;
        $observability.finish_trace(&trace_id)?;
        
        result
    }};
}

/// Macro for logging with trace context
#[macro_export]
macro_rules! log_with_trace {
    ($observability:expr, $level:expr, $layer:expr, $component:expr, $message:expr, $trace_id:expr, $span_id:expr) => {
        $observability.log(
            $level,
            $layer,
            $component,
            $message,
            None,
            $trace_id,
            $span_id,
            std::collections::HashMap::new(),
        )?;
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::MonitoringConfig;

    #[test]
    fn test_zig_metrics_collector() {
        let mut collector = ZigMetricsCollector::new(Duration::from_millis(100));
        
        // Test that collector initializes properly
        assert!(collector.last_metrics.is_none());
        assert!(collector.collection_interval == Duration::from_millis(100));
    }

    #[test]
    fn test_trace_context_manager() {
        let mut manager = TraceContextManager::new();
        
        // Start a trace
        let trace_id = manager.start_trace("test_operation");
        assert!(trace_id.starts_with("trace_"));
        
        // Start a span
        let span_id = manager.start_span(&trace_id, "test_span", "rust", Some(0)).unwrap();
        assert!(span_id.starts_with("span_"));
        
        // Finish the span
        manager.finish_span(&trace_id, &span_id).unwrap();
        
        // Finish the trace
        let context = manager.finish_trace(&trace_id).unwrap();
        assert_eq!(context.trace_id, trace_id);
        assert!(!context.span_stack.is_empty());
    }

    #[test]
    fn test_performance_monitor() {
        let mut monitor = PerformanceMonitor::new();
        
        // Record some timings
        monitor.record_timing("test_op", Duration::from_millis(100));
        monitor.record_timing("test_op", Duration::from_millis(150));
        monitor.record_timing("test_op", Duration::from_millis(200));
        
        // Check percentiles were calculated
        assert!(monitor.latency_percentiles.contains_key("test_op"));
        
        let percentiles = &monitor.latency_percentiles["test_op"];
        assert!(percentiles.min <= percentiles.p50);
        assert!(percentiles.p50 <= percentiles.p90);
        assert!(percentiles.p90 <= percentiles.p95);
        assert!(percentiles.p95 <= percentiles.p99);
        assert!(percentiles.p99 <= percentiles.max);
    }

    #[test]
    fn test_unified_logger() {
        let mut logger = UnifiedLogger::new(100);
        
        // Log some messages
        logger.log_rust(
            LogLevel::Info,
            "test_component",
            "Test message from Rust",
            Some("trace_123".to_string()),
            Some("span_456".to_string()),
            HashMap::new(),
        );
        
        logger.log_zig(
            LogLevel::Warn,
            "zig_component",
            "Test message from Zig",
            0,
            Some("trace_123".to_string()),
            Some("span_789".to_string()),
            HashMap::new(),
        );
        
        // Check entries were logged
        let entries = logger.get_recent_entries(10);
        assert_eq!(entries.len(), 2);
        
        assert_eq!(entries[0].layer, "rust");
        assert_eq!(entries[1].layer, "zig");
        
        // Test filtering by trace ID
        let trace_entries = logger.get_entries_by_trace("trace_123");
        assert_eq!(trace_entries.len(), 2);
        
        // Test filtering by log level
        let warn_entries = logger.get_entries_by_level(LogLevel::Warn);
        assert_eq!(warn_entries.len(), 1);
        assert_eq!(warn_entries[0].level, LogLevel::Warn);
    }

    #[test]
    fn test_cross_layer_observability_manager() {
        let monitoring_config = MonitoringConfig::default();
        let monitoring = Arc::new(MonitoringSystem::new(monitoring_config));
        let manager = CrossLayerObservabilityManager::new(monitoring);
        
        // Test trace operations
        let trace_id = manager.start_trace("test_operation").unwrap();
        let span_id = manager.start_span(&trace_id, "test_span", "rust", Some(0)).unwrap();
        
        // Record performance
        manager.record_performance("test_op", Duration::from_millis(50), Some(1024)).unwrap();
        
        // Log a message
        manager.log(
            LogLevel::Info,
            "rust",
            "test_component",
            "Test log message",
            None,
            Some(trace_id.clone()),
            Some(span_id.clone()),
            HashMap::new(),
        ).unwrap();
        
        // Finish operations
        manager.finish_span(&trace_id, &span_id).unwrap();
        let _context = manager.finish_trace(&trace_id).unwrap();
        
        // Get recent logs
        let logs = manager.get_recent_logs(10).unwrap();
        assert!(!logs.is_empty());
    }

    #[test]
    fn test_log_levels() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Fatal);
    }

    #[test]
    fn test_latency_percentiles() {
        let mut monitor = PerformanceMonitor::new();
        
        // Add a range of timings
        for i in 1..=100 {
            monitor.record_timing("test", Duration::from_millis(i));
        }
        
        let percentiles = &monitor.latency_percentiles["test"];
        
        // Verify percentiles make sense
        assert_eq!(percentiles.min, Duration::from_millis(1));
        assert_eq!(percentiles.max, Duration::from_millis(100));
        assert!(percentiles.p50 >= Duration::from_millis(40));
        assert!(percentiles.p50 <= Duration::from_millis(60));
        assert!(percentiles.p90 >= Duration::from_millis(85));
        assert!(percentiles.p99 >= Duration::from_millis(95));
    }
}