// Monitoring and Observability System for OQL Database

use crate::document::{Document, DocumentId};
use crate::error::{OQLError, OQLResult};
use crate::parser::OQLQuery;
use chrono::{DateTime, Utc, Duration};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

/// Query execution metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMetrics {
    pub query_id: String,
    pub query_text: String,
    pub schema: String,
    pub operation_type: String,
    pub execution_time_ms: u64,
    pub documents_scanned: usize,
    pub documents_returned: usize,
    pub index_used: Option<String>,
    pub cache_hit: bool,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub error_message: Option<String>,
}

impl QueryMetrics {
    /// Create new query metrics
    pub fn new(
        query: &OQLQuery,
        execution_time_ms: u64,
        documents_scanned: usize,
        documents_returned: usize,
    ) -> Self {
        Self {
            query_id: uuid::Uuid::new_v4().to_string(),
            query_text: format!("{:?}", query), // Simplified query representation
            schema: query.collection.clone(),
            operation_type: Self::extract_operation_type(query),
            execution_time_ms,
            documents_scanned,
            documents_returned,
            index_used: None,
            cache_hit: false,
            timestamp: Utc::now(),
            success: true,
            error_message: None,
        }
    }

    /// Create error metrics
    pub fn error(query: &OQLQuery, execution_time_ms: u64, error: &str) -> Self {
        let mut metrics = Self::new(query, execution_time_ms, 0, 0);
        metrics.success = false;
        metrics.error_message = Some(error.to_string());
        metrics
    }

    /// Set index usage information
    pub fn with_index(mut self, index_name: String) -> Self {
        self.index_used = Some(index_name);
        self
    }

    /// Set cache hit information
    pub fn with_cache_hit(mut self, cache_hit: bool) -> Self {
        self.cache_hit = cache_hit;
        self
    }

    /// Extract operation type from query
    fn extract_operation_type(query: &OQLQuery) -> String {
        if query.operations.is_empty() {
            return "scan".to_string();
        }

        // Get the primary operation type
        match &query.operations[0] {
            crate::parser::Operation::Where(_) => "select".to_string(),
            crate::parser::Operation::Select(_) => "select".to_string(),
            crate::parser::Operation::Insert(_) => "insert".to_string(),
            crate::parser::Operation::InsertMany(_) => "batch_insert".to_string(),
            crate::parser::Operation::Update(_) => "update".to_string(),
            crate::parser::Operation::Delete => "delete".to_string(),
            crate::parser::Operation::GroupBy(_) => "aggregate".to_string(),
            crate::parser::Operation::Having(_) => "filter".to_string(),
            crate::parser::Operation::Count => "aggregate".to_string(),
            crate::parser::Operation::Sum(_) => "aggregate".to_string(),
            crate::parser::Operation::Avg(_) => "aggregate".to_string(),
            crate::parser::Operation::Min(_) => "aggregate".to_string(),
            crate::parser::Operation::Max(_) => "aggregate".to_string(),
            crate::parser::Operation::Sort(_) => "sort".to_string(),
            crate::parser::Operation::Limit(_) => "limit".to_string(),
            crate::parser::Operation::FunctionDef(_) => "function".to_string(),
        }
    }
}

/// System performance metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub timestamp: DateTime<Utc>,
    pub active_connections: usize,
    pub queries_per_second: f64,
    pub avg_query_time_ms: f64,
    pub storage_size_bytes: u64,
    pub memory_usage_bytes: u64,
    pub cache_hit_rate: f64,
    pub error_rate: f64,
    pub uptime_seconds: u64,
}

impl SystemMetrics {
    /// Create new system metrics
    pub fn new() -> Self {
        Self {
            timestamp: Utc::now(),
            active_connections: 0,
            queries_per_second: 0.0,
            avg_query_time_ms: 0.0,
            storage_size_bytes: 0,
            memory_usage_bytes: 0,
            cache_hit_rate: 0.0,
            error_rate: 0.0,
            uptime_seconds: 0,
        }
    }
}

