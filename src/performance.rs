use crate::simulation::Clock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Performance metrics for query execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMetrics {
    pub query_hash: String,
    pub execution_time: Duration,
    pub result_count: usize,
    pub index_hits: usize,
    pub index_misses: usize,
    pub documents_scanned: usize,
    pub references_resolved: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Aggregated performance statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceStats {
    pub total_queries: usize,
    pub avg_execution_time: Duration,
    pub min_execution_time: Duration,
    pub max_execution_time: Duration,
    pub total_documents_scanned: usize,
    pub total_references_resolved: usize,
    pub index_hit_rate: f64,
    pub cache_hit_rate: f64,
    pub queries_per_second: f64,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Performance monitor for tracking query execution metrics with Clock dependency injection
#[allow(dead_code)]
pub struct PerformanceMonitor {
    metrics: Arc<Mutex<Vec<QueryMetrics>>>,
    start_time: Instant,
    clock: Arc<dyn Clock>,
}

impl PerformanceMonitor {
    /// Create a new performance monitor with Clock dependency injection
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            metrics: Arc::new(Mutex::new(Vec::new())),
            start_time: clock.now(),
            clock,
        }
    }

    /// Create a new performance monitor with production clock (backward compatibility)
    pub fn new_with_real_clock() -> Self {
        use crate::simulation::RealClock;
        Self::new(Arc::new(RealClock::new()))
    }

    /// Record metrics for a query execution
    pub fn record_query(&self, metrics: QueryMetrics) {
        let mut metrics_vec = self.metrics.lock().unwrap();
        metrics_vec.push(metrics);

        // Keep only the last 10,000 metrics to prevent unbounded growth
        if metrics_vec.len() > 10_000 {
            metrics_vec.drain(0..1_000);
        }
    }

    /// Get aggregated performance statistics
    pub fn get_stats(&self) -> PerformanceStats {
        let metrics_vec = self.metrics.lock().unwrap();

        if metrics_vec.is_empty() {
            return PerformanceStats {
                total_queries: 0,
                avg_execution_time: Duration::from_millis(0),
                min_execution_time: Duration::from_millis(0),
                max_execution_time: Duration::from_millis(0),
                total_documents_scanned: 0,
                total_references_resolved: 0,
                index_hit_rate: 0.0,
                cache_hit_rate: 0.0,
                queries_per_second: 0.0,
                last_updated: chrono::Utc::now(),
            };
        }

        let total_queries = metrics_vec.len();
        let total_execution_time: Duration = metrics_vec.iter().map(|m| m.execution_time).sum();

        let avg_execution_time = total_execution_time / total_queries as u32;
        let min_execution_time = metrics_vec
            .iter()
            .map(|m| m.execution_time)
            .min()
            .unwrap_or(Duration::from_millis(0));
        let max_execution_time = metrics_vec
            .iter()
            .map(|m| m.execution_time)
            .max()
            .unwrap_or(Duration::from_millis(0));

        let total_documents_scanned: usize = metrics_vec.iter().map(|m| m.documents_scanned).sum();
        let total_references_resolved: usize =
            metrics_vec.iter().map(|m| m.references_resolved).sum();

        let total_index_operations: usize = metrics_vec
            .iter()
            .map(|m| m.index_hits + m.index_misses)
            .sum();
        let total_index_hits: usize = metrics_vec.iter().map(|m| m.index_hits).sum();
        let index_hit_rate = if total_index_operations > 0 {
            total_index_hits as f64 / total_index_operations as f64
        } else {
            0.0
        };

        let total_cache_operations: usize = metrics_vec
            .iter()
            .map(|m| m.cache_hits + m.cache_misses)
            .sum();
        let total_cache_hits: usize = metrics_vec.iter().map(|m| m.cache_hits).sum();
        let cache_hit_rate = if total_cache_operations > 0 {
            total_cache_hits as f64 / total_cache_operations as f64
        } else {
            0.0
        };

        let elapsed_seconds = self.start_time.elapsed().as_secs_f64();
        let queries_per_second = if elapsed_seconds > 0.0 {
            total_queries as f64 / elapsed_seconds
        } else {
            0.0
        };

        PerformanceStats {
            total_queries,
            avg_execution_time,
            min_execution_time,
            max_execution_time,
            total_documents_scanned,
            total_references_resolved,
            index_hit_rate,
            cache_hit_rate,
            queries_per_second,
            last_updated: chrono::Utc::now(),
        }
    }

    /// Get metrics for specific query patterns
    pub fn get_query_pattern_stats(&self, pattern: &str) -> Vec<QueryMetrics> {
        let metrics_vec = self.metrics.lock().unwrap();
        metrics_vec
            .iter()
            .filter(|m| m.query_hash.contains(pattern))
            .cloned()
            .collect()
    }

    /// Get slowest queries
    pub fn get_slowest_queries(&self, limit: usize) -> Vec<QueryMetrics> {
        let metrics_vec = self.metrics.lock().unwrap();
        let mut sorted_metrics = metrics_vec.clone();
        sorted_metrics.sort_by(|a, b| b.execution_time.cmp(&a.execution_time));
        sorted_metrics.into_iter().take(limit).collect()
    }

    /// Get most frequent queries
    pub fn get_frequent_queries(&self, limit: usize) -> Vec<(String, usize, Duration)> {
        let metrics_vec = self.metrics.lock().unwrap();
        let mut query_counts: HashMap<String, (usize, Duration)> = HashMap::new();

        for metric in metrics_vec.iter() {
            let entry = query_counts
                .entry(metric.query_hash.clone())
                .or_insert((0, Duration::from_millis(0)));
            entry.0 += 1;
            entry.1 += metric.execution_time;
        }

        let mut frequent_queries: Vec<_> = query_counts
            .into_iter()
            .map(|(query, (count, total_time))| (query, count, total_time / count as u32))
            .collect();

        frequent_queries.sort_by(|a, b| b.1.cmp(&a.1));
        frequent_queries.into_iter().take(limit).collect()
    }

    /// Clear all metrics
    pub fn clear_metrics(&self) {
        let mut metrics_vec = self.metrics.lock().unwrap();
        metrics_vec.clear();
    }

    /// Export metrics to JSON
    pub fn export_metrics(&self) -> Result<String, serde_json::Error> {
        let metrics_vec = self.metrics.lock().unwrap();
        serde_json::to_string_pretty(&*metrics_vec)
    }
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self::new_with_real_clock()
    }
}

