// Comprehensive Function System Tests for OQL
// Tests all built-in functions with edge cases, user-defined function execution,
// and conditional logic in complex queries

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::{parse_oql, FunctionDefinition, FunctionParameter, Expression, Literal, FieldAccess, Condition, ComparisonOperator},
    storage::{RocksDBStore, DocumentStore},
    error::{OQLResult},
};
use std::sync::Arc;
use std::collections::HashMap;
use chrono::{Utc, TimeZone};
use tempfile::TempDir;

/// Comprehensive test fixture for function system testing
struct FunctionSystemTestFixture {
    executor: QueryExecutor<RocksDBStore>,
    store: Arc<RocksDBStore>,
    _temp_dir: TempDir,
}

impl FunctionSystemTestFixture {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).expect("Failed to create store"));
        let executor = QueryExecutor::new(store.clone());
        
        Self {
            executor,
            store,
            _temp_dir: temp_dir,
        }
    }
    
    fn setup_comprehensive_test_data(&self) -> OQLResult<()> {
        // Create customers with various data types and edge cases
        let customers = vec![
            self.create_customer("cust_1", "Alice Johnson", 30, 15000.50, true, "2020-01-15"),
            self.create_customer("cust_2", "Bob Smith", 25, 8500.75, false, "2021-06-20"),
            self.create_customer("cust_3", "Charlie Brown", 45, 25000.00, true, "2019-03-10"),
            self.create_customer("cust_4", "Diana Prince", 35, 0.0, false, "2022-12-01"),
            self.create_customer("cust_5", "Eve Wilson", 28, -100.0, true, "2023-02-14"), // Negative balance for edge case testing
        ];
        
        // Create orders with references to customers
        let orders = vec![
            self.create_order("ord_1", "cust_1", 150.75, "completed", "2023-10-01"),
            self.create_order("ord_2", "cust_1", 89.99, "pending", "2023-10-15"),
            self.create_order("ord_3", "cust_2", 299.50, "completed", "2023-09-20"),
            self.create_order("ord_4", "cust_3", 1200.00, "completed", "2023-08-10"),
            self.create_order("ord_5", "cust_4", 45.25, "cancelled", "2023-11-05"),
        ];
        
        // Create products for testing
        let products = vec![
            self.create_product("prod_1", "Laptop", 999.99, 50),
            self.create_product("prod_2", "Mouse", 25.50, 200),
            self.create_product("prod_3", "Keyboard", 75.00, 100),
        ];
        
        // Store all test data
        for customer in customers {
            self.store.put_document(&customer)?;
        }
        for order in orders {
            self.store.put_document(&order)?;
        }
        for product in products {
            self.store.put_document(&product)?;
        }
        
        Ok(())
    }
    
    fn create_customer(&self, id: &str, name: &str, age: i64, balance: f64, premium: bool, joined_date: &str) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        fields.insert("age".to_string(), Value::Integer(age));
        fields.insert("balance".to_string(), Value::Float(balance));
        fields.insert("premium".to_string(), Value::Boolean(premium));
        fields.insert("joined_date".to_string(), Value::String(joined_date.to_string()));
        fields.insert("email".to_string(), Value::String(format!("{}@example.com", name.to_lowercase().replace(" ", "."))));
        
        Document::with_id(id.to_string(), "customers".to_string(), fields)
    }
    
    fn create_order(&self, id: &str, customer_id: &str, total: f64, status: &str, order_date: &str) -> Document {
        let mut fields = HashMap::new();
        fields.insert("customer_id".to_string(), Value::String(customer_id.to_string()));
        fields.insert("total".to_string(), Value::Float(total));
        fields.insert("status".to_string(), Value::String(status.to_string()));
        fields.insert("order_date".to_string(), Value::String(order_date.to_string()));
        
        Document::with_id(id.to_string(), "orders".to_string(), fields)
    }
    
    fn create_product(&self, id: &str, name: &str, price: f64, inventory: i64) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        fields.insert("price".to_string(), Value::Float(price));
        fields.insert("inventory".to_string(), Value::Integer(inventory));
        
        Document::with_id(id.to_string(), "products".to_string(), fields)
    }
}