/// Alert severity levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// System alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub severity: AlertSeverity,
    pub title: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub resolved: bool,
    pub metadata: HashMap<String, String>,
}

impl Alert {
    /// Create a new alert
    pub fn new(severity: AlertSeverity, title: String, message: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            severity,
            title,
            message,
            timestamp: Utc::now(),
            resolved: false,
            metadata: HashMap::new(),
        }
    }

    /// Add metadata to the alert
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Resolve the alert
    pub fn resolve(&mut self) {
        self.resolved = true;
    }
}

/// Monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    pub metrics_retention_hours: u32,
    pub alert_thresholds: AlertThresholds,
    pub enable_query_logging: bool,
    pub enable_performance_monitoring: bool,
    pub enable_distributed_tracing: bool,
    pub metrics_collection_interval_seconds: u32,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            metrics_retention_hours: 24,
            alert_thresholds: AlertThresholds::default(),
            enable_query_logging: true,
            enable_performance_monitoring: true,
            enable_distributed_tracing: false,
            metrics_collection_interval_seconds: 60,
        }
    }
}

/// Alert threshold configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertThresholds {
    pub max_query_time_ms: u64,
    pub max_error_rate: f64,
    pub min_cache_hit_rate: f64,
    pub max_memory_usage_bytes: u64,
    pub max_storage_size_bytes: u64,
}

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            max_query_time_ms: 5000,     // 5 seconds
            max_error_rate: 0.05,        // 5%
            min_cache_hit_rate: 0.8,     // 80%
            max_memory_usage_bytes: 1024 * 1024 * 1024, // 1GB
            max_storage_size_bytes: 10 * 1024 * 1024 * 1024, // 10GB
        }
    }
}

/// Distributed tracing span
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    pub span_id: String,
    pub trace_id: String,
    pub parent_span_id: Option<String>,
    pub operation_name: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub tags: HashMap<String, String>,
    pub logs: Vec<TraceLog>,
}

impl TraceSpan {
    /// Create a new trace span
    pub fn new(trace_id: String, operation_name: String) -> Self {
        Self {
            span_id: uuid::Uuid::new_v4().to_string(),
            trace_id,
            parent_span_id: None,
            operation_name,
            start_time: Utc::now(),
            end_time: None,
            duration_ms: None,
            tags: HashMap::new(),
            logs: Vec::new(),
        }
    }

    /// Create a child span
    pub fn child(&self, operation_name: String) -> Self {
        let mut span = Self::new(self.trace_id.clone(), operation_name);
        span.parent_span_id = Some(self.span_id.clone());
        span
    }

    /// Add a tag to the span
    pub fn tag(mut self, key: String, value: String) -> Self {
        self.tags.insert(key, value);
        self
    }

    /// Add a log entry to the span
    pub fn log(&mut self, level: String, message: String) {
        self.logs.push(TraceLog {
            timestamp: Utc::now(),
            level,
            message,
        });
    }

    /// Finish the span
    pub fn finish(&mut self) {
        let end_time = Utc::now();
        self.end_time = Some(end_time);
        self.duration_ms = Some(
            (end_time - self.start_time).num_milliseconds() as u64
        );
    }
}

/// Trace log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceLog {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub message: String,
}

/// Performance monitoring and observability system
pub struct MonitoringSystem {
    config: MonitoringConfig,
    query_metrics: Arc<Mutex<VecDeque<QueryMetrics>>>,
    system_metrics: Arc<RwLock<SystemMetrics>>,
    alerts: Arc<Mutex<Vec<Alert>>>,
    traces: Arc<Mutex<HashMap<String, Vec<TraceSpan>>>>,
    start_time: Instant,
    system_start_time: DateTime<Utc>,
}

impl MonitoringSystem {
    /// Create a new monitoring system
    pub fn new(config: MonitoringConfig) -> Self {
        Self {
            config,
            query_metrics: Arc::new(Mutex::new(VecDeque::new())),
            system_metrics: Arc::new(RwLock::new(SystemMetrics::new())),
            alerts: Arc::new(Mutex::new(Vec::new())),
            traces: Arc::new(Mutex::new(HashMap::new())),
            start_time: Instant::now(),
            system_start_time: Utc::now(),
        }
    }

