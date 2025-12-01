//! Integration tests for cross-layer error handling
//!
//! Task 8.14: Add cross-layer error handling (Rust + Zig)

use owl_db::cross_layer_error_handling::*;
use owl_db::error::*;

#[test]
fn test_cross_layer_error_propagation() {
    let manager = CrossLayerErrorManager::new();
    
    // Test basic error handling
    let zig_error = ZigError::new(
        ZigErrorCode::TransactionTimedOut,
        "Transaction timed out after 30 seconds".to_string(),
    )
    .with_shard(0)
    .with_transaction(123)
    .with_context("operation".to_string(), "commit".to_string());
    
    let oql_error = manager.handle_zig_error(zig_error);
    
    match oql_error {
        OQLError::TransactionError { message, context } => {
            assert!(message.contains("Transaction timed out"));
            assert!(context.is_some());
            let ctx = context.unwrap();
            assert_eq!(ctx.additional_info.get("zig_error_code"), Some(&"TransactionTimedOut".to_string()));
            assert_eq!(ctx.additional_info.get("shard_id"), Some(&"0".to_string()));
            assert_eq!(ctx.additional_info.get("transaction_id"), Some(&"123".to_string()));
        }
        _ => panic!("Expected TransactionError"),
    }
}

#[test]
fn test_error_recovery_strategies() {
    // Test transient error recovery
    let timeout_error = ZigError::new(ZigErrorCode::TransactionTimedOut, "Timeout".to_string());
    let recovery = ErrorRecoveryStrategy::analyze_error(&timeout_error);
    
    match recovery {
        RecoveryAction::Retry { max_attempts, backoff_ms } => {
            assert_eq!(max_attempts, 3);
            assert_eq!(backoff_ms, 1000);
        }
        _ => panic!("Expected Retry recovery action"),
    }
    
    // Test fatal error
    let corruption_error = ZigError::new(ZigErrorCode::CorruptedData, "Data corrupted".to_string());
    let fatal_recovery = ErrorRecoveryStrategy::analyze_error(&corruption_error);
    assert_eq!(fatal_recovery, RecoveryAction::Fatal);
    
    // Test recovery instructions
    let instructions = ErrorRecoveryStrategy::get_recovery_instructions(&timeout_error);
    assert!(instructions.iter().any(|s| s.contains("timeout")));
    assert!(instructions.iter().any(|s| s.contains("retry")));
}

#[test]
fn test_error_context_preservation() {
    let manager = CrossLayerErrorManager::new();
    
    // Push context onto stack
    let context1 = ErrorContext::new("outer_operation")
        .with_location("test_module")
        .with_info("test_id", "123");
    
    manager.push_context(context1).unwrap();
    
    let context2 = ErrorContext::new("inner_operation")
        .with_info("nested", "true");
    
    manager.push_context(context2).unwrap();
    
    // Create error and verify context is preserved
    let zig_error = ZigError::new(ZigErrorCode::StorageError, "Storage failure".to_string());
    let oql_error = manager.handle_zig_error(zig_error);
    
    match oql_error {
        OQLError::StorageError { context, .. } => {
            assert!(context.is_some());
            let ctx = context.unwrap();
            assert_eq!(ctx.operation, "inner_operation");
            assert_eq!(ctx.additional_info.get("nested"), Some(&"true".to_string()));
        }
        _ => panic!("Expected StorageError"),
    }
    
    // Clean up context stack
    manager.pop_context();
    manager.pop_context();
}

#[test]
fn test_error_statistics() {
    let manager = CrossLayerErrorManager::new();
    
    // Generate some errors
    let errors = vec![
        ZigError::new(ZigErrorCode::TransactionTimedOut, "Timeout 1".to_string()),
        ZigError::new(ZigErrorCode::TransactionTimedOut, "Timeout 2".to_string()),
        ZigError::new(ZigErrorCode::OutOfMemory, "OOM".to_string()),
        ZigError::new(ZigErrorCode::LockConflict, "Lock conflict".to_string()),
    ];
    
    for error in errors {
        let _ = manager.handle_zig_error(error);
    }
    
    // Handle a Rust error
    let rust_error = OQLError::parse_error("Parse failure", None);
    manager.handle_rust_error(&rust_error);
    
    let stats = manager.get_statistics().unwrap();
    assert_eq!(stats.total_errors, 5);
    assert_eq!(stats.zig_errors, 4);
    assert_eq!(stats.rust_errors, 1);
    assert_eq!(stats.transient_errors, 2); // Two timeout errors
    assert_eq!(stats.fatal_errors, 1); // One OOM error
    
    // Check error counts by code
    assert_eq!(stats.errors_by_code.get("TransactionTimedOut"), Some(&2));
    assert_eq!(stats.errors_by_code.get("OutOfMemory"), Some(&1));
    assert_eq!(stats.errors_by_code.get("LockConflict"), Some(&1));
}