// ========== STRING FUNCTION EDGE CASE TESTS ==========

#[test]
fn test_string_functions_edge_cases() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test trim with various whitespace scenarios
    let query = parse_oql(r#"customers.select { 
        name,
        trimmed_empty: trim(""),
        trimmed_spaces: trim("   hello   "),
        trimmed_tabs: trim("	tab	"),
        trimmed_newlines: trim("
line
")
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert!(results.len() > 0);
    let first_result = &results[0];
    assert_eq!(first_result.get_field("trimmed_empty"), Some(&Value::String("".to_string())));
    assert_eq!(first_result.get_field("trimmed_spaces"), Some(&Value::String("hello".to_string())));
    
    // Test substring with edge cases
    let query = parse_oql(r#"customers.where { .id == "cust_1" }.select { 
        name,
        substr_empty: substring("", 0),
        substr_out_of_bounds: substring(.name, 100),
        substr_negative_length: substring(.name, 0, 0),
        substr_partial: substring(.name, 0, 5)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.get_field("substr_empty"), Some(&Value::String("".to_string())));
    assert_eq!(result.get_field("substr_out_of_bounds"), Some(&Value::String("".to_string())));
    assert_eq!(result.get_field("substr_negative_length"), Some(&Value::String("".to_string())));
    assert_eq!(result.get_field("substr_partial"), Some(&Value::String("Alice".to_string())));
}

#[test]
fn test_string_functions_with_null_and_empty() {
    let fixture = FunctionSystemTestFixture::new();
    
    // Create document with null and empty string values
    let mut fields = HashMap::new();
    fields.insert("null_field".to_string(), Value::Null);
    fields.insert("empty_field".to_string(), Value::String("".to_string()));
    fields.insert("whitespace_field".to_string(), Value::String("   ".to_string()));
    
    let test_doc = Document::with_id("test_null".to_string(), "test_docs".to_string(), fields);
    fixture.store.put_document(&test_doc).expect("Failed to store test document");
    
    // Test string functions with null values
    let query = parse_oql(r#"test_docs.select { 
        upper_null: upper(.null_field),
        lower_null: lower(.null_field),
        trim_null: trim(.null_field),
        length_null: length(.null_field),
        length_empty: length(.empty_field),
        trim_whitespace: trim(.whitespace_field)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.get_field("upper_null"), Some(&Value::Null));
    assert_eq!(result.get_field("lower_null"), Some(&Value::Null));
    assert_eq!(result.get_field("trim_null"), Some(&Value::Null));
    assert_eq!(result.get_field("length_null"), Some(&Value::Null));
    assert_eq!(result.get_field("length_empty"), Some(&Value::Integer(0)));
    assert_eq!(result.get_field("trim_whitespace"), Some(&Value::String("".to_string())));
}

#[test]
fn test_string_functions_unicode_support() {
    let fixture = FunctionSystemTestFixture::new();
    
    // Create document with Unicode strings
    let mut fields = HashMap::new();
    fields.insert("unicode_text".to_string(), Value::String("Hello 世界 🌍".to_string()));
    fields.insert("emoji_text".to_string(), Value::String("🚀🎉🔥".to_string()));
    
    let unicode_doc = Document::with_id("unicode_test".to_string(), "test_docs".to_string(), fields);
    fixture.store.put_document(&unicode_doc).expect("Failed to store unicode document");
    
    let query = parse_oql(r#"test_docs.select { 
        unicode_upper: upper(.unicode_text),
        unicode_length: length(.unicode_text),
        emoji_length: length(.emoji_text),
        unicode_contains: contains(.unicode_text, "世界")
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.get_field("unicode_upper"), Some(&Value::String("HELLO 世界 🌍".to_string())));
    assert_eq!(result.get_field("unicode_contains"), Some(&Value::Boolean(true)));
}

// ========== MATH FUNCTION EDGE CASE TESTS ==========

#[test]
fn test_math_functions_edge_cases() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test math functions with edge values
    let query = parse_oql(r#"customers.where { .id == "cust_5" }.select { 
        name,
        absnegative: abs(.balance),
        sqrtzero: sqrt(0),
        powzero: pow(0, 5),
        powone: pow(5, 0),
        minsame: min(5, 5),
        maxsame: max(10, 10)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.get_field("absnegative"), Some(&Value::Float(100.0)));
    assert_eq!(result.get_field("sqrtzero"), Some(&Value::Float(0.0)));
    assert_eq!(result.get_field("powzero"), Some(&Value::Float(0.0)));
    assert_eq!(result.get_field("powone"), Some(&Value::Float(1.0)));
    assert_eq!(result.get_field("minsame"), Some(&Value::Integer(5)));
    assert_eq!(result.get_field("maxsame"), Some(&Value::Integer(10)));
}