    /// Record query execution metrics
    pub fn record_query_metrics(&self, metrics: QueryMetrics) -> OQLResult<()> {
        if !self.config.enable_query_logging {
            return Ok(());
        }

        let mut query_metrics = self.query_metrics.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire query metrics lock".to_string(),
            }
        })?;

        query_metrics.push_back(metrics.clone());

        // Trim old metrics based on retention policy
        let retention_duration = Duration::hours(self.config.metrics_retention_hours as i64);
        let cutoff_time = Utc::now() - retention_duration;

        while let Some(front) = query_metrics.front() {
            if front.timestamp < cutoff_time {
                query_metrics.pop_front();
            } else {
                break;
            }
        }

        // Check for alerts based on this query
        self.check_query_alerts(&metrics)?;

        Ok(())
    }

    /// Update system metrics
    pub fn update_system_metrics(&self, metrics: SystemMetrics) -> OQLResult<()> {
        if !self.config.enable_performance_monitoring {
            return Ok(());
        }

        {
            let mut system_metrics = self.system_metrics.write().map_err(|_| {
                OQLError::Generic {
                    message: "Failed to acquire system metrics write lock".to_string(),
                }
            })?;
            *system_metrics = metrics;
        }

        // Check for system-level alerts
        self.check_system_alerts()?;

        Ok(())
    }

    /// Get current system metrics
    pub fn get_system_metrics(&self) -> OQLResult<SystemMetrics> {
        let mut metrics = self.system_metrics.read().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire system metrics read lock".to_string(),
            }
        })?.clone();

        // Update uptime
        metrics.uptime_seconds = self.start_time.elapsed().as_secs();
        metrics.timestamp = Utc::now();

        Ok(metrics)
    }

    /// Get query metrics for a time range
    pub fn get_query_metrics(&self, since: Option<DateTime<Utc>>) -> OQLResult<Vec<QueryMetrics>> {
        let query_metrics = self.query_metrics.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire query metrics lock".to_string(),
            }
        })?;

        let cutoff_time = since.unwrap_or_else(|| Utc::now() - Duration::hours(1));
        
        Ok(query_metrics
            .iter()
            .filter(|m| m.timestamp >= cutoff_time)
            .cloned()
            .collect())
    }

    /// Get query performance statistics
    pub fn get_query_stats(&self) -> OQLResult<QueryStats> {
        let query_metrics = self.query_metrics.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire query metrics lock".to_string(),
            }
        })?;

        let total_queries = query_metrics.len();
        if total_queries == 0 {
            return Ok(QueryStats::default());
        }

        let successful_queries = query_metrics.iter().filter(|m| m.success).count();
        let total_execution_time: u64 = query_metrics.iter().map(|m| m.execution_time_ms).sum();
        let cache_hits = query_metrics.iter().filter(|m| m.cache_hit).count();

        let avg_execution_time = if total_queries > 0 {
            total_execution_time as f64 / total_queries as f64
        } else {
            0.0
        };

        let success_rate = if total_queries > 0 {
            successful_queries as f64 / total_queries as f64
        } else {
            0.0
        };

        let cache_hit_rate = if total_queries > 0 {
            cache_hits as f64 / total_queries as f64
        } else {
            0.0
        };

        // Calculate queries per second over the last minute
        let one_minute_ago = Utc::now() - Duration::minutes(1);
        let recent_queries = query_metrics
            .iter()
            .filter(|m| m.timestamp >= one_minute_ago)
            .count();
        let queries_per_second = recent_queries as f64 / 60.0;

        Ok(QueryStats {
            total_queries,
            successful_queries,
            avg_execution_time_ms: avg_execution_time,
            success_rate,
            cache_hit_rate,
            queries_per_second,
        })
    }

    /// Create a new alert
    pub fn create_alert(&self, alert: Alert) -> OQLResult<()> {
        let mut alerts = self.alerts.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire alerts lock".to_string(),
            }
        })?;

        alerts.push(alert);
        Ok(())
    }

    /// Get active alerts
    pub fn get_active_alerts(&self) -> OQLResult<Vec<Alert>> {
        let alerts = self.alerts.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire alerts lock".to_string(),
            }
        })?;

        Ok(alerts.iter().filter(|a| !a.resolved).cloned().collect())
    }

    /// Resolve an alert
    pub fn resolve_alert(&self, alert_id: &str) -> OQLResult<bool> {
        let mut alerts = self.alerts.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire alerts lock".to_string(),
            }
        })?;

        for alert in alerts.iter_mut() {
            if alert.id == alert_id {
                alert.resolve();
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Start a new trace
    pub fn start_trace(&self, operation_name: String) -> OQLResult<TraceSpan> {
        if !self.config.enable_distributed_tracing {
            // Return a dummy span if tracing is disabled
            return Ok(TraceSpan::new(
                "disabled".to_string(),
                operation_name,
            ));
        }

        let trace_id = uuid::Uuid::new_v4().to_string();
        let span = TraceSpan::new(trace_id, operation_name);
        Ok(span)
    }

    /// Finish a trace span
    pub fn finish_span(&self, mut span: TraceSpan) -> OQLResult<()> {
        if !self.config.enable_distributed_tracing {
            return Ok(());
        }

        span.finish();

        let mut traces = self.traces.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire traces lock".to_string(),
            }
        })?;

        traces
            .entry(span.trace_id.clone())
            .or_insert_with(Vec::new)
            .push(span);

        Ok(())
    }

    /// Get traces for a specific trace ID
    pub fn get_trace(&self, trace_id: &str) -> OQLResult<Vec<TraceSpan>> {
        let traces = self.traces.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire traces lock".to_string(),
            }
        })?;

        Ok(traces.get(trace_id).cloned().unwrap_or_default())
    }

    /// Check for query-level alerts
    fn check_query_alerts(&self, metrics: &QueryMetrics) -> OQLResult<()> {
        // Check for slow queries
        if metrics.execution_time_ms > self.config.alert_thresholds.max_query_time_ms {
            let alert = Alert::new(
                AlertSeverity::Warning,
                "Slow Query Detected".to_string(),
                format!(
                    "Query took {}ms to execute (threshold: {}ms)",
                    metrics.execution_time_ms,
                    self.config.alert_thresholds.max_query_time_ms
                ),
            )
            .with_metadata("query_id".to_string(), metrics.query_id.clone())
            .with_metadata("schema".to_string(), metrics.schema.clone())
            .with_metadata("execution_time_ms".to_string(), metrics.execution_time_ms.to_string());

            self.create_alert(alert)?;
        }

        // Check for query errors
        if !metrics.success {
            let alert = Alert::new(
                AlertSeverity::Error,
                "Query Execution Failed".to_string(),
                format!(
                    "Query failed: {}",
                    metrics.error_message.as_deref().unwrap_or("Unknown error")
                ),
            )
            .with_metadata("query_id".to_string(), metrics.query_id.clone())
            .with_metadata("schema".to_string(), metrics.schema.clone());

            self.create_alert(alert)?;
        }

        Ok(())
    }

    /// Check for system-level alerts
    fn check_system_alerts(&self) -> OQLResult<()> {
        let metrics = self.system_metrics.read().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire system metrics read lock".to_string(),
            }
        })?;

        // Check error rate
        if metrics.error_rate > self.config.alert_thresholds.max_error_rate {
            let alert = Alert::new(
                AlertSeverity::Warning,
                "High Error Rate".to_string(),
                format!(
                    "Error rate is {:.2}% (threshold: {:.2}%)",
                    metrics.error_rate * 100.0,
                    self.config.alert_thresholds.max_error_rate * 100.0
                ),
            );
            self.create_alert(alert)?;
        }

        // Check cache hit rate
        if metrics.cache_hit_rate < self.config.alert_thresholds.min_cache_hit_rate {
            let alert = Alert::new(
                AlertSeverity::Warning,
                "Low Cache Hit Rate".to_string(),
                format!(
                    "Cache hit rate is {:.2}% (threshold: {:.2}%)",
                    metrics.cache_hit_rate * 100.0,
                    self.config.alert_thresholds.min_cache_hit_rate * 100.0
                ),
            );
            self.create_alert(alert)?;
        }

        // Check memory usage
        if metrics.memory_usage_bytes > self.config.alert_thresholds.max_memory_usage_bytes {
            let alert = Alert::new(
                AlertSeverity::Critical,
                "High Memory Usage".to_string(),
                format!(
                    "Memory usage is {} bytes (threshold: {} bytes)",
                    metrics.memory_usage_bytes,
                    self.config.alert_thresholds.max_memory_usage_bytes
                ),
            );
            self.create_alert(alert)?;
        }

        Ok(())
    }
}

