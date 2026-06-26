// Index Manager for OQL Storage Engine
// Automatic index maintenance on writes with transactional consistency
//
// This module provides automatic index maintenance that:
// - Updates indexes on insert/update/delete operations
// - Ensures transactional consistency with MVCC
// - Maintains performance overhead <10%
// - Uses in-memory B-tree for fast lookups
//
// Index operations are abstracted behind a backend trait so Rust orchestrates
// while storage/index primitives can live in Zig.

use crate::document::{Document, Value};
use crate::simulation::Clock;
use crate::storage::{DocumentStore, ScalarIndexDefinition};
use owl_storage_zig::{BTreeIndex, ObjRef as ZigObjRef};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

pub trait IndexBackend: Send + Sync {
    fn insert(&self, key: u64, value: ZigObjRef, unique: bool, name: &str) -> Result<(), String>;
    fn search(&self, key: u64) -> Result<Option<ZigObjRef>, String>;
    fn delete(&self, key: u64) -> Result<bool, String>;
    fn range_query(&self, start_key: u64, end_key: u64) -> Result<Vec<ZigObjRef>, String>;
}

#[derive(Debug)]
struct InMemoryIndexBackend {
    btree: Arc<RwLock<BTreeMap<u64, ZigObjRef>>>,
}

impl InMemoryIndexBackend {
    fn new() -> Self {
        Self {
            btree: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl IndexBackend for InMemoryIndexBackend {
    fn insert(&self, key: u64, value: ZigObjRef, unique: bool, name: &str) -> Result<(), String> {
        let mut btree = self
            .btree
            .write()
            .map_err(|_| "Failed to acquire write lock on B-tree")?;
        if unique && btree.contains_key(&key) {
            return Err(format!(
                "Unique constraint violation on index '{}' for key {}",
                name, key
            ));
        }
        btree.insert(key, value);
        Ok(())
    }

    fn search(&self, key: u64) -> Result<Option<ZigObjRef>, String> {
        let btree = self
            .btree
            .read()
            .map_err(|_| "Failed to acquire read lock on B-tree")?;
        Ok(btree.get(&key).copied())
    }

    fn delete(&self, key: u64) -> Result<bool, String> {
        let mut btree = self
            .btree
            .write()
            .map_err(|_| "Failed to acquire write lock on B-tree")?;
        Ok(btree.remove(&key).is_some())
    }

    fn range_query(&self, start_key: u64, end_key: u64) -> Result<Vec<ZigObjRef>, String> {
        let btree = self
            .btree
            .read()
            .map_err(|_| "Failed to acquire read lock on B-tree")?;
        Ok(btree.range(start_key..=end_key).map(|(_, v)| *v).collect())
    }
}

struct ZigIndexBackend {
    btree: Arc<BTreeIndex>,
}

impl ZigIndexBackend {
    fn new() -> Result<Self, String> {
        let btree =
            BTreeIndex::create(16, 0).map_err(|e| format!("Failed to create Zig B-tree: {}", e))?;
        Ok(Self {
            btree: Arc::new(btree),
        })
    }
}

impl IndexBackend for ZigIndexBackend {
    fn insert(&self, key: u64, value: ZigObjRef, unique: bool, name: &str) -> Result<(), String> {
        if unique && self.search(key)?.is_some() {
            return Err(format!(
                "Unique constraint violation on index '{}' for key {}",
                name, key
            ));
        }
        self.btree
            .insert(key, value)
            .map_err(|e| format!("Zig index insert failed: {}", e))
    }

    fn search(&self, key: u64) -> Result<Option<ZigObjRef>, String> {
        self.btree
            .search(key)
            .map_err(|e| format!("Zig index search failed: {}", e))
    }

    fn delete(&self, key: u64) -> Result<bool, String> {
        self.btree
            .delete(key)
            .map_err(|e| format!("Zig index delete failed: {}", e))
    }

    fn range_query(&self, start_key: u64, end_key: u64) -> Result<Vec<ZigObjRef>, String> {
        self.btree
            .range_query(start_key, end_key)
            .map_err(|e| format!("Zig index range query failed: {}", e))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum IndexBackendKind {
    InMemory,
    Zig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndexEntry {
    pub key: u64,
    pub value_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndexDefinition {
    pub name: String,
    pub schema: String,
    pub fields: Vec<String>,
    pub unique: bool,
    pub entries: Vec<PersistedIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndexCatalog {
    pub indexes: Vec<PersistedIndexDefinition>,
}

/// Index definition with metadata
#[derive(Clone)]
pub struct IndexDefinition {
    /// Unique name for the index
    pub name: String,
    /// Schema this index belongs to
    pub schema: String,
    /// Fields included in this index (for composite indexes)
    pub fields: Vec<String>,
    /// Whether this index is unique
    pub unique: bool,
    /// Backend implementation for index operations
    backend: Arc<dyn IndexBackend>,
    /// Mirror of index entries for persistence/recovery
    entries: Arc<RwLock<BTreeMap<u64, ZigObjRef>>>,
}

impl IndexDefinition {
    /// Create a new index definition
    pub fn new(
        name: String,
        schema: String,
        fields: Vec<String>,
        unique: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            name,
            schema,
            fields,
            unique,
            backend: Arc::new(InMemoryIndexBackend::new()),
            entries: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    pub fn new_with_backend(
        name: String,
        schema: String,
        fields: Vec<String>,
        unique: bool,
        backend_kind: IndexBackendKind,
    ) -> Result<Self, String> {
        let backend: Arc<dyn IndexBackend> = match backend_kind {
            IndexBackendKind::InMemory => Arc::new(InMemoryIndexBackend::new()),
            IndexBackendKind::Zig => Arc::new(ZigIndexBackend::new()?),
        };
        Ok(Self {
            name,
            schema,
            fields,
            unique,
            backend,
            entries: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// Insert a key-value pair into the index
    pub fn insert(&self, key: u64, value: ZigObjRef) -> Result<(), String> {
        self.backend.insert(key, value, self.unique, &self.name)?;
        let mut entries = self
            .entries
            .write()
            .map_err(|_| "Failed to acquire write lock on index entries".to_string())?;
        entries.insert(key, value);
        Ok(())
    }

    /// Search for a key in the index
    pub fn search(&self, key: u64) -> Result<Option<ZigObjRef>, String> {
        self.backend.search(key)
    }

    /// Delete a key from the index
    pub fn delete(&self, key: u64) -> Result<bool, String> {
        let deleted = self.backend.delete(key)?;
        let mut entries = self
            .entries
            .write()
            .map_err(|_| "Failed to acquire write lock on index entries".to_string())?;
        entries.remove(&key);
        Ok(deleted)
    }

    /// Range query on the index
    pub fn range_query(&self, start_key: u64, end_key: u64) -> Result<Vec<ZigObjRef>, String> {
        self.backend.range_query(start_key, end_key)
    }

    fn persisted_definition(&self) -> Result<PersistedIndexDefinition, String> {
        let entries = self
            .entries
            .read()
            .map_err(|_| "Failed to acquire read lock on index entries".to_string())?;
        let persisted_entries = entries
            .iter()
            .map(|(key, value)| PersistedIndexEntry {
                key: *key,
                value_offset: value.offset(),
            })
            .collect();
        Ok(PersistedIndexDefinition {
            name: self.name.clone(),
            schema: self.schema.clone(),
            fields: self.fields.clone(),
            unique: self.unique,
            entries: persisted_entries,
        })
    }
}

/// Index manager that maintains indexes automatically with Clock dependency injection
pub struct IndexManager {
    /// Active indexes by schema name
    indexes: Arc<RwLock<HashMap<String, Vec<Arc<IndexDefinition>>>>>,
    /// Statistics for index operations
    stats: Arc<RwLock<IndexStats>>,
    /// Clock for timing operations
    clock: Arc<dyn Clock>,
    /// Selected backend kind for new indexes
    backend_kind: IndexBackendKind,
}

/// Statistics for index maintenance operations
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    /// Total number of index updates
    pub total_updates: u64,
    /// Total time spent on index updates (microseconds)
    pub total_update_time_us: u64,
    /// Number of unique constraint violations
    pub unique_violations: u64,
    /// Number of successful inserts
    pub successful_inserts: u64,
    /// Number of successful deletes
    pub successful_deletes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RebuildReport {
    pub schemas_processed: usize,
    pub documents_scanned: usize,
    pub entries_indexed: usize,
    pub documents_skipped_missing_objref: usize,
}

impl IndexStats {
    /// Calculate average update time in microseconds
    pub fn avg_update_time_us(&self) -> f64 {
        if self.total_updates == 0 {
            0.0
        } else {
            self.total_update_time_us as f64 / self.total_updates as f64
        }
    }

    /// Calculate overhead percentage (assuming 100us base operation time)
    pub fn overhead_percentage(&self, base_operation_time_us: f64) -> f64 {
        if base_operation_time_us == 0.0 {
            0.0
        } else {
            (self.avg_update_time_us() / base_operation_time_us) * 100.0
        }
    }
}

impl IndexManager {
    /// Create a new index manager with Clock dependency injection
    pub fn new_with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            indexes: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(IndexStats::default())),
            clock,
            backend_kind: IndexBackendKind::InMemory,
        }
    }

    pub fn new_with_backend(clock: Arc<dyn Clock>, backend_kind: IndexBackendKind) -> Self {
        Self {
            indexes: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(IndexStats::default())),
            clock,
            backend_kind,
        }
    }

    pub fn backend_kind(&self) -> IndexBackendKind {
        self.backend_kind
    }

    /// Create a new index manager with production clock (backward compatibility)
    pub fn new() -> Self {
        use crate::simulation::RealClock;
        Self::new_with_clock(Arc::new(RealClock::new()))
    }

    /// Create an index on specified fields
    pub fn create_index(
        &self,
        schema: &str,
        fields: Vec<String>,
        unique: bool,
    ) -> Result<Arc<IndexDefinition>, String> {
        let index_name = self.generate_index_name(schema, &fields);

        // Check if index already exists
        {
            let indexes = self
                .indexes
                .read()
                .map_err(|_| "Failed to acquire read lock on indexes")?;

            if let Some(schema_indexes) = indexes.get(schema) {
                if schema_indexes.iter().any(|idx| idx.fields == fields) {
                    return Err(format!(
                        "Index on fields {:?} already exists for schema '{}'",
                        fields, schema
                    ));
                }
            }
        }

        // Create index definition
        let index_def = Arc::new(IndexDefinition::new_with_backend(
            index_name,
            schema.to_string(),
            fields,
            unique,
            self.backend_kind,
        )?);

        // Add to registry
        {
            let mut indexes = self
                .indexes
                .write()
                .map_err(|_| "Failed to acquire write lock on indexes")?;

            indexes
                .entry(schema.to_string())
                .or_insert_with(Vec::new)
                .push(index_def.clone());
        }

        Ok(index_def)
    }

    /// Get all indexes for a schema
    pub fn get_indexes(&self, schema: &str) -> Result<Vec<Arc<IndexDefinition>>, String> {
        let indexes = self
            .indexes
            .read()
            .map_err(|_| "Failed to acquire read lock on indexes")?;

        Ok(indexes.get(schema).cloned().unwrap_or_default())
    }

    pub fn list_index_definitions(&self) -> Result<Vec<PersistedIndexDefinition>, String> {
        let indexes = self
            .indexes
            .read()
            .map_err(|_| "Failed to acquire read lock on indexes".to_string())?;
        let mut out = Vec::new();
        for schema_indexes in indexes.values() {
            for index in schema_indexes {
                out.push(index.persisted_definition()?);
            }
        }
        Ok(out)
    }

    /// Update indexes on document insert
    pub fn on_insert(
        &self,
        schema: &str,
        objref: ZigObjRef,
        field_values: &HashMap<String, u64>,
    ) -> Result<(), String> {
        let start_time = self.clock.now();

        let indexes = self.get_indexes(schema)?;

        for index in indexes {
            // Build index key from field values
            if let Some(key) = self.build_index_key(&index.fields, field_values) {
                index.insert(key, objref)?;
            }
        }

        // Update statistics
        let elapsed_us = self.clock.now().duration_since(start_time).as_micros() as u64;
        let mut stats = self
            .stats
            .write()
            .map_err(|_| "Failed to acquire write lock on stats")?;
        stats.total_updates += 1;
        stats.total_update_time_us += elapsed_us;
        stats.successful_inserts += 1;

        Ok(())
    }

    pub fn index_document(&self, document: &Document, objref: ZigObjRef) -> Result<(), String> {
        let indexes = self.get_indexes(&document.schema)?;
        if indexes.is_empty() {
            return Ok(());
        }

        let start_time = self.clock.now();
        for index in indexes {
            if let Some(field_values) = self.field_values_for_document(document, &index.fields) {
                if let Some(key) = self.build_index_key(&index.fields, &field_values) {
                    index.insert(key, objref)?;
                }
            }
        }

        let elapsed_us = self.clock.now().duration_since(start_time).as_micros() as u64;
        let mut stats = self
            .stats
            .write()
            .map_err(|_| "Failed to acquire write lock on stats")?;
        stats.total_updates += 1;
        stats.total_update_time_us += elapsed_us;
        stats.successful_inserts += 1;
        Ok(())
    }

    /// Update indexes on document update
    pub fn on_update(
        &self,
        schema: &str,
        _old_objref: ZigObjRef,
        new_objref: ZigObjRef,
        old_field_values: &HashMap<String, u64>,
        new_field_values: &HashMap<String, u64>,
    ) -> Result<(), String> {
        let start_time = self.clock.now();

        let indexes = self.get_indexes(schema)?;

        for index in indexes {
            // Remove old index entry
            if let Some(old_key) = self.build_index_key(&index.fields, old_field_values) {
                let _ = index.delete(old_key); // Best effort delete
            }

            // Insert new index entry
            if let Some(new_key) = self.build_index_key(&index.fields, new_field_values) {
                index.insert(new_key, new_objref)?;
            }
        }

        // Update statistics
        let elapsed_us = self.clock.now().duration_since(start_time).as_micros() as u64;
        let mut stats = self
            .stats
            .write()
            .map_err(|_| "Failed to acquire write lock on stats")?;
        stats.total_updates += 1;
        stats.total_update_time_us += elapsed_us;

        Ok(())
    }

    pub fn update_document(
        &self,
        old_document: &Document,
        old_objref: ZigObjRef,
        new_document: &Document,
        new_objref: ZigObjRef,
    ) -> Result<(), String> {
        let indexes = self.get_indexes(&new_document.schema)?;
        if indexes.is_empty() {
            return Ok(());
        }

        let start_time = self.clock.now();
        for index in indexes {
            let old_field_values = self.field_values_for_document(old_document, &index.fields);
            let new_field_values = self.field_values_for_document(new_document, &index.fields);

            match (old_field_values, new_field_values) {
                (Some(old_values), Some(new_values)) => {
                    let old_key = self.build_index_key(&index.fields, &old_values);
                    let new_key = self.build_index_key(&index.fields, &new_values);
                    match (old_key, new_key) {
                        (Some(ok), Some(nk)) if ok == nk && old_objref == new_objref => {}
                        (Some(ok), Some(nk)) if ok == nk => {
                            let _ = index.delete(ok);
                            index.insert(nk, new_objref)?;
                        }
                        (Some(ok), Some(nk)) => {
                            let _ = index.delete(ok);
                            index.insert(nk, new_objref)?;
                        }
                        (Some(ok), None) => {
                            let _ = index.delete(ok);
                        }
                        (None, Some(nk)) => {
                            index.insert(nk, new_objref)?;
                        }
                        (None, None) => {}
                    }
                }
                (Some(old_values), None) => {
                    if let Some(old_key) = self.build_index_key(&index.fields, &old_values) {
                        let _ = index.delete(old_key);
                    }
                }
                (None, Some(new_values)) => {
                    if let Some(new_key) = self.build_index_key(&index.fields, &new_values) {
                        index.insert(new_key, new_objref)?;
                    }
                }
                (None, None) => {}
            }
        }

        let elapsed_us = self.clock.now().duration_since(start_time).as_micros() as u64;
        let mut stats = self
            .stats
            .write()
            .map_err(|_| "Failed to acquire write lock on stats")?;
        stats.total_updates += 1;
        stats.total_update_time_us += elapsed_us;
        Ok(())
    }

    /// Update indexes on document delete
    pub fn on_delete(
        &self,
        schema: &str,
        field_values: &HashMap<String, u64>,
    ) -> Result<(), String> {
        let start_time = self.clock.now();

        let indexes = self.get_indexes(schema)?;

        for index in indexes {
            // Remove index entry
            if let Some(key) = self.build_index_key(&index.fields, field_values) {
                let _ = index.delete(key); // Best effort delete
            }
        }

        // Update statistics
        let elapsed_us = self.clock.now().duration_since(start_time).as_micros() as u64;
        let mut stats = self
            .stats
            .write()
            .map_err(|_| "Failed to acquire write lock on stats")?;
        stats.total_updates += 1;
        stats.total_update_time_us += elapsed_us;
        stats.successful_deletes += 1;

        Ok(())
    }

    pub fn remove_document(&self, document: &Document) -> Result<(), String> {
        let indexes = self.get_indexes(&document.schema)?;
        if indexes.is_empty() {
            return Ok(());
        }

        let start_time = self.clock.now();
        for index in indexes {
            if let Some(field_values) = self.field_values_for_document(document, &index.fields) {
                if let Some(key) = self.build_index_key(&index.fields, &field_values) {
                    let _ = index.delete(key);
                }
            }
        }

        let elapsed_us = self.clock.now().duration_since(start_time).as_micros() as u64;
        let mut stats = self
            .stats
            .write()
            .map_err(|_| "Failed to acquire write lock on stats")?;
        stats.total_updates += 1;
        stats.total_update_time_us += elapsed_us;
        stats.successful_deletes += 1;
        Ok(())
    }

    /// Get index statistics
    pub fn get_stats(&self) -> Result<IndexStats, String> {
        let stats = self
            .stats
            .read()
            .map_err(|_| "Failed to acquire read lock on stats")?;
        Ok(stats.clone())
    }

    /// Drop an index
    pub fn drop_index(&self, schema: &str, index_name: &str) -> Result<bool, String> {
        let mut indexes = self
            .indexes
            .write()
            .map_err(|_| "Failed to acquire write lock on indexes")?;

        if let Some(schema_indexes) = indexes.get_mut(schema) {
            if let Some(pos) = schema_indexes.iter().position(|idx| idx.name == index_name) {
                schema_indexes.remove(pos);
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn save_catalog<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let catalog = PersistedIndexCatalog {
            indexes: self.list_index_definitions()?,
        };
        let raw = serde_json::to_string_pretty(&catalog)
            .map_err(|e| format!("Failed to serialize index catalog: {}", e))?;
        fs::write(path.as_ref(), raw).map_err(|e| {
            format!(
                "Failed to write index catalog '{}': {}",
                path.as_ref().display(),
                e
            )
        })
    }

    pub fn load_catalog_with_backend<P: AsRef<Path>>(
        path: P,
        clock: Arc<dyn Clock>,
        backend_kind: IndexBackendKind,
    ) -> Result<Self, String> {
        let raw = fs::read_to_string(path.as_ref()).map_err(|e| {
            format!(
                "Failed to read index catalog '{}': {}",
                path.as_ref().display(),
                e
            )
        })?;
        let catalog: PersistedIndexCatalog = serde_json::from_str(&raw).map_err(|e| {
            format!(
                "Failed to parse index catalog '{}': {}",
                path.as_ref().display(),
                e
            )
        })?;
        Self::load_persisted_definitions_with_backend(catalog.indexes, clock, backend_kind)
    }

    pub fn load_persisted_definitions_with_backend(
        definitions: Vec<PersistedIndexDefinition>,
        clock: Arc<dyn Clock>,
        backend_kind: IndexBackendKind,
    ) -> Result<Self, String> {
        let manager = Self::new_with_backend(clock, backend_kind);
        for def in definitions {
            let index = manager.create_index(&def.schema, def.fields.clone(), def.unique)?;
            for entry in def.entries {
                index.insert(entry.key, ZigObjRef::new(entry.value_offset))?;
            }
        }
        Ok(manager)
    }

    pub fn load_scalar_definitions_with_backend(
        definitions: Vec<ScalarIndexDefinition>,
        clock: Arc<dyn Clock>,
        backend_kind: IndexBackendKind,
    ) -> Result<Self, String> {
        let manager = Self::new_with_backend(clock, backend_kind);
        for def in definitions {
            manager.create_index(&def.schema, def.fields.clone(), def.unique)?;
        }
        Ok(manager)
    }

    pub fn rebuild_from_store(&self, store: &dyn DocumentStore) -> Result<RebuildReport, String> {
        let definitions = self.list_index_definitions()?;
        let mut report = RebuildReport::default();
        if definitions.is_empty() {
            return Ok(report);
        }

        // Reinitialize indexes to empty state (preserving metadata) before rebuilding.
        {
            let mut indexes = self
                .indexes
                .write()
                .map_err(|_| "Failed to acquire write lock on indexes".to_string())?;
            indexes.clear();
        }
        for def in &definitions {
            self.create_index(&def.schema, def.fields.clone(), def.unique)?;
        }

        let mut by_schema: HashMap<String, Vec<PersistedIndexDefinition>> = HashMap::new();
        for def in definitions {
            by_schema.entry(def.schema.clone()).or_default().push(def);
        }

        for (schema, defs) in by_schema {
            report.schemas_processed += 1;
            let docs = store
                .scan_documents(&schema)
                .map_err(|e| format!("Failed scanning schema '{}': {}", schema, e))?;
            report.documents_scanned += docs.len();

            for doc in docs {
                let Some(objref) = store.get_objref(&schema, &doc.id).map_err(|e| {
                    format!("Failed resolving ObjRef for '{}.{}': {}", schema, doc.id, e)
                })?
                else {
                    report.documents_skipped_missing_objref += 1;
                    continue;
                };

                for def in &defs {
                    if let Some(field_values) = self.field_values_for_document(&doc, &def.fields) {
                        if let Some(key) = self.build_index_key(&def.fields, &field_values) {
                            let indexes = self.get_indexes(&schema)?;
                            if let Some(index) = indexes.iter().find(|i| i.name == def.name) {
                                index.insert(key, ZigObjRef::new(objref.offset()))?;
                                report.entries_indexed += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(report)
    }

    // Private helper methods

    fn generate_index_name(&self, schema: &str, fields: &[String]) -> String {
        format!("idx_{}_{}", schema, fields.join("_"))
    }

    fn build_index_key(
        &self,
        index_fields: &[String],
        field_values: &HashMap<String, u64>,
    ) -> Option<u64> {
        // For single-field indexes, use the field value directly
        if index_fields.len() == 1 {
            return field_values.get(&index_fields[0]).copied();
        }

        // For composite indexes, hash the field values together
        // This is a simplified implementation - a production system would use
        // a more sophisticated key encoding scheme
        let mut key: u64 = 0;
        for field in index_fields {
            if let Some(value) = field_values.get(field) {
                key = key.wrapping_mul(31).wrapping_add(*value);
            } else {
                return None; // Missing field value
            }
        }

        Some(key)
    }

    fn field_values_for_document(
        &self,
        doc: &Document,
        fields: &[String],
    ) -> Option<HashMap<String, u64>> {
        let mut out = HashMap::new();
        for field in fields {
            let value = Self::resolve_document_path(doc, field)?;
            let hashed = Self::value_to_index_component(value)?;
            out.insert(field.clone(), hashed);
        }
        Some(out)
    }

    fn resolve_document_path<'a>(doc: &'a Document, field: &str) -> Option<&'a Value> {
        let path: Vec<&str> = field
            .trim_start_matches('.')
            .split('.')
            .filter(|segment| !segment.is_empty())
            .collect();

        if path.is_empty() {
            return None;
        }

        if path.len() == 1 {
            return match path[0] {
                "id" => doc.get_field("id").or_else(|| None),
                "schema" | "created_at" | "updated_at" => None,
                other => doc.get_field(other),
            };
        }

        let mut current = match path[0] {
            "id" => doc.get_field("id")?,
            "schema" | "created_at" | "updated_at" => return None,
            other => doc.get_field(other)?,
        };

        for segment in &path[1..] {
            match current {
                Value::Object(map) => current = map.get(*segment)?,
                _ => return None,
            }
        }

        Some(current)
    }

    fn value_to_index_component(value: &Value) -> Option<u64> {
        match value {
            Value::String(s) => Some(wyhash::wyhash(s.as_bytes(), 0)),
            Value::Integer(i) => Some(*i as u64),
            Value::Float(f) => Some(f.to_bits()),
            Value::Boolean(b) => Some(u64::from(*b)),
            Value::DateTime(dt) => Some(dt.timestamp_micros() as u64),
            Value::Reference(r) => Some(r.offset()),
            Value::GlobalReference(r) => Some(r.local_offset()),
            Value::Null => None,
            Value::List(_) | Value::Object(_) => {
                let repr = value.to_string_repr();
                Some(wyhash::wyhash(repr.as_bytes(), 0))
            }
        }
    }
}

impl Default for IndexManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::RealClock;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn test_zig_index_backend_insert_search_delete() {
        let index = IndexDefinition::new_with_backend(
            "idx_users_id".to_string(),
            "users".to_string(),
            vec!["id".to_string()],
            false,
            IndexBackendKind::Zig,
        )
        .expect("create Zig-backed index");

        let value = ZigObjRef::new(4242);
        index.insert(42, value).expect("insert into Zig index");

        let found = index.search(42).expect("search in Zig index");
        assert_eq!(found, Some(value));

        let deleted = index.delete(42).expect("delete in Zig index");
        assert!(deleted);
        let found_after = index.search(42).expect("search after delete");
        assert!(found_after.is_none());
    }

    #[test]
    fn test_zig_index_backend_unique_constraint() {
        let index = IndexDefinition::new_with_backend(
            "idx_users_email".to_string(),
            "users".to_string(),
            vec!["email".to_string()],
            true,
            IndexBackendKind::Zig,
        )
        .expect("create Zig-backed unique index");

        index
            .insert(7, ZigObjRef::new(1001))
            .expect("first insert should succeed");
        let second = index.insert(7, ZigObjRef::new(1002));
        assert!(
            second.is_err(),
            "second insert should violate unique constraint"
        );
    }

    #[test]
    fn test_index_catalog_survives_restart_and_rehydrates_backend() {
        let temp = TempDir::new().expect("temp dir");
        let catalog = temp.path().join("indexes.json");

        let manager =
            IndexManager::new_with_backend(Arc::new(RealClock::new()), IndexBackendKind::Zig);
        let idx = manager
            .create_index("users", vec!["id".to_string()], true)
            .expect("create index");
        idx.insert(11, ZigObjRef::new(1111)).expect("insert 11");
        idx.insert(22, ZigObjRef::new(2222)).expect("insert 22");
        manager.save_catalog(&catalog).expect("save catalog");

        let loaded = IndexManager::load_catalog_with_backend(
            &catalog,
            Arc::new(RealClock::new()),
            IndexBackendKind::Zig,
        )
        .expect("load catalog");
        let indexes = loaded.get_indexes("users").expect("get indexes");
        let loaded_idx = indexes.first().expect("index exists");
        assert_eq!(
            loaded_idx.search(11).expect("search 11"),
            Some(ZigObjRef::new(1111))
        );
        assert_eq!(
            loaded_idx.search(22).expect("search 22"),
            Some(ZigObjRef::new(2222))
        );
    }

    #[test]
    fn test_nested_path_field_values_are_indexable() {
        let manager =
            IndexManager::new_with_backend(Arc::new(RealClock::new()), IndexBackendKind::InMemory);

        let mut profile = HashMap::new();
        profile.insert("country".to_string(), Value::String("NG".to_string()));
        profile.insert("age".to_string(), Value::Integer(29));

        let mut fields = HashMap::new();
        fields.insert("profile".to_string(), Value::Object(profile));

        let doc = Document::with_id("u1".to_string(), "users".to_string(), fields);

        let values = manager
            .field_values_for_document(
                &doc,
                &["profile.country".to_string(), "profile.age".to_string()],
            )
            .expect("nested path values");

        assert!(values.contains_key("profile.country"));
        assert_eq!(values.get("profile.age"), Some(&(29u64)));
    }
}