#[test]
fn test_math_functions_error_conditions() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test sqrt with negative number - should return error
    let query = parse_oql(r#"customers.where { .id == "cust_5" }.select { 
        name,
        sqrt_negative: sqrt(.balance)
    }"#).expect("Failed to parse query");
    let result = fixture.executor.execute_query(&query);
    assert!(result.is_err(), "sqrt of negative number should return error");
    
    // Test log with zero - should return error
    let query = parse_oql(r#"customers.where { .id == "cust_4" }.select { 
        name,
        log_zero: log(.balance)
    }"#).expect("Failed to parse query");
    let result = fixture.executor.execute_query(&query);
    assert!(result.is_err(), "log of zero should return error");
}

#[test]
fn test_math_functions_with_mixed_types() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test math functions with integer and float combinations
    let query = parse_oql(r#"customers.where { .id == "cust_1" }.select { 
        name,
        minmixed: min(.age, .balance),
        maxmixed: max(.age, .balance),
        powmixed: pow(.age, 2.5)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    
    // min(30, 15000.50) should be 30.0 (as float)
    if let Some(Value::Float(min_val)) = result.get_field("minmixed") {
        assert!((min_val - 30.0).abs() < 0.001);
    } else {
        panic!("Expected float result for mixed type min");
    }
    
    // max(30, 15000.50) should be 15000.50
    if let Some(Value::Float(max_val)) = result.get_field("maxmixed") {
        assert!((max_val - 15000.50).abs() < 0.001);
    } else {
        panic!("Expected float result for mixed type max");
    }
}

#[test]
fn test_math_functions_precision() {
    let fixture = FunctionSystemTestFixture::new();
    
    // Create document with precise decimal values
    let mut fields = HashMap::new();
    fields.insert("precise_value".to_string(), Value::Float(3.141592653589793));
    fields.insert("small_value".to_string(), Value::Float(0.000001));
    fields.insert("large_value".to_string(), Value::Float(1e15));
    
    let precision_doc = Document::with_id("precision_test".to_string(), "test_docs".to_string(), fields);
    fixture.store.put_document(&precision_doc).expect("Failed to store precision document");
    
    let query = parse_oql(r#"test_docs.select { 
        rounded_pi: round(.precise_value),
        floored_pi: floor(.precise_value),
        ceiled_pi: ceil(.precise_value),
        sqrt_small: sqrt(.small_value),
        abs_large: abs(.large_value)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.get_field("rounded_pi"), Some(&Value::Integer(3)));
    assert_eq!(result.get_field("floored_pi"), Some(&Value::Integer(3)));
    assert_eq!(result.get_field("ceiled_pi"), Some(&Value::Integer(4)));
}

// ========== DATE/TIME FUNCTION COMPREHENSIVE TESTS ==========

#[test]
fn test_datetime_functions_comprehensive() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test various datetime functions
    let query = parse_oql(r#"customers.where { .id == "cust_1" }.select { 
        name,
        current_time: now(),
        current_date: today(),
        past_week: days_ago(7),
        past_hour: hours_ago(1),
        past_minute: minutes_ago(30)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    
    // All datetime functions should return DateTime values
    assert!(matches!(result.get_field("current_time"), Some(Value::DateTime(_))));
    assert!(matches!(result.get_field("current_date"), Some(Value::DateTime(_))));
    assert!(matches!(result.get_field("past_week"), Some(Value::DateTime(_))));
    assert!(matches!(result.get_field("past_hour"), Some(Value::DateTime(_))));
    assert!(matches!(result.get_field("past_minute"), Some(Value::DateTime(_))));
}

