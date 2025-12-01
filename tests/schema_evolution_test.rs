use owl_db::document::{Document, Value};
use owl_db::schema::{Schema, FieldType, ReferenceType};
use owl_db::schema::evolution::{
    SchemaVersion, SchemaEvolution, FieldChange, CompatibilityLevel,
};
use owl_db::schema::migration::{
    Migration, MigrationPlan, MigrationScript, MigrationStep, FieldTransform,
    MigrationStatus,
};
use std::collections::HashMap;

// Helper function to create a basic schema
fn create_basic_schema(name: &str) -> Schema {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), FieldType::String);
    fields.insert("name".to_string(), FieldType::String);
    Schema::new(name.to_string(), fields)
}

// Helper function to create a document with initial fields
fn create_test_document(schema: &str, initial_fields: HashMap<String, Value>) -> Document {
    Document::new(schema.to_string(), initial_fields)
}

#[test]
fn test_schema_version_creation() {
    let schema = create_basic_schema("users");
    let version = SchemaVersion::initial(schema.clone());
    
    assert_eq!(version.version, 1);
    assert_eq!(version.schema_name, "users");
    assert_eq!(version.description, "Initial schema version");
    assert!(version.changes.is_empty());
}

#[test]
fn test_schema_version_next() {
    let schema1 = create_basic_schema("users");
    let version1 = SchemaVersion::initial(schema1);
    
    let mut schema2 = create_basic_schema("users");
    schema2.fields.insert("email".to_string(), FieldType::String);
    
    let changes = vec![FieldChange::AddField {
        field_name: "email".to_string(),
        field_type: FieldType::String,
        default_value: None,
    }];
    
    let version2 = version1.next(schema2, "Added email field".to_string(), changes.clone());
    
    assert_eq!(version2.version, 2);
    assert_eq!(version2.changes.len(), 1);
}

#[test]
fn test_detect_add_field() {
    let mut old_fields = HashMap::new();
    old_fields.insert("name".to_string(), FieldType::String);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("name".to_string(), FieldType::String);
    new_fields.insert("email".to_string(), FieldType::String);
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let changes = evolution.detect_changes(&old_schema, &new_schema);

    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FieldChange::AddField { field_name, field_type, .. } => {
            assert_eq!(field_name, "email");
            assert_eq!(field_type, &FieldType::String);
        }
        _ => panic!("Expected AddField change"),
    }
}

#[test]
fn test_detect_remove_field() {
    let mut old_fields = HashMap::new();
    old_fields.insert("name".to_string(), FieldType::String);
    old_fields.insert("email".to_string(), FieldType::String);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("name".to_string(), FieldType::String);
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let changes = evolution.detect_changes(&old_schema, &new_schema);

    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FieldChange::RemoveField { field_name } => {
            assert_eq!(field_name, "email");
        }
        _ => panic!("Expected RemoveField change"),
    }
}

#[test]
fn test_detect_type_change() {
    let mut old_fields = HashMap::new();
    old_fields.insert("age".to_string(), FieldType::Integer);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("age".to_string(), FieldType::String);
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let changes = evolution.detect_changes(&old_schema, &new_schema);

    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FieldChange::ChangeType { field_name, old_type, new_type } => {
            assert_eq!(field_name, "age");
            assert_eq!(old_type, &FieldType::Integer);
            assert_eq!(new_type, &FieldType::String);
        }
        _ => panic!("Expected ChangeType change"),
    }
}

#[test]
fn test_detect_make_optional() {
    let mut old_fields = HashMap::new();
    old_fields.insert("email".to_string(), FieldType::String);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let changes = evolution.detect_changes(&old_schema, &new_schema);

    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FieldChange::MakeOptional { field_name } => {
            assert_eq!(field_name, "email");
        }
        _ => panic!("Expected MakeOptional change"),
    }
}

#[test]
fn test_detect_make_required() {
    let mut old_fields = HashMap::new();
    old_fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("email".to_string(), FieldType::String);
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let changes = evolution.detect_changes(&old_schema, &new_schema);

    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FieldChange::MakeRequired { field_name, .. } => {
            assert_eq!(field_name, "email");
        }
        _ => panic!("Expected MakeRequired change"),
    }
}

