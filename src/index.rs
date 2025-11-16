// OQL Database Indexing System
// Provides automatic B-tree indexes for query optimization
//
// ⚠️ DEPRECATED: This module uses RocksDB for index storage.
// It will be replaced with Zig B-tree indexes in Phase 10.
// See docs/ROCKSDB_DEPRECATION.md for details.
//
// TODO: Reimplement using Zig B-tree storage (Phase 10)

use crate::document::{Document, Value, ObjRef};
use crate::error::{OQLError, OQLResult};
use crate::schema::{FieldType, Schema};
use rocksdb::{ColumnFamily, DB};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Index metadata and configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexDefinition {
    /// Unique name for the index
    pub name: String,
    /// Schema this index belongs to
    pub schema: String,
    /// Fields included in this index (for composite indexes)
    pub fields: Vec<String>,
    /// Whether this index is unique
    pub unique: bool,
    /// Whether this index was created automatically
    pub auto_created: bool,
    /// Index type (currently only BTree supported)
    pub index_type: IndexType,
}

/// Types of indexes supported
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IndexType {
    /// B-tree index for range queries and equality
    BTree,
    /// Hash index for equality queries only (future)
    Hash,
}

/// Index key for storage - combines field values into a sortable key
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IndexKey {
    /// Encoded field values in order
    pub values: Vec<IndexValue>,
}

/// Individual value in an index key
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexValue {
    String(String),
    Integer(i64),
    Float(OrderedFloat),
    Boolean(bool),
    DateTime(i64), // Unix timestamp
    Reference(u64), // Store ObjRef as u64 for ordering
    Null,
}

/// Wrapper for f64 to make it orderable
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct OrderedFloat(f64);

impl Eq for OrderedFloat {}

impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Index manager handles creation, maintenance, and querying of indexes
pub struct IndexManager {
    db: Arc<DB>,
    /// Active indexes by schema and field combination
    indexes: HashMap<String, Vec<IndexDefinition>>,
    /// Statistics for index usage
    usage_stats: HashMap<String, IndexUsageStats>,
}

/// Statistics for index usage and performance
#[derive(Debug, Clone, Default)]
pub struct IndexUsageStats {
    pub query_count: u64,
    pub hit_count: u64,
    pub miss_count: u64,
    pub avg_scan_time_ms: f64,
}

impl IndexManager {
    /// Create a new index manager
    pub fn new(db: Arc<DB>) -> Self {
        Self {
            db,
            indexes: HashMap::new(),
            usage_stats: HashMap::new(),
        }
    }
    
    /// Initialize index manager by loading existing indexes
    pub fn initialize(&mut self) -> OQLResult<()> {
        self.load_existing_indexes()?;
        Ok(())
    }
    
    /// Create an index on specified fields
    pub fn create_index(
        &mut self,
        schema: &str,
        fields: Vec<String>,
        unique: bool,
        auto_created: bool,
    ) -> OQLResult<IndexDefinition> {
        let index_name = self.generate_index_name(schema, &fields);
        
        // Check if index already exists
        if self.index_exists(schema, &fields) {
            return Err(OQLError::Generic {
                message: format!("Index on fields {:?} already exists for schema '{}'", fields, schema),
            });
        }
        
        let index_def = IndexDefinition {
            name: index_name.clone(),
            schema: schema.to_string(),
            fields: fields.clone(),
            unique,
            auto_created,
            index_type: IndexType::BTree,
        };
        
        // Store index definition
        self.store_index_definition(&index_def)?;
        
        // Add to in-memory index registry
        self.indexes
            .entry(schema.to_string())
            .or_insert_with(Vec::new)
            .push(index_def.clone());
        
        // Initialize usage stats
        self.usage_stats.insert(index_name, IndexUsageStats::default());
        
        Ok(index_def)
    }
    
