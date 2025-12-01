// Comprehensive Logging System
// Provides structured logging with correlation IDs across Rust-Zig boundary

use crate::error::{OQLError, OQLResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn, Level};
use uuid::Uuid;

/// Correlation ID for tracking operations across layers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// Create a new correlation ID
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
    
    /// Create from string
    pub fn from_string(s: String) -> Self {
        Self(s)
    }
    
    /// Get the string representation
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

/// Log level
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<LogLevel> for Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => Level::TRACE,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Info => Level::INFO,
            LogLevel::Warn => Level::WARN,
            LogLevel::Error => Level::ERROR,
        }
    }
}

/// Structured log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub correlation_id: CorrelationId,
    pub level: LogLevel,
    pub message: String,
    pub layer: String,  // "rust" or "zig"
    pub component: String,
    pub timestamp_ms: u64,
    pub duration_ms: Option<u64>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl LogEntry {
    /// Create a new log entry
    pub fn new(
        correlation_id: CorrelationId,
        level: LogLevel,
        message: String,
        layer: String,
        component: String,
    ) -> Self {
        Self {
            correlation_id,
            level,
            message,
            layer,
            component,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            duration_ms: None,
            metadata: std::collections::HashMap::new(),
        }
    }
    
    /// Add metadata
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }
    
    /// Set duration
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
    
    /// Log this entry using tracing
    pub fn log(&self) {
        let level: Level = self.level.into();
        
        match level {
            Level::TRACE => debug!(
                correlation_id = %self.correlation_id.as_str(),
                layer = %self.layer,
                component = %self.component,
                duration_ms = ?self.duration_ms,
                "{}",
                self.message
            ),
            Level::DEBUG => debug!(
                correlation_id = %self.correlation_id.as_str(),
                layer = %self.layer,
                component = %self.component,
                duration_ms = ?self.duration_ms,
                "{}",
                self.message
            ),
            Level::INFO => info!(
                correlation_id = %self.correlation_id.as_str(),
                layer = %self.layer,
                component = %self.component,
                duration_ms = ?self.duration_ms,
                "{}",
                self.message
            ),
            Level::WARN => warn!(
                correlation_id = %self.correlation_id.as_str(),
                layer = %self.layer,
                component = %self.component,
                duration_ms = ?self.duration_ms,
                "{}",
                self.message
            ),
            Level::ERROR => error!(
                correlation_id = %self.correlation_id.as_str(),
                layer = %self.layer,
                component = %self.component,
                duration_ms = ?self.duration_ms,
                "{}",
                self.message
            ),
        }
    }
}

/// Logger for a specific component
pub struct ComponentLogger {
    component: String,
    layer: String,
    correlation_id: Arc<std::sync::RwLock<Option<CorrelationId>>>,
}

impl ComponentLogger {
    /// Create a new component logger
    pub fn new(component: String, layer: String) -> Self {
        Self {
            component,
            layer,
            correlation_id: Arc::new(std::sync::RwLock::new(None)),
        }
    }
    
    /// Set correlation ID for this logger
    pub fn set_correlation_id(&self, id: CorrelationId) {
        if let Ok(mut cid) = self.correlation_id.write() {
            *cid = Some(id);
        }
    }
    
    /// Get or create correlation ID
    fn get_correlation_id(&self) -> CorrelationId {
        if let Ok(cid) = self.correlation_id.read() {
            if let Some(ref id) = *cid {
                return id.clone();
            }
        }
        CorrelationId::new()
    }
    
    /// Log a message
    pub fn log(&self, level: LogLevel, message: String) {
        let entry = LogEntry::new(
            self.get_correlation_id(),
            level,
            message,
            self.layer.clone(),
            self.component.clone(),
        );
        entry.log();
    }
    
