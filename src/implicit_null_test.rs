// Test suite for implicit null handling in OWL field access
// Demonstrates migration-free schema evolution

#[cfg(test)]
mod tests {
    use crate::document::{Document, DocumentId, Value};
    use crate::error::OQLResult;
    use crate::executor::QueryExecutor;
    use crate::parser::FieldAccess;
    use crate::schema::Schema;
    use crate::storage::DocumentStore;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Simple in-memory document store for testing
    pub struct MemoryDocumentStore {
        documents: HashMap<String, Document>,
        schemas: HashMap<String, Schema>,
    }

    impl MemoryDocumentStore {
        pub fn new() -> Self {
            Self {
                documents: HashMap::new(),
                schemas: HashMap::new(),
            }
        }
    }

    impl DocumentStore for MemoryDocumentStore {
        fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>> {
            let key = format!("{}#{}", schema, id);
            Ok(self.documents.get(&key).cloned())
        }

        fn put_document(&self, _document: &Document) -> OQLResult<()> {
            // Not needed for these tests
            Ok(())
        }

        fn delete_document(&self, _schema: &str, _id: &DocumentId) -> OQLResult<bool> {
            // Not needed for these tests
            Ok(false)
        }

        fn scan_documents(&self, _schema: &str) -> OQLResult<Vec<Document>> {
            // Not needed for these tests
            unimplemented!("scan_documents not implemented for test store")
        }

        fn get_schema(&self, name: &str) -> OQLResult<Option<Schema>> {
            Ok(self.schemas.get(name).cloned())
        }

        fn put_schema(&self, _schema: &Schema) -> OQLResult<()> {
            // Not needed for these tests
            Ok(())
        }

        fn list_schemas(&self) -> OQLResult<Vec<String>> {
            Ok(self.schemas.keys().cloned().collect())
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// Create a test document with nested structure
    fn create_test_document_with_nested_fields() -> Document {
        let mut fields = HashMap::new();

        // Basic fields
        fields.insert("name".to_string(), Value::String("Alice".to_string()));
        fields.insert("age".to_string(), Value::Integer(30));

        // Nested profile object
        let mut profile = HashMap::new();
        profile.insert(
            "avatar".to_string(),
            Value::String("avatar.jpg".to_string()),
        );
        profile.insert(
            "bio".to_string(),
            Value::String("Software engineer".to_string()),
        );

        // Deeply nested preferences
        let mut preferences = HashMap::new();
        preferences.insert("theme".to_string(), Value::String("dark".to_string()));
        preferences.insert("notifications".to_string(), Value::Boolean(true));

        // Even deeper nested settings
        let mut advanced_settings = HashMap::new();
        advanced_settings.insert("beta_features".to_string(), Value::Boolean(false));
        advanced_settings.insert("debug_mode".to_string(), Value::Boolean(true));
        preferences.insert("advanced".to_string(), Value::Object(advanced_settings));

        profile.insert("preferences".to_string(), Value::Object(preferences));
        fields.insert("profile".to_string(), Value::Object(profile));

        Document::with_id("user_001".to_string(), "users".to_string(), fields)
    }

    /// Create a test document with minimal fields (simulating old schema)
    fn create_legacy_document() -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String("Bob".to_string()));
        fields.insert("age".to_string(), Value::Integer(25));
        // No profile field - simulating old schema

        Document::with_id("user_002".to_string(), "users".to_string(), fields)
    }

    #[test]
    fn test_existing_nested_field_access() {
        let executor = create_test_executor();
        let doc = create_test_document_with_nested_fields();

        // Test accessing existing nested fields
        let field_access = FieldAccess {
            path: vec!["profile".to_string(), "avatar".to_string()],
        };
        let result = executor.get_field_value(&doc, &field_access).unwrap();
        assert_eq!(result, Value::String("avatar.jpg".to_string()));

        // Test deeply nested access
        let deep_access = FieldAccess {
            path: vec![
                "profile".to_string(),
                "preferences".to_string(),
                "advanced".to_string(),
                "beta_features".to_string(),
            ],
        };
        let result = executor.get_field_value(&doc, &deep_access).unwrap();
        assert_eq!(result, Value::Boolean(false));
    }