    /// Automatically create indexes based on query patterns
    pub fn auto_create_indexes(&mut self, schema: &Schema) -> OQLResult<Vec<IndexDefinition>> {
        let mut created_indexes = Vec::new();
        
        // Create single-field indexes for commonly queried types
        for (field_name, field_type) in &schema.fields {
            if self.should_auto_index(field_type) && !self.index_exists(&schema.name, &[field_name.clone()]) {
                let index = self.create_index(
                    &schema.name,
                    vec![field_name.clone()],
                    false,
                    true,
                )?;
                created_indexes.push(index);
            }
        }
        
        // Create reference field indexes
        for (field_name, _ref_type) in schema.get_reference_fields() {
            if !self.index_exists(&schema.name, &[field_name.clone()]) {
                let index = self.create_index(
                    &schema.name,
                    vec![field_name.clone()],
                    false,
                    true,
                )?;
                created_indexes.push(index);
            }
        }
        
        Ok(created_indexes)
    }
    
    /// Add a document to all relevant indexes
    pub fn index_document(&self, document: &Document) -> OQLResult<()> {
        if let Some(schema_indexes) = self.indexes.get(&document.schema) {
            for index_def in schema_indexes {
                self.add_to_index(index_def, document)?;
            }
        }
        Ok(())
    }
    
    /// Remove a document from all relevant indexes
    pub fn unindex_document(&self, document: &Document) -> OQLResult<()> {
        if let Some(schema_indexes) = self.indexes.get(&document.schema) {
            for index_def in schema_indexes {
                self.remove_from_index(index_def, document)?;
            }
        }
        Ok(())
    }
    
    /// Update document in indexes (remove old, add new)
    pub fn update_document_index(&self, old_doc: &Document, new_doc: &Document) -> OQLResult<()> {
        self.unindex_document(old_doc)?;
        self.index_document(new_doc)?;
        Ok(())
    }
    
    /// Find the best index for a query on specific fields
    pub fn find_best_index(&self, schema: &str, fields: &[String]) -> Option<&IndexDefinition> {
        let schema_indexes = self.indexes.get(schema)?;
        
        // Look for exact match first
        for index in schema_indexes {
            if index.fields == fields {
                return Some(index);
            }
        }
        
        // Look for index that starts with these fields (prefix match)
        for index in schema_indexes {
            if fields.len() <= index.fields.len() {
                let matches = fields.iter()
                    .zip(index.fields.iter())
                    .all(|(query_field, index_field)| query_field == index_field);
                
                if matches {
                    return Some(index);
                }
            }
        }
        
        None
    }
    
    /// Query documents using an index
    pub fn query_index(
        &mut self,
        index_def: &IndexDefinition,
        key_values: &[Value],
        range_type: IndexRangeType,
    ) -> OQLResult<Vec<String>> {
        let start_time = std::time::Instant::now();
        let index_name = index_def.name.clone();
        
        // Update usage stats
        {
            let stats = self.usage_stats.entry(index_name.clone()).or_default();
            stats.query_count += 1;
        }
        
        let result = self.perform_index_query(index_def, key_values, range_type);
        
        // Update timing stats
        let elapsed = start_time.elapsed().as_millis() as f64;
        {
            let stats = self.usage_stats.entry(index_name).or_default();
            stats.avg_scan_time_ms = (stats.avg_scan_time_ms * (stats.query_count - 1) as f64 + elapsed) / stats.query_count as f64;
            
            match &result {
                Ok(doc_ids) => {
                    if doc_ids.is_empty() {
                        stats.miss_count += 1;
                    } else {
                        stats.hit_count += 1;
                    }
                }
                Err(_) => stats.miss_count += 1,
            }
        }
        
        result
    }
    
    /// Get index usage statistics
    pub fn get_usage_stats(&self, index_name: &str) -> Option<&IndexUsageStats> {
        self.usage_stats.get(index_name)
    }
    
    /// List all indexes for a schema
    pub fn list_indexes(&self, schema: &str) -> Vec<&IndexDefinition> {
        self.indexes.get(schema).map(|indexes| indexes.iter().collect()).unwrap_or_default()
    }
    
    /// Drop an index
    pub fn drop_index(&mut self, schema: &str, index_name: &str) -> OQLResult<bool> {
        if let Some(schema_indexes) = self.indexes.get_mut(schema) {
            if let Some(pos) = schema_indexes.iter().position(|idx| idx.name == index_name) {
                let index_def = schema_indexes.remove(pos);
                
                // Remove from storage
                self.remove_index_definition(&index_def)?;
                self.clear_index_data(&index_def)?;
                
                // Remove usage stats
                self.usage_stats.remove(index_name);
                
                return Ok(true);
            }
        }
        Ok(false)
    }
    
