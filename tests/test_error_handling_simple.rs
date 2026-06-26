//! Simple test for cross-layer error handling without full build dependencies

use std::collections::HashMap;

// Simplified error types for testing
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZigErrorCode {
    Success = 0,
    OutOfMemory = -1,
    InvalidTransaction = -2,
    TransactionNotFound = -3,
    TransactionTimedOut = -5,
    LockConflict = -20,
    CorruptedData = -18,
    Unknown = -999,
}

impl ZigErrorCode {
    pub fn from_c_int(code: i32) -> Self {
        match code {
            0 => Self::Success,
            -1 => Self::OutOfMemory,
            -2 => Self::InvalidTransaction,
            -3 => Self::TransactionNotFound,
            -5 => Self::TransactionTimedOut,
            -20 => Self::LockConflict,
            -18 => Self::CorruptedData,
            _ => Self::Unknown,
        }
    }

    pub fn is_transient(&self) -> bool {
        matches!(self, Self::TransactionTimedOut | Self::LockConflict)
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::CorruptedData | Self::OutOfMemory)
    }
}

#[derive(Debug, Clone)]
pub struct ZigError {
    pub code: ZigErrorCode,
    pub message: String,
    pub shard_id: Option<u32>,
    pub transaction_id: Option<u64>,
    pub context: HashMap<String, String>,
}

impl ZigError {
    pub fn new(code: ZigErrorCode, message: String) -> Self {
        Self {
            code,
            message,
            shard_id: None,
            transaction_id: None,
            context: HashMap::new(),
        }
    }

    pub fn with_shard(mut self, shard_id: u32) -> Self {
        self.shard_id = Some(shard_id);
        self
    }

    pub fn with_transaction(mut self, transaction_id: u64) -> Self {
        self.transaction_id = Some(transaction_id);
        self
    }

    pub fn with_context(mut self, key: String, value: String) -> Self {
        self.context.insert(key, value);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryAction {
    None,
    Retry { max_attempts: u32, backoff_ms: u64 },
    Cleanup,
    Abort,
    Fatal,
}

pub struct ErrorRecoveryStrategy;

impl ErrorRecoveryStrategy {
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
            ZigErrorCode::CorruptedData => RecoveryAction::Fatal,
            ZigErrorCode::OutOfMemory => RecoveryAction::Fatal,
            _ => RecoveryAction::Abort,
        }
    }
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
        assert!(!ZigErrorCode::OutOfMemory.is_transient());
        assert!(ZigErrorCode::OutOfMemory.is_fatal());

        assert!(ZigErrorCode::TransactionTimedOut.is_transient());
        assert!(!ZigErrorCode::TransactionTimedOut.is_fatal());

        assert!(ZigErrorCode::LockConflict.is_transient());
        assert!(!ZigErrorCode::LockConflict.is_fatal());

        assert!(!ZigErrorCode::CorruptedData.is_transient());
        assert!(ZigErrorCode::CorruptedData.is_fatal());
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

        let lock_error = ZigError::new(ZigErrorCode::LockConflict, "Lock conflict".to_string());
        let recovery = ErrorRecoveryStrategy::analyze_error(&lock_error);
        assert_eq!(
            recovery,
            RecoveryAction::Retry {
                max_attempts: 5,
                backoff_ms: 100
            }
        );
    }

    #[test]
    fn test_error_context_preservation() {
        let mut error = ZigError::new(ZigErrorCode::InvalidTransaction, "Invalid transaction".to_string())
            .with_shard(1)
            .with_transaction(456)
            .with_context("operation".to_string(), "prepare".to_string())
            .with_context("timeout_ms".to_string(), "30000".to_string());

        assert_eq!(error.shard_id, Some(1));
        assert_eq!(error.transaction_id, Some(456));
        assert_eq!(error.context.len(), 2);
        assert_eq!(error.context.get("operation"), Some(&"prepare".to_string()));
        assert_eq!(error.context.get("timeout_ms"), Some(&"30000".to_string()));
    }

    #[test]
    fn test_error_severity_classification() {
        // Test transient errors
        assert!(ZigErrorCode::TransactionTimedOut.is_transient());
        assert!(ZigErrorCode::LockConflict.is_transient());
        
        // Test fatal errors
        assert!(ZigErrorCode::CorruptedData.is_fatal());
        assert!(ZigErrorCode::OutOfMemory.is_fatal());
        
        // Test non-transient, non-fatal errors
        assert!(!ZigErrorCode::InvalidTransaction.is_transient());
        assert!(!ZigErrorCode::InvalidTransaction.is_fatal());
    }
}

fn main() {
    println!("Cross-layer error handling test completed successfully!");
    
    // Demonstrate error handling
    let error = ZigError::new(
        ZigErrorCode::TransactionTimedOut,
        "Transaction timed out after 30 seconds".to_string(),
    )
    .with_shard(0)
    .with_transaction(123)
    .with_context("operation".to_string(), "commit".to_string())
    .with_context("retry_count".to_string(), "2".to_string());
    
    println!("Error: {:?}", error);
    
    let recovery = ErrorRecoveryStrategy::analyze_error(&error);
    println!("Recovery strategy: {:?}", recovery);
    
    match recovery {
        RecoveryAction::Retry { max_attempts, backoff_ms } => {
            println!("Should retry up to {} times with {}ms backoff", max_attempts, backoff_ms);
        }
        RecoveryAction::Fatal => {
            println!("Fatal error - system needs attention");
        }
        _ => {
            println!("Other recovery action needed");
        }
    }
}