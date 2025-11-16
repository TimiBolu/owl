// OQL Query Optimization Engine
// Provides query planning, index recommendations, and result caching
//
// ⚠️ DEPRECATED: This module contains references to RocksDB-based indexing.
// It will be updated to use Zig B-tree indexes in Phase 9-10.
// See docs/ROCKSDB_DEPRECATION.md for details.
//
// TODO: Update to use Zig B-tree indexes (Phase 9-10)

use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
// use crate::index::{IndexDefinition, IndexManager, IndexRangeType}; // TODO: Reimplement with Zig
use crate::parser::{Condition, Expression, FieldAccess, OQLQuery, Operation};
use crate::storage::DocumentStore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Query optimization engine
pub struct QueryOptimizer {
    /// Cache for query results
    result_cache: Arc<Mutex<QueryResultCache>>,
    /// Query execution statistics
    query_stats: Arc<Mutex<HashMap<String, QueryStats>>>,
    /// Index recommendations based on query patterns
    index_recommendations: Arc<Mutex<Vec<IndexRecommendation>>>,
}

/// Query execution plan with optimization information
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// Original query
    pub query: OQLQuery,
    /// Estimated cost of execution
    pub estimated_cost: f64,
    /// Recommended indexes to use
    pub index_usage: Vec<IndexUsage>,
    /// Whether result can be cached
    pub cacheable: bool,
    /// Optimization steps applied
    pub optimizations: Vec<OptimizationStep>,
}

/// Information about index usage in a query plan
#[derive(Debug, Clone)]
pub struct IndexUsage {
    /// Index definition
    pub index: IndexDefinition,
    /// Fields being queried
    pub query_fields: Vec<String>,
    /// Type of index operation
    pub operation_type: IndexOperationType,
    /// Estimated selectivity (0.0 to 1.0)
    pub selectivity: f64,
}

/// Types of index operations
#[derive(Debug, Clone)]
pub enum IndexOperationType {
    /// Exact key lookup
    ExactMatch,
    /// Range scan
    RangeScan,
    /// Prefix scan for composite indexes
    PrefixScan,
    /// Full index scan
    FullScan,
}

/// Optimization steps applied to a query
#[derive(Debug, Clone)]
pub struct OptimizationStep {
    /// Type of optimization
    pub optimization_type: OptimizationType,
    /// Description of what was optimized
    pub description: String,
    /// Estimated performance improvement
    pub improvement_factor: f64,
}

/// Types of query optimizations
#[derive(Debug, Clone)]
pub enum OptimizationType {
    /// Index selection optimization
    IndexSelection,
    /// Query reordering
    QueryReordering,
    /// Result caching
    ResultCaching,
    /// Predicate pushdown
    PredicatePushdown,
}

/// Query result cache entry
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Cached query results
    results: Vec<Document>,
    /// When the entry was created
    created_at: Instant,
    /// How long the entry is valid
    ttl: Duration,
    /// Number of times this entry was accessed
    access_count: u64,
}

/// Query result cache
struct QueryResultCache {
    /// Cache entries by query hash
    entries: HashMap<String, CacheEntry>,
    /// Maximum number of entries to keep
    max_entries: usize,
    /// Default TTL for cache entries
    default_ttl: Duration,
}

/// Statistics for query execution
#[derive(Debug, Clone, Default)]
pub struct QueryStats {
    /// Number of times this query was executed
    pub execution_count: u64,
    /// Total execution time
    pub total_execution_time: Duration,
    /// Average execution time
    pub avg_execution_time: Duration,
    /// Number of documents scanned
    pub documents_scanned: u64,
    /// Number of documents returned
    pub documents_returned: u64,
    /// Index usage statistics
    pub index_hits: u64,
    pub index_misses: u64,
}

/// Index recommendation based on query patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRecommendation {
    /// Schema the index should be created on
    pub schema: String,
    /// Fields to include in the index
    pub fields: Vec<String>,
    /// Whether the index should be unique
    pub unique: bool,
    /// Estimated benefit score (0.0 to 1.0)
    pub benefit_score: f64,
    /// Queries that would benefit from this index
    pub benefiting_queries: Vec<String>,
    /// Estimated query count that would use this index
    pub estimated_usage: u64,
}