#[test]
fn test_compatibility_add_optional_field() {
    let mut old_fields = HashMap::new();
    old_fields.insert("name".to_string(), FieldType::String);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("name".to_string(), FieldType::String);
    new_fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let compat = evolution.check_compatibility(&old_schema, &new_schema);

    assert_eq!(compat.level, CompatibilityLevel::BackwardCompatible);
    assert!(compat.is_safe());
    assert!(compat.migration_required);
}

#[test]
fn test_compatibility_add_required_field() {
    let mut old_fields = HashMap::new();
    old_fields.insert("name".to_string(), FieldType::String);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("name".to_string(), FieldType::String);
    new_fields.insert("email".to_string(), FieldType::String);
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let compat = evolution.check_compatibility(&old_schema, &new_schema);

    assert_eq!(compat.level, CompatibilityLevel::Breaking);
    assert!(!compat.is_safe());
    assert!(compat.migration_required);
    assert!(!compat.issues.is_empty());
}

#[test]
fn test_compatibility_remove_field() {
    let mut old_fields = HashMap::new();
    old_fields.insert("name".to_string(), FieldType::String);
    old_fields.insert("email".to_string(), FieldType::String);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("name".to_string(), FieldType::String);
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let compat = evolution.check_compatibility(&old_schema, &new_schema);

    assert_eq!(compat.level, CompatibilityLevel::Breaking);
    assert!(!compat.is_safe());
}

#[test]
fn test_compatibility_make_optional() {
    let mut old_fields = HashMap::new();
    old_fields.insert("email".to_string(), FieldType::String);
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let compat = evolution.check_compatibility(&old_schema, &new_schema);

    assert_eq!(compat.level, CompatibilityLevel::BackwardCompatible);
    assert!(compat.is_safe());
}