#[test]
fn test_datetime_extraction_functions() {
    let fixture = FunctionSystemTestFixture::new();
    
    // Create document with specific datetime
    let test_date = Utc.with_ymd_and_hms(2023, 10, 15, 14, 30, 45).unwrap();
    let mut fields = HashMap::new();
    fields.insert("test_datetime".to_string(), Value::DateTime(test_date));
    
    let datetime_doc = Document::with_id("datetime_test".to_string(), "test_docs".to_string(), fields);
    fixture.store.put_document(&datetime_doc).expect("Failed to store datetime document");
    
    let query = parse_oql(r#"test_docs.select { 
        extracted_year: year(.test_datetime),
        extracted_month: month(.test_datetime),
        extracted_day: day(.test_datetime),
        extracted_hour: hour(.test_datetime),
        extracted_minute: minute(.test_datetime),
        extracted_second: second(.test_datetime),
        extracted_weekday: weekday(.test_datetime)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.get_field("extracted_year"), Some(&Value::Integer(2023)));
    assert_eq!(result.get_field("extracted_month"), Some(&Value::Integer(10)));
    assert_eq!(result.get_field("extracted_day"), Some(&Value::Integer(15)));
    assert_eq!(result.get_field("extracted_hour"), Some(&Value::Integer(14)));
    assert_eq!(result.get_field("extracted_minute"), Some(&Value::Integer(30)));
    assert_eq!(result.get_field("extracted_second"), Some(&Value::Integer(45)));
}

#[test]
fn test_datetime_arithmetic_functions() {
    let fixture = FunctionSystemTestFixture::new();
    
    // Create document with base datetime
    let base_date = Utc.with_ymd_and_hms(2023, 6, 15, 12, 0, 0).unwrap();
    let mut fields = HashMap::new();
    fields.insert("base_date".to_string(), Value::DateTime(base_date));
    
    let datetime_doc = Document::with_id("datetime_test".to_string(), "test_docs".to_string(), fields);
    fixture.store.put_document(&datetime_doc).expect("Failed to store datetime document");
    
    let query = parse_oql(r#"test_docs.select { 
        add_5_days: add_days(.base_date, 5),
        add_3_hours: add_hours(.base_date, 3),
        diff_from_now: diff_days(.base_date, now())
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    
    // Verify datetime arithmetic results
    assert!(matches!(result.get_field("add_5_days"), Some(Value::DateTime(_))));
    assert!(matches!(result.get_field("add_3_hours"), Some(Value::DateTime(_))));
    assert!(matches!(result.get_field("diff_from_now"), Some(Value::Integer(_))));
}

// ========== USER-DEFINED FUNCTION COMPREHENSIVE TESTS ==========

#[test]
fn test_complex_user_defined_functions() {
    let mut fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Define customer lifetime value function
    let clv_function = FunctionDefinition {
        name: "customer_lifetime_value".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "balance".to_string(),
                param_type: "Float".to_string(),
            },
            FunctionParameter {
                name: "age".to_string(),
                param_type: "Integer".to_string(),
            },
        ],
        return_type: Some("Float".to_string()),
        body: Expression::FunctionCall {
            name: "max".to_string(),
            args: vec![
                Expression::FieldAccess(FieldAccess { path: vec!["balance".to_string()] }),
                Expression::FunctionCall {
                    name: "pow".to_string(),
                    args: vec![
                        Expression::FieldAccess(FieldAccess { path: vec!["age".to_string()] }),
                        Expression::Literal(Literal::Float(1.5)),
                    ],
                },
            ],
        },
    };
    
    fixture.executor.register_user_function(clv_function).expect("Failed to register CLV function");
    
    // Define customer risk score function
    let risk_function = FunctionDefinition {
        name: "customer_risk_score".to_string(),
        parameters: vec![
            FunctionParameter {
                name: "balance".to_string(),
                param_type: "Float".to_string(),
            },
        ],
        return_type: Some("String".to_string()),
        body: Expression::IfThenElse {
            condition: Box::new(Condition::Comparison {
                left: Expression::FieldAccess(FieldAccess { path: vec!["balance".to_string()] }),
                operator: ComparisonOperator::GreaterThan,
                right: Expression::Literal(Literal::Float(10000.0)),
            }),
            then_expr: Box::new(Expression::Literal(Literal::String("low".to_string()))),
            else_expr: Box::new(Expression::IfThenElse {
                condition: Box::new(Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess { path: vec!["balance".to_string()] }),
                    operator: ComparisonOperator::GreaterThan,
                    right: Expression::Literal(Literal::Float(5000.0)),
                }),
                then_expr: Box::new(Expression::Literal(Literal::String("medium".to_string()))),
                else_expr: Box::new(Expression::Literal(Literal::String("high".to_string()))),
            }),
        },
    };
    
    fixture.executor.register_user_function(risk_function).expect("Failed to register risk function");
    
    // Test the user-defined functions
    let query = parse_oql(r#"customers.select { 
        name,
        clv: customer_lifetime_value(.balance, .age),
        risk: customer_risk_score(.balance)
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert!(results.len() > 0);
    
    // Verify results for specific customers
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice Johnson")
    }).expect("Alice not found");
    
    // Alice has balance 15000.50, so risk should be "low"
    assert_eq!(alice.get_field("risk"), Some(&Value::String("low".to_string())));
    
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob Smith")
    }).expect("Bob not found");
    
    // Bob has balance 8500.75, so risk should be "medium"
    assert_eq!(bob.get_field("risk"), Some(&Value::String("medium".to_string())));
}

