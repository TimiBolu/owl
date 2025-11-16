// Tests for Built-in Functions in OQL
// Tests string manipulation, math, and date/time functions

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::parse_oql,
    storage::{DocumentStore, RocksDBStore},
};
use chrono::{DateTime, Utc, TimeZone};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

/// Test fixture for function tests
struct FunctionTestFixture {
    executor: QueryExecutor<RocksDBStore>,
    store: Arc<RocksDBStore>,
    _temp_dir: TempDir,
}

impl FunctionTestFixture {
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
    
    fn create_test_data(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Create test documents with various data types
        let test_docs = vec![
            create_test_doc("doc1", "Hello World", 42, 3.14, true),
            create_test_doc("doc2", "  TRIM ME  ", -10, 2.71, false),
            create_test_doc("doc3", "lowercase text", 0, 0.0, true),
            create_test_doc("doc4", "MiXeD cAsE", 100, -1.5, false),
        ];
        
        for doc in test_docs {
            self.store.put_document(&doc)?;
        }
        
        Ok(())
    }
}

fn create_test_doc(id: &str, text: &str, number: i64, decimal: f64, flag: bool) -> Document {
    let mut fields = HashMap::new();
    fields.insert("text".to_string(), Value::String(text.to_string()));
    fields.insert("number".to_string(), Value::Integer(number));
    fields.insert("decimal".to_string(), Value::Float(decimal));
    fields.insert("flag".to_string(), Value::Boolean(flag));
    fields.insert("created_at".to_string(), Value::DateTime(Utc::now()));
    
    Document::with_id(id.to_string(), "test_docs".to_string(), fields)
}

// ========== STRING FUNCTION TESTS ==========

#[test]
fn test_string_functions_basic() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test trim function
    let query = parse_oql(r#"test_docs.where { .id == "doc2" }.select { trimmed: trim(.text) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("trimmed"), Some(&Value::String("TRIM ME".to_string())));
    
    // Test upper function
    let query = parse_oql(r#"test_docs.where { .id == "doc3" }.select { upper_text: upper(.text) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("upper_text"), Some(&Value::String("LOWERCASE TEXT".to_string())));
    
    // Test lower function
    let query = parse_oql(r#"test_docs.where { .id == "doc4" }.select { lower_text: lower(.text) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("lower_text"), Some(&Value::String("mixed case".to_string())));
}

#[test]
fn test_string_length_and_substring() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test length function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { text_length: length(.text) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("text_length"), Some(&Value::Integer(11))); // "Hello World" = 11 chars
    
    // Test substring function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { substr: substring(.text, 6) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("substr"), Some(&Value::String("World".to_string())));
}

#[test]
fn test_string_contains_and_starts_with() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test contains function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { has_world: contains(.text, "World") }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("has_world"), Some(&Value::Boolean(true)));
    
    // Test starts_with function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { starts_hello: starts_with(.text, "Hello") }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("starts_hello"), Some(&Value::Boolean(true)));
    
    // Test ends_with function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { ends_world: ends_with(.text, "World") }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("ends_world"), Some(&Value::Boolean(true)));
}

#[test]
fn test_string_concat_and_replace() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test concat function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { greeting: concat(.text, "!") }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("greeting"), Some(&Value::String("Hello World!".to_string())));
    
    // Test replace function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { replaced: replace(.text, "World", "Universe") }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("replaced"), Some(&Value::String("Hello Universe".to_string())));
}

// ========== MATH FUNCTION TESTS ==========

#[test]
fn test_math_functions_basic() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test abs function with negative number
    let query = parse_oql(r#"test_docs.where { .id == "doc2" }.select { absolute: abs(.number) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("absolute"), Some(&Value::Integer(10)));
    
    // Test abs function with positive number
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { absolute: abs(.number) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("absolute"), Some(&Value::Integer(42)));
}

#[test]
fn test_math_rounding_functions() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test round function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { rounded: round(.decimal) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("rounded"), Some(&Value::Integer(3)));
    
    // Test floor function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { floored: floor(.decimal) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("floored"), Some(&Value::Integer(3)));
    
    // Test ceil function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { ceiled: ceil(.decimal) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("ceiled"), Some(&Value::Integer(4)));
}

#[test]
fn test_math_advanced_functions() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test sqrt function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { square_root: sqrt(.number) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    if let Some(Value::Float(sqrt_val)) = results[0].get_field("square_root") {
        assert!((sqrt_val - (42.0_f64).sqrt()).abs() < 0.001);
    } else {
        panic!("Expected float result for sqrt");
    }
    
    // Test pow function
    let query = parse_oql(r#"test_docs.where { .id == "doc3" }.select { powered: pow(.number, 2) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    if let Some(Value::Float(pow_val)) = results[0].get_field("powered") {
        assert!((pow_val - 0.0).abs() < 0.001); // 0^2 = 0
    } else {
        panic!("Expected float result for pow");
    }
}

#[test]
fn test_math_min_max_functions() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test min function with two integers
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { minimum: min(.number, 50) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("minimum"), Some(&Value::Integer(42)));
    
    // Test max function with two integers
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { maximum: max(.number, 50) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("maximum"), Some(&Value::Integer(50)));
}