    // Private helper methods
    
    fn generate_index_name(&self, schema: &str, fields: &[String]) -> String {
        format!("idx_{}_{}", schema, fields.join("_"))
    }
    
    fn index_exists(&self, schema: &str, fields: &[String]) -> bool {
        if let Some(schema_indexes) = self.indexes.get(schema) {
            schema_indexes.iter().any(|idx| idx.fields == fields)
        } else {
            false
        }
    }
    
    fn should_auto_index(&self, field_type: &FieldType) -> bool {
        match field_type {
            FieldType::String | FieldType::Integer | FieldType::DateTime => true,
            FieldType::Reference(_) => true,
            FieldType::Optional(inner) => self.should_auto_index(inner),
            _ => false,
        }
    }
    
    fn load_existing_indexes(&mut self) -> OQLResult<()> {
        let cf = self.get_indexes_cf()?;
        let iter = self.db.iterator_cf(cf, rocksdb::IteratorMode::Start);
        
        for item in iter {
            let (key, value) = item?;
            let key_str = String::from_utf8_lossy(&key);
            
            if key_str.starts_with("def_") {
                let index_def: IndexDefinition = serde_json::from_slice(&value)?;
                self.indexes
                    .entry(index_def.schema.clone())
                    .or_insert_with(Vec::new)
                    .push(index_def.clone());
                
                self.usage_stats.insert(index_def.name.clone(), IndexUsageStats::default());
            }
        }
        
        Ok(())
    }
    
    fn store_index_definition(&self, index_def: &IndexDefinition) -> OQLResult<()> {
        let cf = self.get_indexes_cf()?;
        let key = format!("def_{}", index_def.name);
        let value = serde_json::to_vec(index_def)?;
        
        self.db.put_cf(cf, key.as_bytes(), value)?;
        Ok(())
    }
    
    fn remove_index_definition(&self, index_def: &IndexDefinition) -> OQLResult<()> {
        let cf = self.get_indexes_cf()?;
        let key = format!("def_{}", index_def.name);
        
        self.db.delete_cf(cf, key.as_bytes())?;
        Ok(())
    }
    
    fn add_to_index(&self, index_def: &IndexDefinition, document: &Document) -> OQLResult<()> {
        let index_key = self.build_index_key(index_def, document)?;
        let storage_key = self.encode_index_storage_key(&index_def.name, &index_key);
        
        let cf = self.get_indexes_cf()?;
        
        // For unique indexes, check if key already exists
        if index_def.unique {
            if let Some(existing_doc_id) = self.db.get_cf(cf, &storage_key)? {
                let existing_id = String::from_utf8_lossy(&existing_doc_id);
                if existing_id != document.id {
                    return Err(OQLError::Generic {
                        message: format!("Unique constraint violation on index '{}' for key {:?}", index_def.name, index_key),
                    });
                }
            }
        }
        
        // Store document ID at this index key
        self.db.put_cf(cf, &storage_key, document.id.as_bytes())?;
        Ok(())
    }
    
    fn remove_from_index(&self, index_def: &IndexDefinition, document: &Document) -> OQLResult<()> {
        let index_key = self.build_index_key(index_def, document)?;
        let storage_key = self.encode_index_storage_key(&index_def.name, &index_key);
        
        let cf = self.get_indexes_cf()?;
        self.db.delete_cf(cf, &storage_key)?;
        Ok(())
    }
    
    fn clear_index_data(&self, index_def: &IndexDefinition) -> OQLResult<()> {
        let cf = self.get_indexes_cf()?;
        let prefix = format!("data_{}#", index_def.name);
        
        let mut keys_to_delete = Vec::new();
        let iter = self.db.prefix_iterator_cf(cf, prefix.as_bytes());
        
        for item in iter {
            let (key, _) = item?;
            keys_to_delete.push(key.to_vec());
        }
        
        for key in keys_to_delete {
            self.db.delete_cf(cf, &key)?;
        }
        
        Ok(())
    }
    
    fn build_index_key(&self, index_def: &IndexDefinition, document: &Document) -> OQLResult<IndexKey> {
        let mut values = Vec::new();
        
        for field_name in &index_def.fields {
            let value = document.get_field(field_name).unwrap_or(&Value::Null);
            values.push(self.value_to_index_value(value));
        }
        
        Ok(IndexKey { values })
    }
    
