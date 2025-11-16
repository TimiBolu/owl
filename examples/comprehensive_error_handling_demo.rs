// Comprehensive Error Handling Demo
// This example demonstrates the enhanced error handling capabilities

use owl_db::{
    Document, DocumentStore, QueryExecutor, Value,
    error::{OQLError, ErrorContext, ErrorSeverity, PartialResult, ErrorRecovery},
    storage::RocksDBStore,
    schema::{Schema, FieldType, ReferenceType},
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Comprehensive Error Handling Demo ===\n");

    // Setup
    let temp_dir = TempDir::new()?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let mut executor = QueryExecutor::new(store.clone());

    // Create schemas
    setup_schemas(&store)?;
    setup_test_data(&store)?;

    // Demo 1: Detailed error messages with context
    println!("1. Detailed Error Messages with Context");
    demo_detailed_errors(&executor)?;

    // Demo 2: Error recovery and graceful degradation
    println!("\n2. Error Recovery and Graceful Degradation");
    demo_error_recovery(&executor)?;

    // Demo 3: Partial results with error annotations
    println!("\n3. Partial Results with Error Annotations");
    demo_partial_results(&executor)?;

    // Demo 4: Error severity and recovery suggestions
    println!("\n4. Error Severity and Recovery Suggestions");
    demo_error_severity_and_suggestions(&executor)?;

    println!("\n=== Demo Complete ===");
    Ok(())
}

fn setup_schemas(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    // Customer schema
    let mut customer_fields = HashMap::new();
    customer_fields.insert("id".to_string(), FieldType::String);
    customer_fields.insert("name".to_string(), FieldType::String);
    customer_fields.insert("email".to_string(), FieldType::String);
    customer_fields.insert("account".to_string(), FieldType::Reference(ReferenceType {
        target_schema: "accounts".to_string(),
        required: false,
    }));

    let customer_schema = Schema::new("customers".to_string(), customer_fields);
    store.create_schema(&customer_schema)?;

    // Account schema
    let mut account_fields = HashMap::new();
    account_fields.insert("id".to_string(), FieldType::String);
    account_fields.insert("balance".to_string(), FieldType::Float);
    account_fields.insert("status".to_string(), FieldType::String);

    let account_schema = Schema::new("accounts".to_string(), account_fields);
    store.create_schema(&account_schema)?;

    Ok(())
}

fn setup_test_data(store: &Arc<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    // Create test customers
    let mut customer_fields = HashMap::new();
    customer_fields.insert("id".to_string(), Value::String("cust_1".to_string()));
    customer_fields.insert("name".to_string(), Value::String("Alice".to_string()));
    customer_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    customer_fields.insert("account".to_string(), Value::Reference("accounts#acc_1".to_string()));

    let customer = Document::new("cust_1".to_string(), "customers".to_string(), customer_fields);
    store.put_document(&customer.id, &customer)?;

    // Create test account
    let mut account_fields = HashMap::new();
    account_fields.insert("id".to_string(), Value::String("acc_1".to_string()));
    account_fields.insert("balance".to_string(), Value::Float(1000.0));
    account_fields.insert("status".to_string(), Value::String("active".to_string()));

    let account = Document::new("acc_1".to_string(), "accounts".to_string(), account_fields);
    store.put_document(&account.id, &account)?;

    Ok(())
}

fn demo_detailed_errors(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing detailed error messages...");

    // Test 1: Document not found with context
    let query = crate::parser::parse_query("nonexistent.where { .id == \"test\" }")?;
    match executor.execute_query(&query) {
        Err(error) => {
            println!("Error: {}", error.user_message());
            println!("Detailed: {}", error.detailed_message());
            println!("Severity: {:?}", error.severity());
            
            if let Some(context) = error.context() {
                println!("Context operation: {}", context.operation);
                if let Some(location) = &context.location {
                    println!("Location: {}", location);
                }
            }
        }
        Ok(_) => println!("Unexpected success"),
    }

    // Test 2: Type error with context
    println!("\nTesting type error...");
    let context = ErrorContext::new("field_comparison")
        .with_document_id("cust_1")
        .with_field_path("balance")
        .with_query_fragment(".balance == \"not_a_number\"");

    let type_error = OQLError::type_error("Float", "String", Some(context));
    println!("Type Error: {}", type_error.user_message());
    println!("Detailed: {}", type_error.detailed_message());

    Ok(())
}

