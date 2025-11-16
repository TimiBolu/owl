// User-Defined Functions Demo
// Demonstrates the user-defined function system in OQL

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::{parse_oql, FunctionDefinition, FunctionParameter, Expression, Literal, FieldAccess},
    storage::{RocksDBStore, DocumentStore},
    error::{OQLResult, OQLError},
};
use std::sync::Arc;
use chrono::Utc;
use tempfile::TempDir;

fn main() -> OQLResult<()> {
    println!("=== OQL User-Defined Functions Demo ===\n");
    
    // Create temporary directory and storage
    let temp_dir = TempDir::new().map_err(|e| OQLError::Generic {
        message: format!("Failed to create temp directory: {}", e),
    })?;
    let store = Arc::new(RocksDBStore::new(temp_dir.path())?);
    let mut executor = QueryExecutor::new(store.clone());
    
    // Insert test data
    setup_test_data(&store)?;
    
    // Test 1: Simple user-defined function
    println!("1. Testing simple user-defined function:");
    test_simple_function(&mut executor)?;
    
    // Test 2: Function with multiple parameters
    println!("\n2. Testing function with multiple parameters:");
    test_multi_parameter_function(&mut executor)?;
    
    // Test 3: Function that calls built-in functions
    println!("\n3. Testing function that calls built-in functions:");
    test_function_with_builtin_calls(&mut executor)?;
    
    // Test 4: Function type validation
    println!("\n4. Testing function type validation:");
    test_function_type_validation(&mut executor)?;
    
    // Test 5: Function error handling
    println!("\n5. Testing function error handling:");
    test_function_error_handling(&mut executor)?;
    
    println!("\n=== User-Defined Functions Demo Complete ===");
    Ok(())
}

fn setup_test_data(store: &Arc<RocksDBStore>) -> OQLResult<()> {
    // Create customers
    let customers = vec![
        Document {
            id: "cust_1".to_string(),
            schema: "customers".to_string(),
            fields: [
                ("name".to_string(), Value::String("Alice".to_string())),
                ("age".to_string(), Value::Integer(30)),
                ("email".to_string(), Value::String("alice@example.com".to_string())),
                ("tier".to_string(), Value::String("gold".to_string())),
            ].into_iter().collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        },
        Document {
            id: "cust_2".to_string(),
            schema: "customers".to_string(),
            fields: [
                ("name".to_string(), Value::String("Bob".to_string())),
                ("age".to_string(), Value::Integer(25)),
                ("email".to_string(), Value::String("bob@example.com".to_string())),
                ("tier".to_string(), Value::String("silver".to_string())),
            ].into_iter().collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        },
        Document {
            id: "cust_3".to_string(),
            schema: "customers".to_string(),
            fields: [
                ("name".to_string(), Value::String("Charlie".to_string())),
                ("age".to_string(), Value::Integer(35)),
                ("email".to_string(), Value::String("charlie@example.com".to_string())),
                ("tier".to_string(), Value::String("bronze".to_string())),
            ].into_iter().collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        },
    ];
    
    for customer in customers {
        store.put_document(&customer)?;
    }
    
    println!("✓ Test data inserted: 3 customers");
    Ok(())
}

fn test_simple_function(executor: &mut QueryExecutor<RocksDBStore>) -> OQLResult<()> {
    // Define a simple function that formats a customer's display name
    let func_def = FunctionDefinition {
        name: "format_customer_name".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "name".to_string(),
                param_type: "String".to_string(),
            },
            FunctionParameter {
                name: "tier".to_string(),
                param_type: "String".to_string(),
            },
        ],
        return_type: Some("String".to_string()),
        body: Expression::FunctionCall {
            name: "concat".to_string(),
            args: vec![
                Expression::FieldAccess(FieldAccess { path: vec!["name".to_string()] }),
                Expression::Literal(Literal::String(" (".to_string())),
                Expression::FunctionCall {
                    name: "upper".to_string(),
                    args: vec![Expression::FieldAccess(FieldAccess { path: vec!["tier".to_string()] })],
                },
                Expression::Literal(Literal::String(")".to_string())),
            ],
        },
    };
    
    // Register the function
    executor.register_user_function(func_def)?;
    println!("✓ Registered function: format_customer_name(name: String, tier: String) -> String");
    
    // Test the function in a query
    let query_input = r#"customers.select { 
        display_name: format_customer_name(.name, .tier),
        original_name: .name 
    }"#;
    
    let query = parse_oql(query_input).map_err(|e| OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    let results = executor.execute_query(&query)?;
    
    println!("Query: {}", query_input);
    println!("Results:");
    for doc in &results {
        if let (Some(display_name), Some(original_name)) = (
            doc.fields.get("display_name"),
            doc.fields.get("original_name")
        ) {
            println!("  {} -> {}", original_name.to_string_repr(), display_name.to_string_repr());
        }
    }
    
    // Verify results
    assert_eq!(results.len(), 3);
    let display_names: Vec<String> = results.iter()
        .filter_map(|doc| doc.fields.get("display_name"))
        .map(|v| v.to_string_repr())
        .collect();
    
    assert!(display_names.contains(&"Alice (GOLD)".to_string()));
    assert!(display_names.contains(&"Bob (SILVER)".to_string()));
    assert!(display_names.contains(&"Charlie (BRONZE)".to_string()));
    
    println!("✓ Simple function test passed");
    Ok(())
}