#[test]
fn test_user_defined_function_error_handling() {
    let mut fixture = FunctionSystemTestFixture::new();
    
    // Test function with invalid parameter count - this should succeed at parse time but fail at execution
    let query = parse_oql(r#"customers.select { result: trim(.name) }"#).expect("Should parse successfully");
    let result = fixture.executor.execute_query(&query);
    // This should succeed since trim is a valid function
    assert!(result.is_ok(), "Valid function should execute successfully");
    
    // Test function registration with duplicate name
    let func1 = FunctionDefinition {
        name: "duplicate_func".to_string(),
        parameters: vec![],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("first".to_string())),
    };
    
    let func2 = FunctionDefinition {
        name: "duplicate_func".to_string(),
        parameters: vec![],
        return_type: Some("String".to_string()),
        body: Expression::Literal(Literal::String("second".to_string())),
    };
    
    fixture.executor.register_user_function(func1).expect("First function should register");
    let result = fixture.executor.register_user_function(func2);
    assert!(result.is_err(), "Duplicate function name should return error");
}

// ========== CONDITIONAL LOGIC COMPREHENSIVE TESTS ==========

#[test]
fn test_complex_conditional_logic() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test complex nested conditional logic
    let query = parse_oql(r#"customers.select { 
        name,
        category: if .premium == true then 
            if .balance > 20000 then "VIP Gold"
            else if .balance > 10000 then "VIP Silver"
            else "VIP Bronze"
        else 
            if .balance > 5000 then "Standard Plus"
            else if .balance > 0 then "Standard"
            else "Basic"
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert!(results.len() > 0);
    
    // Verify categorization logic
    let charlie = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Charlie Brown")
    }).expect("Charlie not found");
    
    // Charlie: premium=true, balance=25000.00, should be "VIP Gold"
    assert_eq!(charlie.get_field("category"), Some(&Value::String("VIP Gold".to_string())));
    
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice Johnson")
    }).expect("Alice not found");
    
    // Alice: premium=true, balance=15000.50, should be "VIP Silver"
    assert_eq!(alice.get_field("category"), Some(&Value::String("VIP Silver".to_string())));
    
    let bob = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Bob Smith")
    }).expect("Bob not found");
    
    // Bob: premium=false, balance=8500.75, should be "Standard Plus"
    assert_eq!(bob.get_field("category"), Some(&Value::String("Standard Plus".to_string())));
}