fn demo_error_recovery(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing error recovery...");

    // Test auto-recoverable concurrency error
    let context = ErrorContext::new("document_update")
        .with_document_id("cust_1")
        .with_info("retry_count", "1");

    let concurrency_error = OQLError::concurrency_error(
        "Document was modified by another transaction",
        Some(context)
    );

    println!("Concurrency Error: {}", concurrency_error.user_message());
    println!("Is auto-recoverable: {}", concurrency_error.is_auto_recoverable());
    
    if let Some(recovery_msg) = ErrorRecovery::attempt_recovery(&concurrency_error) {
        println!("Recovery attempt: {}", recovery_msg);
    }

    // Test recovery suggestions
    let suggestions = ErrorRecovery::suggest_fix(&concurrency_error);
    println!("Recovery suggestions:");
    for suggestion in suggestions {
        println!("  - {}", suggestion);
    }

    Ok(())
}

fn demo_partial_results(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing partial results...");

    // Simulate a query that partially succeeds
    let mut partial_result = PartialResult::new(vec!["result1", "result2"]);

    // Add some warnings (non-fatal errors)
    let warning = OQLError::reference_error(
        "Referenced document not found, using null value",
        Some(ErrorContext::new("reference_resolution")
            .with_document_id("cust_2")
            .with_field_path("account.balance"))
    );
    partial_result.add_error(warning);

    // Add a non-critical error
    let context = ErrorContext::new("field_access")
        .with_document_id("cust_3")
        .with_field_path("invalid_field");
    let field_error = OQLError::QueryExecution {
        message: "Field not found, skipping document".to_string(),
        context: Some(context),
        severity: ErrorSeverity::Warning,
        recovery: Some(crate::error::RecoverySuggestion {
            suggestion: "Check field names in schema".to_string(),
            example: Some("Use .name, .email, etc.".to_string()),
            auto_recoverable: false,
        }),
    };
    partial_result.add_error(field_error);

    println!("Partial result data: {:?}", partial_result.data);
    println!("Has errors: {}", partial_result.has_errors());
    println!("Has warnings: {}", partial_result.has_warnings());
    
    println!("All issues:");
    for issue in partial_result.all_issues() {
        println!("  - {}: {}", 
            if issue.is_warning() { "WARNING" } else { "ERROR" },
            issue.user_message()
        );
    }

    Ok(())
}

fn demo_error_severity_and_suggestions(executor: &QueryExecutor<RocksDBStore>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing error severity and suggestions...");

    // Critical storage error
    let critical_error = OQLError::Storage {
        message: "Database corruption detected".to_string(),
        context: Some(ErrorContext::new("database_read")
            .with_location("storage_layer")
            .with_info("corruption_type", "checksum_mismatch")),
        severity: ErrorSeverity::Critical,
        recovery: None,
        source: None,
    };

    println!("Critical Error: {}", critical_error.user_message());
    println!("Is critical: {}", critical_error.is_critical());
    println!("Detailed: {}", critical_error.detailed_message());

    // Parse error with suggestions
    let parse_error = OQLError::parse_error(
        "Unexpected token: expected '}' but found 'EOF'",
        Some(ErrorContext::new("query_parsing")
            .with_query_fragment("customers.where { .name == \"Alice\"")
            .with_location("line 1, column 35"))
    );

    println!("\nParse Error: {}", parse_error.user_message());
    let suggestions = ErrorRecovery::suggest_fix(&parse_error);
    println!("Suggestions:");
    for suggestion in suggestions {
        println!("  - {}", suggestion);
    }

    // Function error with recovery
    let function_error = OQLError::function_error(
        "Function 'unknown_func' not found",
        Some(ErrorContext::new("function_call")
            .with_query_fragment("unknown_func(.balance)")
            .with_info("available_functions", "count, sum, avg, min, max"))
    );

    println!("\nFunction Error: {}", function_error.user_message());
    println!("Detailed: {}", function_error.detailed_message());

    Ok(())
}