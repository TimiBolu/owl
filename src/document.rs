use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// Import ObjRef from the Zig storage bindings
#[cfg(feature = "zig-storage")]
use owl_storage_zig::ObjRef;

// Add helper methods for zig-storage ObjRef
#[cfg(feature = "zig-storage")]
impl ObjRef {
    pub fn from_string(s: &str) -> Option<Self> {
        // Parse string reference ID to ObjRef
        // For backward compatibility, we'll use a simple hash
        if s.is_empty() {
            return Some(Self::null());
        }
        // Use a simple hash of the string as the offset
        let hash = s.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        Some(Self::new(hash))
    }
    
    pub fn to_string_id(&self) -> String {
        if self.is_null() {
            String::new()
        } else {
            format!("ref_{:016x}", self.offset())
        }
    }
}

// For backward compatibility when zig-storage is not enabled
#[cfg(not(feature = "zig-storage"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ObjRef(u64);

#[cfg(not(feature = "zig-storage"))]
impl ObjRef {
    pub const fn new(offset: u64) -> Self {
        Self(offset)
    }
    
    pub const fn null() -> Self {
        Self(0)
    }
    
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
    
    pub const fn offset(self) -> u64 {
        self.0
    }
    
    pub const fn page_id(self) -> u32 {
        (self.0 / 4096) as u32
    }
    
    pub const fn slot_id(self) -> u16 {
        ((self.0 % 4096) / 64) as u16
    }
    
    pub const fn from_page_slot(page_id: u32, slot_id: u16) -> Self {
        let offset = (page_id as u64 * 4096) + (slot_id as u64 * 64);
        Self(offset)
    }
    
    pub fn from_string(s: &str) -> Option<Self> {
        // Parse string reference ID to ObjRef
        // For backward compatibility, we'll use a simple hash
        if s.is_empty() {
            return Some(Self::null());
        }
        // Use a simple hash of the string as the offset
        let hash = s.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        Some(Self::new(hash))
    }
    
    pub fn to_string_id(&self) -> String {
        if self.is_null() {
            String::new()
        } else {
            format!("ref_{:016x}", self.0)
        }
    }
}

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
}

/// Value types supported in OQL documents
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Reference(ObjRef), // Physical pointer to referenced document
    List(Vec<Value>),
    Object(HashMap<String, Value>), // Nested objects for schema evolution
    Null,
}

impl Value {
    /// Create a reference from a string ID (for backward compatibility)
    pub fn reference_from_string(s: String) -> Self {
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
            Value::List(list) => format!("[{}]", list.iter().map(|v| v.to_string_repr()).collect::<Vec<_>>().join(", ")),
            Value::Object(obj) => {
                let pairs: Vec<String> = obj.iter()
                    .map(|(k, v)| format!("{}: {}", k, v.to_string_repr()))
                    .collect();
                format!("{{{}}}", pairs.join(", "))
            },
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
        assert_eq!(doc.get_field("name"), Some(&Value::String("Alice".to_string())));
        assert_eq!(doc.get_field("age"), Some(&Value::Integer(30)));
        assert!(!doc.id.is_empty());
    }
    
    #[test]
    fn test_document_update() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String("Alice".to_string()));
        
        let mut doc = Document::new("users".to_string(), fields);
        let original_updated_at = doc.updated_at;
        
        // Small delay to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(1));
        
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