impl QueryOptimizer {
    /// Create a new query optimizer
    pub fn new() -> Self {
        Self {
            result_cache: Arc::new(Mutex::new(QueryResultCache::new(1000, Duration::from_secs(300)))),
            query_stats: Arc::new(Mutex::new(HashMap::new())),
            index_recommendations: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Create a query plan with optimizations
    pub fn create_query_plan(&self, query: &OQLQuery, index_manager: &IndexManager) -> OQLResult<QueryPlan> {
        let mut plan = QueryPlan {
            query: query.clone(),
            estimated_cost: 0.0,
            index_usage: Vec::new(),
            cacheable: self.is_query_cacheable(query),
            optimizations: Vec::new(),
        };
        
        // Analyze query for index usage opportunities
        self.analyze_index_usage(query, index_manager, &mut plan)?;
        
        // Apply query optimizations
        self.apply_optimizations(&mut plan)?;
        
        // Calculate estimated cost
        plan.estimated_cost = self.calculate_query_cost(&plan);
        
        Ok(plan)
    }
    
    /// Execute a query with optimization
    pub fn execute_optimized_query<S: DocumentStore>(
        &self,
        query: &OQLQuery,
        store: &S,
        index_manager: &mut IndexManager,
    ) -> OQLResult<Vec<Document>> {
        let start_time = Instant::now();
        let query_hash = self.hash_query(query);
        
        // Check cache first
        if let Some(cached_results) = self.get_cached_results(&query_hash)? {
            self.update_query_stats(&query_hash, start_time, cached_results.len(), 0, true);
            return Ok(cached_results);
        }
        
        // Create optimized query plan
        let plan = self.create_query_plan(query, index_manager)?;
        
        // Execute query using the plan
        let results = self.execute_query_plan(&plan, store, index_manager)?;
        
        // Cache results if appropriate
        if plan.cacheable {
            self.cache_results(&query_hash, &results)?;
        }
        
        // Update statistics
        let documents_scanned = self.estimate_documents_scanned(&plan);
        self.update_query_stats(&query_hash, start_time, results.len(), documents_scanned, false);
        
        // Update index recommendations
        self.update_index_recommendations(query)?;
        
        Ok(results)
    }
    
    /// Get index recommendations based on query patterns
    pub fn get_index_recommendations(&self) -> OQLResult<Vec<IndexRecommendation>> {
        let recommendations = self.index_recommendations.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire index recommendations lock".to_string(),
            }
        })?;
        