// ========== DATE/TIME FUNCTION TESTS ==========

#[test]
fn test_datetime_now_and_today() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test now function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { current_time: now() }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].get_field("current_time"), Some(Value::DateTime(_))));
    
    // Test today function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { current_date: today() }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].get_field("current_date"), Some(Value::DateTime(_))));
}

#[test]
fn test_datetime_extraction_functions() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test year extraction
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { current_year: year(.created_at) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    if let Some(Value::Integer(year_val)) = results[0].get_field("current_year") {
        assert!(*year_val >= 2024); // Should be current year or later
    } else {
        panic!("Expected integer result for year");
    }
    
    // Test month extraction
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { current_month: month(.created_at) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    if let Some(Value::Integer(month_val)) = results[0].get_field("current_month") {
        assert!(*month_val >= 1 && *month_val <= 12);
    } else {
        panic!("Expected integer result for month");
    }
}

#[test]
fn test_datetime_relative_functions() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test days_ago function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { past_date: days_ago(7) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].get_field("past_date"), Some(Value::DateTime(_))));
    
    // Test hours_ago function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { past_hour: hours_ago(2) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].get_field("past_hour"), Some(Value::DateTime(_))));
}

#[test]
fn test_datetime_format_function() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test format_date function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { formatted_date: format_date(.created_at, "%Y-%m-%d") }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    if let Some(Value::String(formatted)) = results[0].get_field("formatted_date") {
        // Should be in YYYY-MM-DD format
        assert!(formatted.len() == 10);
        assert!(formatted.contains('-'));
    } else {
        panic!("Expected string result for formatted date");
    }
}

// ========== UTILITY FUNCTION TESTS ==========

#[test]
fn test_utility_functions() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test coalesce function (first non-null value)
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { first_value: coalesce(.text, "default") }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("first_value"), Some(&Value::String("Hello World".to_string())));
    
    // Test to_string function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { number_as_string: to_string(.number) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("number_as_string"), Some(&Value::String("42".to_string())));
    
    // Test to_int function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { decimal_as_int: to_int(.decimal) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("decimal_as_int"), Some(&Value::Integer(3)));
    
    // Test type_of function
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { text_type: type_of(.text) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("text_type"), Some(&Value::String("String".to_string())));
}

// ========== ERROR HANDLING TESTS ==========

#[test]
fn test_function_error_handling() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test sqrt with negative number should handle gracefully
    let query = parse_oql(r#"test_docs.where { .id == "doc2" }.select { sqrt_negative: sqrt(.number) }"#).expect("Failed to parse query");
    let result = fixture.executor.execute_query(&query);
    
    // Should return an error for sqrt of negative number
    assert!(result.is_err());
    
    // Test unknown function should return error
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { unknown: nonexistent_function(.text) }"#).expect("Failed to parse query");
    let result = fixture.executor.execute_query(&query);
    
    // Should return an error for unknown function
    assert!(result.is_err());
}

#[test]
fn test_function_with_null_values() {
    let fixture = FunctionTestFixture::new();
    
    // Create a document with null values
    let mut fields = HashMap::new();
    fields.insert("text".to_string(), Value::Null);
    fields.insert("number".to_string(), Value::Null);
    
    let null_doc = Document::with_id("null_doc".to_string(), "test_docs".to_string(), fields);
    fixture.store.put_document(&null_doc).expect("Failed to store null document");
    
    // Test string functions with null
    let query = parse_oql(r#"test_docs.where { .id == "null_doc" }.select { trimmed_null: trim(.text) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("trimmed_null"), Some(&Value::Null));
    
    // Test math functions with null
    let query = parse_oql(r#"test_docs.where { .id == "null_doc" }.select { abs_null: abs(.number) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("abs_null"), Some(&Value::Null));
}

// ========== COMPLEX FUNCTION COMBINATIONS ==========

#[test]
fn test_nested_function_calls() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test nested function calls: upper(trim(text))
    let query = parse_oql(r#"test_docs.where { .id == "doc2" }.select { clean_upper: upper(trim(.text)) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("clean_upper"), Some(&Value::String("TRIM ME".to_string())));
}

#[test]
fn test_functions_in_where_clauses() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test using functions in WHERE clauses
    let query = parse_oql(r#"test_docs.where { length(.text) > 10 }.select { id, text }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should return documents where text length > 10
    assert!(results.len() > 0);
    for result in &results {
        if let Some(Value::String(text)) = result.get_field("text") {
            assert!(text.len() > 10);
        }
    }
}

#[test]
fn test_math_functions_with_mixed_types() {
    let fixture = FunctionTestFixture::new();
    fixture.create_test_data().expect("Failed to create test data");
    
    // Test min/max with integer and float
    let query = parse_oql(r#"test_docs.where { .id == "doc1" }.select { minmixed: min(.number, .decimal) }"#).expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    if let Some(Value::Float(min_val)) = results[0].get_field("minmixed") {
        assert!((min_val - 3.14).abs() < 0.001); // min(42, 3.14) = 3.14
    } else {
        panic!("Expected float result for mixed type min");
    }
}