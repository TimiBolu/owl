//! Cross-Layer Error Handling (Rust)
//!
//! Task 8.13: Implement Rust-Zig transaction coordination (Rust + Zig)
//!
//! This module provides error handling and context preservation across the
//! Rust-Zig language boundary. It ensures that errors from the Zig layer
//! are properly propagated and contextualized in the Rust layer, and vice versa.

use crate::error::{ErrorContext, OQLError, OQLResult};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::{Arc, Mutex};

/// Error codes that can be returned from Zig layer
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZigErrorCode {
    Success = 0,
    OutOfMemory = -1,
    InvalidTransaction = -2,
    TransactionNotFound = -3,
    TransactionNotActive = -4,
    TransactionTimedOut = -5,
    PrepareFailure = -6,
    CommitFailure = -7,
    AbortFailure = -8,
    InvalidOperation = -9,
    InvalidShard = -10,
    ShardNotFound = -11,
    WALError = -12,
    StorageError = -13,
    NetworkError = -14,
    SerializationError = -15,
    DeserializationError = -16,
    ChecksumMismatch = -17,
    CorruptedData = -18,
    InsufficientSpace = -19,
    LockConflict = -20,
    DeadlockDetected = -21,
    Unknown = -999,
}

impl ZigErrorCode {
    /// Convert from C integer to error code
    pub fn from_c_int(code: c_int) -> Self {
        match code {
            0 => Self::Success,
            -1 => Self::OutOfMemory,
            -2 => Self::InvalidTransaction,
            -3 => Self::TransactionNotFound,
            -4 => Self::TransactionNotActive,
            -5 => Self::TransactionTimedOut,
            -6 => Self::PrepareFailure,
            -7 => Self::CommitFailure,
            -8 => Self::AbortFailure,
            -9 => Self::InvalidOperation,
            -10 => Self::InvalidShard,
            -11 => Self::ShardNotFound,
            -12 => Self::WALError,
            -13 => Self::StorageError,
            -14 => Self::NetworkError,
            -15 => Self::SerializationError,
            -16 => Self::DeserializationError,
            -17 => Self::ChecksumMismatch,
            -18 => Self::CorruptedData,
            -19 => Self::InsufficientSpace,
            -20 => Self::LockConflict,
            -21 => Self::DeadlockDetected,
            _ => Self::Unknown,
        }
    }

    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Operation completed successfully",
            Self::OutOfMemory => "Out of memory",
            Self::InvalidTransaction => "Invalid transaction",
            Self::TransactionNotFound => "Transaction not found",
            Self::TransactionNotActive => "Transaction is not active",
            Self::TransactionTimedOut => "Transaction timed out",
            Self::PrepareFailure => "Transaction prepare failed",
            Self::CommitFailure => "Transaction commit failed",
            Self::AbortFailure => "Transaction abort failed",
            Self::InvalidOperation => "Invalid operation",
            Self::InvalidShard => "Invalid shard",
            Self::ShardNotFound => "Shard not found",
            Self::WALError => "Write-ahead log error",
            Self::StorageError => "Storage layer error",
            Self::NetworkError => "Network communication error",
            Self::SerializationError => "Data serialization error",
            Self::DeserializationError => "Data deserialization error",
            Self::ChecksumMismatch => "Data checksum mismatch",
            Self::CorruptedData => "Corrupted data detected",
            Self::InsufficientSpace => "Insufficient storage space",
            Self::LockConflict => "Lock conflict detected",
            Self::DeadlockDetected => "Deadlock detected",
            Self::Unknown => "Unknown error",
        }
    }

    /// Check if this represents a success
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    /// Check if this represents a transient error that might be retryable
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::TransactionTimedOut
                | Self::NetworkError
                | Self::LockConflict
                | Self::InsufficientSpace
        )
    }

    /// Check if this represents a fatal error that requires abort
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::CorruptedData
                | Self::ChecksumMismatch
                | Self::DeadlockDetected
                | Self::OutOfMemory
        )
    }
}

/// Error information from Zig layer
#[derive(Debug, Clone)]
pub struct ZigError {
    /// Error code
    pub code: ZigErrorCode,
    /// Error message from Zig layer
    pub message: String,
    /// Shard ID where error occurred (if applicable)
    pub shard_id: Option<u32>,
    /// Transaction ID where error occurred (if applicable)
    pub transaction_id: Option<u64>,
    /// Additional context information
    pub context: HashMap<String, String>,
}

