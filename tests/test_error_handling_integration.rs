// Integration tests for comprehensive error handling

use owl_db::{
    Document, DocumentStore, QueryExecutor, Value,
    error::{OQLError, ErrorContext, ErrorSeverity, PartialResult, ErrorRecovery},
    storage::RocksDBStore,
    schema::{Schema, FieldType},
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn test_enhanced_error_handling_integration() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let executor = QueryExecutor::new(store.clone());

    // Test 1: Enhanced error creation with full context
    let context = ErrorContext::new("integration_test")
        .with_location("test_suite")
        .with_document_id("test_doc_123")
        .with_field_path("nested.field.path")
        .with_query_fragment("test.where { .field == \"value\" }")
        .with_info("test_case", "enhanced_error_creation")
        .with_info("iteration", "1");

    let enhanced_error = OQLError::type_error("String", "Integer", Some(context));
    
    // Verify enhanced error properties
    assert_eq!(enhanced_error.severity(), ErrorSeverity::Error);
    assert!(!enhanced_error.is_warning());
    assert!(!enhanced_error.is_critical());
    assert!(!enhanced_error.is_auto_recoverable());
    assert!(enhanced_error.recovery_suggestion().is_some());
    assert!(enhanced_error.context().is_some());

    // Verify detailed message contains context
    let detailed_msg = enhanced_error.detailed_message();
    assert!(detailed_msg.contains("Document: test_doc_123"));
    assert!(detailed_msg.contains("Field: nested.field.path"));
    assert!(detailed_msg.contains("Suggestion:"));
    assert!(detailed_msg.contains("test_case: enhanced_error_creation"));

    // Test 2: Partial results with mixed error types
    let mut partial_result = PartialResult::new(vec!["data1", "data2", "data3"]);
    
    // Add warning
    let warning = OQLError::reference_error(
        "Reference not found",
        Some(ErrorContext::new("reference_test").with_document_id("ref_doc"))
    );
    partial_result.add_error(warning);
    
    // Add error
    let error = OQLError::function_error(
        "Function not found",
        Some(ErrorContext::new("function_test").with_info("function_name", "unknown_func"))
    );
    partial_result.add_error(error);

    assert!(partial_result.has_warnings());
    assert!(partial_result.has_errors());
    assert_eq!(partial_result.warnings.len(), 1);
    assert_eq!(partial_result.errors.len(), 1);

    // Should fail conversion due to errors
    let result = partial_result.into_result();
    assert!(result.is_err());

    // Test 3: Auto-recoverable errors
    let concurrency_error = OQLError::concurrency_error(
        "Lock conflict",
        Some(ErrorContext::new("concurrency_test"))
    );
    
    assert!(concurrency_error.is_auto_recoverable());
    assert!(concurrency_error.is_warning());
    
    let recovery_attempt = ErrorRecovery::attempt_recovery(&concurrency_error);
    assert!(recovery_attempt.is_some());
    assert!(recovery_attempt.unwrap().contains("Retrying"));

    // Test 4: Error recovery suggestions
    let suggestions = ErrorRecovery::suggest_fix(&enhanced_error);
    assert!(!suggestions.is_empty());
    assert!(suggestions.iter().any(|s| s.contains("Convert the value")));

    // Test 5: Error cloning
    let cloned_error = enhanced_error.clone();
    assert_eq!(cloned_error.user_message(), enhanced_error.user_message());
    assert_eq!(cloned_error.severity(), enhanced_error.severity());

    Ok(())
}

#[test]
fn test_error_severity_classification() {
    // Test warning errors
    let doc_not_found = OQLError::document_not_found("test", None);
    assert!(doc_not_found.is_warning());
    assert!(!doc_not_found.is_critical());

    let ref_error = OQLError::reference_error("ref not found", None);
    assert!(ref_error.is_warning());

    let concurrency_error = OQLError::concurrency_error("conflict", None);
    assert!(concurrency_error.is_warning());

    // Test error severity
    let type_error = OQLError::type_error("String", "Int", None);
    assert!(!type_error.is_warning());
    assert!(!type_error.is_critical());
    assert_eq!(type_error.severity(), ErrorSeverity::Error);

    let function_error = OQLError::function_error("func not found", None);
    assert_eq!(function_error.severity(), ErrorSeverity::Error);

    let parse_error = OQLError::parse_error("syntax error", None);
    assert_eq!(parse_error.severity(), ErrorSeverity::Error);

    // Test critical error
    let critical_error = OQLError::EnhancedStorage {
        message: "Critical failure".to_string(),
        context: None,
        severity: ErrorSeverity::Critical,
        recovery: None,
        source: None,
    };
    assert!(critical_error.is_critical());
    assert!(!critical_error.is_warning());
}

