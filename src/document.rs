use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::distributed::GlobalObjRef;
pub use crate::storage_engine::ObjRef;

/// Unique identifier for documents
pub type DocumentId = String;

/// Core document structure for the OQL database
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Document {
    pub id: DocumentId,
    pub schema: String,
    pub fields: HashMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: u64, // Version number for optimistic locking
}

impl Document {
    /// Create a new document with the given schema and fields
    pub fn new(schema: String, fields: HashMap<String, Value>) -> Self {
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();

        Self {
            id,
            schema,
            fields,
            created_at: now,
            updated_at: now,
            version: 1, // Start with version 1
        }
    }

    /// Create a document with a specific ID
    pub fn with_id(id: DocumentId, schema: String, fields: HashMap<String, Value>) -> Self {
        let now = Utc::now();

        Self {
            id,
            schema,
            fields,
            created_at: now,
            updated_at: now,
            version: 1, // Start with version 1
        }
    }

    /// Update the document's fields and timestamp
    pub fn update_fields(&mut self, new_fields: HashMap<String, Value>) {
        self.fields.extend(new_fields);
        self.updated_at = Utc::now();
        self.version += 1; // Increment version on update
    }

    /// Get a field value by name
    pub fn get_field(&self, field_name: &str) -> Option<&Value> {
        self.fields.get(field_name)
    }

    /// Set a field value
    pub fn set_field(&mut self, field_name: String, value: Value) {
        self.fields.insert(field_name, value);
        self.updated_at = Utc::now();
        self.version += 1; // Increment version on update
    }

    /// Increment the document version
    pub fn increment_version(&mut self) {
        self.version += 1;
        self.updated_at = Utc::now();
    }

    /// Create a new version of this document with updated fields
    pub fn new_version_with_fields(&self, new_fields: HashMap<String, Value>) -> Self {
        let mut updated_fields = self.fields.clone();
        updated_fields.extend(new_fields);

        Self {
            id: self.id.clone(),
            schema: self.schema.clone(),
            fields: updated_fields,
            created_at: self.created_at,
            updated_at: Utc::now(),
            version: self.version + 1,
        }
    }

    /// Preserve document identity and metadata while replacing the visible
    /// field set for a query projection.
    pub fn with_projected_fields(&self, fields: HashMap<String, Value>) -> Self {
        Self {
            id: self.id.clone(),
            schema: self.schema.clone(),
            fields,
            created_at: self.created_at,
            updated_at: self.updated_at,
            version: self.version,
        }
    }
}

/// Value types supported in OQL documents
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Reference(ObjRef),             // Physical pointer to referenced document
    GlobalReference(GlobalObjRef), // Shard-aware physical pointer for distributed traversal
    List(Vec<Value>),
    Object(HashMap<String, Value>), // Nested objects for schema evolution
    Null,
}

impl Value {
    /// Create a reference from a string ID (for backward compatibility)
    pub fn reference_from_string(s: String) -> Self {
        if let Some(global_ref) = GlobalObjRef::from_string(&s) {
            return Value::GlobalReference(global_ref);
        }
        if let Some(objref) = ObjRef::from_string(&s) {
            Value::Reference(objref)
        } else {
            Value::Reference(ObjRef::null())
        }
    }

    /// Get the string representation of a reference (for backward compatibility)
    pub fn reference_to_string(&self) -> Option<String> {
        match self {
            Value::Reference(objref) => Some(objref.to_string_id()),
            Value::GlobalReference(objref) => Some(objref.to_string_id()),
            _ => None,
        }
    }
}