impl ZigError {
    /// Create a new Zig error
    pub fn new(code: ZigErrorCode, message: String) -> Self {
        Self {
            code,
            message,
            shard_id: None,
            transaction_id: None,
            context: HashMap::new(),
        }
    }

    /// Create from extended Zig error information
    pub fn from_extended_info(
        code: ZigErrorCode,
        message: String,
        shard_id: Option<u32>,
        transaction_id: Option<u64>,
        context: HashMap<String, String>,
    ) -> Self {
        Self {
            code,
            message,
            shard_id,
            transaction_id,
            context,
        }
    }

    /// Add shard context
    pub fn with_shard(mut self, shard_id: u32) -> Self {
        self.shard_id = Some(shard_id);
        self
    }

    /// Add transaction context
    pub fn with_transaction(mut self, transaction_id: u64) -> Self {
        self.transaction_id = Some(transaction_id);
        self
    }

    /// Add additional context
    pub fn with_context(mut self, key: String, value: String) -> Self {
        self.context.insert(key, value);
        self
    }

    /// Convert to OQL error
    pub fn to_oql_error(&self) -> OQLError {
        let mut error_context = ErrorContext::new("zig_layer_error")
            .with_location("cross_layer_boundary")
            .with_info("zig_error_code", &format!("{:?}", self.code))
            .with_info("zig_error_message", &self.message);

        if let Some(shard_id) = self.shard_id {
            error_context = error_context.with_info("shard_id", &shard_id.to_string());
        }

        if let Some(transaction_id) = self.transaction_id {
            error_context = error_context.with_info("transaction_id", &transaction_id.to_string());
        }

        for (key, value) in &self.context {
            error_context = error_context.with_info(key, value);
        }

        match self.code {
            ZigErrorCode::Success => {
                // This shouldn't happen, but handle gracefully
                OQLError::Generic {
                    message: "Unexpected success code in error context".to_string(),
                }
            }
            ZigErrorCode::OutOfMemory => OQLError::storage_error(
                &format!("Zig layer out of memory: {}", self.message),
                Some(error_context),
            ),
            ZigErrorCode::InvalidTransaction
            | ZigErrorCode::TransactionNotFound
            | ZigErrorCode::TransactionNotActive
            | ZigErrorCode::TransactionTimedOut
            | ZigErrorCode::PrepareFailure
            | ZigErrorCode::CommitFailure
            | ZigErrorCode::AbortFailure => OQLError::transaction_error(
                &format!("Zig transaction error: {}", self.message),
                Some(error_context),
            ),
            ZigErrorCode::LockConflict | ZigErrorCode::DeadlockDetected => {
                OQLError::concurrency_error(
                    &format!("Zig concurrency error: {}", self.message),
                    Some(error_context),
                )
            }
            ZigErrorCode::WALError
            | ZigErrorCode::StorageError
            | ZigErrorCode::ChecksumMismatch
            | ZigErrorCode::CorruptedData
            | ZigErrorCode::InsufficientSpace => OQLError::storage_error(
                &format!("Zig storage error: {}", self.message),
                Some(error_context),
            ),
            ZigErrorCode::NetworkError => OQLError::network_error(
                &format!("Zig network error: {}", self.message),
                Some(error_context),
            ),
            ZigErrorCode::SerializationError | ZigErrorCode::DeserializationError => {
                OQLError::serialization_error(
                    &format!("Zig serialization error: {}", self.message),
                    Some(error_context),
                )
            }
            _ => OQLError::Generic {
                message: format!("Zig layer error: {} ({})", self.message, self.code.description()),
            },
        }
    }
}

/// Cross-layer error manager
pub struct CrossLayerErrorManager {
    /// Error context stack for nested operations
    error_stack: Arc<Mutex<Vec<ErrorContext>>>,
    /// Error statistics
    error_stats: Arc<Mutex<ErrorStatistics>>,
}

/// Error statistics across layers
#[derive(Debug, Clone, Default)]
pub struct ErrorStatistics {
    pub total_errors: u64,
    pub rust_errors: u64,
    pub zig_errors: u64,
    pub transient_errors: u64,
    pub fatal_errors: u64,
    pub errors_by_code: HashMap<String, u64>,
}

impl CrossLayerErrorManager {
    /// Create a new cross-layer error manager
    pub fn new() -> Self {
        // Initialize Zig error handling system
        unsafe {
            zig_error_init();
        }

        Self {
            error_stack: Arc::new(Mutex::new(Vec::new())),
            error_stats: Arc::new(Mutex::new(ErrorStatistics::default())),
        }
    }