    #[test]
    fn test_missing_field_returns_null() {
        let executor = create_test_executor();
        let doc = create_test_document_with_nested_fields();

        // Test accessing non-existent top-level field
        let field_access = FieldAccess {
            path: vec!["nonexistent".to_string()],
        };
        let result = executor.get_field_value(&doc, &field_access).unwrap();
        assert_eq!(result, Value::Null);

        // Test accessing non-existent nested field
        let nested_access = FieldAccess {
            path: vec!["profile".to_string(), "nonexistent".to_string()],
        };
        let result = executor.get_field_value(&doc, &nested_access).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_missing_nested_structure_returns_null() {
        let executor = create_test_executor();
        let legacy_doc = create_legacy_document();

        // Test accessing nested field on document that doesn't have the parent structure
        let field_access = FieldAccess {
            path: vec!["profile".to_string(), "avatar".to_string()],
        };
        let result = executor
            .get_field_value(&legacy_doc, &field_access)
            .unwrap();
        assert_eq!(result, Value::Null);

        // Test deeply nested access on legacy document
        let deep_access = FieldAccess {
            path: vec![
                "profile".to_string(),
                "preferences".to_string(),
                "theme".to_string(),
            ],
        };
        let result = executor.get_field_value(&legacy_doc, &deep_access).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_navigation_into_non_object_returns_null() {
        let executor = create_test_executor();
        let doc = create_test_document_with_nested_fields();

        // Try to navigate into a string field
        let field_access = FieldAccess {
            path: vec!["name".to_string(), "length".to_string()],
        };
        let result = executor.get_field_value(&doc, &field_access).unwrap();
        assert_eq!(result, Value::Null);

        // Try to navigate into an integer field
        let field_access = FieldAccess {
            path: vec!["age".to_string(), "years".to_string()],
        };
        let result = executor.get_field_value(&doc, &field_access).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_empty_path_returns_null() {
        let executor = create_test_executor();
        let doc = create_test_document_with_nested_fields();

        let field_access = FieldAccess { path: vec![] };
        let result = executor.get_field_value(&doc, &field_access).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_schema_evolution_compatibility() {
        let executor = create_test_executor();

        // Create documents representing different schema versions
        let new_doc = create_test_document_with_nested_fields();
        let old_doc = create_legacy_document();

        let documents = vec![new_doc, old_doc];

        // Test that the same field access works on both documents
        let field_access = FieldAccess {
            path: vec![
                "profile".to_string(),
                "preferences".to_string(),
                "theme".to_string(),
            ],
        };

        for doc in documents {
            let result = executor.get_field_value(&doc, &field_access).unwrap();
            // New document returns the actual value, old document returns null
            // Both are valid and don't throw errors
            assert!(matches!(result, Value::String(_) | Value::Null));
        }
    }

    #[test]
    fn test_mixed_field_access_patterns() {
        let executor = create_test_executor();
        let doc = create_test_document_with_nested_fields();

        // Test various field access patterns that should all work gracefully
        let test_cases = vec![
            // Existing fields
            (vec!["name"], Some("Alice")),
            (vec!["profile", "avatar"], Some("avatar.jpg")),
            (vec!["profile", "preferences", "theme"], Some("dark")),
            // Missing fields at various levels
            (vec!["missing_field"], None),
            (vec!["profile", "missing_nested"], None),
            (vec!["profile", "preferences", "missing_deep"], None),
            (vec!["missing_parent", "missing_child"], None),
            // Navigation into non-objects
            (vec!["name", "invalid_navigation"], None),
            (vec!["age", "invalid_navigation"], None),
        ];

        for (path, expected) in test_cases {
            let field_access = FieldAccess {
                path: path.iter().map(|s| s.to_string()).collect(),
            };
            let result = executor.get_field_value(&doc, &field_access).unwrap();

            match expected {
                Some(expected_str) => {
                    assert_eq!(result, Value::String(expected_str.to_string()));
                }
                None => {
                    assert_eq!(result, Value::Null);
                }
            }
        }
    }

    // Helper function to create a test executor
    fn create_test_executor() -> QueryExecutor<MemoryDocumentStore> {
        let store = Arc::new(MemoryDocumentStore::new());
        QueryExecutor::new_with_real_clock(store)
    }
}
