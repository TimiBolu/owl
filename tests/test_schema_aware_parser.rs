// Schema-aware parser tests for type checking during parsing

use owl_db::{
    parser::{parse_oql_with_schema, Tokenizer, Parser},
    schema::{Schema, SchemaRegistry, FieldType, ReferenceType, CascadeBehavior},
};
use std::collections::HashMap;

fn create_test_schema_registry() -> SchemaRegistry {
    let registry = SchemaRegistry::new();
    
    // Create users schema
    let mut user_fields = HashMap::new();
    user_fields.insert("name".to_string(), FieldType::String);
    user_fields.insert("age".to_string(), FieldType::Integer);
    user_fields.insert("email".to_string(), FieldType::String);
    user_fields.insert("active".to_string(), FieldType::Boolean);
    user_fields.insert("score".to_string(), FieldType::Float);
    let users_schema = Schema::new("users".to_string(), user_fields);
    registry.register(users_schema).unwrap();
    
    // Create orders schema with reference to users
    let mut order_fields = HashMap::new();
    order_fields.insert("order_id".to_string(), FieldType::String);
    order_fields.insert("amount".to_string(), FieldType::Float);
    order_fields.insert("quantity".to_string(), FieldType::Integer);
    order_fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("users".to_string())));
    let orders_schema = Schema::new("orders".to_string(), order_fields);
    registry.register(orders_schema).unwrap();
    
    registry
}

#[test]
fn test_type_checking_valid_comparison() {
    let registry = create_test_schema_registry();
    
    // Valid: comparing integer with integer
    let query = r#"users.where { .age > 25 }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept valid integer comparison");
}

#[test]
fn test_type_checking_invalid_operator_for_type() {
    let registry = create_test_schema_registry();
    
    // Invalid: using string operator on integer field
    let query = r#"users.where { .age contains "25" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject invalid operator for integer type");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Type error"), "Error should mention type error");
}

#[test]
fn test_type_checking_incompatible_types() {
    let registry = create_test_schema_registry();
    
    // Invalid: comparing integer with string
    let query = r#"users.where { .age == "twenty-five" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject incompatible type comparison");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Type error") || err.to_string().contains("Cannot compare"), 
            "Error should mention type incompatibility");
}

#[test]
fn test_type_checking_valid_string_operators() {
    let registry = create_test_schema_registry();
    
    // Valid: using string operators on string field
    let queries = vec![
        r#"users.where { .name contains "Alice" }"#,
        r#"users.where { .name starts_with "A" }"#,
        r#"users.where { .name ends_with "Smith" }"#,
    ];
    
    for query in queries {
        let result = parse_oql_with_schema(query, registry.clone());
        assert!(result.is_ok(), "Should accept valid string operator: {}", query);
    }
}

#[test]
fn test_type_checking_function_arguments() {
    let registry = create_test_schema_registry();
    
    // Valid: sum on numeric field
    let query = r#"users.sum(.age)"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept sum on integer field");
    
    // Valid: avg on float field
    let query = r#"users.avg(.score)"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept avg on float field");
}

#[test]
fn test_type_checking_invalid_function_arguments() {
    let registry = create_test_schema_registry();
    
    // Invalid: sum on string field
    let query = r#"users.sum(.name)"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject sum on string field");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Type error") || err.to_string().contains("numeric"), 
            "Error should mention type requirement");
}

#[test]
fn test_type_checking_nonexistent_schema() {
    let registry = create_test_schema_registry();
    
    // Invalid: schema doesn't exist
    let query = r#"products.where { .price > 100 }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject query on nonexistent schema");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Schema") || err.to_string().contains("not found"), 
            "Error should mention schema not found");
}

#[test]
fn test_type_checking_nonexistent_field() {
    let registry = create_test_schema_registry();
    
    // Invalid: field doesn't exist in schema
    let query = r#"users.where { .nonexistent_field == "value" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject query with nonexistent field");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Field") || err.to_string().contains("not found"), 
            "Error should mention field not found");
}

#[test]
fn test_type_checking_insert_with_validation() {
    let registry = create_test_schema_registry();
    
    // Valid: insert with correct types
    let query = r#"users.insert { name: "Alice", age: 30, email: "alice@example.com", active: true, score: 95.5 }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept insert with correct types");
}

#[test]
fn test_type_checking_insert_with_wrong_types() {
    let registry = create_test_schema_registry();
    
    // Invalid: insert with wrong type for age (string instead of integer)
    let query = r#"users.insert { name: "Alice", age: "thirty", email: "alice@example.com" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject insert with wrong type");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Type error") || err.to_string().contains("Cannot compare"), 
            "Error should mention type mismatch");
}

#[test]
fn test_type_checking_update_with_validation() {
    let registry = create_test_schema_registry();
    
    // Valid: update with correct types
    let query = r#"users.update { age: 31, score: 96.0 }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept update with correct types");
}

#[test]
fn test_type_checking_update_with_wrong_types() {
    let registry = create_test_schema_registry();
    
    // Invalid: update with wrong type
    let query = r#"users.update { age: "thirty-one" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject update with wrong type");
}

#[test]
fn test_type_checking_boolean_field() {
    let registry = create_test_schema_registry();
    
    // Valid: comparing boolean with boolean
    let query = r#"users.where { .active == true }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept boolean comparison");
    
    // Invalid: comparing boolean with string
    let query = r#"users.where { .active == "yes" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject boolean compared with string");
}

#[test]
fn test_field_existence_validation_in_select() {
    let registry = create_test_schema_registry();
    
    // Valid: selecting existing fields
    let query = r#"users.select { name, age, email }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept select with existing fields");
    
    // Invalid: selecting nonexistent field
    let query = r#"users.select { name, nonexistent_field }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject select with nonexistent field");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Field") || err.to_string().contains("not found"), 
            "Error should mention field not found");
}