    /// Cleanup the error manager
    pub fn cleanup(&self) {
        unsafe {
            zig_error_cleanup();
        }
    }

    /// Push error context onto the stack
    pub fn push_context(&self, context: ErrorContext) -> OQLResult<()> {
        let mut stack = self.error_stack.lock().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire error stack lock", None)
        })?;
        stack.push(context);
        Ok(())
    }

    /// Pop error context from the stack
    pub fn pop_context(&self) -> Option<ErrorContext> {
        let mut stack = self.error_stack.lock().ok()?;
        stack.pop()
    }

    /// Get current error context (top of stack)
    pub fn current_context(&self) -> Option<ErrorContext> {
        let stack = self.error_stack.lock().ok()?;
        stack.last().cloned()
    }

    /// Handle a Zig error and convert it to OQL error with context
    pub fn handle_zig_error(&self, zig_error: ZigError) -> OQLError {
        // Update statistics
        if let Ok(mut stats) = self.error_stats.lock() {
            stats.total_errors += 1;
            stats.zig_errors += 1;

            if zig_error.code.is_transient() {
                stats.transient_errors += 1;
            }
            if zig_error.code.is_fatal() {
                stats.fatal_errors += 1;
            }

            let code_key = format!("{:?}", zig_error.code);
            *stats.errors_by_code.entry(code_key).or_insert(0) += 1;
        }

        // Convert to OQL error with additional context from stack
        let mut oql_error = zig_error.to_oql_error();

        // Add context from error stack if available
        if let Some(current_context) = self.current_context() {
            oql_error = match oql_error {
                OQLError::TransactionError { message, context: _ } => {
                    OQLError::transaction_error(&message, Some(current_context))
                }
                OQLError::ConcurrencyError { message, context: _ } => {
                    OQLError::concurrency_error(&message, Some(current_context))
                }
                OQLError::StorageError { message, context: _ } => {
                    OQLError::storage_error(&message, Some(current_context))
                }
                OQLError::NetworkError { message, context: _ } => {
                    OQLError::network_error(&message, Some(current_context))
                }
                OQLError::SerializationError { message, context: _ } => {
                    OQLError::serialization_error(&message, Some(current_context))
                }
                other => other,
            };
        }

        oql_error
    }

    /// Handle a Rust error and update statistics
    pub fn handle_rust_error(&self, error: &OQLError) {
        if let Ok(mut stats) = self.error_stats.lock() {
            stats.total_errors += 1;
            stats.rust_errors += 1;

            // Categorize Rust errors
            let error_type = match error {
                OQLError::TransactionError { .. } => "TransactionError",
                OQLError::ConcurrencyError { .. } => "ConcurrencyError",
                OQLError::StorageError { .. } => "StorageError",
                OQLError::NetworkError { .. } => "NetworkError",
                OQLError::SerializationError { .. } => "SerializationError",
                OQLError::ParseError { .. } => "ParseError",
                OQLError::ValidationError { .. } => "ValidationError",
                OQLError::Generic { .. } => "Generic",
            };

            *stats.errors_by_code.entry(error_type.to_string()).or_insert(0) += 1;
        }
    }

    /// Get error statistics
    pub fn get_statistics(&self) -> OQLResult<ErrorStatistics> {
        let stats = self.error_stats.lock().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire error stats lock", None)
        })?;
        Ok(stats.clone())
    }

    /// Clear error statistics
    pub fn clear_statistics(&self) -> OQLResult<()> {
        let mut stats = self.error_stats.lock().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire error stats lock", None)
        })?;
        *stats = ErrorStatistics::default();
        Ok(())
    }
}