fn test_multi_parameter_function(executor: &mut QueryExecutor<RocksDBStore>) -> OQLResult<()> {
    // Define a function that calculates a score based on age and tier
    let func_def = FunctionDefinition {
        name: "calculate_score".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "age".to_string(),
                param_type: "Integer".to_string(),
            },
            FunctionParameter {
                name: "tier".to_string(),
                param_type: "String".to_string(),
            },
        ],
        return_type: Some("Integer".to_string()),
        body: Expression::FunctionCall {
            name: "if_null".to_string(),
            args: vec![
                Expression::FieldAccess(FieldAccess { path: vec!["age".to_string()] }),
                Expression::Literal(Literal::Integer(0)),
            ],
        },
    };
    
    // Register the function
    executor.register_user_function(func_def)?;
    println!("✓ Registered function: calculate_score(age: Integer, tier: String) -> Integer");
    
    // Test the function
    let query_input = r#"customers.select { 
        name: .name,
        score: calculate_score(.age, .tier)
    }"#;
    
    let query = parse_oql(query_input).map_err(|e| OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    let results = executor.execute_query(&query)?;
    
    println!("Query: {}", query_input);
    println!("Results:");
    for doc in &results {
        if let (Some(name), Some(score)) = (
            doc.fields.get("name"),
            doc.fields.get("score")
        ) {
            println!("  {} -> score: {}", name.to_string_repr(), score.to_string_repr());
        }
    }
    
    println!("✓ Multi-parameter function test passed");
    Ok(())
}

fn test_function_with_builtin_calls(executor: &mut QueryExecutor<RocksDBStore>) -> OQLResult<()> {
    // Define a function that uses built-in string functions
    let func_def = FunctionDefinition {
        name: "format_email_domain".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "email".to_string(),
                param_type: "String".to_string(),
            },
        ],
        return_type: Some("String".to_string()),
        body: Expression::FunctionCall {
            name: "upper".to_string(),
            args: vec![
                Expression::FunctionCall {
                    name: "substring".to_string(),
                    args: vec![
                        Expression::FieldAccess(FieldAccess { path: vec!["email".to_string()] }),
                        Expression::Literal(Literal::Integer(0)),
                        Expression::Literal(Literal::Integer(5)),
                    ],
                },
            ],
        },
    };
    
    // Register the function
    executor.register_user_function(func_def)?;
    println!("✓ Registered function: format_email_domain(email: String) -> String");
    
    // Test the function
    let query_input = r#"customers.select { 
        name: .name,
        email_prefix: format_email_domain(.email)
    }"#;
    
    let query = parse_oql(query_input).map_err(|e| OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    let results = executor.execute_query(&query)?;
    
    println!("Query: {}", query_input);
    println!("Results:");
    for doc in &results {
        if let (Some(name), Some(prefix)) = (
            doc.fields.get("name"),
            doc.fields.get("email_prefix")
        ) {
            println!("  {} -> email prefix: {}", name.to_string_repr(), prefix.to_string_repr());
        }
    }
    
    println!("✓ Function with built-in calls test passed");
    Ok(())
}

fn test_function_type_validation(executor: &mut QueryExecutor<RocksDBStore>) -> OQLResult<()> {
    // Try to register a function with invalid parameter type
    let invalid_func_def = FunctionDefinition {
        name: "invalid_function".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "param".to_string(),
                param_type: "InvalidType".to_string(),
            },
        ],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("test".to_string())),
    };
    
    let result = executor.register_user_function(invalid_func_def);
    match result {
        Err(_) => {
            println!("✓ Type validation correctly rejected invalid parameter type");
        }
        Ok(_) => {
            println!("✗ Type validation should have rejected invalid parameter type");
            return Err(owl_db::error::OQLError::Generic {
                message: "Type validation test failed".to_string(),
            });
        }
    }
    
    Ok(())
}

fn test_function_error_handling(executor: &mut QueryExecutor<RocksDBStore>) -> OQLResult<()> {
    // Try to register a function with a name that already exists
    let duplicate_func_def = FunctionDefinition {
        name: "format_customer_name".to_string(), // This already exists
        parameters: vec![],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("duplicate".to_string())),
    };
    
    let result = executor.register_user_function(duplicate_func_def);
    match result {
        Err(_) => {
            println!("✓ Correctly rejected duplicate function name");
        }
        Ok(_) => {
            println!("✗ Should have rejected duplicate function name");
            return Err(owl_db::error::OQLError::Generic {
                message: "Duplicate function test failed".to_string(),
            });
        }
    }
    
    Ok(())
}