    fn value_to_index_value(&self, value: &Value) -> IndexValue {
        match value {
            Value::String(s) => IndexValue::String(s.clone()),
            Value::Integer(i) => IndexValue::Integer(*i),
            Value::Float(f) => IndexValue::Float(OrderedFloat(*f)),
            Value::Boolean(b) => IndexValue::Boolean(*b),
            Value::DateTime(dt) => IndexValue::DateTime(dt.timestamp()),
            Value::Reference(objref) => IndexValue::Reference(objref.offset()),
            Value::Null => IndexValue::Null,
            Value::List(_) => IndexValue::Null, // Lists not directly indexable
            Value::Object(_) => IndexValue::Null, // Objects not directly indexable
        }
    }
    
    fn encode_index_storage_key(&self, index_name: &str, index_key: &IndexKey) -> Vec<u8> {
        let mut key = format!("data_{}#", index_name);
        
        for (i, value) in index_key.values.iter().enumerate() {
            if i > 0 {
                key.push('#');
            }
            
            match value {
                IndexValue::String(s) => key.push_str(&format!("s:{}", s)),
                IndexValue::Integer(i) => key.push_str(&format!("i:{:020}", i)), // Zero-padded for sorting
                IndexValue::Float(f) => key.push_str(&format!("f:{:020.10}", f.0)), // Fixed precision
                IndexValue::Boolean(b) => key.push_str(&format!("b:{}", if *b { "1" } else { "0" })),
                IndexValue::DateTime(dt) => key.push_str(&format!("d:{:020}", dt)),
                IndexValue::Reference(offset) => key.push_str(&format!("r:{:016x}", offset)),
                IndexValue::Null => key.push_str("n:null"),
            }
        }
        
        key.into_bytes()
    }
    
    fn perform_index_query(
        &self,
        index_def: &IndexDefinition,
        key_values: &[Value],
        range_type: IndexRangeType,
    ) -> OQLResult<Vec<String>> {
        let cf = self.get_indexes_cf()?;
        let mut document_ids = Vec::new();
        
        match range_type {
            IndexRangeType::Exact => {
                // Build exact key and lookup
                let index_values: Vec<IndexValue> = key_values.iter()
                    .map(|v| self.value_to_index_value(v))
                    .collect();
                let index_key = IndexKey { values: index_values };
                let storage_key = self.encode_index_storage_key(&index_def.name, &index_key);
                
                if let Some(doc_id_bytes) = self.db.get_cf(cf, &storage_key)? {
                    let doc_id = String::from_utf8_lossy(&doc_id_bytes).to_string();
                    document_ids.push(doc_id);
                }
            }
            IndexRangeType::Prefix => {
                // Build prefix and scan
                let prefix = if key_values.is_empty() {
                    format!("data_{}#", index_def.name)
                } else {
                    let index_values: Vec<IndexValue> = key_values.iter()
                        .map(|v| self.value_to_index_value(v))
                        .collect();
                    let partial_key = IndexKey { values: index_values };
                    let encoded_key = self.encode_index_storage_key(&index_def.name, &partial_key);
                    let mut prefix = String::from_utf8_lossy(&encoded_key).to_string();
                    // Add separator to ensure we only match the prefix of the next field
                    prefix.push('#');
                    prefix
                };
                
                let iter = self.db.prefix_iterator_cf(cf, prefix.as_bytes());
                for item in iter {
                    let (key_bytes, doc_id_bytes) = item?;
                    let key_str = String::from_utf8_lossy(&key_bytes);
                    
                    // Only process data keys, not definition keys
                    if key_str.starts_with(&format!("data_{}", index_def.name)) {
                        let doc_id = String::from_utf8_lossy(&doc_id_bytes).to_string();
                        document_ids.push(doc_id);
                    }
                }
            }
            IndexRangeType::Range { start, end } => {
                // Range scan between start and end keys
                let start_key = if let Some(start_values) = start {
                    let index_values: Vec<IndexValue> = start_values.iter()
                        .map(|v| self.value_to_index_value(v))
                        .collect();
                    let index_key = IndexKey { values: index_values };
                    self.encode_index_storage_key(&index_def.name, &index_key)
                } else {
                    format!("data_{}#", index_def.name).into_bytes()
                };
                
                let iter = self.db.iterator_cf(cf, rocksdb::IteratorMode::From(&start_key, rocksdb::Direction::Forward));
                
                for item in iter {
                    let (key_bytes, doc_id_bytes) = item?;
                    
                    // Check if we've exceeded the end key
                    if let Some(ref end_values) = end {
                        let index_values: Vec<IndexValue> = end_values.iter()
                            .map(|v| self.value_to_index_value(v))
                            .collect();
                        let end_key = IndexKey { values: index_values };
                        let end_storage_key = self.encode_index_storage_key(&index_def.name, &end_key);
                        
                        if key_bytes.as_ref() > end_storage_key.as_slice() {
                            break;
                        }
                    }
                    
                    let doc_id = String::from_utf8_lossy(&doc_id_bytes).to_string();
                    document_ids.push(doc_id);
                }
            }
        }
        
        Ok(document_ids)
    }
    
