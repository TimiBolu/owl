use crate::document::{Value, ObjRef};
use crate::error::{OQLError, OQLResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// Schema evolution and migration submodules
pub mod evolution;
pub mod migration;

/// Schema definition for document collections
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    pub name: String,
    pub fields: HashMap<String, FieldType>,
}

impl Schema {
    /// Create a new schema
    pub fn new(name: String, fields: HashMap<String, FieldType>) -> Self {
        Self { name, fields }
    }
    
    /// Validate a document's fields against this schema
    pub fn validate_document(&self, fields: &HashMap<String, Value>) -> OQLResult<()> {
        // Check that all required fields are present and have correct types
        for (field_name, field_type) in &self.fields {
            if let Some(value) = fields.get(field_name) {
                self.validate_field_value(field_name, field_type, value)?;
            } else if !field_type.is_optional() {
                return Err(OQLError::SchemaValidation {
                    message: format!("Required field '{}' is missing", field_name),
                });
            }
        }
        
        // Check for unexpected fields
        for field_name in fields.keys() {
            if !self.fields.contains_key(field_name) {
                return Err(OQLError::SchemaValidation {
                    message: format!("Unexpected field '{}' not in schema", field_name),
                });
            }
        }
        
        Ok(())
    }
    
    /// Validate a single field value against its type
    fn validate_field_value(&self, field_name: &str, field_type: &FieldType, value: &Value) -> OQLResult<()> {
        match (field_type, value) {
            (FieldType::String, Value::String(_)) => Ok(()),
            (FieldType::Integer, Value::Integer(_)) => Ok(()),
            (FieldType::Float, Value::Float(_)) => Ok(()),
            (FieldType::Boolean, Value::Boolean(_)) => Ok(()),
            (FieldType::DateTime, Value::DateTime(_)) => Ok(()),
            (FieldType::Reference(_ref_type), Value::Reference(objref)) => {
                // Validate reference is not null (basic validation)
                if objref.is_null() {
                    return Err(OQLError::SchemaValidation {
                        message: format!("Reference field '{}' cannot have empty ID", field_name),
                    });
                }
                Ok(())
            }
            (FieldType::Reference(ref_type), Value::Null) => {
                if ref_type.required {
                    Err(OQLError::SchemaValidation {
                        message: format!("Required reference field '{}' cannot be null", field_name),
                    })
                } else {
                    Ok(())
                }
            }
            (FieldType::List(inner_type), Value::List(values)) => {
                for (i, val) in values.iter().enumerate() {
                    self.validate_field_value(&format!("{}[{}]", field_name, i), inner_type, val)?;
                }
                Ok(())
            }
            (FieldType::Optional(inner_type), value) => {
                if value.is_null() {
                    Ok(())
                } else {
                    self.validate_field_value(field_name, inner_type, value)
                }
            }
            (_, Value::Null) if field_type.is_optional() => Ok(()),
            _ => Err(OQLError::TypeError {
                expected: field_type.type_name().to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }
    
    /// Get field type by name
    pub fn get_field_type(&self, field_name: &str) -> Option<&FieldType> {
        self.fields.get(field_name)
    }
    
    /// Get all reference fields in this schema
    pub fn get_reference_fields(&self) -> Vec<(&String, &ReferenceType)> {
        self.fields
            .iter()
            .filter_map(|(name, field_type)| {
                field_type.reference_type().map(|ref_type| (name, ref_type))
            })
            .collect()
    }
    
    /// Check if a field is a reference to a specific schema
    pub fn is_reference_to(&self, field_name: &str, target_schema: &str) -> bool {
        self.get_field_type(field_name)
            .and_then(|ft| ft.referenced_schema())
            .map(|schema| schema == target_schema)
            .unwrap_or(false)
    }
    
    /// Validate that all referenced schemas exist
    pub fn validate_references<F>(&self, schema_exists: F) -> OQLResult<()>
    where
        F: Fn(&str) -> bool,
    {
        for (field_name, ref_type) in self.get_reference_fields() {
            if !schema_exists(&ref_type.target_schema) {
                return Err(OQLError::SchemaValidation {
                    message: format!(
                        "Reference field '{}' points to non-existent schema '{}'",
                        field_name, ref_type.target_schema
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Field types supported in schemas
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FieldType {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Reference(ReferenceType), // Reference to another schema with metadata
    List(Box<FieldType>),
    Optional(Box<FieldType>),
}

/// Reference type with metadata for validation and navigation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReferenceType {
    /// The target schema name that this reference points to
    pub target_schema: String,
    /// Whether the reference is required to exist (referential integrity)
    pub required: bool,
    /// Optional cascade behavior for deletions
    pub on_delete: CascadeBehavior,
}

/// Cascade behavior for reference deletions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CascadeBehavior {
    /// Set reference to null when target is deleted
    SetNull,
    /// Prevent deletion if references exist
    Restrict,
    /// Delete referencing documents when target is deleted
    Cascade,
    /// No action (allow dangling references)
    NoAction,
}

impl ReferenceType {
    /// Create a new reference type with default settings
    pub fn new(target_schema: String) -> Self {
        Self {
            target_schema,
            required: true,
            on_delete: CascadeBehavior::Restrict,
        }
    }
    
    /// Create an optional reference (allows null values)
    pub fn optional(target_schema: String) -> Self {
        Self {
            target_schema,
            required: false,
            on_delete: CascadeBehavior::SetNull,
        }
    }
    
    /// Create a reference with custom cascade behavior
    pub fn with_cascade(target_schema: String, on_delete: CascadeBehavior) -> Self {
        Self {
            target_schema,
            required: true,
            on_delete,
        }
    }
}

impl FieldType {
    /// Get the type name as a string
    pub fn type_name(&self) -> &'static str {
        match self {
            FieldType::String => "String",
            FieldType::Integer => "Integer",
            FieldType::Float => "Float",
            FieldType::Boolean => "Boolean",
            FieldType::DateTime => "DateTime",
            FieldType::Reference(_) => "Reference",
            FieldType::List(_) => "List",
            FieldType::Optional(_) => "Optional",
        }
    }
    
    /// Check if this field type is optional
    pub fn is_optional(&self) -> bool {
        match self {
            FieldType::Optional(_) => true,
            FieldType::Reference(ref_type) => !ref_type.required,
            _ => false,
        }
    }
    
    /// Get the inner type for optional and list types
    pub fn inner_type(&self) -> Option<&FieldType> {
        match self {
            FieldType::Optional(inner) => Some(inner),
            FieldType::List(inner) => Some(inner),
            _ => None,
        }
    }
    
    /// Get the referenced schema name for reference types
    pub fn referenced_schema(&self) -> Option<&str> {
        match self {
            FieldType::Reference(ref_type) => Some(&ref_type.target_schema),
            FieldType::Optional(inner) => inner.referenced_schema(),
            FieldType::List(inner) => inner.referenced_schema(),
            _ => None,
        }
    }
    
    /// Get the reference type metadata for reference fields
    pub fn reference_type(&self) -> Option<&ReferenceType> {
        match self {
            FieldType::Reference(ref_type) => Some(ref_type),
            FieldType::Optional(inner) => inner.reference_type(),
            FieldType::List(inner) => inner.reference_type(),
            _ => None,
        }
    }
}

/// Schema registry for managing multiple schemas
#[derive(Debug, Clone)]
pub struct SchemaRegistry {
    schemas: Arc<RwLock<HashMap<String, Schema>>>,
}

impl SchemaRegistry {
    /// Create a new empty schema registry
    pub fn new() -> Self {
        Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Register a schema
    pub fn register(&self, schema: Schema) -> OQLResult<()> {
        let mut schemas = self.schemas.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock on schema registry"))?;
        schemas.insert(schema.name.clone(), schema);
        Ok(())
    }
    
    /// Get a schema by name
    pub fn get(&self, name: &str) -> OQLResult<Option<Schema>> {
        let schemas = self.schemas.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock on schema registry"))?;
        Ok(schemas.get(name).cloned())
    }
    
    /// Check if a schema exists
    pub fn exists(&self, name: &str) -> bool {
        self.schemas.read()
            .map(|schemas| schemas.contains_key(name))
            .unwrap_or(false)
    }
    
    /// List all schema names
    pub fn list_schemas(&self) -> Vec<String> {
        self.schemas.read()
            .map(|schemas| schemas.keys().cloned().collect())
            .unwrap_or_default()
    }
    
    /// Validate that a field exists in a schema
    pub fn validate_field(&self, schema_name: &str, field_path: &[String]) -> OQLResult<()> {
        let schemas = self.schemas.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock on schema registry"))?;
        
        let schema = schemas.get(schema_name)
            .ok_or_else(|| OQLError::SchemaValidation {
                message: format!("Schema '{}' not found", schema_name),
            })?;
        
        // Validate the first field in the path
        if field_path.is_empty() {
            return Ok(());
        }
        
        let field_name = &field_path[0];
        let field_type = schema.get_field_type(field_name)
            .ok_or_else(|| OQLError::SchemaValidation {
                message: format!("Field '{}' not found in schema '{}'", field_name, schema_name),
            })?;
        
        // If there are more fields in the path, validate nested access
        if field_path.len() > 1 {
            // Check if this is a reference field
            if let Some(target_schema) = field_type.referenced_schema() {
                // Recursively validate the rest of the path in the target schema
                self.validate_field(target_schema, &field_path[1..])?;
            } else {
                return Err(OQLError::SchemaValidation {
                    message: format!(
                        "Field '{}' in schema '{}' is not a reference, cannot navigate to '{}'",
                        field_name, schema_name, field_path[1]
                    ),
                });
            }
        }
        
        Ok(())
    }
    
    /// Get the type of a field in a schema
    pub fn get_field_type(&self, schema_name: &str, field_name: &str) -> OQLResult<Option<FieldType>> {
        let schemas = self.schemas.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock on schema registry"))?;
        
        Ok(schemas.get(schema_name)
            .and_then(|schema| schema.get_field_type(field_name))
            .cloned())
    }
    
    /// Validate a reference target exists
    pub fn validate_reference(&self, source_schema: &str, field_name: &str, target_schema: &str) -> OQLResult<()> {
        let schemas = self.schemas.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock on schema registry"))?;
        
        // Check source schema exists
        let source = schemas.get(source_schema)
            .ok_or_else(|| OQLError::SchemaValidation {
                message: format!("Source schema '{}' not found", source_schema),
            })?;
        
        // Check field is a reference
        let field_type = source.get_field_type(field_name)
            .ok_or_else(|| OQLError::SchemaValidation {
                message: format!("Field '{}' not found in schema '{}'", field_name, source_schema),
            })?;
        
        let actual_target = field_type.referenced_schema()
            .ok_or_else(|| OQLError::SchemaValidation {
                message: format!("Field '{}' in schema '{}' is not a reference", field_name, source_schema),
            })?;
        
        // Check target schema matches
        if actual_target != target_schema {
            return Err(OQLError::SchemaValidation {
                message: format!(
                    "Reference field '{}' in schema '{}' points to '{}', not '{}'",
                    field_name, source_schema, actual_target, target_schema
                ),
            });
        }
        
        // Check target schema exists
        if !schemas.contains_key(target_schema) {
            return Err(OQLError::SchemaValidation {
                message: format!("Target schema '{}' not found", target_schema),
            });
        }
        
        Ok(())
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    
    #[test]
    fn test_schema_validation_success() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("age".to_string(), FieldType::Integer);
        fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
        
        let schema = Schema::new("users".to_string(), fields);
        
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Alice".to_string()));
        doc_fields.insert("age".to_string(), Value::Integer(30));
        
        assert!(schema.validate_document(&doc_fields).is_ok());
    }
    
    #[test]
    fn test_schema_validation_missing_required_field() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("age".to_string(), FieldType::Integer);
        
        let schema = Schema::new("users".to_string(), fields);
        
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Alice".to_string()));
        // Missing required 'age' field
        
        let result = schema.validate_document(&doc_fields);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Required field 'age' is missing"));
    }
    
    #[test]
    fn test_schema_validation_type_mismatch() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("age".to_string(), FieldType::Integer);
        
        let schema = Schema::new("users".to_string(), fields);
        
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Alice".to_string()));
        doc_fields.insert("age".to_string(), Value::String("thirty".to_string())); // Wrong type
        
        let result = schema.validate_document(&doc_fields);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type error"));
    }
    
    #[test]
    fn test_schema_validation_optional_field() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
        
        let schema = Schema::new("users".to_string(), fields);
        
        // Test with optional field present
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Alice".to_string()));
        doc_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
        assert!(schema.validate_document(&doc_fields).is_ok());
        
        // Test with optional field missing
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Alice".to_string()));
        assert!(schema.validate_document(&doc_fields).is_ok());
        
        // Test with optional field as null
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Alice".to_string()));
        doc_fields.insert("email".to_string(), Value::Null);
        assert!(schema.validate_document(&doc_fields).is_ok());
    }
    
    #[test]
    fn test_field_type_properties() {
        assert_eq!(FieldType::String.type_name(), "String");
        assert!(!FieldType::String.is_optional());
        
        let optional_string = FieldType::Optional(Box::new(FieldType::String));
        assert!(optional_string.is_optional());
        assert_eq!(optional_string.inner_type(), Some(&FieldType::String));
        
        let reference = FieldType::Reference(ReferenceType::new("users".to_string()));
        assert_eq!(reference.referenced_schema(), Some("users"));
        assert!(reference.reference_type().unwrap().required);
        
        let optional_reference = FieldType::Reference(ReferenceType::optional("customers".to_string()));
        assert!(!optional_reference.reference_type().unwrap().required);
    }
    
    #[test]
    fn test_reference_validation() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
        fields.insert("optional_ref".to_string(), FieldType::Reference(ReferenceType::optional("orders".to_string())));
        
        let schema = Schema::new("orders".to_string(), fields);
        
        // Test valid reference
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Order 1".to_string()));
        doc_fields.insert("customer".to_string(), Value::reference_from_string("cust_123".to_string()));
        assert!(schema.validate_document(&doc_fields).is_ok());
        
        // Test null required reference (should fail)
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Order 1".to_string()));
        doc_fields.insert("customer".to_string(), Value::Null);
        assert!(schema.validate_document(&doc_fields).is_err());
        
        // Test null optional reference (should pass)
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Order 1".to_string()));
        doc_fields.insert("customer".to_string(), Value::reference_from_string("cust_123".to_string()));
        doc_fields.insert("optional_ref".to_string(), Value::Null);
        assert!(schema.validate_document(&doc_fields).is_ok());
        
        // Test empty reference ID (should fail)
        let mut doc_fields = HashMap::new();
        doc_fields.insert("name".to_string(), Value::String("Order 1".to_string()));
        doc_fields.insert("customer".to_string(), Value::reference_from_string("".to_string()));
        assert!(schema.validate_document(&doc_fields).is_err());
    }
    
    #[test]
    fn test_schema_reference_methods() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
        fields.insert("account".to_string(), FieldType::Reference(ReferenceType::optional("accounts".to_string())));
        
        let schema = Schema::new("orders".to_string(), fields);
        
        // Test get_reference_fields
        let ref_fields = schema.get_reference_fields();
        assert_eq!(ref_fields.len(), 2);
        
        // Test is_reference_to
        assert!(schema.is_reference_to("customer", "customers"));
        assert!(schema.is_reference_to("account", "accounts"));
        assert!(!schema.is_reference_to("name", "customers"));
        assert!(!schema.is_reference_to("customer", "accounts"));
        
        // Test validate_references
        let schema_exists = |name: &str| matches!(name, "customers" | "accounts");
        assert!(schema.validate_references(schema_exists).is_ok());
        
        let schema_exists_partial = |name: &str| name == "customers";
        assert!(schema.validate_references(schema_exists_partial).is_err());
    }
    
    #[test]
    fn test_schema_registry_basic() {
        let registry = SchemaRegistry::new();
        
        // Test empty registry
        assert!(!registry.exists("users"));
        assert_eq!(registry.list_schemas().len(), 0);
        
        // Register a schema
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("age".to_string(), FieldType::Integer);
        let schema = Schema::new("users".to_string(), fields);
        
        assert!(registry.register(schema).is_ok());
        assert!(registry.exists("users"));
        assert_eq!(registry.list_schemas().len(), 1);
        
        // Get schema
        let retrieved = registry.get("users").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "users");
    }
    
    #[test]
    fn test_schema_registry_field_validation() {
        let registry = SchemaRegistry::new();
        
        // Create users schema
        let mut user_fields = HashMap::new();
        user_fields.insert("name".to_string(), FieldType::String);
        user_fields.insert("email".to_string(), FieldType::String);
        let users_schema = Schema::new("users".to_string(), user_fields);
        registry.register(users_schema).unwrap();
        
        // Test valid field
        assert!(registry.validate_field("users", &vec!["name".to_string()]).is_ok());
        assert!(registry.validate_field("users", &vec!["email".to_string()]).is_ok());
        
        // Test invalid field
        assert!(registry.validate_field("users", &vec!["invalid".to_string()]).is_err());
        
        // Test invalid schema
        assert!(registry.validate_field("nonexistent", &vec!["name".to_string()]).is_err());
    }
    
    #[test]
    fn test_schema_registry_reference_validation() {
        let registry = SchemaRegistry::new();
        
        // Create customers schema
        let mut customer_fields = HashMap::new();
        customer_fields.insert("name".to_string(), FieldType::String);
        let customers_schema = Schema::new("customers".to_string(), customer_fields);
        registry.register(customers_schema).unwrap();
        
        // Create orders schema with reference to customers
        let mut order_fields = HashMap::new();
        order_fields.insert("order_id".to_string(), FieldType::String);
        order_fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
        let orders_schema = Schema::new("orders".to_string(), order_fields);
        registry.register(orders_schema).unwrap();
        
        // Test valid reference
        assert!(registry.validate_reference("orders", "customer", "customers").is_ok());
        
        // Test invalid reference target
        assert!(registry.validate_reference("orders", "customer", "users").is_err());
        
        // Test non-reference field
        assert!(registry.validate_reference("orders", "order_id", "customers").is_err());
        
        // Test nonexistent field
        assert!(registry.validate_reference("orders", "invalid", "customers").is_err());
    }
    
    #[test]
    fn test_schema_registry_nested_field_validation() {
        let registry = SchemaRegistry::new();
        
        // Create profile schema
        let mut profile_fields = HashMap::new();
        profile_fields.insert("bio".to_string(), FieldType::String);
        profile_fields.insert("avatar".to_string(), FieldType::String);
        let profile_schema = Schema::new("profiles".to_string(), profile_fields);
        registry.register(profile_schema).unwrap();
        
        // Create users schema with reference to profiles
        let mut user_fields = HashMap::new();
        user_fields.insert("name".to_string(), FieldType::String);
        user_fields.insert("profile".to_string(), FieldType::Reference(ReferenceType::new("profiles".to_string())));
        let users_schema = Schema::new("users".to_string(), user_fields);
        registry.register(users_schema).unwrap();
        
        // Test nested field access through reference
        assert!(registry.validate_field("users", &vec!["profile".to_string(), "bio".to_string()]).is_ok());
        assert!(registry.validate_field("users", &vec!["profile".to_string(), "avatar".to_string()]).is_ok());
        
        // Test invalid nested field
        assert!(registry.validate_field("users", &vec!["profile".to_string(), "invalid".to_string()]).is_err());
        
        // Test nested access on non-reference field
        assert!(registry.validate_field("users", &vec!["name".to_string(), "something".to_string()]).is_err());
    }
}