#[test]
fn test_partial_results_edge_cases() {
    // Test partial result with only warnings
    let mut partial_warnings_only = PartialResult::new("success");
    let warning = OQLError::reference_error("minor issue", None);
    partial_warnings_only.add_error(warning);

    assert!(partial_warnings_only.has_warnings());
    assert!(!partial_warnings_only.has_errors());

    // Should succeed even with warnings
    let result = partial_warnings_only.into_result();
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "success");

    // Test partial result with mixed errors
    let errors = vec![
        OQLError::reference_error("warning 1", None),
        OQLError::type_error("String", "Int", None),
        OQLError::concurrency_error("warning 2", None),
    ];

    let partial_mixed = PartialResult::with_errors("data", errors);
    assert!(partial_mixed.has_warnings());
    assert!(partial_mixed.has_errors());
    assert_eq!(partial_mixed.warnings.len(), 2); // reference_error and concurrency_error are warnings
    assert_eq!(partial_mixed.errors.len(), 1);   // type_error is an error
}

#[test]
fn test_error_context_builder_pattern() {
    let context = ErrorContext::new("test_operation")
        .with_location("test_location")
        .with_document_id("doc_123")
        .with_field_path("field.path")
        .with_query_fragment("query fragment")
        .with_info("key1", "value1")
        .with_info("key2", "value2");

    assert_eq!(context.operation, "test_operation");
    assert_eq!(context.location, Some("test_location".to_string()));
    assert_eq!(context.document_id, Some("doc_123".to_string()));
    assert_eq!(context.field_path, Some("field.path".to_string()));
    assert_eq!(context.query_fragment, Some("query fragment".to_string()));
    assert_eq!(context.additional_info.len(), 2);
    assert_eq!(context.additional_info.get("key1"), Some(&"value1".to_string()));
    assert_eq!(context.additional_info.get("key2"), Some(&"value2".to_string()));
}

#[test]
fn test_error_recovery_context_specific_suggestions() {
    // Test reference error with field path
    let ref_context = ErrorContext::new("reference_resolution")
        .with_field_path("customer.account.balance");
    
    let ref_error = OQLError::reference_error("Reference not found", Some(ref_context));
    let ref_suggestions = ErrorRecovery::suggest_fix(&ref_error);
    
    assert!(ref_suggestions.iter().any(|s| s.contains("customer.account.balance")));

    // Test type error with field path
    let type_context = ErrorContext::new("type_check")
        .with_field_path("user.age");
    
    let type_error = OQLError::type_error("Integer", "String", Some(type_context));
    let type_suggestions = ErrorRecovery::suggest_fix(&type_error);
    
    assert!(type_suggestions.iter().any(|s| s.contains("user.age")));
}

#[test]
fn test_legacy_error_compatibility() {
    // Test that legacy error variants still work
    let legacy_doc_not_found = OQLError::DocumentNotFound { 
        id: "test_id".to_string() 
    };
    
    assert_eq!(legacy_doc_not_found.severity(), ErrorSeverity::Warning);
    assert!(legacy_doc_not_found.is_warning());
    assert!(legacy_doc_not_found.context().is_none());
    assert!(legacy_doc_not_found.recovery_suggestion().is_none());

    let legacy_type_error = OQLError::TypeError {
        expected: "String".to_string(),
        actual: "Integer".to_string(),
    };
    
    assert_eq!(legacy_type_error.severity(), ErrorSeverity::Error);
    assert!(!legacy_type_error.is_warning());
    assert!(legacy_type_error.context().is_none());

    // Test cloning legacy errors
    let cloned_legacy = legacy_doc_not_found.clone();
    assert_eq!(cloned_legacy.user_message(), legacy_doc_not_found.user_message());
}