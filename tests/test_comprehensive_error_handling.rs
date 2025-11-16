// Comprehensive Error Handling Tests

use oql_db::{
    Document, DocumentStore, QueryExecutor, Value,
    error::{OQLError, ErrorContext, ErrorSeverity, PartialResult, ErrorRecovery},
    storage::RocksDBStore,
    schema::{Schema, FieldType},
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn test_error_context_creation() {
    let context = ErrorContext::new("test_operation")
        .with_location("test_location")
        .with_query_fragment("test.query { .field == \"value\" }")
        .with_document_id("doc_123")
        .with_field_path("nested.field")
        .with_info("custom_key", "custom_value");

    assert_eq!(context.operation, "test_operation");
    assert_eq!(context.location, Some("test_location".to_string()));
    assert_eq!(context.query_fragment, Some("test.query { .field == \"value\" }".to_string()));
    assert_eq!(context.document_id, Some("doc_123".to_string()));
    assert_eq!(context.field_path, Some("nested.field".to_string()));
    assert_eq!(context.additional_info.get("custom_key"), Some(&"custom_value".to_string()));
}

#[test]
fn test_error_severity_classification() {
    let warning_error = OQLError::document_not_found("test_id", None);
    assert_eq!(warning_error.severity(), ErrorSeverity::Warning);
    assert!(warning_error.is_warning());
    assert!(!warning_error.is_critical());

    let critical_error = OQLError::Storage {
        message: "Database corruption".to_string(),
        context: None,
        severity: ErrorSeverity::Critical,
        recovery: None,
        source: None,
    };
    assert_eq!(critical_error.severity(), ErrorSeverity::Critical);
    assert!(critical_error.is_critical());
    assert!(!critical_error.is_warning());
}

#[test]
fn test_error_recovery_suggestions() {
    let context = ErrorContext::new("test")
        .with_field_path("customer.account");

    let ref_error = OQLError::reference_error("Reference not found", Some(context));
    
    assert!(ref_error.recovery_suggestion().is_some());
    assert!(!ref_error.is_auto_recoverable());

    let suggestions = ErrorRecovery::suggest_fix(&ref_error);
    assert!(!suggestions.is_empty());
    assert!(suggestions.iter().any(|s| s.contains("reference path")));
}

#[test]
fn test_auto_recoverable_errors() {
    let concurrency_error = OQLError::concurrency_error("Lock conflict", None);
    assert!(concurrency_error.is_auto_recoverable());

    let recovery_msg = ErrorRecovery::attempt_recovery(&concurrency_error);
    assert!(recovery_msg.is_some());
    assert!(recovery_msg.unwrap().contains("Retrying"));

    let transaction_error = OQLError::transaction_error("Transaction conflict", None);
    assert!(transaction_error.is_auto_recoverable());
}

#[test]
fn test_detailed_error_messages() {
    let context = ErrorContext::new("field_access")
        .with_document_id("doc_123")
        .with_field_path("customer.name")
        .with_query_fragment("customers.where { .name == \"Alice\" }")
        .with_location("line 1, column 15")
        .with_info("schema", "customers")
        .with_info("operation_id", "op_456");

    let error = OQLError::type_error("String", "Integer", Some(context));

    let user_msg = error.user_message();
    assert!(user_msg.contains("Type error"));
    assert!(user_msg.contains("Document: doc_123"));
    assert!(user_msg.contains("Field: customer.name"));

    let detailed_msg = error.detailed_message();
    assert!(detailed_msg.contains("Suggestion:"));
    assert!(detailed_msg.contains("schema: customers"));
    assert!(detailed_msg.contains("operation_id: op_456"));
}

#[test]
fn test_partial_results() {
    let mut partial = PartialResult::new(vec!["data1", "data2"]);
    
    // Add a warning
    let warning = OQLError::Storage {
        message: "Minor issue".to_string(),
        context: None,
        severity: ErrorSeverity::Warning,
        recovery: None,
        source: None,
    };
    partial.add_error(warning);

    // Add an error
    let error = OQLError::Storage {
        message: "Serious issue".to_string(),
        context: None,
        severity: ErrorSeverity::Error,
        recovery: None,
        source: None,
    };
    partial.add_error(error);

    assert!(partial.has_warnings());
    assert!(partial.has_errors());
    assert_eq!(partial.warnings.len(), 1);
    assert_eq!(partial.errors.len(), 1);
    assert_eq!(partial.all_issues().len(), 2);

    // Test conversion to Result
    let result = partial.into_result();
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].user_message().contains("Serious issue"));
}