    /// Log with metadata
    pub fn log_with_metadata(
        &self,
        level: LogLevel,
        message: String,
        metadata: std::collections::HashMap<String, String>,
    ) {
        let mut entry = LogEntry::new(
            self.get_correlation_id(),
            level,
            message,
            self.layer.clone(),
            self.component.clone(),
        );
        entry.metadata = metadata;
        entry.log();
    }
    
    /// Create a timed operation
    pub fn timed_operation(&self, operation: String) -> TimedOperation {
        TimedOperation {
            logger: self,
            operation,
            start: Instant::now(),
        }
    }
    
    /// Convenience methods
    pub fn trace(&self, message: String) {
        self.log(LogLevel::Trace, message);
    }
    
    pub fn debug(&self, message: String) {
        self.log(LogLevel::Debug, message);
    }
    
    pub fn info(&self, message: String) {
        self.log(LogLevel::Info, message);
    }
    
    pub fn warn(&self, message: String) {
        self.log(LogLevel::Warn, message);
    }
    
    pub fn error(&self, message: String) {
        self.log(LogLevel::Error, message);
    }
}

/// A timed operation that logs duration on completion
pub struct TimedOperation<'a> {
    logger: &'a ComponentLogger,
    operation: String,
    start: Instant,
}

impl<'a> TimedOperation<'a> {
    /// Finish the operation and log duration
    pub fn finish(self) {
        let duration = self.start.elapsed();
        let entry = LogEntry::new(
            self.logger.get_correlation_id(),
            LogLevel::Info,
            format!("Operation '{}' completed", self.operation),
            self.logger.layer.clone(),
            self.logger.component.clone(),
        )
        .with_duration(duration.as_millis() as u64);
        
        entry.log();
    }
    
    /// Finish with error
    pub fn finish_with_error(self, error: &str) {
        let duration = self.start.elapsed();
        let entry = LogEntry::new(
            self.logger.get_correlation_id(),
            LogLevel::Error,
            format!("Operation '{}' failed: {}", self.operation, error),
            self.logger.layer.clone(),
            self.logger.component.clone(),
        )
        .with_duration(duration.as_millis() as u64);
        
        entry.log();
    }
}

impl<'a> Drop for TimedOperation<'a> {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        let entry = LogEntry::new(
            self.logger.get_correlation_id(),
            LogLevel::Debug,
            format!("Operation '{}' completed (auto)", self.operation),
            self.logger.layer.clone(),
            self.logger.component.clone(),
        )
        .with_duration(duration.as_millis() as u64);
        
        entry.log();
    }
}

/// Query execution logger
pub struct QueryLogger {
    logger: ComponentLogger,
}

impl QueryLogger {
    /// Create a new query logger
    pub fn new() -> Self {
        Self {
            logger: ComponentLogger::new("query_executor".to_string(), "rust".to_string()),
        }
    }
    
    /// Log query start
    pub fn log_query_start(&self, query: &str, correlation_id: CorrelationId) {
        self.logger.set_correlation_id(correlation_id);
        self.logger.info(format!("Query started: {}", query));
    }
    
    /// Log query completion
    pub fn log_query_complete(&self, query: &str, duration_ms: u64, rows_returned: usize) {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("query".to_string(), query.to_string());
        metadata.insert("duration_ms".to_string(), duration_ms.to_string());
        metadata.insert("rows_returned".to_string(), rows_returned.to_string());
        
        self.logger.log_with_metadata(
            LogLevel::Info,
            format!("Query completed in {}ms, returned {} rows", duration_ms, rows_returned),
            metadata,
        );
    }
    
    /// Log slow query
    pub fn log_slow_query(&self, query: &str, duration_ms: u64, threshold_ms: u64) {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("query".to_string(), query.to_string());
        metadata.insert("duration_ms".to_string(), duration_ms.to_string());
        metadata.insert("threshold_ms".to_string(), threshold_ms.to_string());
        
        self.logger.log_with_metadata(
            LogLevel::Warn,
            format!(
                "Slow query detected: {}ms (threshold: {}ms)",
                duration_ms, threshold_ms
            ),
            metadata,
        );
    }
    