#[test]
fn test_field_existence_validation_nested_reference() {
    let registry = create_test_schema_registry();
    
    // Valid: accessing field through reference
    let query = r#"orders.where { .customer.name == "Alice" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept nested field access through reference");
    
    // Invalid: accessing nonexistent field in referenced schema
    let query = r#"orders.where { .customer.nonexistent == "value" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject nonexistent field in referenced schema");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Field") || err.to_string().contains("not found"), 
            "Error should mention field not found");
}

#[test]
fn test_field_existence_validation_in_insert() {
    let registry = create_test_schema_registry();
    
    // Invalid: inserting into nonexistent field
    let query = r#"users.insert { name: "Alice", nonexistent_field: "value" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject insert with nonexistent field");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Field") || err.to_string().contains("not found"), 
            "Error should mention field not found");
}

#[test]
fn test_field_existence_validation_in_update() {
    let registry = create_test_schema_registry();
    
    // Invalid: updating nonexistent field
    let query = r#"users.update { nonexistent_field: "value" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject update with nonexistent field");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Field") || err.to_string().contains("not found"), 
            "Error should mention field not found");
}

#[test]
fn test_field_existence_validation_in_aggregation() {
    let registry = create_test_schema_registry();
    
    // Valid: aggregating existing field
    let query = r#"users.sum(.age)"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept aggregation on existing field");
    
    // Invalid: aggregating nonexistent field
    let query = r#"users.sum(.nonexistent_field)"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject aggregation on nonexistent field");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Field") || err.to_string().contains("not found"), 
            "Error should mention field not found");
}

#[test]
fn test_field_existence_validation_complex_expression() {
    let registry = create_test_schema_registry();
    
    // Valid: complex expression with existing fields
    let query = r#"users.where { .age > 25 and .active == true }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept complex expression with existing fields");
    
    // Invalid: complex expression with nonexistent field
    let query = r#"users.where { .age > 25 and .nonexistent == true }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject complex expression with nonexistent field");
}

#[test]
fn test_reference_target_validation() {
    let registry = create_test_schema_registry();
    
    // Valid: reference field points to existing schema
    let query = r#"orders.where { .customer.name == "Alice" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept reference to existing schema");
}

#[test]
fn test_reference_target_validation_invalid_navigation() {
    let registry = create_test_schema_registry();
    
    // Invalid: trying to navigate through non-reference field
    let query = r#"orders.where { .amount.something == "value" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject navigation through non-reference field");
    
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not a reference") || err.to_string().contains("cannot navigate"), 
            "Error should mention invalid navigation");
}

#[test]
fn test_reference_target_validation_in_select() {
    let registry = create_test_schema_registry();
    
    // Valid: selecting through reference
    let query = r#"orders.select { customer_name: .customer.name, customer_email: .customer.email }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept select with reference navigation");
}

#[test]
fn test_reference_cascade_behavior() {
    // Create a registry with cascade settings
    let registry = SchemaRegistry::new();
    
    // Create profiles schema
    let mut profile_fields = HashMap::new();
    profile_fields.insert("bio".to_string(), FieldType::String);
    let profiles_schema = Schema::new("profiles".to_string(), profile_fields);
    registry.register(profiles_schema).unwrap();
    
    // Create users schema with reference to profiles
    let mut user_fields = HashMap::new();
    user_fields.insert("name".to_string(), FieldType::String);
    
    // Create reference with cascade delete
    let mut ref_type = ReferenceType::new("profiles".to_string());
    ref_type.on_delete = CascadeBehavior::Cascade;
    user_fields.insert("profile".to_string(), FieldType::Reference(ref_type));
    
    let users_schema = Schema::new("users".to_string(), user_fields);
    registry.register(users_schema).unwrap();
    
    // Valid: reference with cascade behavior
    let query = r#"users.where { .profile.bio == "Developer" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_ok(), "Should accept reference with cascade behavior");
}

#[test]
fn test_error_message_with_field_suggestions() {
    let registry = create_test_schema_registry();
    
    // Typo in field name: "nam" instead of "name"
    let query = r#"users.where { .nam == "Alice" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject query with typo");
    
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("Did you mean"), "Error should suggest similar fields");
    assert!(err_msg.contains("name"), "Error should suggest 'name' field");
}

#[test]
fn test_error_message_with_type_mismatch() {
    let registry = create_test_schema_registry();
    
    // Type mismatch: comparing integer with string
    let query = r#"users.where { .age == "twenty-five" }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject type mismatch");
    
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("Type mismatch") || err_msg.contains("expected"), 
            "Error should mention type mismatch");
    assert!(err_msg.contains("Integer") || err_msg.contains("String"), 
            "Error should show the types involved");
}

#[test]
fn test_error_message_for_invalid_operator() {
    let registry = create_test_schema_registry();
    
    // Invalid operator for type
    let query = r#"users.where { .age contains 25 }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject invalid operator");
    
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("Operator") || err_msg.contains("not valid"), 
            "Error should mention operator validity");
    assert!(err_msg.contains("Integer") || err_msg.contains("type"), 
            "Error should mention the type");
}

#[test]
fn test_error_message_for_nonexistent_schema() {
    let registry = create_test_schema_registry();
    
    // Nonexistent schema
    let query = r#"products.where { .price > 100 }"#;
    let result = parse_oql_with_schema(query, registry.clone());
    assert!(result.is_err(), "Should reject nonexistent schema");
    
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("Schema") && err_msg.contains("not found"), 
            "Error should clearly state schema not found");
    assert!(err_msg.contains("products"), 
            "Error should mention the schema name");
}