#[test]
fn test_batch_error_handling() {
    let manager = CrossLayerErrorManager::new();
    
    // Create a batch of operations with mixed success/failure
    let operations: Vec<(&str, Box<dyn FnOnce() -> OQLResult<i32>>)> = vec![
        ("op1", Box::new(|| Ok(1))),
        ("op2", Box::new(|| Err(OQLError::storage_error("Storage error", None)))),
        ("op3", Box::new(|| Ok(3))),
        ("op4", Box::new(|| Err(OQLError::network_error("Network error", None)))),
        ("op5", Box::new(|| Ok(5))),
    ];
    
    let (results, errors) = BatchErrorHandling::execute_batch(&manager, 0, operations);
    
    assert_eq!(results.len(), 3); // 3 successful operations
    assert_eq!(errors.len(), 2); // 2 failed operations
    assert_eq!(results, vec![1, 3, 5]);
}

#[test]
fn test_partial_failure_tolerance() {
    let manager = CrossLayerErrorManager::new();
    
    // Test with failure rate below threshold (should succeed)
    let operations_low_failure: Vec<Box<dyn FnOnce() -> OQLResult<i32>>> = vec![
        Box::new(|| Ok(1)),
        Box::new(|| Ok(2)),
        Box::new(|| Err(OQLError::storage_error("Error", None))), // 1 failure out of 3 = 33%
    ];
    
    let result = BatchErrorHandling::execute_with_partial_failure(
        &manager,
        0,
        "test_operation",
        operations_low_failure,
        0.5, // 50% threshold
    );
    
    assert!(result.is_ok());
    let results = result.unwrap();
    assert_eq!(results.len(), 2);
    
    // Test with failure rate above threshold (should fail)
    let operations_high_failure: Vec<Box<dyn FnOnce() -> OQLResult<i32>>> = vec![
        Box::new(|| Ok(1)),
        Box::new(|| Err(OQLError::storage_error("Error 1", None))),
        Box::new(|| Err(OQLError::storage_error("Error 2", None))), // 2 failures out of 3 = 67%
    ];
    
    let result = BatchErrorHandling::execute_with_partial_failure(
        &manager,
        0,
        "test_operation",
        operations_high_failure,
        0.5, // 50% threshold
    );
    
    assert!(result.is_err());
    let error = result.unwrap_err();
    match error {
        OQLError::StorageError { message, .. } => {
            assert!(message.contains("failure rate exceeds"));
        }
        _ => panic!("Expected StorageError"),
    }
}

#[test]
fn test_error_chaining() {
    let primary = OQLError::transaction_error("Primary transaction error", None);
    let secondary = vec![
        OQLError::network_error("Network timeout", None),
        OQLError::storage_error("Disk full", None),
    ];
    
    let chained = ErrorPropagation::chain_errors(primary, secondary);
    
    match chained {
        OQLError::TransactionError { message, context } => {
            assert_eq!(message, "Primary transaction error");
            assert!(context.is_some());
            let ctx = context.unwrap();
            assert!(ctx.additional_info.contains_key("chained_error_0"));
            assert!(ctx.additional_info.contains_key("chained_error_1"));
        }
        _ => panic!("Expected TransactionError"),
    }
}

#[test]
fn test_error_severity_classification() {
    // Test transient errors
    assert!(ZigErrorCode::TransactionTimedOut.is_transient());
    assert!(ZigErrorCode::LockConflict.is_transient());
    assert!(ZigErrorCode::NetworkError.is_transient());
    
    // Test fatal errors
    assert!(ZigErrorCode::CorruptedData.is_fatal());
    assert!(ZigErrorCode::OutOfMemory.is_fatal());
    assert!(ZigErrorCode::DeadlockDetected.is_fatal());
    
    // Test non-transient, non-fatal errors
    assert!(!ZigErrorCode::InvalidTransaction.is_transient());
    assert!(!ZigErrorCode::InvalidTransaction.is_fatal());
}

#[test]
fn test_error_manager_cleanup() {
    {
        let manager = CrossLayerErrorManager::new();
        
        // Use the manager
        let error = ZigError::new(ZigErrorCode::StorageError, "Test error".to_string());
        let _ = manager.handle_zig_error(error);
        
        // Manager should clean up automatically when dropped
    }
    
    // Test that we can create a new manager after cleanup
    let _new_manager = CrossLayerErrorManager::new();
}