/// Query execution context for performance tracking with Clock dependency injection
pub struct QueryExecutionContext {
    pub query_hash: String,
    pub start_time: Instant,
    pub documents_scanned: usize,
    pub references_resolved: usize,
    pub index_hits: usize,
    pub index_misses: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    clock: Arc<dyn Clock>,
}

impl QueryExecutionContext {
    /// Create a new query execution context with Clock dependency injection
    pub fn new_with_clock(query: &str, clock: Arc<dyn Clock>) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);
        let query_hash = format!("{:x}", hasher.finish());

        Self {
            query_hash,
            start_time: clock.now(),
            documents_scanned: 0,
            references_resolved: 0,
            index_hits: 0,
            index_misses: 0,
            cache_hits: 0,
            cache_misses: 0,
            clock,
        }
    }

    /// Create a new query execution context with production clock (backward compatibility)
    pub fn new(query: &str) -> Self {
        use crate::simulation::RealClock;
        Self::new_with_clock(query, Arc::new(RealClock::new()))
    }

    pub fn record_document_scan(&mut self, count: usize) {
        self.documents_scanned += count;
    }

    pub fn record_reference_resolution(&mut self, count: usize) {
        self.references_resolved += count;
    }

    pub fn record_index_hit(&mut self) {
        self.index_hits += 1;
    }

    pub fn record_index_miss(&mut self) {
        self.index_misses += 1;
    }

    pub fn record_cache_hit(&mut self) {
        self.cache_hits += 1;
    }

    pub fn record_cache_miss(&mut self) {
        self.cache_misses += 1;
    }

    pub fn finish(self, result_count: usize) -> QueryMetrics {
        let execution_time = self.clock.now().duration_since(self.start_time);
        QueryMetrics {
            query_hash: self.query_hash,
            execution_time,
            result_count,
            index_hits: self.index_hits,
            index_misses: self.index_misses,
            documents_scanned: self.documents_scanned,
            references_resolved: self.references_resolved,
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Performance benchmark suite with Clock dependency injection
pub struct PerformanceBenchmark {
    monitor: PerformanceMonitor,
    clock: Arc<dyn Clock>,
}

impl PerformanceBenchmark {
    /// Create a new performance benchmark with Clock dependency injection
    pub fn new_with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            monitor: PerformanceMonitor::new(clock.clone()),
            clock,
        }
    }

    /// Create a new performance benchmark with production clock (backward compatibility)
    pub fn new() -> Self {
        use crate::simulation::RealClock;
        let clock = Arc::new(RealClock::new());
        Self::new_with_clock(clock)
    }

    /// Run a performance benchmark with the given name and closure
    pub fn benchmark<F, R>(&self, name: &str, f: F) -> (R, Duration)
    where
        F: FnOnce() -> R,
    {
        println!("Running benchmark: {}", name);
        let start_time = self.clock.now();
        let result = f();
        let duration = self.clock.now().duration_since(start_time);
        println!("  Completed in {:?}", duration);
        (result, duration)
    }

    /// Run multiple iterations of a benchmark and return statistics
    pub fn benchmark_iterations<F, R>(
        &self,
        name: &str,
        iterations: usize,
        f: F,
    ) -> BenchmarkResults
    where
        F: Fn() -> R,
    {
        println!("Running benchmark: {} ({} iterations)", name, iterations);
        let mut durations = Vec::with_capacity(iterations);

        for i in 0..iterations {
            let start_time = self.clock.now();
            let _result = f();
            let duration = self.clock.now().duration_since(start_time);
            durations.push(duration);

            if i % (iterations / 10).max(1) == 0 {
                println!("  Iteration {}/{} completed", i + 1, iterations);
            }
        }

        let total_duration: Duration = durations.iter().sum();
        let avg_duration = total_duration / iterations as u32;
        let min_duration = *durations.iter().min().unwrap();
        let max_duration = *durations.iter().max().unwrap();

        // Calculate percentiles
        let mut sorted_durations = durations.clone();
        sorted_durations.sort();
        let p50 = sorted_durations[iterations / 2];
        let p95 = sorted_durations[(iterations * 95) / 100];
        let p99 = sorted_durations[(iterations * 99) / 100];

        let results = BenchmarkResults {
            name: name.to_string(),
            iterations,
            total_duration,
            avg_duration,
            min_duration,
            max_duration,
            p50_duration: p50,
            p95_duration: p95,
            p99_duration: p99,
        };

        println!(
            "  Results: avg={:?}, min={:?}, max={:?}, p95={:?}",
            results.avg_duration, results.min_duration, results.max_duration, results.p95_duration
        );

        results
    }

    pub fn get_monitor(&self) -> &PerformanceMonitor {
        &self.monitor
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkResults {
    pub name: String,
    pub iterations: usize,
    pub total_duration: Duration,
    pub avg_duration: Duration,
    pub min_duration: Duration,
    pub max_duration: Duration,
    pub p50_duration: Duration,
    pub p95_duration: Duration,
    pub p99_duration: Duration,
}

impl BenchmarkResults {
    pub fn print_summary(&self) {
        println!("\n=== Benchmark Results: {} ===", self.name);
        println!("Iterations: {}", self.iterations);
        println!("Total time: {:?}", self.total_duration);
        println!("Average: {:?}", self.avg_duration);
        println!("Min: {:?}", self.min_duration);
        println!("Max: {:?}", self.max_duration);
        println!("P50: {:?}", self.p50_duration);
        println!("P95: {:?}", self.p95_duration);
        println!("P99: {:?}", self.p99_duration);
        println!(
            "Throughput: {:.2} ops/sec",
            self.iterations as f64 / self.total_duration.as_secs_f64()
        );
    }
}
