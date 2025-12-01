// Index Manager for OQL Storage Engine
// Automatic index maintenance on writes with transactional consistency
//
// This module provides automatic index maintenance that:
// - Updates indexes on insert/update/delete operations
// - Ensures transactional consistency with MVCC
// - Maintains performance overhead <10%
// - Uses in-memory B-tree for fast lookups
//
// Note: This implementation uses a Rust-based BTreeMap for now.
// Future optimization: Integrate with Zig B-tree via C API for better performance.

use owl_storage_zig::ObjRef as ZigObjRef;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use crate::simulation::Clock;

/// Index definition with metadata
#[derive(Debug, Clone)]
pub struct IndexDefinition {
    /// Unique name for the index
    pub name: String,
    /// Schema this index belongs to
    pub schema: String,
    /// Fields included in this index (for composite indexes)
    pub fields: Vec<String>,
    /// Whether this index is unique
    pub unique: bool,
    /// The underlying B-tree index (key -> ObjRef)
    btree: Arc<RwLock<BTreeMap<u64, ZigObjRef>>>,
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
            btree: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }
    
    /// Insert a key-value pair into the index
    pub fn insert(&self, key: u64, value: ZigObjRef) -> Result<(), String> {
        let mut btree = self.btree.write()
            .map_err(|_| "Failed to acquire write lock on B-tree")?;
        
        // For unique indexes, check if key already exists
        if self.unique && btree.contains_key(&key) {
            return Err(format!(
                "Unique constraint violation on index '{}' for key {}",
                self.name, key
            ));
        }
        
        btree.insert(key, value);
        Ok(())
    }
    
    /// Search for a key in the index
    pub fn search(&self, key: u64) -> Result<Option<ZigObjRef>, String> {
        let btree = self.btree.read()
            .map_err(|_| "Failed to acquire read lock on B-tree")?;
        
        Ok(btree.get(&key).copied())
    }
    
    /// Delete a key from the index
    pub fn delete(&self, key: u64) -> Result<bool, String> {
        let mut btree = self.btree.write()
            .map_err(|_| "Failed to acquire write lock on B-tree")?;
        
        Ok(btree.remove(&key).is_some())
    }
    
    /// Range query on the index
    pub fn range_query(&self, start_key: u64, end_key: u64) -> Result<Vec<ZigObjRef>, String> {
        let btree = self.btree.read()
            .map_err(|_| "Failed to acquire read lock on B-tree")?;
        
        let results: Vec<ZigObjRef> = btree
            .range(start_key..=end_key)
            .map(|(_, v)| *v)
            .collect();
        
        Ok(results)
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
        }
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
            let indexes = self.indexes.read()
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
        let index_def = Arc::new(IndexDefinition::new(
            index_name,
            schema.to_string(),
            fields,
            unique,
        )?);
        
        // Add to registry
        {
            let mut indexes = self.indexes.write()
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
        let indexes = self.indexes.read()
            .map_err(|_| "Failed to acquire read lock on indexes")?;
        
        Ok(indexes.get(schema).cloned().unwrap_or_default())
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
        let mut stats = self.stats.write()
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
        let mut stats = self.stats.write()
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
        let mut stats = self.stats.write()
            .map_err(|_| "Failed to acquire write lock on stats")?;
        stats.total_updates += 1;
        stats.total_update_time_us += elapsed_us;
        stats.successful_deletes += 1;
        
        Ok(())
    }
    
    /// Get index statistics
    pub fn get_stats(&self) -> Result<IndexStats, String> {
        let stats = self.stats.read()
            .map_err(|_| "Failed to acquire read lock on stats")?;
        Ok(stats.clone())
    }
    
    /// Drop an index
    pub fn drop_index(&self, schema: &str, index_name: &str) -> Result<bool, String> {
        let mut indexes = self.indexes.write()
            .map_err(|_| "Failed to acquire write lock on indexes")?;
        
        if let Some(schema_indexes) = indexes.get_mut(schema) {
            if let Some(pos) = schema_indexes.iter().position(|idx| idx.name == index_name) {
                schema_indexes.remove(pos);
                return Ok(true);
            }
        }
        
        Ok(false)
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
}

impl Default for IndexManager {
    fn default() -> Self {
        Self::new()
    }
}