    fn get_indexes_cf(&self) -> OQLResult<&ColumnFamily> {
        self.db.cf_handle("indexes").ok_or_else(|| {
            OQLError::Generic {
                message: "Indexes column family not found".to_string(),
            }
        })
    }
}

/// Types of index range queries
#[derive(Debug, Clone)]
pub enum IndexRangeType {
    /// Exact key match
    Exact,
    /// Prefix match (for composite indexes)
    Prefix,
    /// Range query between start and end values
    Range {
        start: Option<Vec<Value>>,
        end: Option<Vec<Value>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::schema::{ReferenceType, Schema};
    use std::collections::HashMap;
    use tempfile::TempDir;
    use rocksdb::{ColumnFamilyDescriptor, Options};

    fn create_test_db() -> (Arc<DB>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        
        let cfs = vec![
            ColumnFamilyDescriptor::new("documents", Options::default()),
            ColumnFamilyDescriptor::new("schemas", Options::default()),
            ColumnFamilyDescriptor::new("indexes", Options::default()),
            ColumnFamilyDescriptor::new("metadata", Options::default()),
        ];
        
        let db = DB::open_cf_descriptors(&opts, temp_dir.path(), cfs).unwrap();
        (Arc::new(db), temp_dir)
    }
    
    fn create_test_schema() -> Schema {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), FieldType::String);
        fields.insert("age".to_string(), FieldType::Integer);
        fields.insert("email".to_string(), FieldType::String);
        fields.insert("customer".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
        
        Schema::new("users".to_string(), fields)
    }
    
    fn create_test_document(id: &str, name: &str, age: i64) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        fields.insert("age".to_string(), Value::Integer(age));
        fields.insert("email".to_string(), Value::String(format!("{}@example.com", name.to_lowercase())));
        