/// Query performance statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryStats {
    pub total_queries: usize,
    pub successful_queries: usize,
    pub avg_execution_time_ms: f64,
    pub success_rate: f64,
    pub cache_hit_rate: f64,
    pub queries_per_second: f64,
}

impl Default for QueryStats {
    fn default() -> Self {
        Self {
            total_queries: 0,
            successful_queries: 0,
            avg_execution_time_ms: 0.0,
            success_rate: 0.0,
            cache_hit_rate: 0.0,
            queries_per_second: 0.0,
        }
    }
}

/// Monitoring dashboard data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardData {
    pub system_metrics: SystemMetrics,
    pub query_stats: QueryStats,
    pub active_alerts: Vec<Alert>,
    pub recent_queries: Vec<QueryMetrics>,
}

impl MonitoringSystem {
    /// Get dashboard data
    pub fn get_dashboard_data(&self) -> OQLResult<DashboardData> {
        Ok(DashboardData {
            system_metrics: self.get_system_metrics()?,
            query_stats: self.get_query_stats()?,
            active_alerts: self.get_active_alerts()?,
            recent_queries: self.get_query_metrics(Some(Utc::now() - Duration::minutes(5)))?,
        })
    }

    /// Export metrics in Prometheus format
    pub fn export_prometheus_metrics(&self) -> OQLResult<String> {
        let system_metrics = self.get_system_metrics()?;
        let query_stats = self.get_query_stats()?;

        let mut output = String::new();

        // System metrics
        output.push_str(&format!("# HELP oql_active_connections Number of active connections\n"));
        output.push_str(&format!("# TYPE oql_active_connections gauge\n"));
        output.push_str(&format!("oql_active_connections {}\n", system_metrics.active_connections));

        output.push_str(&format!("# HELP oql_queries_per_second Queries per second\n"));
        output.push_str(&format!("# TYPE oql_queries_per_second gauge\n"));
        output.push_str(&format!("oql_queries_per_second {}\n", query_stats.queries_per_second));

        output.push_str(&format!("# HELP oql_avg_query_time_ms Average query execution time in milliseconds\n"));
        output.push_str(&format!("# TYPE oql_avg_query_time_ms gauge\n"));
        output.push_str(&format!("oql_avg_query_time_ms {}\n", query_stats.avg_execution_time_ms));

        output.push_str(&format!("# HELP oql_cache_hit_rate Cache hit rate\n"));
        output.push_str(&format!("# TYPE oql_cache_hit_rate gauge\n"));
        output.push_str(&format!("oql_cache_hit_rate {}\n", query_stats.cache_hit_rate));

        output.push_str(&format!("# HELP oql_error_rate Error rate\n"));
        output.push_str(&format!("# TYPE oql_error_rate gauge\n"));
        output.push_str(&format!("oql_error_rate {}\n", system_metrics.error_rate));

        output.push_str(&format!("# HELP oql_storage_size_bytes Storage size in bytes\n"));
        output.push_str(&format!("# TYPE oql_storage_size_bytes gauge\n"));
        output.push_str(&format!("oql_storage_size_bytes {}\n", system_metrics.storage_size_bytes));

        output.push_str(&format!("# HELP oql_memory_usage_bytes Memory usage in bytes\n"));
        output.push_str(&format!("# TYPE oql_memory_usage_bytes gauge\n"));
        output.push_str(&format!("oql_memory_usage_bytes {}\n", system_metrics.memory_usage_bytes));

        output.push_str(&format!("# HELP oql_uptime_seconds System uptime in seconds\n"));
        output.push_str(&format!("# TYPE oql_uptime_seconds counter\n"));
        output.push_str(&format!("oql_uptime_seconds {}\n", system_metrics.uptime_seconds));

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Operation, WhereBlock, Condition, Expression, FieldAccess, ComparisonOperator, Literal};