#[test]
fn test_partial_results_with_warnings_only() {
    let mut partial = PartialResult::new("success_data");
    
    let warning = OQLError::Storage {
        message: "Minor warning".to_string(),
        context: None,
        severity: ErrorSeverity::Warning,
        recovery: None,
        source: None,
    };
    partial.add_error(warning);

    assert!(partial.has_warnings());
    assert!(!partial.has_errors());

    // Should succeed even with warnings
    let result = partial.into_result();
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "success_data");
}

#[test]
fn test_error_with_multiple_errors() {
    let errors = vec![
        OQLError::document_not_found("doc1", None),
        OQLError::type_error("String", "Integer", None),
    ];

    let partial = PartialResult::with_errors(vec!["partial_data"], errors);
    
    assert!(partial.has_warnings());  // document_not_found is a warning
    assert!(partial.has_errors());    // type_error is an error
    assert_eq!(partial.warnings.len(), 1);
    assert_eq!(partial.errors.len(), 1);
}

#[test]
fn test_error_conversion_from_standard_errors() {
    // Test rocksdb::Error conversion
    let rocksdb_err = rocksdb::Error::new("Test RocksDB error".to_string());
    let oql_error: OQLError = rocksdb_err.into();
    
    match oql_error {
        OQLError::Storage { message, severity, .. } => {
            assert!(message.contains("Test RocksDB error"));
            assert_eq!(severity, ErrorSeverity::Error);
        }
        _ => panic!("Expected Storage error"),
    }

    // Test serde_json::Error conversion
    let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
    let oql_error: OQLError = json_err.into();
    
    match oql_error {
        OQLError::Serialization { message, severity, .. } => {
            assert!(message.contains("expected"));
            assert_eq!(severity, ErrorSeverity::Error);
        }
        _ => panic!("Expected Serialization error"),
    }
}

#[test]
fn test_convenience_constructors() {
    let context = ErrorContext::new("test");

    // Test document_not_found constructor
    let doc_error = OQLError::document_not_found("test_id", Some(context.clone()));
    assert_eq!(doc_error.severity(), ErrorSeverity::Warning);
    assert!(doc_error.recovery_suggestion().is_some());

    // Test type_error constructor
    let type_error = OQLError::type_error("String", "Integer", Some(context.clone()));
    assert_eq!(type_error.severity(), ErrorSeverity::Error);
    assert!(type_error.recovery_suggestion().is_some());

    // Test parse_error constructor
    let parse_error = OQLError::parse_error("Syntax error", Some(context.clone()));
    assert_eq!(parse_error.severity(), ErrorSeverity::Error);
    assert!(parse_error.recovery_suggestion().is_some());

    // Test function_error constructor
    let func_error = OQLError::function_error("Unknown function", Some(context.clone()));
    assert_eq!(func_error.severity(), ErrorSeverity::Error);
    assert!(func_error.recovery_suggestion().is_some());

    // Test transaction_error constructor
    let txn_error = OQLError::transaction_error("Transaction failed", Some(context.clone()));
    assert_eq!(txn_error.severity(), ErrorSeverity::Error);
    assert!(txn_error.is_auto_recoverable());

    // Test concurrency_error constructor
    let conc_error = OQLError::concurrency_error("Lock conflict", Some(context));
    assert_eq!(conc_error.severity(), ErrorSeverity::Warning);
    assert!(conc_error.is_auto_recoverable());
}

#[test]
fn test_error_recovery_suggestions_context_specific() {
    // Test reference error with field path
    let context = ErrorContext::new("reference_resolution")
        .with_field_path("customer.account.balance");
    
    let ref_error = OQLError::reference_error("Reference not found", Some(context));
    let suggestions = ErrorRecovery::suggest_fix(&ref_error);
    
    assert!(suggestions.iter().any(|s| s.contains("customer.account.balance")));

    // Test type error with field path
    let context = ErrorContext::new("type_check")
        .with_field_path("user.age");
    
    let type_error = OQLError::type_error("Integer", "String", Some(context));
    let suggestions = ErrorRecovery::suggest_fix(&type_error);
    
    assert!(suggestions.iter().any(|s| s.contains("user.age")));
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_error_handling_in_query_execution() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
        let executor = QueryExecutor::new(store.clone());

        // Test querying non-existent collection
        let query = crate::parser::parse_query("nonexistent.where { .id == \"test\" }")?;
        let result = executor.execute_query(&query);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        
        // Should have context information
        assert!(error.context().is_some());
        
        // Should have user-friendly message
        let user_msg = error.user_message();
        assert!(!user_msg.is_empty());
        
        // Should have detailed message with suggestions
        let detailed_msg = error.detailed_message();
        assert!(detailed_msg.len() > user_msg.len());

        Ok(())
    }
}