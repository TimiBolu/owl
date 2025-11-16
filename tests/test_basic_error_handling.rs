// Basic Error Handling Tests - Testing new error system

use owl_db::error::{OQLError, ErrorContext, ErrorSeverity, PartialResult, ErrorRecovery};

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

    let critical_error = OQLError::EnhancedStorage {
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
fn test_partial_results() {
    let mut partial = PartialResult::new(vec!["data1", "data2"]);
    
    // Add a warning
    let warning = OQLError::EnhancedStorage {
        message: "Minor issue".to_string(),
        context: None,
        severity: ErrorSeverity::Warning,
        recovery: None,
        source: None,
    };
    partial.add_error(warning);

    // Add an error
    let error = OQLError::EnhancedStorage {
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