// Comprehensive tests for user-defined functions in OQL

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::{parse_oql, parse_oql_statement, OQLStatement, FunctionDefinition, FunctionParameter, Expression, Literal, FieldAccess},
    storage::{RocksDBStore, DocumentStore},
    error::{OQLResult, OQLError},
};
use std::sync::Arc;
use chrono::Utc;
use tempfile::TempDir;

fn create_test_executor() -> (QueryExecutor<RocksDBStore>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
    let executor = QueryExecutor::new(store);
    (executor, temp_dir)
}

fn setup_test_data(executor: &QueryExecutor<RocksDBStore>) -> OQLResult<()> {
    let customers = vec![
        Document {
            id: "cust_1".to_string(),
            schema: "customers".to_string(),
            fields: [
                ("name".to_string(), Value::String("Alice".to_string())),
                ("age".to_string(), Value::Integer(30)),
                ("email".to_string(), Value::String("alice@example.com".to_string())),
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
            ].into_iter().collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        },
    ];
    
    for customer in customers {
        executor.store().put_document(&customer)?;
    }
    
    Ok(())
}

#[test]
fn test_function_definition_parsing() {
    // Test parsing function definitions
    let input = r#"fn greet(name: String) -> String { concat("Hello, ", name) }"#;
    
    let result = parse_oql_statement(input).unwrap();
    match result {
        OQLStatement::FunctionDef(func_def) => {
            assert_eq!(func_def.name, "greet");
            assert_eq!(func_def.parameters.len(), 1);
            assert_eq!(func_def.parameters[0].name, "name");
            assert_eq!(func_def.parameters[0].param_type, "String");
            assert_eq!(func_def.return_type, Some("String".to_string()));
        }
        _ => panic!("Expected function definition"),
    }
}

#[test]
fn test_function_registration_and_execution() {
    let (mut executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor).unwrap();
    
    // Define a simple function
    let func_def = FunctionDefinition {
        name: "double_age".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "age".to_string(),
                param_type: "Integer".to_string(),
            },
        ],
        return_type: Some("Integer".to_string()),
        body: Expression::FunctionCall {
            name: "abs".to_string(), // Use a built-in function for simplicity
            args: vec![Expression::FieldAccess(FieldAccess { path: vec!["age".to_string()] })],
        },
    };
    
    // Register the function
    executor.register_user_function(func_def).unwrap();
    
    // Test the function in a query
    let query_input = r#"customers.select { name: .name, doubled_age: double_age(.age) }"#;
    let query = parse_oql(query_input).unwrap();
    let results = executor.execute_query(&query).unwrap();
    
    assert_eq!(results.len(), 2);
    
    // Verify results
    for doc in &results {
        let name = doc.fields.get("name").unwrap();
        let doubled_age = doc.fields.get("doubled_age").unwrap();
        
        match name {
            Value::String(n) if n == "Alice" => {
                assert_eq!(*doubled_age, Value::Integer(30)); // abs(30) = 30
            }
            Value::String(n) if n == "Bob" => {
                assert_eq!(*doubled_age, Value::Integer(25)); // abs(25) = 25
            }
            _ => panic!("Unexpected name: {:?}", name),
        }
    }
}

#[test]
fn test_function_with_multiple_parameters() {
    let (mut executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor).unwrap();
    
    // Define a function with multiple parameters
    let func_def = FunctionDefinition {
        name: "format_info".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "name".to_string(),
                param_type: "String".to_string(),
            },
            FunctionParameter {
                name: "age".to_string(),
                param_type: "Integer".to_string(),
            },
        ],
        return_type: Some("String".to_string()),
        body: Expression::FunctionCall {
            name: "concat".to_string(),
            args: vec![
                Expression::FieldAccess(FieldAccess { path: vec!["name".to_string()] }),
                Expression::Literal(Literal::String(" is ".to_string())),
                Expression::FunctionCall {
                    name: "to_string".to_string(),
                    args: vec![Expression::FieldAccess(FieldAccess { path: vec!["age".to_string()] })],
                },
            ],
        },
    };
    
    // Register the function
    executor.register_user_function(func_def).unwrap();
    
    // Test the function
    let query_input = r#"customers.select { info: format_info(.name, .age) }"#;
    let query = parse_oql(query_input).unwrap();
    let results = executor.execute_query(&query).unwrap();
    
    assert_eq!(results.len(), 2);
    
    // Verify results contain expected format
    let info_values: Vec<String> = results.iter()
        .filter_map(|doc| doc.fields.get("info"))
        .filter_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    
    assert!(info_values.contains(&"Alice is 30".to_string()));
    assert!(info_values.contains(&"Bob is 25".to_string()));
}