impl Default for CrossLayerErrorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CrossLayerErrorManager {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Error propagation utilities
pub struct ErrorPropagation;

impl ErrorPropagation {
    /// Propagate error from Zig to Rust with additional context
    pub fn propagate_with_context(
        manager: &CrossLayerErrorManager,
        shard_id: u32,
        operation: &str,
        location: &str,
    ) -> OQLResult<()> {
        if let Some(zig_error) = manager.get_zig_extended_error(shard_id)? {
            let context = ErrorContext::new(operation)
                .with_location(location)
                .with_info("shard_id", &shard_id.to_string())
                .with_info("error_source", "zig_layer");

            let mut oql_error = zig_error.to_oql_error();
            
            // Enhance with additional context
            oql_error = match oql_error {
                OQLError::TransactionError { message, .. } => {
                    OQLError::transaction_error(&message, Some(context))
                }
                OQLError::StorageError { message, .. } => {
                    OQLError::storage_error(&message, Some(context))
                }
                OQLError::ConcurrencyError { message, .. } => {
                    OQLError::concurrency_error(&message, Some(context))
                }
                OQLError::NetworkError { message, .. } => {
                    OQLError::network_error(&message, Some(context))
                }
                other => other,
            };

            Err(manager.handle_zig_error(zig_error))
        } else {
            Ok(())
        }
    }

    /// Chain errors from multiple operations
    pub fn chain_errors(primary: OQLError, secondary: Vec<OQLError>) -> OQLError {
        let mut context = primary.context().cloned().unwrap_or_else(|| {
            ErrorContext::new("chained_operation")
        });

        for (i, error) in secondary.iter().enumerate() {
            context = context.with_info(
                &format!("chained_error_{}", i),
                &error.to_string(),
            );
        }

        match primary {
            OQLError::TransactionError { message, .. } => {
                OQLError::transaction_error(&message, Some(context))
            }
            OQLError::StorageError { message, .. } => {
                OQLError::storage_error(&message, Some(context))
            }
            OQLError::ConcurrencyError { message, .. } => {
                OQLError::concurrency_error(&message, Some(context))
            }
            OQLError::NetworkError { message, .. } => {
                OQLError::network_error(&message, Some(context))
            }
            other => other,
        }
    }
}

/// Batch error handling utilities
pub struct BatchErrorHandling;

impl BatchErrorHandling {
    /// Execute multiple operations and collect errors
    pub fn execute_batch<F, R>(
        manager: &CrossLayerErrorManager,
        shard_id: u32,
        operations: Vec<(&str, F)>,
    ) -> (Vec<R>, Vec<OQLError>)
    where
        F: FnOnce() -> OQLResult<R>,
    {
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for (operation_name, operation) in operations {
            let context = ErrorContext::new(operation_name)
                .with_info("shard_id", &shard_id.to_string())
                .with_info("execution_type", "batch");

            let _ = manager.push_context(context);

            match operation() {
                Ok(result) => {
                    // Check for Zig errors even on success
                    if let Err(zig_error) = manager.check_zig_error(shard_id) {
                        errors.push(zig_error);
                    } else {
                        results.push(result);
                    }
                }
                Err(error) => {
                    errors.push(error);
                }
            }

            manager.pop_context();
        }

        (results, errors)
    }