#[test]
fn test_compatibility_make_required() {
    let mut old_fields = HashMap::new();
    old_fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
    let old_schema = Schema::new("users".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("email".to_string(), FieldType::String);
    let new_schema = Schema::new("users".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let compat = evolution.check_compatibility(&old_schema, &new_schema);

    assert_eq!(compat.level, CompatibilityLevel::Breaking);
    assert!(!compat.is_safe());
}

#[test]
fn test_schema_evolution_register_version() {
    let mut evolution = SchemaEvolution::new();
    
    let schema = create_basic_schema("users");
    let version = SchemaVersion::initial(schema);
    
    evolution.register_version(version.clone()).unwrap();
    
    let latest = evolution.get_latest("users");
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().version, 1);
}

#[test]
fn test_schema_evolution_multiple_versions() {
    let mut evolution = SchemaEvolution::new();
    
    let schema1 = create_basic_schema("users");
    let version1 = SchemaVersion::initial(schema1);
    evolution.register_version(version1.clone()).unwrap();
    
    let mut schema2 = create_basic_schema("users");
    schema2.fields.insert("email".to_string(), FieldType::String);
    let version2 = version1.next(schema2, "Added email".to_string(), vec![]);
    evolution.register_version(version2).unwrap();
    
    let latest = evolution.get_latest("users");
    assert_eq!(latest.unwrap().version, 2);
    
    let all_versions = evolution.get_all_versions("users");
    assert_eq!(all_versions.len(), 2);
}

#[test]
fn test_migration_script_add_field() {
    let mut script = MigrationScript::new(
        "users".to_string(),
        1,
        2,
        "Add email field".to_string(),
    );

    script.add_step(MigrationStep::AddField {
        field_name: "email".to_string(),
        default_value: Value::String("unknown@example.com".to_string()),
    });

    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    let mut doc = create_test_document("user1", fields);

    script.apply_to_document(&mut doc).unwrap();

    assert_eq!(doc.fields.len(), 2);
    assert_eq!(
        doc.fields.get("email"),
        Some(&Value::String("unknown@example.com".to_string()))
    );
}

#[test]
fn test_migration_script_remove_field() {
    let mut script = MigrationScript::new(
        "users".to_string(),
        1,
        2,
        "Remove old field".to_string(),
    );

    script.add_step(MigrationStep::RemoveField {
        field_name: "old_field".to_string(),
    });

    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    fields.insert("old_field".to_string(), Value::String("deprecated".to_string()));
    let mut doc = create_test_document("user1", fields);

    script.apply_to_document(&mut doc).unwrap();

    assert_eq!(doc.fields.len(), 1);
    assert!(!doc.fields.contains_key("old_field"));
}

#[test]
fn test_migration_script_rename_field() {
    let mut script = MigrationScript::new(
        "users".to_string(),
        1,
        2,
        "Rename field".to_string(),
    );

    script.add_step(MigrationStep::RenameField {
        old_name: "email".to_string(),
        new_name: "email_address".to_string(),
    });

    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    let mut doc = create_test_document("user1", fields);

    script.apply_to_document(&mut doc).unwrap();

    assert!(!doc.fields.contains_key("email"));
    assert_eq!(
        doc.fields.get("email_address"),
        Some(&Value::String("alice@example.com".to_string()))
    );
}

#[test]
fn test_migration_transform_to_string() {
    let mut script = MigrationScript::new(
        "users".to_string(),
        1,
        2,
        "Transform field".to_string(),
    );

    script.add_step(MigrationStep::TransformField {
        field_name: "age".to_string(),
        transform: FieldTransform::ToString,
    });

    let mut fields = HashMap::new();
    fields.insert("age".to_string(), Value::Integer(30));
    let mut doc = create_test_document("user1", fields);

    script.apply_to_document(&mut doc).unwrap();

    assert_eq!(doc.fields.get("age"), Some(&Value::String("30".to_string())));
}

#[test]
fn test_migration_transform_to_integer() {
    let mut script = MigrationScript::new(
        "users".to_string(),
        1,
        2,
        "Transform field".to_string(),
    );

    script.add_step(MigrationStep::TransformField {
        field_name: "age".to_string(),
        transform: FieldTransform::ToInteger,
    });

    let mut fields = HashMap::new();
    fields.insert("age".to_string(), Value::String("30".to_string()));
    let mut doc = create_test_document("user1", fields);

    script.apply_to_document(&mut doc).unwrap();

    assert_eq!(doc.fields.get("age"), Some(&Value::Integer(30)));
}

#[test]
fn test_migration_transform_to_float() {
    let mut script = MigrationScript::new(
        "users".to_string(),
        1,
        2,
        "Transform field".to_string(),
    );

    script.add_step(MigrationStep::TransformField {
        field_name: "price".to_string(),
        transform: FieldTransform::ToFloat,
    });

    let mut fields = HashMap::new();
    fields.insert("price".to_string(), Value::Integer(100));
    let mut doc = create_test_document("product1", fields);

    script.apply_to_document(&mut doc).unwrap();

    assert_eq!(doc.fields.get("price"), Some(&Value::Float(100.0)));
}

#[test]
fn test_migration_transform_to_boolean() {
    let mut script = MigrationScript::new(
        "users".to_string(),
        1,
        2,
        "Transform field".to_string(),
    );

    script.add_step(MigrationStep::TransformField {
        field_name: "active".to_string(),
        transform: FieldTransform::ToBoolean,
    });

    let mut fields = HashMap::new();
    fields.insert("active".to_string(), Value::String("true".to_string()));
    let mut doc = create_test_document("user1", fields);

    script.apply_to_document(&mut doc).unwrap();

    assert_eq!(doc.fields.get("active"), Some(&Value::Boolean(true)));
}

#[test]
fn test_migration_plan_multiple_steps() {
    let mut plan = MigrationPlan::new("users".to_string(), 1, 3);

    let mut script1 = MigrationScript::new("users".to_string(), 1, 2, "Step 1".to_string());
    script1.add_step(MigrationStep::AddField {
        field_name: "email".to_string(),
        default_value: Value::Null,
    });

    let mut script2 = MigrationScript::new("users".to_string(), 2, 3, "Step 2".to_string());
    script2.add_step(MigrationStep::AddField {
        field_name: "phone".to_string(),
        default_value: Value::Null,
    });

    plan.add_script(script1);
    plan.add_script(script2);

    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    let mut doc = create_test_document("user1", fields);

    plan.apply_to_document(&mut doc).unwrap();

    assert_eq!(doc.fields.len(), 3);
    assert!(doc.fields.contains_key("email"));
    assert!(doc.fields.contains_key("phone"));
}

#[test]
fn test_migration_status_lifecycle() {
    let mut migration = Migration::new("users".to_string(), 1, 2);
    assert_eq!(migration.status, MigrationStatus::Pending);

    migration.start();
    assert!(matches!(migration.status, MigrationStatus::InProgress { .. }));

    migration.update_progress(0.5);
    if let MigrationStatus::InProgress { progress, .. } = migration.status {
        assert_eq!(progress, 0.5);
    } else {
        panic!("Expected InProgress status");
    }

    migration.complete(100);
    if let MigrationStatus::Completed { documents_migrated, .. } = migration.status {
        assert_eq!(documents_migrated, 100);
    } else {
        panic!("Expected Completed status");
    }
}

#[test]
fn test_migration_status_failure() {
    let mut migration = Migration::new("users".to_string(), 1, 2);
    migration.start();
    migration.fail("Test error".to_string());

    if let MigrationStatus::Failed { error, .. } = migration.status {
        assert_eq!(error, "Test error");
    } else {
        panic!("Expected Failed status");
    }
}

#[test]
fn test_field_change_description() {
    let change = FieldChange::AddField {
        field_name: "email".to_string(),
        field_type: FieldType::String,
        default_value: None,
    };
    assert!(change.description().contains("Add field 'email'"));

    let change = FieldChange::RemoveField {
        field_name: "old_field".to_string(),
    };
    assert!(change.description().contains("Remove field 'old_field'"));

    let change = FieldChange::RenameField {
        old_name: "email".to_string(),
        new_name: "email_address".to_string(),
    };
    assert!(change.description().contains("Rename field 'email' to 'email_address'"));
}

#[test]
fn test_reference_target_change_detection() {
    let mut old_fields = HashMap::new();
    old_fields.insert("customer".to_string(), 
        FieldType::Reference(ReferenceType::new("customers".to_string())));
    let old_schema = Schema::new("orders".to_string(), old_fields);

    let mut new_fields = HashMap::new();
    new_fields.insert("customer".to_string(), 
        FieldType::Reference(ReferenceType::new("users".to_string())));
    let new_schema = Schema::new("orders".to_string(), new_fields);

    let evolution = SchemaEvolution::new();
    let changes = evolution.detect_changes(&old_schema, &new_schema);

    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FieldChange::ChangeReferenceTarget { field_name, old_target, new_target } => {
            assert_eq!(field_name, "customer");
            assert_eq!(old_target, "customers");
            assert_eq!(new_target, "users");
        }
        _ => panic!("Expected ChangeReferenceTarget change"),
    }
}

#[test]
fn test_validate_evolution_version_order() {
    let evolution = SchemaEvolution::new();
    
    let schema1 = create_basic_schema("users");
    let version1 = SchemaVersion::new(schema1, 1, "V1".to_string());
    
    let schema2 = create_basic_schema("users");
    let version2 = SchemaVersion::new(schema2, 1, "V2".to_string()); // Same version number
    
    let result = evolution.validate_evolution(&version1, &version2);
    assert!(result.is_err());
}

#[test]
fn test_validate_evolution_schema_name_mismatch() {
    let evolution = SchemaEvolution::new();
    
    let schema1 = create_basic_schema("users");
    let version1 = SchemaVersion::new(schema1, 1, "V1".to_string());
    
    let schema2 = create_basic_schema("orders");
    let version2 = SchemaVersion::new(schema2, 2, "V2".to_string());
    
    let result = evolution.validate_evolution(&version1, &version2);
    assert!(result.is_err());
}