#[test]
fn test_conditional_logic_with_functions() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test conditional logic combined with function calls
    let query = parse_oql(r#"customers.select { 
        name,
        status: if length(.name) > 10 then 
            concat("Long name: ", upper(.name))
        else 
            concat("Short name: ", lower(.name)),
        age_category: if .age >= 40 then "Senior"
                     else if .age >= 30 then "Adult"
                     else if .age >= 18 then "Young Adult"
                     else "Minor"
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert!(results.len() > 0);
    
    // Verify function integration with conditionals
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice Johnson")
    }).expect("Alice not found");
    
    // Alice Johnson (13 chars) should have "Long name: ALICE JOHNSON"
    assert_eq!(alice.get_field("status"), Some(&Value::String("Long name: ALICE JOHNSON".to_string())));
    // Alice (30) should be "Adult"
    assert_eq!(alice.get_field("age_category"), Some(&Value::String("Adult".to_string())));
    
    let charlie = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Charlie Brown")
    }).expect("Charlie not found");
    
    // Charlie Brown (13 chars) should have "Long name: CHARLIE BROWN"
    assert_eq!(charlie.get_field("status"), Some(&Value::String("Long name: CHARLIE BROWN".to_string())));
    // Charlie (45) should be "Senior"
    assert_eq!(charlie.get_field("age_category"), Some(&Value::String("Senior".to_string())));
}

#[test]
fn test_match_expressions_comprehensive() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test match expressions with various patterns
    let query = parse_oql(r#"customers.select { 
        name,
        tier: match .premium {
            true -> "Premium Customer",
            false -> "Standard Customer",
            _ -> "Unknown"
        },
        age_group: if .age >= 40 then "40+"
                  else if .age >= 30 then "30-39"
                  else if .age >= 20 then "20-29"
                  else "Under 20"
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert!(results.len() > 0);
    
    // Verify match expression results
    for result in &results {
        let premium_tier = result.get_field("tier").expect("tier field should exist");
        assert!(matches!(premium_tier, 
            Value::String(s) if s == "Premium Customer" || s == "Standard Customer"
        ));
        
        let age_group = result.get_field("age_group").expect("age_group field should exist");
        assert!(matches!(age_group, 
            Value::String(s) if ["40+", "30-39", "20-29", "Under 20"].contains(&s.as_str())
        ));
    }
}

// ========== INTEGRATION TESTS WITH COMPLEX QUERIES ==========

#[test]
fn test_functions_in_where_clauses() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test using functions in WHERE clauses
    let query = parse_oql(r#"customers.where { length(.name) > 10 }.select { name, balance }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should return customers where name length > 10
    assert!(results.len() > 0);
    for result in &results {
        if let Some(Value::String(name)) = result.get_field("name") {
            assert!(name.len() > 10, "Name '{}' should be longer than 10 characters", name);
        }
    }
}

#[test]
fn test_nested_function_calls_complex() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test deeply nested function calls
    let query = parse_oql(r#"customers.select { 
        name,
        processed_name: upper(trim(substring(lower(.name), 0, 5))),
        calculated_value: round(sqrt(abs(.balance))),
        formatted_info: concat(
            "Customer: ",
            .name,
            " (Age: ",
            to_string(.age),
            ", Balance: $",
            to_string(round(.balance)),
            ")"
        )
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert!(results.len() > 0);
    
    // Verify nested function call results
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice Johnson")
    }).expect("Alice not found");
    
    // Alice Johnson -> lower -> substring(0,5) -> trim -> upper = "ALICE"
    assert_eq!(alice.get_field("processed_name"), Some(&Value::String("ALICE".to_string())));
    
    // Verify formatted info contains expected components
    if let Some(Value::String(formatted)) = alice.get_field("formatted_info") {
        assert!(formatted.contains("Customer: Alice Johnson"));
        assert!(formatted.contains("Age: 30"));
        assert!(formatted.contains("Balance: $15001")); // rounded
    } else {
        panic!("Expected formatted info string");
    }
}