#[test]
fn test_function_type_validation() {
    let (mut executor, _temp_dir) = create_test_executor();
    
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
    assert!(result.is_err());
    
    match result {
        Err(OQLError::TypeError { expected, actual }) => {
            assert_eq!(expected, "valid type");
            assert_eq!(actual, "InvalidType");
        }
        _ => panic!("Expected TypeError"),
    }
}

#[test]
fn test_duplicate_function_names() {
    let (mut executor, _temp_dir) = create_test_executor();
    
    // Register a function
    let func_def1 = FunctionDefinition {
        name: "test_function".to_string(),
        parameters: vec![],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("first".to_string())),
    };
    
    executor.register_user_function(func_def1).unwrap();
    
    // Try to register another function with the same name
    let func_def2 = FunctionDefinition {
        name: "test_function".to_string(),
        parameters: vec![],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("second".to_string())),
    };
    
    let result = executor.register_user_function(func_def2);
    assert!(result.is_err());
    
    match result {
        Err(OQLError::Generic { message }) => {
            assert!(message.contains("already exists"));
        }
        _ => panic!("Expected Generic error"),
    }
}

#[test]
fn test_function_argument_count_validation() {
    let (mut executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor).unwrap();
    
    // Define a function that expects 2 parameters
    let func_def = FunctionDefinition {
        name: "two_param_func".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "param1".to_string(),
                param_type: "String".to_string(),
            },
            FunctionParameter {
                name: "param2".to_string(),
                param_type: "Integer".to_string(),
            },
        ],
        return_type: Some("String".to_string()),
        body: Expression::FieldAccess(FieldAccess { path: vec!["param1".to_string()] }),
    };
    
    executor.register_user_function(func_def).unwrap();
    
    // Try to call it with wrong number of arguments
    let query_input = r#"customers.select { result: two_param_func(.name) }"#; // Only 1 arg instead of 2
    let query = parse_oql(query_input).unwrap();
    let result = executor.execute_query(&query);
    
    assert!(result.is_err());
    match result {
        Err(OQLError::Generic { message }) => {
            assert!(message.contains("expects 2 arguments, got 1"));
        }
        _ => panic!("Expected argument count error"),
    }
}

#[test]
fn test_function_with_builtin_calls() {
    let (mut executor, _temp_dir) = create_test_executor();
    setup_test_data(&executor).unwrap();
    
    // Define a function that uses built-in functions
    let func_def = FunctionDefinition {
        name: "format_name".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "name".to_string(),
                param_type: "String".to_string(),
            },
        ],
        return_type: Some("String".to_string()),
        body: Expression::FunctionCall {
            name: "upper".to_string(),
            args: vec![
                Expression::FunctionCall {
                    name: "trim".to_string(),
                    args: vec![Expression::FieldAccess(FieldAccess { path: vec!["name".to_string()] })],
                },
            ],
        },
    };
    
    executor.register_user_function(func_def).unwrap();
    
    // Test the function
    let query_input = r#"customers.select { formatted_name: format_name(.name) }"#;
    let query = parse_oql(query_input).unwrap();
    let results = executor.execute_query(&query).unwrap();
    
    assert_eq!(results.len(), 2);
    
    // Verify results are uppercase
    for doc in &results {
        let formatted_name = doc.fields.get("formatted_name").unwrap();
        match formatted_name {
            Value::String(s) => {
                assert!(s == "ALICE" || s == "BOB");
            }
            _ => panic!("Expected string result"),
        }
    }
}

#[test]
fn test_function_registry_function_names() {
    let (mut executor, _temp_dir) = create_test_executor();
    
    // Register a few functions
    let func_def1 = FunctionDefinition {
        name: "func1".to_string(),
        parameters: vec![],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("test1".to_string())),
    };
    
    let func_def2 = FunctionDefinition {
        name: "func2".to_string(),
        parameters: vec![],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("test2".to_string())),
    };
    
    executor.register_user_function(func_def1).unwrap();
    executor.register_user_function(func_def2).unwrap();
    
    // Check that function names include both built-in and user-defined functions
    let function_names = executor.function_names();
    
    // Should include built-in functions
    assert!(function_names.contains(&"upper".to_string()));
    assert!(function_names.contains(&"lower".to_string()));
    assert!(function_names.contains(&"trim".to_string()));
    
    // Should include user-defined functions
    assert!(function_names.contains(&"func1".to_string()));
    assert!(function_names.contains(&"func2".to_string()));
}