        Document::with_id(id.to_string(), "users".to_string(), fields)
    }
    
    #[test]
    fn test_index_creation() {
        let (db, _temp_dir) = create_test_db();
        let mut index_manager = IndexManager::new(db);
        index_manager.initialize().unwrap();
        
        let index = index_manager.create_index(
            "users",
            vec!["name".to_string()],
            false,
            false,
        ).unwrap();
        
        assert_eq!(index.schema, "users");
        assert_eq!(index.fields, vec!["name".to_string()]);
        assert!(!index.unique);
        assert!(!index.auto_created);
        
        // Test duplicate index creation fails
        let result = index_manager.create_index(
            "users",
            vec!["name".to_string()],
            false,
            false,
        );
        assert!(result.is_err());
    }
    
    #[test]
    fn test_auto_index_creation() {
        let (db, _temp_dir) = create_test_db();
        let mut index_manager = IndexManager::new(db);
        index_manager.initialize().unwrap();
        
        let schema = create_test_schema();
        let indexes = index_manager.auto_create_indexes(&schema).unwrap();
        
        // Should create indexes for name, age, email, and customer fields
        assert_eq!(indexes.len(), 4);
        
        let index_names: Vec<String> = indexes.iter().map(|idx| idx.name.clone()).collect();
        assert!(index_names.contains(&"idx_users_name".to_string()));
        assert!(index_names.contains(&"idx_users_age".to_string()));
        assert!(index_names.contains(&"idx_users_email".to_string()));
        assert!(index_names.contains(&"idx_users_customer".to_string()));
    }
    
    #[test]
    fn test_document_indexing() {
        let (db, _temp_dir) = create_test_db();
        let mut index_manager = IndexManager::new(db);
        index_manager.initialize().unwrap();
        
        // Create index
        let _index = index_manager.create_index(
            "users",
            vec!["name".to_string()],
            false,
            false,
        ).unwrap();
        
        // Index a document
        let doc = create_test_document("user_1", "Alice", 30);
        index_manager.index_document(&doc).unwrap();
        
        // Query the index
        let index_def = index_manager.indexes["users"][0].clone();
        let doc_ids = index_manager.query_index(
            &index_def,
            &[Value::String("Alice".to_string())],
            IndexRangeType::Exact,
        ).unwrap();
        
        assert_eq!(doc_ids, vec!["user_1".to_string()]);
    }
    
    #[test]
    fn test_composite_index() {
        let (db, _temp_dir) = create_test_db();
        let mut index_manager = IndexManager::new(db);
        index_manager.initialize().unwrap();
        
        // Create composite index
        let _index = index_manager.create_index(
            "users",
            vec!["name".to_string(), "age".to_string()],
            false,
            false,
        ).unwrap();
        
        // Index documents
        let doc1 = create_test_document("user_1", "Alice", 30);
        let doc2 = create_test_document("user_2", "Alice", 25);
        
        index_manager.index_document(&doc1).unwrap();
        index_manager.index_document(&doc2).unwrap();
        
        // Query with exact composite key
        let index_def = index_manager.indexes["users"][0].clone();
        let doc_ids = index_manager.query_index(
            &index_def,
            &[Value::String("Alice".to_string()), Value::Integer(30)],
            IndexRangeType::Exact,
        ).unwrap();
        
        assert_eq!(doc_ids, vec!["user_1".to_string()]);
        
        // Query with prefix (just name)
        let index_def = index_manager.indexes["users"][0].clone();
        let doc_ids = index_manager.query_index(
            &index_def,
            &[Value::String("Alice".to_string())],
            IndexRangeType::Prefix,
        ).unwrap();
        

        assert_eq!(doc_ids.len(), 2);
        assert!(doc_ids.contains(&"user_1".to_string()));
        assert!(doc_ids.contains(&"user_2".to_string()));
    }
    
    #[test]
    fn test_unique_index() {
        let (db, _temp_dir) = create_test_db();
        let mut index_manager = IndexManager::new(db);
        index_manager.initialize().unwrap();
        
        // Create unique index
        let _index = index_manager.create_index(
            "users",
            vec!["email".to_string()],
            true,
            false,
        ).unwrap();
        
        // Index first document
        let doc1 = create_test_document("user_1", "Alice", 30);
        index_manager.index_document(&doc1).unwrap();
        
        // Try to index document with same email - should fail
        let doc2 = Document::with_id("user_2".to_string(), "users".to_string(), {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String("Bob".to_string()));
            fields.insert("age".to_string(), Value::Integer(25));
            fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
            fields
        });
        
        let result = index_manager.index_document(&doc2);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unique constraint violation"));
    }
    
    #[test]
    fn test_index_best_match() {
        let (db, _temp_dir) = create_test_db();
        let mut index_manager = IndexManager::new(db);
        index_manager.initialize().unwrap();
        
        // Create various indexes
        index_manager.create_index("users", vec!["name".to_string()], false, false).unwrap();
        index_manager.create_index("users", vec!["name".to_string(), "age".to_string()], false, false).unwrap();
        index_manager.create_index("users", vec!["email".to_string()], false, false).unwrap();
        
        // Test exact match
        let best = index_manager.find_best_index("users", &["name".to_string()]);
        assert!(best.is_some());
        assert_eq!(best.unwrap().fields, vec!["name".to_string()]);
        
        // Test composite match
        let best = index_manager.find_best_index("users", &["name".to_string(), "age".to_string()]);
        assert!(best.is_some());
        assert_eq!(best.unwrap().fields, vec!["name".to_string(), "age".to_string()]);
        
        // Test prefix match (should use composite index)
        let best = index_manager.find_best_index("users", &["name".to_string()]);
        assert!(best.is_some());
        // Should prefer exact match over prefix match
        assert_eq!(best.unwrap().fields, vec!["name".to_string()]);
    }
}