    /// Execute operations with partial failure tolerance
    pub fn execute_with_partial_failure<F, R>(
        manager: &CrossLayerErrorManager,
        shard_id: u32,
        operation_name: &str,
        operations: Vec<F>,
        failure_threshold: f32, // 0.0 to 1.0
    ) -> OQLResult<Vec<R>>
    where
        F: FnOnce() -> OQLResult<R>,
    {
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for (i, operation) in operations.into_iter().enumerate() {
            let context = ErrorContext::new(operation_name)
                .with_info("shard_id", &shard_id.to_string())
                .with_info("batch_index", &i.to_string());

            let _ = manager.push_context(context);

            match operation() {
                Ok(result) => {
                    if let Err(zig_error) = manager.check_zig_error(shard_id) {
                        errors.push(zig_error);
                    } else {
                        results.push(result);
                    }
                }
                Err(error) => {
                    errors.push(error);
                }
            }

            manager.pop_context();
        }

        let total_operations = results.len() + errors.len();
        let failure_rate = errors.len() as f32 / total_operations as f32;

        if failure_rate > failure_threshold {
            let context = ErrorContext::new(operation_name)
                .with_info("shard_id", &shard_id.to_string())
                .with_info("failure_rate", &failure_rate.to_string())
                .with_info("threshold", &failure_threshold.to_string())
                .with_info("total_operations", &total_operations.to_string())
                .with_info("failed_operations", &errors.len().to_string());

            Err(OQLError::storage_error(
                &format!(
                    "Batch operation failed: {:.1}% failure rate exceeds {:.1}% threshold",
                    failure_rate * 100.0,
                    failure_threshold * 100.0
                ),
                Some(context),
            ))
        } else {
            Ok(results)
        }
    }
}

/// FFI functions for error handling from Zig layer
extern "C" {
    /// Get last error from Zig layer
    fn zig_get_last_error(
        shard_id: u32,
        error_code: *mut c_int,
        message_buffer: *mut c_char,
        buffer_size: usize,
    ) -> bool;

    /// Get extended error information from Zig layer
    fn zig_get_extended_error_info(
        shard_id: u32,
        error_code: *mut c_int,
        severity: *mut u8,
        shard_id_out: *mut u32,
        transaction_id_out: *mut u64,
        has_transaction: *mut bool,
        timestamp_ns: *mut u64,
        message_buffer: *mut c_char,
        message_buffer_size: usize,
        context_keys: *mut [[c_char; 64]; 16],
        context_values: *mut [[c_char; 256]; 16],
        context_count: *mut u8,
    ) -> bool;

    /// Clear last error for a shard
    fn zig_clear_error(shard_id: u32);

    /// Initialize Zig error handling system
    fn zig_error_init() -> bool;

    /// Cleanup Zig error handling system
    fn zig_error_cleanup();
}

/// Helper functions for FFI error handling
impl CrossLayerErrorManager {
    /// Get the last error from Zig layer for a specific shard
    pub fn get_zig_last_error(&self, shard_id: u32) -> OQLResult<Option<ZigError>> {
        let mut error_code: c_int = 0;
        let mut message_buffer = vec![0u8; 1024]; // 1KB buffer for error message

        let has_error = unsafe {
            zig_get_last_error(
                shard_id,
                &mut error_code,
                message_buffer.as_mut_ptr() as *mut c_char,
                message_buffer.len(),
            )
        };

        if !has_error {
            return Ok(None);
        }

        let zig_error_code = ZigErrorCode::from_c_int(error_code);
        if zig_error_code.is_success() {
            return Ok(None);
        }

        // Convert C string to Rust string
        let message = unsafe {
            CStr::from_ptr(message_buffer.as_ptr() as *const c_char)
                .to_string_lossy()
                .into_owned()
        };

        let zig_error = ZigError::new(zig_error_code, message).with_shard(shard_id);

        Ok(Some(zig_error))
    }

    /// Get extended error information from Zig layer
    pub fn get_zig_extended_error(&self, shard_id: u32) -> OQLResult<Option<ZigError>> {
        let mut error_code: c_int = 0;
        let mut severity: u8 = 0;
        let mut shard_id_out: u32 = 0;
        let mut transaction_id_out: u64 = 0;
        let mut has_transaction: bool = false;
        let mut timestamp_ns: u64 = 0;
        let mut message_buffer = vec![0u8; 1024];
        let mut context_keys: [[c_char; 64]; 16] = [[0; 64]; 16];
        let mut context_values: [[c_char; 256]; 16] = [[0; 256]; 16];
        let mut context_count: u8 = 0;

        let has_error = unsafe {
            zig_get_extended_error_info(
                shard_id,
                &mut error_code,
                &mut severity,
                &mut shard_id_out,
                &mut transaction_id_out,
                &mut has_transaction,
                &mut timestamp_ns,
                message_buffer.as_mut_ptr() as *mut c_char,
                message_buffer.len(),
                &mut context_keys,
                &mut context_values,
                &mut context_count,
            )
        };

        if !has_error {
            return Ok(None);
        }

        let zig_error_code = ZigErrorCode::from_c_int(error_code);
        if zig_error_code.is_success() {
            return Ok(None);
        }

        // Convert message
        let message = unsafe {
            CStr::from_ptr(message_buffer.as_ptr() as *const c_char)
                .to_string_lossy()
                .into_owned()
        };

        // Build context map
        let mut context = HashMap::new();
        for i in 0..context_count as usize {
            let key = unsafe {
                CStr::from_ptr(context_keys[i].as_ptr())
                    .to_string_lossy()
                    .into_owned()
            };
            let value = unsafe {
                CStr::from_ptr(context_values[i].as_ptr())
                    .to_string_lossy()
                    .into_owned()
            };
            context.insert(key, value);
        }

        // Add additional context
        context.insert("severity".to_string(), severity.to_string());
        context.insert("timestamp_ns".to_string(), timestamp_ns.to_string());

        let zig_error = ZigError::from_extended_info(
            zig_error_code,
            message,
            Some(shard_id_out),
            if has_transaction { Some(transaction_id_out) } else { None },
            context,
        );

        Ok(Some(zig_error))
    }