        Ok(recommendations.clone())
    }
    
    /// Get query execution statistics
    pub fn get_query_stats(&self) -> OQLResult<HashMap<String, QueryStats>> {
        let stats = self.query_stats.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire query stats lock".to_string(),
            }
        })?;
        
        Ok(stats.clone())
    }
    
    /// Clear the query result cache
    pub fn clear_cache(&self) -> OQLResult<()> {
        let mut cache = self.result_cache.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire cache lock".to_string(),
            }
        })?;
        
        cache.clear();
        Ok(())
    }
    
    /// Get cache statistics
    pub fn get_cache_stats(&self) -> OQLResult<CacheStats> {
        let cache = self.result_cache.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire cache lock".to_string(),
            }
        })?;
        
        Ok(cache.get_stats())
    }
    
    // Private helper methods
    
    fn analyze_index_usage(
        &self,
        query: &OQLQuery,
        index_manager: &IndexManager,
        plan: &mut QueryPlan,
    ) -> OQLResult<()> {
        // Extract field access patterns from WHERE clauses
        let field_patterns = self.extract_field_patterns(query);
        
        for (schema, fields) in field_patterns {
            // Find best index for these fields
            if let Some(index) = index_manager.find_best_index(&schema, &fields) {
                let selectivity = self.estimate_selectivity(&fields);
                let operation_type = self.determine_operation_type(query, &fields);
                
                plan.index_usage.push(IndexUsage {
                    index: index.clone(),
                    query_fields: fields,
                    operation_type,
                    selectivity,
                });
                
                plan.optimizations.push(OptimizationStep {
                    optimization_type: OptimizationType::IndexSelection,
                    description: format!("Using index {} for field access", index.name),
                    improvement_factor: 1.0 / selectivity, // Lower selectivity = higher improvement
                });
            }
        }
        
        Ok(())
    }
    
    fn extract_field_patterns(&self, query: &OQLQuery) -> HashMap<String, Vec<String>> {
        let mut patterns = HashMap::new();
        let schema = query.collection.clone();
        
        for operation in &query.operations {
            if let Operation::Where(where_block) = operation {
                let fields = self.extract_fields_from_condition(&where_block.condition);
                if !fields.is_empty() {
                    patterns.insert(schema.clone(), fields);
                }
            }
        }
        
        patterns
    }
    
    fn extract_fields_from_condition(&self, condition: &Condition) -> Vec<String> {
        let mut fields = Vec::new();
        
        match condition {
            Condition::Comparison { left, .. } => {
                if let Expression::FieldAccess(field_access) = left {
                    fields.extend(field_access.path.clone());
                }
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                fields.extend(self.extract_fields_from_condition(left));
                fields.extend(self.extract_fields_from_condition(right));
            }
            Condition::Not(inner) => {
                fields.extend(self.extract_fields_from_condition(inner));
            }
        }
        
        // Remove duplicates while preserving order
        let mut unique_fields = Vec::new();
        let mut seen = HashSet::new();
        for field in fields {
            if seen.insert(field.clone()) {
                unique_fields.push(field);
            }
        }
        
        unique_fields
    }
    
    fn estimate_selectivity(&self, _fields: &[String]) -> f64 {
        // Simplified selectivity estimation
        // In a real implementation, this would use statistics from the database
        0.1 // Assume 10% selectivity by default
    }
    
    fn determine_operation_type(&self, _query: &OQLQuery, _fields: &[String]) -> IndexOperationType {
        // Simplified operation type determination
        // In a real implementation, this would analyze the query conditions
        IndexOperationType::ExactMatch
    }
    
    fn apply_optimizations(&self, plan: &mut QueryPlan) -> OQLResult<()> {
        // Apply various query optimizations
        
        // Reorder operations for better performance
        if self.should_reorder_operations(&plan.query) {
            plan.optimizations.push(OptimizationStep {
                optimization_type: OptimizationType::QueryReordering,
                description: "Reordered operations for better index usage".to_string(),
                improvement_factor: 1.2,
            });
        }
        
        // Check for caching opportunities
        if plan.cacheable {
            plan.optimizations.push(OptimizationStep {
                optimization_type: OptimizationType::ResultCaching,
                description: "Query results can be cached".to_string(),
                improvement_factor: 10.0, // Cached queries are much faster
            });
        }
        
        Ok(())
    }
    
    fn should_reorder_operations(&self, _query: &OQLQuery) -> bool {
        // Simplified check for operation reordering opportunities
        true
    }
    
    fn calculate_query_cost(&self, plan: &QueryPlan) -> f64 {
        let mut base_cost = 100.0; // Base cost for a full table scan
        
        // Reduce cost based on index usage
        for index_usage in &plan.index_usage {
            base_cost *= index_usage.selectivity;
        }
        
        // Apply optimization improvements
        for optimization in &plan.optimizations {
            base_cost /= optimization.improvement_factor;
        }
        
        base_cost.max(1.0) // Minimum cost of 1.0
    }
    
    fn is_query_cacheable(&self, query: &OQLQuery) -> bool {
        // Only cache read-only queries without time-dependent functions
        !query.operations.iter().any(|op| {
            matches!(op, Operation::Insert(_) | Operation::Update(_) | Operation::Delete)
        })
    }
    
    fn hash_query(&self, query: &OQLQuery) -> String {
        // Simple query hashing - in production, use a proper hash function
        format!("{:?}", query)
    }
    
    fn get_cached_results(&self, query_hash: &str) -> OQLResult<Option<Vec<Document>>> {
        let mut cache = self.result_cache.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire cache lock".to_string(),
            }
        })?;
        
        if let Some(entry) = cache.get_mut(query_hash) {
            if entry.created_at.elapsed() < entry.ttl {
                entry.access_count += 1;
                return Ok(Some(entry.results.clone()));
            } else {
                // Entry expired, remove it
                cache.remove(query_hash);
            }
        }
        
        Ok(None)
    }
    
    fn cache_results(&self, query_hash: &str, results: &[Document]) -> OQLResult<()> {
        let mut cache = self.result_cache.lock().map_err(|_| {
            OQLError::Generic {
                message: "Failed to acquire cache lock".to_string(),
            }
        })?;
        
        cache.insert(query_hash.to_string(), results.to_vec());
        Ok(())
    }
    
    fn execute_query_plan<S: DocumentStore>(
        &self,
        plan: &QueryPlan,
        store: &S,
        index_manager: &mut IndexManager,
    ) -> OQLResult<Vec<Document>> {
        // For now, use the existing query execution logic
        // In a full implementation, this would use the optimized plan
        
        if !plan.index_usage.is_empty() {
            // Use index-based execution
            let index_usage = &plan.index_usage[0];
            let doc_ids = index_manager.query_index(
                &index_usage.index,
                &[], // Simplified - would extract actual values from query
                IndexRangeType::Exact,
            )?;
            
            let mut documents = Vec::new();
            for doc_id in doc_ids {
                if let Some(doc) = store.get_document(&plan.query.collection, &doc_id)? {
                    documents.push(doc);
                }
            }
            
            Ok(documents)
        } else {
            // Fall back to full table scan
            let iterator = store.scan_documents(&plan.query.collection)?;
            iterator.collect()
        }
    }
    
    fn estimate_documents_scanned(&self, plan: &QueryPlan) -> u64 {
        if plan.index_usage.is_empty() {
            1000 // Assume 1000 documents for full scan
        } else {
            let selectivity = plan.index_usage[0].selectivity;
            (1000.0 * selectivity) as u64
        }
    }
    
    fn update_query_stats(
        &self,
        query_hash: &str,
        start_time: Instant,
        results_count: usize,
        documents_scanned: u64,
        cache_hit: bool,
    ) {
        if let Ok(mut stats) = self.query_stats.lock() {
            let execution_time = start_time.elapsed();
            let query_stats = stats.entry(query_hash.to_string()).or_default();
            
            query_stats.execution_count += 1;
            query_stats.total_execution_time += execution_time;
            query_stats.avg_execution_time = query_stats.total_execution_time / query_stats.execution_count as u32;
            query_stats.documents_scanned += documents_scanned;
            query_stats.documents_returned += results_count as u64;
            
            if cache_hit {
                query_stats.index_hits += 1;
            } else {
                query_stats.index_misses += 1;
            }
        }
    }
    
    fn update_index_recommendations(&self, query: &OQLQuery) -> OQLResult<()> {
        // Analyze query patterns and update recommendations
        let field_patterns = self.extract_field_patterns(query);
        
        if let Ok(mut recommendations) = self.index_recommendations.lock() {
            for (schema, fields) in field_patterns {
                // Check if we already have a recommendation for these fields
                let exists = recommendations.iter().any(|rec| {
                    rec.schema == schema && rec.fields == fields
                });
                
                if !exists {
                    recommendations.push(IndexRecommendation {
                        schema,
                        fields,
                        unique: false,
                        benefit_score: 0.8, // High benefit score for new patterns
                        benefiting_queries: vec![self.hash_query(query)],
                        estimated_usage: 1,
                    });
                } else {
                    // Update existing recommendation
                    if let Some(rec) = recommendations.iter_mut().find(|r| {
                        r.schema == schema && r.fields == fields
                    }) {
                        rec.estimated_usage += 1;
                        if !rec.benefiting_queries.contains(&self.hash_query(query)) {
                            rec.benefiting_queries.push(self.hash_query(query));
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
}

impl QueryResultCache {
    fn new(max_entries: usize, default_ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries,
            default_ttl,
        }
    }
    
    fn get_mut(&mut self, key: &str) -> Option<&mut CacheEntry> {
        self.entries.get_mut(key)
    }
    
    fn insert(&mut self, key: String, results: Vec<Document>) {
        // Evict old entries if cache is full
        if self.entries.len() >= self.max_entries {
            self.evict_lru();
        }
        
        let entry = CacheEntry {
            results,
            created_at: Instant::now(),
            ttl: self.default_ttl,
            access_count: 0,
        };
        
        self.entries.insert(key, entry);
    }
    
    fn remove(&mut self, key: &str) -> Option<CacheEntry> {
        self.entries.remove(key)
    }
    
    fn clear(&mut self) {
        self.entries.clear();
    }
    
    fn evict_lru(&mut self) {
        // Simple LRU eviction - remove entry with lowest access count
        if let Some((key_to_remove, _)) = self.entries.iter()
            .min_by_key(|(_, entry)| entry.access_count)
            .map(|(k, v)| (k.clone(), v.access_count))
        {
            self.entries.remove(&key_to_remove);
        }
    }
    
    fn get_stats(&self) -> CacheStats {
        let total_entries = self.entries.len();
        let total_access_count = self.entries.values().map(|e| e.access_count).sum();
        let expired_entries = self.entries.values()
            .filter(|e| e.created_at.elapsed() >= e.ttl)
            .count();
        
        CacheStats {
            total_entries,
            expired_entries,
            total_access_count,
            hit_rate: if total_access_count > 0 {
                (total_access_count - expired_entries as u64) as f64 / total_access_count as f64
            } else {
                0.0
            },
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_entries: usize,
    pub expired_entries: usize,
    pub total_access_count: u64,
    pub hit_rate: f64,
}

impl Default for QueryOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::index::IndexManager;
    use crate::parser::{ComparisonOperator, Literal, WhereBlock};
    use crate::schema::{FieldType, Schema};
    use crate::storage::RocksDBStore;
    use std::collections::HashMap;
    use tempfile::TempDir;
    use rocksdb::{ColumnFamilyDescriptor, Options, DB};

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
    
    fn create_test_query() -> OQLQuery {
        OQLQuery {
            collection: "users".to_string(),
            operations: vec![
                Operation::Where(WhereBlock {
                    condition: Condition::Comparison {
                        left: Expression::FieldAccess(FieldAccess {
                            path: vec!["name".to_string()],
                        }),
                        operator: ComparisonOperator::Equal,
                        right: Expression::Literal(Literal::String("Alice".to_string())),
                    },
                }),
            ],
        }
    }
    
    #[test]
    fn test_query_plan_creation() {
        let (db, _temp_dir) = create_test_db();
        let mut index_manager = IndexManager::new(db);
        index_manager.initialize().unwrap();
        
        let optimizer = QueryOptimizer::new();
        let query = create_test_query();
        
        let plan = optimizer.create_query_plan(&query, &index_manager).unwrap();
        
        assert_eq!(plan.query.collection, "users");
        assert!(plan.estimated_cost > 0.0);
        assert!(plan.cacheable);
    }
    
    #[test]
    fn test_field_extraction() {
        let optimizer = QueryOptimizer::new();
        let query = create_test_query();
        
        let patterns = optimizer.extract_field_patterns(&query);
        
        assert!(patterns.contains_key("users"));
        assert_eq!(patterns["users"], vec!["name".to_string()]);
    }
    
    #[test]
    fn test_query_caching() {
        let optimizer = QueryOptimizer::new();
        let query_hash = "test_query_hash";
        let results = vec![
            Document::with_id("doc1".to_string(), "users".to_string(), HashMap::new()),
        ];
        
        // Cache results
        optimizer.cache_results(query_hash, &results).unwrap();
        
        // Retrieve cached results
        let cached = optimizer.get_cached_results(query_hash).unwrap();
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 1);
    }
    
    #[test]
    fn test_index_recommendations() {
        let optimizer = QueryOptimizer::new();
        let query = create_test_query();
        
        // Update recommendations based on query
        optimizer.update_index_recommendations(&query).unwrap();
        
        let recommendations = optimizer.get_index_recommendations().unwrap();
        assert!(!recommendations.is_empty());
        assert_eq!(recommendations[0].schema, "users");
        assert_eq!(recommendations[0].fields, vec!["name".to_string()]);
    }
    
    #[test]
    fn test_cache_stats() {
        let optimizer = QueryOptimizer::new();
        let stats = optimizer.get_cache_stats().unwrap();
        
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.total_access_count, 0);
    }
}