impl Value {
    /// Get the type name of this value
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "String",
            Value::Integer(_) => "Integer",
            Value::Float(_) => "Float",
            Value::Boolean(_) => "Boolean",
            Value::DateTime(_) => "DateTime",
            Value::Reference(_) => "Reference",
            Value::GlobalReference(_) => "Reference",
            Value::List(_) => "List",
            Value::Object(_) => "Object",
            Value::Null => "Null",
        }
    }

    /// Convert value to string representation
    pub fn to_string_repr(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Boolean(b) => b.to_string(),
            Value::DateTime(dt) => dt.to_rfc3339(),
            Value::Reference(objref) => objref.to_string_id(),
            Value::GlobalReference(objref) => objref.to_string_id(),
            Value::List(list) => format!(
                "[{}]",
                list.iter()
                    .map(|v| v.to_string_repr())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Value::Object(obj) => {
                let pairs: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v.to_string_repr()))
                    .collect();
                format!("{{{}}}", pairs.join(", "))
            }
            Value::Null => "null".to_string(),
        }
    }

    /// Check if this value is null
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Extract integer value if this is an Integer
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Extract string value if this is a String
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Extract float value if this is a Float
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Extract boolean value if this is a Boolean
    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Extract object value if this is an Object
    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Value::Object(obj) => Some(obj),
            _ => None,
        }
    }

    /// Extract mutable object value if this is an Object
    pub fn as_object_mut(&mut self) -> Option<&mut HashMap<String, Value>> {
        match self {
            Value::Object(obj) => Some(obj),
            _ => None,
        }
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Integer(i)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Boolean(b)
    }
}

impl From<DateTime<Utc>> for Value {
    fn from(dt: DateTime<Utc>) -> Self {
        Value::DateTime(dt)
    }
}

impl From<HashMap<String, Value>> for Value {
    fn from(obj: HashMap<String, Value>) -> Self {
        Value::Object(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_creation() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String("Alice".to_string()));
        fields.insert("age".to_string(), Value::Integer(30));

        let doc = Document::new("users".to_string(), fields);

        assert_eq!(doc.schema, "users");
        assert_eq!(
            doc.get_field("name"),
            Some(&Value::String("Alice".to_string()))
        );
        assert_eq!(doc.get_field("age"), Some(&Value::Integer(30)));
        assert!(!doc.id.is_empty());
    }

    #[test]
    fn test_document_update() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String("Alice".to_string()));

        let mut doc = Document::new("users".to_string(), fields);
        let original_updated_at = doc.updated_at;

        // Small delay to ensure timestamp difference (removed for simulation compatibility)

        doc.set_field("age".to_string(), Value::Integer(30));

        assert_eq!(doc.get_field("age"), Some(&Value::Integer(30)));
        assert!(doc.updated_at > original_updated_at);
    }

    #[test]
    fn test_value_type_names() {
        assert_eq!(Value::String("test".to_string()).type_name(), "String");
        assert_eq!(Value::Integer(42).type_name(), "Integer");
        assert_eq!(Value::Float(3.14).type_name(), "Float");
        assert_eq!(Value::Boolean(true).type_name(), "Boolean");
        assert_eq!(Value::Null.type_name(), "Null");
    }
}

impl Value {
    /// Convert Value to clean JSON without type wrappers
    pub fn to_clean_json(&self) -> serde_json::Value {
        match self {
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Integer(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Boolean(b) => serde_json::Value::Bool(*b),
            Value::Null => serde_json::Value::Null,
            Value::List(items) => {
                let json_items: Vec<serde_json::Value> =
                    items.iter().map(|item| item.to_clean_json()).collect();
                serde_json::Value::Array(json_items)
            }
            Value::DateTime(dt) => serde_json::Value::String(dt.to_rfc3339()),
            Value::Reference(objref) => {
                // For references, show the reference ID
                serde_json::Value::String(objref.to_string_id())
            }
            Value::GlobalReference(objref) => serde_json::Value::String(objref.to_string_id()),
            Value::Object(obj) => {
                let mut json_obj = serde_json::Map::new();
                for (key, value) in obj {
                    json_obj.insert(key.clone(), value.to_clean_json());
                }
                serde_json::Value::Object(json_obj)
            }
        }
    }
}

impl Document {
    /// Convert Document fields to clean JSON without type wrappers
    pub fn to_clean_json(&self) -> serde_json::Value {
        let mut json_obj = serde_json::Map::new();

        // Add document ID
        json_obj.insert(
            "_id".to_string(),
            serde_json::Value::String(self.id.clone()),
        );

        // Add all fields
        for (key, value) in &self.fields {
            json_obj.insert(key.clone(), value.to_clean_json());
        }

        serde_json::Value::Object(json_obj)
    }

    /// Convert Document fields to clean JSON without document ID
    pub fn fields_to_clean_json(&self) -> serde_json::Value {
        let mut json_obj = serde_json::Map::new();

        for (key, value) in &self.fields {
            json_obj.insert(key.clone(), value.to_clean_json());
        }

        serde_json::Value::Object(json_obj)
    }
}