    /// Log query error
    pub fn log_query_error(&self, query: &str, error: &str) {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("query".to_string(), query.to_string());
        metadata.insert("error".to_string(), error.to_string());
        
        self.logger.log_with_metadata(
            LogLevel::Error,
            format!("Query failed: {}", error),
            metadata,
        );
    }
}

impl Default for QueryLogger {
    fn default() -> Self {
        Self::new()
    }
}

/// Storage operation logger
pub struct StorageLogger {
    logger: ComponentLogger,
}

impl StorageLogger {
    /// Create a new storage logger
    pub fn new() -> Self {
        Self {
            logger: ComponentLogger::new("storage".to_string(), "rust".to_string()),
        }
    }
    
    /// Log storage operation
    pub fn log_operation(&self, operation: &str, correlation_id: CorrelationId) {
        self.logger.set_correlation_id(correlation_id);
        self.logger.debug(format!("Storage operation: {}", operation));
    }
    
    /// Log storage statistics
    pub fn log_statistics(&self, stats: &str) {
        self.logger.info(format!("Storage statistics: {}", stats));
    }
}

impl Default for StorageLogger {
    fn default() -> Self {
        Self::new()
    }
}

// FFI for Zig layer logging
#[repr(C)]
pub struct ZigLogEntry {
    correlation_id: [u8; 36],  // UUID string
    level: u8,  // 0=trace, 1=debug, 2=info, 3=warn, 4=error
    message: *const u8,
    message_len: usize,
    component: *const u8,
    component_len: usize,
    duration_ms: u64,
}

/// Log from Zig layer
#[no_mangle]
pub extern "C" fn owl_log_from_zig(entry: *const ZigLogEntry) {
    if entry.is_null() {
        return;
    }
    
    unsafe {
        let entry_ref = &*entry;
        
        // Parse correlation ID
        let correlation_id = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(entry_ref.correlation_id.as_ptr(), 36)
        );
        
        // Parse message
        let message = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(entry_ref.message, entry_ref.message_len)
        );
        
        // Parse component
        let component = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(entry_ref.component, entry_ref.component_len)
        );
        
        // Convert level
        let level = match entry_ref.level {
            0 => LogLevel::Trace,
            1 => LogLevel::Debug,
            2 => LogLevel::Info,
            3 => LogLevel::Warn,
            4 => LogLevel::Error,
            _ => LogLevel::Info,
        };
        
        // Create and log entry
        let log_entry = LogEntry::new(
            CorrelationId::from_string(correlation_id.to_string()),
            level,
            message.to_string(),
            "zig".to_string(),
            component.to_string(),
        )
        .with_duration(entry_ref.duration_ms);
        
        log_entry.log();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_correlation_id() {
        let id1 = CorrelationId::new();
        let id2 = CorrelationId::new();
        
        assert_ne!(id1.as_str(), id2.as_str());
        assert_eq!(id1.as_str().len(), 36); // UUID length
    }
    
    #[test]
    fn test_log_entry() {
        let entry = LogEntry::new(
            CorrelationId::new(),
            LogLevel::Info,
            "Test message".to_string(),
            "rust".to_string(),
            "test".to_string(),
        );
        
        assert_eq!(entry.message, "Test message");
        assert_eq!(entry.layer, "rust");
        assert_eq!(entry.component, "test");
    }
    
    #[test]
    fn test_component_logger() {
        let logger = ComponentLogger::new("test".to_string(), "rust".to_string());
        logger.info("Test message".to_string());
        logger.debug("Debug message".to_string());
        logger.warn("Warning message".to_string());
    }
    
    #[test]
    fn test_timed_operation() {
        let logger = ComponentLogger::new("test".to_string(), "rust".to_string());
        let op = logger.timed_operation("test_operation".to_string());
        std::thread::sleep(std::time::Duration::from_millis(10));
        op.finish();
    }
}
