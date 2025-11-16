// Error Handling Demo - Demonstrates enhanced error handling capabilities

use owl_db::error::{
    OQLError, ErrorContext, ErrorSeverity, PartialResult, ErrorRecovery, RecoverySuggestion
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Error Handling Demo ===\n");

    // Demo 1: Basic error creation with context
    println!("1. Basic Error Creation with Context");
    demo_basic_error_creation();

    // Demo 2: Error severity and recovery suggestions
    println!("\n2. Error Severity and Recovery Suggestions");
    demo_error_severity_and_recovery();

    // Demo 3: Partial results with error annotations
    println!("\n3. Partial Results with Error Annotations");
    demo_partial_results();

    // Demo 4: Error recovery utilities
    println!("\n4. Error Recovery Utilities");
    demo_error_recovery_utilities();

    println!("\n=== Demo Complete ===");
    Ok(())
}

fn demo_basic_error_creation() {
    // Create error context
    let context = ErrorContext::new("document_retrieval")
        .with_location("query_executor")
        .with_document_id("customer_123")
        .with_field_path("account.balance")
        .with_query_fragment("customers.where { .id == \"customer_123\" }")
        .with_info("operation_id", "op_456")
        .with_info("retry_count", "1");

    // Create a document not found error
    let doc_error = OQLError::document_not_found("customer_123", Some(context.clone()));
    
    println!("Document Not Found Error:");
    println!("  User message: {}", doc_error.user_message());
    println!("  Detailed message: {}", doc_error.detailed_message());
    println!("  Severity: {:?}", doc_error.severity());
    println!("  Is warning: {}", doc_error.is_warning());
    println!("  Is auto-recoverable: {}", doc_error.is_auto_recoverable());

    // Create a type error
    let type_error = OQLError::type_error("Integer", "String", Some(context));
    
    println!("\nType Error:");
    println!("  User message: {}", type_error.user_message());
    println!("  Detailed message: {}", type_error.detailed_message());
    println!("  Severity: {:?}", type_error.severity());
}

fn demo_error_severity_and_recovery() {
    // Create different severity errors
    let warning_error = OQLError::reference_error(
        "Referenced document not found, using null value",
        Some(ErrorContext::new("reference_resolution")
            .with_document_id("order_456")
            .with_field_path("customer.profile"))
    );

    let critical_error = OQLError::EnhancedStorage {
        message: "Database corruption detected".to_string(),
        context: Some(ErrorContext::new("database_read")
            .with_location("storage_layer")
            .with_info("corruption_type", "checksum_mismatch")),
        severity: ErrorSeverity::Critical,
        recovery: None,
        source: None,
    };

    println!("Warning Error:");
    println!("  Message: {}", warning_error.user_message());
    println!("  Severity: {:?}", warning_error.severity());
    println!("  Is warning: {}", warning_error.is_warning());

    println!("\nCritical Error:");
    println!("  Message: {}", critical_error.user_message());
    println!("  Severity: {:?}", critical_error.severity());
    println!("  Is critical: {}", critical_error.is_critical());
}

fn demo_partial_results() {
    // Create a partial result with some data and errors
    let mut partial_result = PartialResult::new(vec!["result1", "result2", "result3"]);

    // Add some warnings (non-fatal errors)
    let warning1 = OQLError::reference_error(
        "Referenced document not found, using null value",
        Some(ErrorContext::new("reference_resolution")
            .with_document_id("customer_789")
            .with_field_path("account.balance"))
    );
    partial_result.add_error(warning1);

    let warning2 = OQLError::EnhancedQueryExecution {
        message: "Field not found, skipping document".to_string(),
        context: Some(ErrorContext::new("field_access")
            .with_document_id("customer_101")
            .with_field_path("invalid_field")),
        severity: ErrorSeverity::Warning,
        recovery: Some(RecoverySuggestion {
            suggestion: "Check field names in schema".to_string(),
            example: Some("Use .name, .email, etc.".to_string()),
            auto_recoverable: false,
        }),
    };
    partial_result.add_error(warning2);

    println!("Partial Result:");
    println!("  Data: {:?}", partial_result.data);
    println!("  Has errors: {}", partial_result.has_errors());
    println!("  Has warnings: {}", partial_result.has_warnings());
    println!("  Warning count: {}", partial_result.warnings.len());
    
    println!("  All issues:");
    for (i, issue) in partial_result.all_issues().iter().enumerate() {
        println!("    {}: {} - {}", 
            i + 1,
            if issue.is_warning() { "WARNING" } else { "ERROR" },
            issue.user_message()
        );
    }

    // Convert to regular Result (should succeed since no errors, only warnings)
    match partial_result.into_result() {
        Ok(data) => println!("  Conversion to Result: Success with data {:?}", data),
        Err(errors) => println!("  Conversion to Result: Failed with {} errors", errors.len()),
    }
}

fn demo_error_recovery_utilities() {
    // Create auto-recoverable errors
    let concurrency_error = OQLError::concurrency_error(
        "Document was modified by another transaction",
        Some(ErrorContext::new("document_update")
            .with_document_id("customer_456")
            .with_info("retry_count", "1"))
    );

    let transaction_error = OQLError::transaction_error(
        "Transaction conflict detected",
        Some(ErrorContext::new("transaction_commit")
            .with_info("transaction_id", "txn_789"))
    );

    println!("Concurrency Error:");
    println!("  Message: {}", concurrency_error.user_message());
    println!("  Is auto-recoverable: {}", concurrency_error.is_auto_recoverable());
    
    if let Some(recovery_msg) = ErrorRecovery::attempt_recovery(&concurrency_error) {
        println!("  Recovery attempt: {}", recovery_msg);
    }

    let suggestions = ErrorRecovery::suggest_fix(&concurrency_error);
    println!("  Recovery suggestions:");
    for suggestion in suggestions {
        println!("    - {}", suggestion);
    }

    println!("\nTransaction Error:");
    println!("  Message: {}", transaction_error.user_message());
    println!("  Is auto-recoverable: {}", transaction_error.is_auto_recoverable());
    
    if let Some(recovery_msg) = ErrorRecovery::attempt_recovery(&transaction_error) {
        println!("  Recovery attempt: {}", recovery_msg);
    }

    let suggestions = ErrorRecovery::suggest_fix(&transaction_error);
    println!("  Recovery suggestions:");
    for suggestion in suggestions {
        println!("    - {}", suggestion);
    }
}