    /// Clear error in Zig layer
    pub fn clear_zig_error(&self, shard_id: u32) {
        unsafe {
            zig_clear_error(shard_id);
        }
    }

    /// Check for errors from Zig layer and convert to OQL error if found
    pub fn check_zig_error(&self, shard_id: u32) -> OQLResult<()> {
        // Try to get extended error information first
        if let Some(zig_error) = self.get_zig_extended_error(shard_id)? {
            Err(self.handle_zig_error(zig_error))
        } else if let Some(zig_error) = self.get_zig_last_error(shard_id)? {
            Err(self.handle_zig_error(zig_error))
        } else {
            Ok(())
        }
    }

    /// Check for errors and clear them from Zig layer
    pub fn check_and_clear_zig_error(&self, shard_id: u32) -> OQLResult<()> {
        let result = self.check_zig_error(shard_id);
        self.clear_zig_error(shard_id);
        result
    }

    /// Execute a Zig operation and handle any errors
    pub fn execute_with_error_handling<F, R>(&self, shard_id: u32, operation: F) -> OQLResult<R>
    where
        F: FnOnce() -> R,
    {
        // Push operation context
        let context = ErrorContext::new("zig_operation")
            .with_location("cross_layer_boundary")
            .with_info("shard_id", &shard_id.to_string());
        self.push_context(context)?;

        // Execute operation
        let result = operation();

        // Check for Zig errors
        let error_check_result = self.check_zig_error(shard_id);

        // Pop context
        self.pop_context();

        // Return error if found, otherwise return result
        error_check_result.map(|_| result)
    }
}

/// Macro for wrapping Zig FFI calls with error handling
#[macro_export]
macro_rules! zig_call_with_error_handling {
    ($error_manager:expr, $shard_id:expr, $call:expr) => {{
        let result = $call;
        $error_manager.check_zig_error($shard_id)?;
        result
    }};
}

/// Macro for wrapping Zig FFI calls with automatic error clearing
#[macro_export]
macro_rules! zig_call_with_cleanup {
    ($error_manager:expr, $shard_id:expr, $call:expr) => {{
        let result = $call;
        $error_manager.check_and_clear_zig_error($shard_id)?;
        result
    }};
}

/// Macro for executing operations with error context
#[macro_export]
macro_rules! with_error_context {
    ($error_manager:expr, $context:expr, $operation:expr) => {{
        $error_manager.push_context($context)?;
        let result = $operation;
        $error_manager.pop_context();
        result
    }};
}

/// Recovery strategies for different types of errors
pub struct ErrorRecoveryStrategy;

impl ErrorRecoveryStrategy {
    /// Determine if an error is recoverable and suggest recovery action
    pub fn analyze_error(error: &ZigError) -> RecoveryAction {
        match error.code {
            ZigErrorCode::Success => RecoveryAction::None,
            ZigErrorCode::TransactionTimedOut => RecoveryAction::Retry {
                max_attempts: 3,
                backoff_ms: 1000,
            },
            ZigErrorCode::LockConflict => RecoveryAction::Retry {
                max_attempts: 5,
                backoff_ms: 100,
            },
            ZigErrorCode::NetworkError => RecoveryAction::Retry {
                max_attempts: 3,
                backoff_ms: 2000,
            },
            ZigErrorCode::InsufficientSpace => RecoveryAction::Cleanup,
            ZigErrorCode::DeadlockDetected => RecoveryAction::Abort,
            ZigErrorCode::CorruptedData | ZigErrorCode::ChecksumMismatch => RecoveryAction::Fatal,
            ZigErrorCode::OutOfMemory => RecoveryAction::Fatal,
            _ => RecoveryAction::Abort,
        }
    }

    /// Attempt automatic recovery for an error
    pub fn attempt_recovery(error: &ZigError) -> Option<String> {
        match Self::analyze_error(error) {
            RecoveryAction::Retry { max_attempts, backoff_ms } => {
                Some(format!(
                    "Retrying operation (max {} attempts, {}ms backoff)",
                    max_attempts, backoff_ms
                ))
            }
            RecoveryAction::Cleanup => {
                Some("Attempting to clean up resources and retry".to_string())
            }
            _ => None,
        }
    }