    fn create_test_query() -> OQLQuery {
        OQLQuery {
            collection: "users".to_string(),
            operations: vec![
                Operation::Where(WhereBlock {
                    condition: Condition::Comparison {
                        left: Expression::FieldAccess(FieldAccess {
                            path: vec!["name".to_string()],
                        }),
                        operator: ComparisonOperator::Equal,
                        right: Expression::Literal(Literal::String("Alice".to_string())),
                    },
                }),
            ],
        }
    }

    #[test]
    fn test_query_metrics_creation() {
        let query = create_test_query();
        let metrics = QueryMetrics::new(&query, 150, 100, 5);

        assert_eq!(metrics.schema, "users");
        assert_eq!(metrics.operation_type, "select");
        assert_eq!(metrics.execution_time_ms, 150);
        assert_eq!(metrics.documents_scanned, 100);
        assert_eq!(metrics.documents_returned, 5);
        assert!(metrics.success);
        assert!(metrics.error_message.is_none());
    }

    #[test]
    fn test_monitoring_system_query_metrics() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        let query = create_test_query();
        let metrics = QueryMetrics::new(&query, 150, 100, 5);

        monitoring.record_query_metrics(metrics).unwrap();

        let stats = monitoring.get_query_stats().unwrap();
        assert_eq!(stats.total_queries, 1);
        assert_eq!(stats.successful_queries, 1);
        assert_eq!(stats.avg_execution_time_ms, 150.0);
        assert_eq!(stats.success_rate, 1.0);
    }

    #[test]
    fn test_alert_creation() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        let alert = Alert::new(
            AlertSeverity::Warning,
            "Test Alert".to_string(),
            "This is a test alert".to_string(),
        );

        let alert_id = alert.id.clone();
        monitoring.create_alert(alert).unwrap();

        let active_alerts = monitoring.get_active_alerts().unwrap();
        assert_eq!(active_alerts.len(), 1);
        assert_eq!(active_alerts[0].title, "Test Alert");

        // Resolve the alert
        let resolved = monitoring.resolve_alert(&alert_id).unwrap();
        assert!(resolved);

        let active_alerts = monitoring.get_active_alerts().unwrap();
        assert_eq!(active_alerts.len(), 0);
    }

    #[test]
    fn test_trace_span() {
        let mut span = TraceSpan::new("trace_123".to_string(), "test_operation".to_string());
        
        span = span.tag("user_id".to_string(), "user_123".to_string());
        span.log("info".to_string(), "Starting operation".to_string());
        
        // Simulate some work
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        span.finish();

        assert!(span.end_time.is_some());
        assert!(span.duration_ms.is_some());
        assert!(span.duration_ms.unwrap() >= 10);
        assert_eq!(span.tags.get("user_id"), Some(&"user_123".to_string()));
        assert_eq!(span.logs.len(), 1);
    }

    #[test]
    fn test_prometheus_export() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        let prometheus_output = monitoring.export_prometheus_metrics().unwrap();
        
        assert!(prometheus_output.contains("oql_active_connections"));
        assert!(prometheus_output.contains("oql_queries_per_second"));
        assert!(prometheus_output.contains("oql_avg_query_time_ms"));
        assert!(prometheus_output.contains("# HELP"));
        assert!(prometheus_output.contains("# TYPE"));
    }
}