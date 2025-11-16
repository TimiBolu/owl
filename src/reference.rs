// Reference Resolution System for OQL
// Handles deep navigation like .customer.account.bank.routing.country.code

use crate::document::{Document, Value, ObjRef};
use crate::error::{OQLError, OQLResult};
use crate::storage::DocumentStore;
use std::collections::HashMap;
use std::sync::Arc;

/// Reference resolver for handling deep navigation queries
pub struct ReferenceResolver<S: DocumentStore> {
    store: Arc<S>,
    /// Cache for resolved documents to avoid repeated lookups
    cache: HashMap<String, Document>,
    /// Track reference chain to detect circular references
    resolution_stack: Vec<String>,
    /// Maximum depth for reference resolution to prevent infinite loops
    max_depth: usize,
}

impl<S: DocumentStore> ReferenceResolver<S> {
    /// Create a new reference resolver
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            cache: HashMap::new(),
            resolution_stack: Vec::new(),
            max_depth: 10, // Default maximum depth to prevent infinite loops
        }
    }
    
    /// Create a new reference resolver with custom maximum depth
    pub fn with_max_depth(store: Arc<S>, max_depth: usize) -> Self {
        Self {
            store,
            cache: HashMap::new(),
            resolution_stack: Vec::new(),
            max_depth,
        }
    }
    
    /// Resolve a field path with arbitrary depth navigation and implicit null handling
    /// Example: resolve_field_path(order_doc, ["customer", "account", "secure_token"])
    /// Returns null gracefully for any missing fields or broken references
    pub fn resolve_field_path(&mut self, document: &Document, path: &[String]) -> OQLResult<Value> {
        // Clear resolution stack for new resolution
        self.resolution_stack.clear();
        // Add the starting document to the resolution stack
        let doc_ref = format!("{}#{}", document.schema, document.id);
        self.resolution_stack.push(doc_ref);
        let result = self.resolve_field_path_internal(document, path, 0);
        // Clear the stack after resolution
        self.resolution_stack.clear();
        result
    }
    
    /// Internal recursive method for field path resolution with implicit null handling
    /// Returns null gracefully for missing fields, broken references, or circular references
    fn resolve_field_path_internal(&mut self, document: &Document, path: &[String], depth: usize) -> OQLResult<Value> {
        // Check maximum depth to prevent infinite loops - return null instead of error
        if depth >= self.max_depth {
            // Graceful degradation: return null instead of throwing error
            return Ok(Value::Null);
        }
        
        if path.is_empty() {
            return Ok(Value::String(document.id.clone()));
        }
        
        let field_name = &path[0];
        let remaining_path = &path[1..];
        
        // Get the field value from the current document
        match document.get_field(field_name) {
            Some(Value::Reference(objref)) if !remaining_path.is_empty() => {
                // Convert ObjRef to string ID for circular reference checking
                let target_doc_id = objref.to_string_id();
                
                // Check for circular reference - return null instead of error
                if self.resolution_stack.contains(&target_doc_id) {
                    return Ok(Value::Null);
                }
                
                // Add to resolution stack
                self.resolution_stack.push(target_doc_id.clone());
                
                // This is a reference and we need to navigate deeper
                let result = match self.resolve_reference_from_objref(objref) {
                    Ok(referenced_doc) => self.resolve_field_path_internal(&referenced_doc, remaining_path, depth + 1),
                    Err(_) => {
                        // Handle missing reference gracefully by returning null
                        Ok(Value::Null)
                    }
                };
                
                // Remove from resolution stack
                self.resolution_stack.pop();
                
                result
            }
            Some(Value::Reference(objref)) if remaining_path.is_empty() => {
                // This is the final reference in the path, return the ObjRef
                Ok(Value::Reference(*objref))
            }
            Some(value) if remaining_path.is_empty() => {
                // This is the final field in the path, return its value
                Ok(value.clone())
            }
            Some(Value::List(_list)) if !remaining_path.is_empty() => {
                // List navigation not implemented - return null gracefully
                Ok(Value::Null)
            }
            Some(_) => {
                // Field exists but we're trying to navigate deeper into a non-reference
                // Return null gracefully instead of error
                Ok(Value::Null)
            }
            None => {
                // Field doesn't exist - return null gracefully
                Ok(Value::Null)
            }
        }
    }
    
    /// Resolve a reference from ObjRef to a document, using cache when possible
    fn resolve_reference_from_objref(&mut self, objref: &ObjRef) -> OQLResult<Document> {
        // Convert ObjRef to string ID for cache lookup
        let ref_id = objref.to_string_id();
        self.resolve_reference(&ref_id)
    }
    
    /// Resolve a reference ID to a document, using cache when possible
    fn resolve_reference(&mut self, ref_id: &str) -> OQLResult<Document> {
        // Check cache first
        if let Some(cached_doc) = self.cache.get(ref_id) {
            return Ok(cached_doc.clone());
        }
        
        // Try to determine the schema from the reference ID format
        // Expected format: "schema_name#document_id" or just "document_id"
        let (schema, doc_id) = if ref_id.contains('#') {
            let parts: Vec<&str> = ref_id.splitn(2, '#').collect();
            (parts[0], parts[1])
        } else if ref_id.starts_with("ref_") {
            // This is a hashed reference - we can't resolve it without a mapping
            return Err(OQLError::ReferenceError {
                message: format!("Cannot resolve hashed reference '{}' without schema mapping", ref_id),
            });
        } else {
            // If no schema prefix, we need to search across schemas
            // For now, we'll return an error and require explicit schema prefixes
            return Err(OQLError::ReferenceError {
                message: format!("Reference ID '{}' must include schema prefix (e.g., 'customers#{})", ref_id, ref_id),
            });
        };
        
        // Load the document from storage
        match self.store.get_document(schema, &doc_id.to_string()) {
            Ok(Some(document)) => {
                // Cache the document for future use
                self.cache.insert(ref_id.to_string(), document.clone());
                Ok(document)
            }
            Ok(None) => Err(OQLError::ReferenceError {
                message: format!("Referenced document '{}' not found in schema '{}'", doc_id, schema),
            }),
            Err(e) => Err(OQLError::ReferenceError {
                message: format!("Failed to load referenced document '{}': {}", ref_id, e),
            }),
        }
    }
    
    /// Batch resolve multiple references for performance optimization
    /// Returns documents in the same order as the input reference IDs
    pub fn batch_resolve_references(&mut self, ref_ids: &[String]) -> OQLResult<Vec<Option<Document>>> {
        let mut results = Vec::with_capacity(ref_ids.len());
        let mut uncached_refs = Vec::new();
        let mut uncached_indices = Vec::new();
        
        // First pass: check cache and collect uncached references
        for (index, ref_id) in ref_ids.iter().enumerate() {
            if let Some(cached_doc) = self.cache.get(ref_id) {
                results.push(Some(cached_doc.clone()));
            } else {
                results.push(None); // Placeholder
                uncached_refs.push(ref_id);
                uncached_indices.push(index);
            }
        }
        
        // Second pass: batch load uncached references
        if !uncached_refs.is_empty() {
            let loaded_docs = self.batch_load_from_storage(&uncached_refs)?;
            
            // Fill in the results with loaded documents
            for (i, doc_opt) in loaded_docs.into_iter().enumerate() {
                let result_index = uncached_indices[i];
                results[result_index] = doc_opt;
            }
        }
        
        Ok(results)
    }
    
    /// Batch load multiple references from storage, optimized for performance
    fn batch_load_from_storage(&mut self, ref_ids: &[&String]) -> OQLResult<Vec<Option<Document>>> {
        let mut results = Vec::with_capacity(ref_ids.len());
        
        // Group references by schema for more efficient storage access
        let mut schema_groups: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        
        for (index, ref_id) in ref_ids.iter().enumerate() {
            let (schema, doc_id) = if ref_id.contains('#') {
                let parts: Vec<&str> = ref_id.splitn(2, '#').collect();
                (parts[0].to_string(), parts[1].to_string())
            } else {
                return Err(OQLError::ReferenceError {
                    message: format!("Reference ID '{}' must include schema prefix (e.g., 'customers#{}')", ref_id, ref_id),
                });
            };
            
            schema_groups.entry(schema).or_insert_with(Vec::new).push((index, doc_id));
        }
        
        // Initialize results vector with None values
        results.resize(ref_ids.len(), None);
        
        // Load documents grouped by schema
        for (schema, doc_refs) in schema_groups {
            for (original_index, doc_id) in doc_refs {
                match self.store.get_document(&schema, &doc_id) {
                    Ok(Some(document)) => {
                        // Cache the document
                        let ref_id = format!("{}#{}", schema, doc_id);
                        self.cache.insert(ref_id, document.clone());
                        results[original_index] = Some(document);
                    }
                    Ok(None) => {
                        // Document not found - leave as None
                        results[original_index] = None;
                    }
                    Err(e) => {
                        return Err(OQLError::ReferenceError {
                            message: format!("Failed to load document '{}#{}': {}", schema, doc_id, e),
                        });
                    }
                }
            }
        }
        
        Ok(results)
    }
    
    /// Batch resolve field paths for multiple documents and paths
    /// Optimized for scenarios where you need to resolve the same path across many documents
    pub fn batch_resolve_field_paths(&mut self, documents: &[Document], path: &[String]) -> OQLResult<Vec<Value>> {
        let mut results = Vec::with_capacity(documents.len());
        let mut ref_ids_to_resolve = Vec::new();
        let mut ref_indices = Vec::new();
        
        // First pass: collect all reference IDs that need resolution
        for (index, document) in documents.iter().enumerate() {
            if path.is_empty() {
                results.push(Value::String(document.id.clone()));
                continue;
            }
            
            let field_name = &path[0];
            match document.get_field(field_name) {
                Some(Value::Reference(objref)) if path.len() > 1 => {
                    // Need to resolve this reference
                    ref_ids_to_resolve.push(objref.to_string_id());
                    ref_indices.push(index);
                    results.push(Value::Null); // Placeholder
                }
                Some(Value::Reference(objref)) if path.len() == 1 => {
                    // Final reference in path
                    results.push(Value::Reference(*objref));
                }
                Some(value) if path.len() == 1 => {
                    // Final field value
                    results.push(value.clone());
                }
                Some(_) if path.len() > 1 => {
                    // Cannot navigate deeper into non-reference
                    results.push(Value::Null);
                }
                Some(_) => {
                    // This shouldn't happen given the conditions above, but handle it
                    results.push(Value::Null);
                }
                None => {
                    // Field doesn't exist
                    results.push(Value::Null);
                }
            }
        }
        
        // Second pass: batch resolve references if needed
        if !ref_ids_to_resolve.is_empty() && path.len() > 1 {
            let remaining_path = &path[1..];
            let resolved_docs = self.batch_resolve_references(&ref_ids_to_resolve)?;
            
            // Third pass: resolve remaining path for each resolved document
            for (i, doc_opt) in resolved_docs.into_iter().enumerate() {
                let result_index = ref_indices[i];
                match doc_opt {
                    Some(doc) => {
                        let resolved_value = self.resolve_field_path_internal(&doc, remaining_path, 1)?;
                        results[result_index] = resolved_value;
                    }
                    None => {
                        // Reference not found, leave as null
                        results[result_index] = Value::Null;
                    }
                }
            }
        }
        
        Ok(results)
    }
    
    /// Clear the cache (useful for testing or memory management)
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.resolution_stack.clear();
    }
    
    /// Get cache statistics for monitoring
    pub fn cache_stats(&self) -> (usize, usize) {
        (self.cache.len(), self.cache.capacity())
    }
    
    /// Validate that a reference exists and is accessible
    pub fn validate_reference(&mut self, ref_id: &str) -> OQLResult<bool> {
        match self.resolve_reference(ref_id) {
            Ok(_) => Ok(true),
            Err(OQLError::ReferenceError { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
    
    /// Resolve field path and return string representation
    pub fn resolve_field_path_as_string(&mut self, document: &Document, path: &[String]) -> OQLResult<String> {
        let value = self.resolve_field_path(document, path)?;
        Ok(value.to_string_repr())
    }
    
    /// Get the current maximum depth setting
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }
    
    /// Set the maximum depth for reference resolution
    pub fn set_max_depth(&mut self, max_depth: usize) {
        self.max_depth = max_depth;
    }
    
    /// Check if a reference chain would create a circular reference
    pub fn would_create_circular_reference(&self, ref_id: &str) -> bool {
        self.resolution_stack.contains(&ref_id.to_string())
    }
    
    /// Get the current resolution stack (for debugging)
    pub fn resolution_stack(&self) -> &[String] {
        &self.resolution_stack
    }
}

/// Helper function to parse field access paths from dot notation
/// Example: ".customer.account.secure_token" -> ["customer", "account", "secure_token"]
pub fn parse_field_path(field_access: &str) -> Vec<String> {
    field_access
        .trim_start_matches('.')
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Helper function to create reference IDs with schema prefixes
pub fn create_reference_id(schema: &str, document_id: &str) -> String {
    format!("{}#{}", schema, document_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::storage::create_storage;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_resolver() -> (ReferenceResolver<dyn DocumentStore>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.owl");
        let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024).unwrap();
        let resolver = ReferenceResolver::new(store);
        (resolver, temp_dir)
    }
    
    fn create_test_documents(store: &crate::storage::Storage) {
        // Create customer document
        let mut customer_fields = HashMap::new();
        customer_fields.insert("name".to_string(), Value::String("Alice".to_string()));
        customer_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
        customer_fields.insert("account".to_string(), Value::reference_from_string("accounts#acc_789".to_string()));
        
        let customer_doc = Document::with_id("cust_456".to_string(), "customers".to_string(), customer_fields);
        store.put_document(&customer_doc).unwrap();
        
        // Create account document
        let mut account_fields = HashMap::new();
        account_fields.insert("balance".to_string(), Value::Float(5000.0));
        account_fields.insert("secure_token".to_string(), Value::String("abc123xyz".to_string()));
        account_fields.insert("bank".to_string(), Value::reference_from_string("banks#bank_001".to_string()));
        
        let account_doc = Document::with_id("acc_789".to_string(), "accounts".to_string(), account_fields);
        store.put_document(&account_doc).unwrap();
        
        // Create bank document
        let mut bank_fields = HashMap::new();
        bank_fields.insert("name".to_string(), Value::String("First National Bank".to_string()));
        bank_fields.insert("routing".to_string(), Value::reference_from_string("routing#rt_001".to_string()));
        
        let bank_doc = Document::with_id("bank_001".to_string(), "banks".to_string(), bank_fields);
        store.put_document(&bank_doc).unwrap();
        
        // Create routing document
        let mut routing_fields = HashMap::new();
        routing_fields.insert("number".to_string(), Value::String("123456789".to_string()));
        routing_fields.insert("country".to_string(), Value::reference_from_string("countries#us".to_string()));
        
        let routing_doc = Document::with_id("rt_001".to_string(), "routing".to_string(), routing_fields);
        store.put_document(&routing_doc).unwrap();
        
        // Create country document
        let mut country_fields = HashMap::new();
        country_fields.insert("name".to_string(), Value::String("United States".to_string()));
        country_fields.insert("code".to_string(), Value::String("US".to_string()));
        
        let country_doc = Document::with_id("us".to_string(), "countries".to_string(), country_fields);
        store.put_document(&country_doc).unwrap();
        
        // Create order document that references customer
        let mut order_fields = HashMap::new();
        order_fields.insert("amount".to_string(), Value::Float(150.0));
        order_fields.insert("customer".to_string(), Value::reference_from_string("customers#cust_456".to_string()));
        
        let order_doc = Document::with_id("ord_123".to_string(), "orders".to_string(), order_fields);
        store.put_document(&order_doc).unwrap();
    }
    
    #[test]
    fn test_single_level_reference_resolution() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // Get order document
        let order_doc = resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap();
        
        // Resolve single level: .customer
        let path = vec!["customer".to_string()];
        let result = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        match result {
            Value::Reference(objref) => {
                // Verify it's not null
                assert!(!objref.is_null());
            },
            _ => panic!("Expected reference value"),
        }
    }
    
    #[test]
    fn test_two_level_reference_resolution() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // Get order document
        let order_doc = resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap();
        
        // Resolve two levels: .customer.name
        let path = vec!["customer".to_string(), "name".to_string()];
        let result = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        match result {
            Value::String(name) => assert_eq!(name, "Alice"),
            _ => panic!("Expected string value, got: {:?}", result),
        }
    }
    
    #[test]
    fn test_three_level_reference_resolution() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // Get order document
        let order_doc = resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap();
        
        // Resolve three levels: .customer.account.secure_token
        let path = vec!["customer".to_string(), "account".to_string(), "secure_token".to_string()];
        let result = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        match result {
            Value::String(token) => assert_eq!(token, "abc123xyz"),
            _ => panic!("Expected string value, got: {:?}", result),
        }
    }
    
    #[test]
    fn test_deep_reference_resolution() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // Get order document
        let order_doc = resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap();
        
        // Resolve deep path: .customer.account.bank.routing.country.code (6 levels!)
        let path = vec![
            "customer".to_string(),
            "account".to_string(), 
            "bank".to_string(),
            "routing".to_string(),
            "country".to_string(),
            "code".to_string()
        ];
        let result = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        match result {
            Value::String(code) => assert_eq!(code, "US"),
            _ => panic!("Expected string value, got: {:?}", result),
        }
    }
    
    #[test]
    fn test_missing_reference_handling() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // Get order document
        let order_doc = resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap();
        
        // Try to resolve non-existent field
        let path = vec!["nonexistent".to_string()];
        let result = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        assert_eq!(result, Value::Null);
    }
    
    #[test]
    fn test_graceful_invalid_reference_handling() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        
        // Create document with invalid reference
        let mut order_fields = HashMap::new();
        order_fields.insert("customer".to_string(), Value::reference_from_string("customers#nonexistent".to_string()));
        let order_doc = Document::with_id("ord_invalid".to_string(), "orders".to_string(), order_fields);
        
        // Try to resolve invalid reference - should now return null gracefully
        let path = vec!["customer".to_string(), "name".to_string()];
        let result = resolver.resolve_field_path(&order_doc, &path);
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Null);
    }
    
    #[test]
    fn test_parse_field_path() {
        assert_eq!(parse_field_path(".customer.name"), vec!["customer", "name"]);
        assert_eq!(parse_field_path("customer.account.secure_token"), 
                   vec!["customer", "account", "secure_token"]);
        assert_eq!(parse_field_path(".customer.account.bank.routing.country.code"), 
                   vec!["customer", "account", "bank", "routing", "country", "code"]);
        assert_eq!(parse_field_path("name"), vec!["name"]);
        assert_eq!(parse_field_path(""), Vec::<String>::new());
    }
    
    #[test]
    fn test_create_reference_id() {
        assert_eq!(create_reference_id("customers", "cust_123"), "customers#cust_123");
        assert_eq!(create_reference_id("orders", "ord_456"), "orders#ord_456");
    }
    
    #[test]
    fn test_cache_functionality() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // First resolution should cache the document
        let order_doc = resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap();
        let path = vec!["customer".to_string(), "name".to_string()];
        let _result1 = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        let (cache_size, _) = resolver.cache_stats();
        assert!(cache_size > 0);
        
        // Second resolution should use cache
        let _result2 = resolver.resolve_field_path(&order_doc, &path).unwrap();
        
        // Clear cache
        resolver.clear_cache();
        let (cache_size_after_clear, _) = resolver.cache_stats();
        assert_eq!(cache_size_after_clear, 0);
    }
    
    #[test]
    fn test_circular_reference_detection() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        
        // Create documents with circular references
        let mut doc_a_fields = HashMap::new();
        doc_a_fields.insert("name".to_string(), Value::String("Document A".to_string()));
        doc_a_fields.insert("ref_to_b".to_string(), Value::reference_from_string("circular#doc_b".to_string()));
        let doc_a = Document::with_id("doc_a".to_string(), "circular".to_string(), doc_a_fields);
        resolver.store.put_document(&doc_a).unwrap();
        
        let mut doc_b_fields = HashMap::new();
        doc_b_fields.insert("name".to_string(), Value::String("Document B".to_string()));
        doc_b_fields.insert("ref_to_a".to_string(), Value::reference_from_string("circular#doc_a".to_string()));
        let doc_b = Document::with_id("doc_b".to_string(), "circular".to_string(), doc_b_fields);
        resolver.store.put_document(&doc_b).unwrap();
        
        // Try to resolve circular reference: doc_a.ref_to_b.ref_to_a.name
        let path = vec!["ref_to_b".to_string(), "ref_to_a".to_string(), "name".to_string()];
        let result = resolver.resolve_field_path(&doc_a, &path);
        
        assert!(result.is_err());
        match result {
            Err(OQLError::ReferenceError { message }) => {
                assert!(message.contains("Circular reference detected"));
            }
            Err(OQLError::EnhancedReferenceError { message, .. }) => {
                assert!(message.contains("Circular reference detected"));
            }
            _ => panic!("Expected circular reference error"),
        }
    }
    
    #[test]
    fn test_max_depth_protection() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        
        // Set a very low max depth
        resolver.set_max_depth(2);
        assert_eq!(resolver.max_depth(), 2);
        
        create_test_documents(&resolver.store);
        
        // Try to resolve a path that exceeds max depth
        let order_doc = resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap();
        let path = vec![
            "customer".to_string(),
            "account".to_string(), 
            "bank".to_string(),
            "routing".to_string(),
            "country".to_string(),
            "code".to_string()
        ];
        let result = resolver.resolve_field_path(&order_doc, &path);
        
        assert!(result.is_err());
        match result {
            Err(OQLError::ReferenceError { message }) => {
                assert!(message.contains("Maximum reference resolution depth"));
            }
            Err(OQLError::EnhancedReferenceError { message, .. }) => {
                assert!(message.contains("Maximum reference resolution depth"));
            }
            _ => panic!("Expected max depth error"),
        }
    }
    
    #[test]
    fn test_batch_resolve_references() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // Test batch resolution
        let ref_ids = vec![
            "customers#cust_456".to_string(),
            "accounts#acc_789".to_string(),
            "banks#bank_001".to_string(),
            "nonexistent#invalid".to_string(), // This should return None
        ];
        
        let results = resolver.batch_resolve_references(&ref_ids).unwrap();
        assert_eq!(results.len(), 4);
        
        // Check that valid references were resolved
        assert!(results[0].is_some()); // customer
        assert!(results[1].is_some()); // account
        assert!(results[2].is_some()); // bank
        assert!(results[3].is_none());  // nonexistent
        
        // Verify the resolved documents
        let customer_doc = results[0].as_ref().unwrap();
        assert_eq!(customer_doc.get_field("name"), Some(&Value::String("Alice".to_string())));
        
        let account_doc = results[1].as_ref().unwrap();
        assert_eq!(account_doc.get_field("secure_token"), Some(&Value::String("abc123xyz".to_string())));
    }
    
    #[test]
    fn test_batch_resolve_field_paths() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        create_test_documents(&resolver.store);
        
        // Create multiple order documents
        let mut order2_fields = HashMap::new();
        order2_fields.insert("amount".to_string(), Value::Float(200.0));
        order2_fields.insert("customer".to_string(), Value::reference_from_string("customers#cust_456".to_string()));
        let order2_doc = Document::with_id("ord_456".to_string(), "orders".to_string(), order2_fields);
        resolver.store.put_document(&order2_doc).unwrap();
        
        let documents = vec![
            resolver.store.get_document("orders", &"ord_123".to_string()).unwrap().unwrap(),
            order2_doc,
        ];
        
        // Batch resolve customer names for all orders
        let path = vec!["customer".to_string(), "name".to_string()];
        let results = resolver.batch_resolve_field_paths(&documents, &path).unwrap();
        
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], Value::String("Alice".to_string()));
        assert_eq!(results[1], Value::String("Alice".to_string()));
    }
    
    #[test]
    fn test_would_create_circular_reference() {
        let (mut resolver, _temp_dir) = create_test_resolver();
        
        // Simulate being in the middle of resolving a reference chain
        resolver.resolution_stack.push("customers#cust_456".to_string());
        resolver.resolution_stack.push("accounts#acc_789".to_string());
        
        // Check if adding another reference would create a circular reference
        assert!(resolver.would_create_circular_reference("customers#cust_456"));
        assert!(resolver.would_create_circular_reference("accounts#acc_789"));
        assert!(!resolver.would_create_circular_reference("banks#bank_001"));
        
        // Check resolution stack
        let stack = resolver.resolution_stack();
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[0], "customers#cust_456");
        assert_eq!(stack[1], "accounts#acc_789");
    }
}