#[test]
fn test_function_performance_with_large_dataset() {
    let fixture = FunctionSystemTestFixture::new();
    
    // Create a larger dataset for performance testing
    for i in 0..100 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("Customer {}", i)));
        fields.insert("value".to_string(), Value::Float(i as f64 * 1.5));
        fields.insert("category".to_string(), Value::String(if i % 2 == 0 { "even".to_string() } else { "odd".to_string() }));
        
        let doc = Document::with_id(format!("perf_{}", i), "performance_test".to_string(), fields);
        fixture.store.put_document(&doc).expect("Failed to store performance test document");
    }
    
    // Test complex query with multiple function calls on large dataset
    let query = parse_oql(r#"performance_test.select { 
        name,
        processed: upper(concat(.name, " - ", .category)),
        calculated: round(sqrt(.value)),
        conditional: if .value > 50 then "high" else "low"
    }"#).expect("Failed to parse query");
    
    let start_time = std::time::Instant::now();
    let results = fixture.executor.execute_query(&query).expect("Failed to execute performance query");
    let duration = start_time.elapsed();
    
    assert_eq!(results.len(), 100);
    println!("Performance test completed in {:?}", duration);
    
    // Verify some results
    let first_result = &results[0];
    assert!(matches!(first_result.get_field("processed"), Some(Value::String(_))));
    assert!(matches!(first_result.get_field("calculated"), Some(Value::Integer(_))));
    assert!(matches!(first_result.get_field("conditional"), Some(Value::String(_))));
}

// ========== ERROR HANDLING AND EDGE CASES ==========

#[test]
fn test_function_error_propagation() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test that errors in nested function calls are properly propagated
    let query = parse_oql(r#"customers.where { .id == "cust_5" }.select { 
        name,
        error_result: sqrt(log(.balance))
    }"#).expect("Failed to parse query");
    
    // This should fail because log of negative number should error
    let result = fixture.executor.execute_query(&query);
    assert!(result.is_err(), "Nested function error should propagate");
}

#[test]
fn test_function_type_coercion() {
    let fixture = FunctionSystemTestFixture::new();
    fixture.setup_comprehensive_test_data().expect("Failed to setup test data");
    
    // Test automatic type coercion in function calls
    let query = parse_oql(r#"customers.select { 
        name,
        string_from_int: to_string(.age),
        int_from_float: to_int(.balance),
        float_from_int: to_float(.age),
        concat_mixed: concat(.name, " is ", to_string(.age), " years old")
    }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert!(results.len() > 0);
    
    let alice = results.iter().find(|doc| {
        matches!(doc.get_field("name"), Some(Value::String(name)) if name == "Alice Johnson")
    }).expect("Alice not found");
    
    assert_eq!(alice.get_field("string_from_int"), Some(&Value::String("30".to_string())));
    assert_eq!(alice.get_field("int_from_float"), Some(&Value::Integer(15000))); // truncated
    assert_eq!(alice.get_field("float_from_int"), Some(&Value::Float(30.0)));
    assert_eq!(alice.get_field("concat_mixed"), Some(&Value::String("Alice Johnson is 30 years old".to_string())));
}

#[test]
fn test_function_system_completeness() {
    let fixture = FunctionSystemTestFixture::new();
    
    // Verify all expected built-in functions are available
    let function_names = fixture.executor.function_names();
    
    // String functions
    let string_functions = ["trim", "upper", "lower", "length", "substring", "concat", 
                           "replace", "split", "starts_with", "ends_with", "contains",
                           "pad_left", "pad_right"];
    for func in &string_functions {
        assert!(function_names.contains(&func.to_string()), "Missing string function: {}", func);
    }
    
    // Math functions
    let math_functions = ["abs", "round", "floor", "ceil", "sqrt", "pow", "min", "max",
                         "sin", "cos", "tan", "log", "ln", "exp", "random"];
    for func in &math_functions {
        assert!(function_names.contains(&func.to_string()), "Missing math function: {}", func);
    }
    
    // DateTime functions
    let datetime_functions = ["now", "today", "days_ago", "hours_ago", "minutes_ago",
                             "format_date", "parse_date", "year", "month", "day",
                             "hour", "minute", "second", "weekday", "add_days", 
                             "add_hours", "diff_days"];
    for func in &datetime_functions {
        assert!(function_names.contains(&func.to_string()), "Missing datetime function: {}", func);
    }
    
    // Utility functions
    let utility_functions = ["coalesce", "if_null", "to_string", "to_int", "to_float", "type_of"];
    for func in &utility_functions {
        assert!(function_names.contains(&func.to_string()), "Missing utility function: {}", func);
    }
    
    println!("Function system completeness verified. Total functions: {}", function_names.len());
}