    /// Get detailed recovery instructions
    pub fn get_recovery_instructions(error: &ZigError) -> Vec<String> {
        let mut instructions = Vec::new();

        match error.code {
            ZigErrorCode::TransactionTimedOut => {
                instructions.push("Consider increasing transaction timeout".to_string());
                instructions.push("Check for long-running operations in transaction".to_string());
                instructions.push("Retry with exponential backoff".to_string());
            }
            ZigErrorCode::LockConflict => {
                instructions.push("Retry the operation after a brief delay".to_string());
                instructions.push("Consider using optimistic locking".to_string());
                instructions.push("Check for deadlock patterns in application logic".to_string());
            }
            ZigErrorCode::OutOfMemory => {
                instructions.push("Increase available memory".to_string());
                instructions.push("Check for memory leaks".to_string());
                instructions.push("Consider reducing batch sizes".to_string());
            }
            ZigErrorCode::CorruptedData => {
                instructions.push("Restore from backup".to_string());
                instructions.push("Run data integrity checks".to_string());
                instructions.push("Contact system administrator".to_string());
            }
            ZigErrorCode::WALError => {
                instructions.push("Check disk space and permissions".to_string());
                instructions.push("Verify WAL directory is writable".to_string());
                instructions.push("Check for filesystem errors".to_string());
            }
            ZigErrorCode::NetworkError => {
                instructions.push("Check network connectivity".to_string());
                instructions.push("Verify shard endpoints are reachable".to_string());
                instructions.push("Retry with exponential backoff".to_string());
            }
            _ => {
                instructions.push("Check system logs for more details".to_string());
                instructions.push("Contact system administrator if problem persists".to_string());
            }
        }

        instructions
    }
}

/// Recovery action recommendations
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryAction {
    /// No recovery needed
    None,
    /// Retry the operation
    Retry { max_attempts: u32, backoff_ms: u64 },
    /// Clean up resources and retry
    Cleanup,
    /// Abort the current transaction
    Abort,
    /// Fatal error - system needs attention
    Fatal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zig_error_code_conversion() {
        assert_eq!(ZigErrorCode::from_c_int(0), ZigErrorCode::Success);
        assert_eq!(ZigErrorCode::from_c_int(-1), ZigErrorCode::OutOfMemory);
        assert_eq!(ZigErrorCode::from_c_int(-2), ZigErrorCode::InvalidTransaction);
        assert_eq!(ZigErrorCode::from_c_int(-999), ZigErrorCode::Unknown);
        assert_eq!(ZigErrorCode::from_c_int(-12345), ZigErrorCode::Unknown);
    }

    #[test]
    fn test_zig_error_properties() {
        assert!(ZigErrorCode::Success.is_success());
        assert!(!ZigErrorCode::OutOfMemory.is_success());

        assert!(ZigErrorCode::TransactionTimedOut.is_transient());
        assert!(ZigErrorCode::NetworkError.is_transient());
        assert!(!ZigErrorCode::CorruptedData.is_transient());

        assert!(ZigErrorCode::CorruptedData.is_fatal());
        assert!(ZigErrorCode::DeadlockDetected.is_fatal());
        assert!(!ZigErrorCode::TransactionTimedOut.is_fatal());
    }

    #[test]
    fn test_zig_error_creation() {
        let error = ZigError::new(
            ZigErrorCode::TransactionNotFound,
            "Transaction 123 not found".to_string(),
        )
        .with_shard(0)
        .with_transaction(123)
        .with_context("operation".to_string(), "commit".to_string());

        assert_eq!(error.code, ZigErrorCode::TransactionNotFound);
        assert_eq!(error.message, "Transaction 123 not found");
        assert_eq!(error.shard_id, Some(0));
        assert_eq!(error.transaction_id, Some(123));
        assert_eq!(error.context.get("operation"), Some(&"commit".to_string()));
    }

    #[test]
    fn test_zig_error_to_oql_error() {
        let zig_error = ZigError::new(
            ZigErrorCode::TransactionTimedOut,
            "Transaction timed out after 30 seconds".to_string(),
        )
        .with_shard(1)
        .with_transaction(456);

        let oql_error = zig_error.to_oql_error();

        match oql_error {
            OQLError::TransactionError { message, context } => {
                assert!(message.contains("Transaction timed out"));
                assert!(context.is_some());
            }
            _ => panic!("Expected TransactionError"),
        }
    }

    #[test]
    fn test_cross_layer_error_manager() {
        let manager = CrossLayerErrorManager::new();

        // Test context stack
        let context1 = ErrorContext::new("test_operation_1");
        let context2 = ErrorContext::new("test_operation_2");

        manager.push_context(context1.clone()).unwrap();
        manager.push_context(context2.clone()).unwrap();

        assert_eq!(manager.current_context().unwrap().operation, "test_operation_2");

        let popped = manager.pop_context().unwrap();
        assert_eq!(popped.operation, "test_operation_2");

        assert_eq!(manager.current_context().unwrap().operation, "test_operation_1");
    }

    #[test]
    fn test_error_statistics() {
        let manager = CrossLayerErrorManager::new();

        // Handle some Zig errors
        let zig_error1 = ZigError::new(ZigErrorCode::TransactionTimedOut, "Timeout".to_string());
        let zig_error2 = ZigError::new(ZigErrorCode::OutOfMemory, "OOM".to_string());

        let _ = manager.handle_zig_error(zig_error1);
        let _ = manager.handle_zig_error(zig_error2);

        // Handle a Rust error
        let rust_error = OQLError::concurrency_error("Lock conflict", None);
        manager.handle_rust_error(&rust_error);

        let stats = manager.get_statistics().unwrap();
        assert_eq!(stats.total_errors, 3);
        assert_eq!(stats.zig_errors, 2);
        assert_eq!(stats.rust_errors, 1);
        assert_eq!(stats.transient_errors, 1); // TransactionTimedOut is transient
        assert_eq!(stats.fatal_errors, 1); // OutOfMemory is fatal
    }

    #[test]
    fn test_recovery_strategy() {
        let timeout_error = ZigError::new(ZigErrorCode::TransactionTimedOut, "Timeout".to_string());
        let recovery = ErrorRecoveryStrategy::analyze_error(&timeout_error);
        assert_eq!(
            recovery,
            RecoveryAction::Retry {
                max_attempts: 3,
                backoff_ms: 1000
            }
        );

        let corruption_error = ZigError::new(ZigErrorCode::CorruptedData, "Corrupted".to_string());
        let recovery = ErrorRecoveryStrategy::analyze_error(&corruption_error);
        assert_eq!(recovery, RecoveryAction::Fatal);

        let deadlock_error = ZigError::new(ZigErrorCode::DeadlockDetected, "Deadlock".to_string());
        let recovery = ErrorRecoveryStrategy::analyze_error(&deadlock_error);
        assert_eq!(recovery, RecoveryAction::Abort);
    }

    #[test]
    fn test_extended_error_creation() {
        let mut context = HashMap::new();
        context.insert("operation".to_string(), "test_op".to_string());
        context.insert("severity".to_string(), "1".to_string());

        let error = ZigError::from_extended_info(
            ZigErrorCode::StorageError,
            "Test storage error".to_string(),
            Some(1),
            Some(456),
            context,
        );

        assert_eq!(error.code, ZigErrorCode::StorageError);
        assert_eq!(error.shard_id, Some(1));
        assert_eq!(error.transaction_id, Some(456));
        assert_eq!(error.context.get("operation"), Some(&"test_op".to_string()));
    }

    #[test]
    fn test_error_recovery_instructions() {
        let oom_error = ZigError::new(ZigErrorCode::OutOfMemory, "OOM".to_string());
        let instructions = ErrorRecoveryStrategy::get_recovery_instructions(&oom_error);
        
        assert!(instructions.contains(&"Increase available memory".to_string()));
        assert!(instructions.contains(&"Check for memory leaks".to_string()));

        let network_error = ZigError::new(ZigErrorCode::NetworkError, "Network failure".to_string());
        let network_instructions = ErrorRecoveryStrategy::get_recovery_instructions(&network_error);
        
        assert!(network_instructions.contains(&"Check network connectivity".to_string()));
        assert!(network_instructions.contains(&"Retry with exponential backoff".to_string()));
    }

    #[test]
    fn test_error_propagation() {
        let manager = CrossLayerErrorManager::new();
        
        // Test error chaining
        let primary = OQLError::storage_error("Primary error", None);
        let secondary = vec![
            OQLError::network_error("Network issue", None),
            OQLError::concurrency_error("Lock conflict", None),
        ];
        
        let chained = ErrorPropagation::chain_errors(primary, secondary);
        
        match chained {
            OQLError::StorageError { context, .. } => {
                assert!(context.is_some());
                let ctx = context.unwrap();
                assert!(ctx.additional_info.contains_key("chained_error_0"));
                assert!(ctx.additional_info.contains_key("chained_error_1"));
            }
            _ => panic!("Expected StorageError"),
        }
    }
}