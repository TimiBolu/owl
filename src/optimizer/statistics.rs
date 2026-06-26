// Statistics Collection for Cost-Based Query Optimization
//
// This module provides statistics collection and management for query optimization.
// Statistics are used by the cost-based optimizer to estimate query costs and
// choose optimal execution plans.

use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::storage::DocumentStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Statistics for a collection/table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableStatistics {
    /// Schema/collection name
    pub schema: String,
    /// Total number of documents
    pub row_count: u64,
    /// Average document size in bytes
    pub avg_row_size: u64,
    /// Total size in bytes
    pub total_size: u64,
    /// Per-field statistics
    pub field_stats: HashMap<String, FieldStatistics>,
    /// Last update timestamp
    pub last_updated: u64,
}

/// Statistics for a single field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldStatistics {
    /// Field name
    pub field_name: String,
    /// Number of distinct values (cardinality)
    pub distinct_count: u64,
    /// Number of null values
    pub null_count: u64,
    /// Minimum value (if applicable)
    pub min_value: Option<Value>,
    /// Maximum value (if applicable)
    pub max_value: Option<Value>,
    /// Histogram for value distribution
    pub histogram: Option<Histogram>,
    /// Average field size in bytes
    pub avg_size: u64,
}

/// Histogram for value distribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Histogram {
    /// Number of buckets
    pub bucket_count: usize,
    /// Histogram buckets
    pub buckets: Vec<HistogramBucket>,
    /// Type of histogram
    pub histogram_type: HistogramType,
}

/// Type of histogram
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HistogramType {
    /// Equal-width buckets
    EqualWidth,
    /// Equal-depth buckets (equal number of values per bucket)
    EqualDepth,
}

/// A single histogram bucket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramBucket {
    /// Lower bound (inclusive)
    pub lower_bound: Value,
    /// Upper bound (exclusive)
    pub upper_bound: Value,
    /// Number of values in this bucket
    pub count: u64,
    /// Number of distinct values in this bucket
    pub distinct_count: u64,
}

/// Index statistics for selectivity estimation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatistics {
    /// Index name
    pub index_name: String,
    /// Schema this index belongs to
    pub schema: String,
    /// Fields in the index
    pub fields: Vec<String>,
    /// Number of index entries
    pub entry_count: u64,
    /// Average key size
    pub avg_key_size: u64,
    /// Index selectivity (0.0 to 1.0)
    /// Lower values mean more selective (better for filtering)
    pub selectivity: f64,
    /// Number of index lookups
    pub lookup_count: u64,
    /// Number of index scans
    pub scan_count: u64,
}

/// Statistics manager
#[derive(Clone)]
pub struct StatisticsManager {
    /// Table statistics by schema name
    table_stats: Arc<RwLock<HashMap<String, TableStatistics>>>,
    /// Index statistics by index name
    index_stats: Arc<RwLock<HashMap<String, IndexStatistics>>>,
    /// Configuration
    config: StatisticsConfig,
}

/// Configuration for statistics collection
#[derive(Debug, Clone)]
pub struct StatisticsConfig {
    /// Number of histogram buckets
    pub histogram_buckets: usize,
    /// Histogram type
    pub histogram_type: HistogramType,
    /// Sample size for statistics (0 = full scan)
    pub sample_size: usize,
    /// Auto-update threshold (update stats after this many changes)
    pub auto_update_threshold: u64,
}

impl Default for StatisticsConfig {
    fn default() -> Self {
        Self {
            histogram_buckets: 100,
            histogram_type: HistogramType::EqualDepth,
            sample_size: 10000,          // Sample 10k rows by default
            auto_update_threshold: 1000, // Update after 1k changes
        }
    }
}

impl StatisticsManager {
    /// Create a new statistics manager
    pub fn new() -> Self {
        Self::with_config(StatisticsConfig::default())
    }

    /// Create a new statistics manager with custom configuration
    pub fn with_config(config: StatisticsConfig) -> Self {
        Self {
            table_stats: Arc::new(RwLock::new(HashMap::new())),
            index_stats: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Collect statistics for a schema
    pub fn collect_table_statistics(
        &self,
        store: &dyn DocumentStore,
        schema: &str,
    ) -> OQLResult<TableStatistics> {
        // Scan documents
        let documents = store.scan_documents(schema)?;

        if documents.is_empty() {
            return Ok(TableStatistics {
                schema: schema.to_string(),
                row_count: 0,
                avg_row_size: 0,
                total_size: 0,
                field_stats: HashMap::new(),
                last_updated: current_timestamp(),
            });
        }

        // Sample documents if needed
        let sample = if self.config.sample_size > 0 && documents.len() > self.config.sample_size {
            self.sample_documents(&documents, self.config.sample_size)
        } else {
            documents.clone()
        };

        // Collect field statistics
        let field_stats = self.collect_field_statistics(&sample)?;

        // Calculate table-level statistics
        let row_count = documents.len() as u64;
        let total_size: u64 = sample
            .iter()
            .map(|doc| self.estimate_document_size(doc))
            .sum();
        let avg_row_size = if !sample.is_empty() {
            total_size / sample.len() as u64
        } else {
            0
        };

        let stats = TableStatistics {
            schema: schema.to_string(),
            row_count,
            avg_row_size,
            total_size: (avg_row_size * row_count), // Extrapolate from sample
            field_stats,
            last_updated: current_timestamp(),
        };

        // Store statistics
        self.store_table_statistics(&stats)?;

        Ok(stats)
    }

    /// Get statistics for a schema
    pub fn get_table_statistics(&self, schema: &str) -> OQLResult<Option<TableStatistics>> {
        let stats = self
            .table_stats
            .read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(stats.get(schema).cloned())
    }

    /// Store table statistics
    fn store_table_statistics(&self, stats: &TableStatistics) -> OQLResult<()> {
        let mut table_stats = self
            .table_stats
            .write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
        table_stats.insert(stats.schema.clone(), stats.clone());
        Ok(())
    }

    /// Collect field-level statistics
    fn collect_field_statistics(
        &self,
        documents: &[Document],
    ) -> OQLResult<HashMap<String, FieldStatistics>> {
        if documents.is_empty() {
            return Ok(HashMap::new());
        }

        // Collect all field names
        let mut field_names = std::collections::HashSet::new();
        for doc in documents {
            for field_name in doc.fields.keys() {
                field_names.insert(field_name.clone());
            }
        }

        // Collect statistics for each field
        let mut field_stats = HashMap::new();
        for field_name in field_names {
            let stats = self.collect_single_field_statistics(&field_name, documents)?;
            field_stats.insert(field_name, stats);
        }

        Ok(field_stats)
    }

    /// Collect statistics for a single field
    fn collect_single_field_statistics(
        &self,
        field_name: &str,
        documents: &[Document],
    ) -> OQLResult<FieldStatistics> {
        let mut values = Vec::new();
        let mut null_count = 0u64;
        let mut total_size = 0u64;

        // Collect values
        for doc in documents {
            if let Some(value) = doc.fields.get(field_name) {
                if value.is_null() {
                    null_count += 1;
                } else {
                    values.push(value.clone());
                    total_size += self.estimate_value_size(value);
                }
            } else {
                null_count += 1;
            }
        }

        // Calculate distinct count
        let distinct_count = self.estimate_distinct_count(&values);

        // Find min/max values
        let (min_value, max_value) = self.find_min_max(&values);

        // Build histogram
        let histogram = if !values.is_empty() && self.can_build_histogram(&values) {
            Some(self.build_histogram(&values)?)
        } else {
            None
        };

        let avg_size = if !values.is_empty() {
            total_size / values.len() as u64
        } else {
            0
        };

        Ok(FieldStatistics {
            field_name: field_name.to_string(),
            distinct_count,
            null_count,
            min_value,
            max_value,
            histogram,
            avg_size,
        })
    }

    /// Estimate distinct count using string representation
    fn estimate_distinct_count(&self, values: &[Value]) -> u64 {
        let mut seen = std::collections::HashSet::new();
        for value in values {
            // Use debug format as a unique string representation
            seen.insert(format!("{:?}", value));
        }
        seen.len() as u64
    }

    /// Find minimum and maximum values
    fn find_min_max(&self, values: &[Value]) -> (Option<Value>, Option<Value>) {
        if values.is_empty() {
            return (None, None);
        }

        let mut min = values[0].clone();
        let mut max = values[0].clone();

        for value in values.iter().skip(1) {
            if self.compare_values(value, &min) < 0 {
                min = value.clone();
            }
            if self.compare_values(value, &max) > 0 {
                max = value.clone();
            }
        }

        (Some(min), Some(max))
    }

    /// Compare two values for ordering
    fn compare_values(&self, a: &Value, b: &Value) -> i32 {
        match (a, b) {
            (Value::Integer(a), Value::Integer(b)) => a.cmp(b) as i32,
            (Value::Float(a), Value::Float(b)) => {
                if a < b {
                    -1
                } else if a > b {
                    1
                } else {
                    0
                }
            }
            (Value::String(a), Value::String(b)) => a.cmp(b) as i32,
            (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b) as i32,
            _ => 0, // Can't compare different types
        }
    }

    /// Check if we can build a histogram for these values
    fn can_build_histogram(&self, values: &[Value]) -> bool {
        if values.is_empty() {
            return false;
        }

        // Check if all values are of the same comparable type
        let first_type = std::mem::discriminant(&values[0]);
        values
            .iter()
            .all(|v| std::mem::discriminant(v) == first_type)
            && matches!(
                values[0],
                Value::Integer(_) | Value::Float(_) | Value::String(_)
            )
    }

    /// Build a histogram for the given values
    fn build_histogram(&self, values: &[Value]) -> OQLResult<Histogram> {
        if values.is_empty() {
            return Err(OQLError::generic(
                "Cannot build histogram from empty values",
            ));
        }

        match self.config.histogram_type {
            HistogramType::EqualWidth => self.build_equal_width_histogram(values),
            HistogramType::EqualDepth => self.build_equal_depth_histogram(values),
        }
    }

    /// Build an equal-width histogram
    fn build_equal_width_histogram(&self, values: &[Value]) -> OQLResult<Histogram> {
        let mut sorted_values = values.to_vec();
        sorted_values.sort_by(|a, b| self.compare_values(a, b).cmp(&0));

        let min = &sorted_values[0];
        let max = &sorted_values[sorted_values.len() - 1];

        let bucket_count = self.config.histogram_buckets.min(sorted_values.len());
        let mut buckets = Vec::with_capacity(bucket_count);

        // Create buckets
        for i in 0..bucket_count {
            let lower_bound = if i == 0 {
                min.clone()
            } else {
                self.interpolate_value(min, max, i as f64 / bucket_count as f64)
            };

            let upper_bound = if i == bucket_count - 1 {
                max.clone()
            } else {
                self.interpolate_value(min, max, (i + 1) as f64 / bucket_count as f64)
            };

            // Count values in this bucket
            let count = sorted_values
                .iter()
                .filter(|v| {
                    self.compare_values(v, &lower_bound) >= 0
                        && (i == bucket_count - 1 || self.compare_values(v, &upper_bound) < 0)
                })
                .count() as u64;

            let distinct_values: std::collections::HashSet<String> = sorted_values
                .iter()
                .filter(|v| {
                    self.compare_values(v, &lower_bound) >= 0
                        && (i == bucket_count - 1 || self.compare_values(v, &upper_bound) < 0)
                })
                .map(|v| format!("{:?}", v))
                .collect();
            let distinct_count = distinct_values.len() as u64;

            buckets.push(HistogramBucket {
                lower_bound,
                upper_bound,
                count,
                distinct_count,
            });
        }

        Ok(Histogram {
            bucket_count,
            buckets,
            histogram_type: HistogramType::EqualWidth,
        })
    }

    /// Build an equal-depth histogram
    fn build_equal_depth_histogram(&self, values: &[Value]) -> OQLResult<Histogram> {
        let mut sorted_values = values.to_vec();
        sorted_values.sort_by(|a, b| self.compare_values(a, b).cmp(&0));

        let bucket_count = self.config.histogram_buckets.min(sorted_values.len());
        let values_per_bucket = sorted_values.len() / bucket_count;
        let mut buckets = Vec::with_capacity(bucket_count);

        for i in 0..bucket_count {
            let start_idx = i * values_per_bucket;
            let end_idx = if i == bucket_count - 1 {
                sorted_values.len()
            } else {
                (i + 1) * values_per_bucket
            };

            let bucket_values = &sorted_values[start_idx..end_idx];
            if bucket_values.is_empty() {
                continue;
            }

            let lower_bound = bucket_values[0].clone();
            let upper_bound = bucket_values[bucket_values.len() - 1].clone();
            let count = bucket_values.len() as u64;
            let distinct_values: std::collections::HashSet<String> =
                bucket_values.iter().map(|v| format!("{:?}", v)).collect();
            let distinct_count = distinct_values.len() as u64;

            buckets.push(HistogramBucket {
                lower_bound,
                upper_bound,
                count,
                distinct_count,
            });
        }

        Ok(Histogram {
            bucket_count: buckets.len(),
            buckets,
            histogram_type: HistogramType::EqualDepth,
        })
    }

    /// Interpolate a value between min and max
    fn interpolate_value(&self, min: &Value, max: &Value, ratio: f64) -> Value {
        match (min, max) {
            (Value::Integer(min_val), Value::Integer(max_val)) => {
                let range = (*max_val - *min_val) as f64;
                Value::Integer(*min_val + (range * ratio) as i64)
            }
            (Value::Float(min_val), Value::Float(max_val)) => {
                Value::Float(*min_val + (*max_val - *min_val) * ratio)
            }
            _ => min.clone(), // Fallback
        }
    }

    /// Sample documents randomly
    fn sample_documents(&self, documents: &[Document], sample_size: usize) -> Vec<Document> {
        if documents.len() <= sample_size {
            return documents.to_vec();
        }

        // Simple sampling: take every nth document
        let step = documents.len() / sample_size;
        documents
            .iter()
            .step_by(step)
            .take(sample_size)
            .cloned()
            .collect()
    }

    /// Estimate document size in bytes
    fn estimate_document_size(&self, doc: &Document) -> u64 {
        let mut size = 0u64;
        size += doc.id.len() as u64;
        size += doc.schema.len() as u64;
        for (key, value) in &doc.fields {
            size += key.len() as u64;
            size += self.estimate_value_size(value);
        }
        size
    }

    /// Estimate value size in bytes
    fn estimate_value_size(&self, value: &Value) -> u64 {
        match value {
            Value::Null => 1,
            Value::Boolean(_) => 1,
            Value::Integer(_) => 8,
            Value::Float(_) => 8,
            Value::String(s) => s.len() as u64,
            Value::DateTime(_) => 8,
            Value::Reference(_) => 8,
            Value::GlobalReference(_) => 16,
            Value::List(items) => items.iter().map(|v| self.estimate_value_size(v)).sum(),
            Value::Object(fields) => fields
                .iter()
                .map(|(k, v)| k.len() as u64 + self.estimate_value_size(v))
                .sum(),
        }
    }

    /// Record index statistics
    pub fn record_index_statistics(&self, stats: IndexStatistics) -> OQLResult<()> {
        let mut index_stats = self
            .index_stats
            .write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
        index_stats.insert(stats.index_name.clone(), stats);
        Ok(())
    }

    /// Get index statistics
    pub fn get_index_statistics(&self, index_name: &str) -> OQLResult<Option<IndexStatistics>> {
        let stats = self
            .index_stats
            .read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(stats.get(index_name).cloned())
    }

    /// Estimate selectivity for a field predicate
    pub fn estimate_selectivity(&self, schema: &str, field: &str, value: &Value) -> OQLResult<f64> {
        let table_stats = self
            .get_table_statistics(schema)?
            .ok_or_else(|| OQLError::generic(&format!("No statistics for schema '{}'", schema)))?;

        let field_stats = table_stats
            .field_stats
            .get(field)
            .ok_or_else(|| OQLError::generic(&format!("No statistics for field '{}'", field)))?;

        // If we have a histogram, use it for better estimation
        if let Some(histogram) = &field_stats.histogram {
            return Ok(self.estimate_selectivity_from_histogram(histogram, value));
        }

        // Fallback: use distinct count
        if field_stats.distinct_count > 0 {
            Ok(1.0 / field_stats.distinct_count as f64)
        } else {
            Ok(0.1) // Default selectivity
        }
    }

    /// Estimate selectivity from histogram
    fn estimate_selectivity_from_histogram(&self, histogram: &Histogram, value: &Value) -> f64 {
        // Find the bucket containing this value
        for bucket in &histogram.buckets {
            if self.compare_values(value, &bucket.lower_bound) >= 0
                && self.compare_values(value, &bucket.upper_bound) < 0
            {
                // Estimate selectivity within this bucket
                if bucket.distinct_count > 0 {
                    return (bucket.count as f64 / bucket.distinct_count as f64)
                        / histogram.buckets.iter().map(|b| b.count).sum::<u64>() as f64;
                }
            }
        }

        // Value not in histogram range
        0.01 // Very selective
    }

    /// Get cardinality estimate for a schema
    pub fn get_cardinality(&self, schema: &str) -> OQLResult<u64> {
        let stats = self
            .get_table_statistics(schema)?
            .ok_or_else(|| OQLError::generic(&format!("No statistics for schema '{}'", schema)))?;
        Ok(stats.row_count)
    }
}

impl Default for StatisticsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Get current timestamp in seconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::create_storage;
    use std::collections::HashMap;

    fn create_test_documents(count: usize) -> Vec<Document> {
        (0..count)
            .map(|i| {
                let mut fields = HashMap::new();
                fields.insert("id".to_string(), Value::Integer(i as i64));
                fields.insert("name".to_string(), Value::String(format!("User {}", i)));
                fields.insert("age".to_string(), Value::Integer(20 + (i % 50) as i64));
                fields.insert("score".to_string(), Value::Float((i % 100) as f64));

                Document::with_id(format!("user_{}", i), "users".to_string(), fields)
            })
            .collect()
    }

    #[test]
    fn test_statistics_manager_creation() {
        let manager = StatisticsManager::new();
        assert!(manager.get_table_statistics("users").unwrap().is_none());
    }

    #[test]
    fn test_collect_table_statistics() {
        let storage = create_storage("test_stats.owl", 10 * 1024 * 1024).unwrap();
        let manager = StatisticsManager::new();

        // Insert test documents
        let documents = create_test_documents(100);
        for doc in &documents {
            storage.put_document(doc).unwrap();
        }

        // Collect statistics
        let stats = manager
            .collect_table_statistics(storage.as_ref(), "users")
            .unwrap();

        assert_eq!(stats.schema, "users");
        assert_eq!(stats.row_count, 100);
        assert!(stats.avg_row_size > 0);
        assert!(stats.field_stats.contains_key("id"));
        assert!(stats.field_stats.contains_key("name"));
        assert!(stats.field_stats.contains_key("age"));

        std::fs::remove_file("test_stats.owl").ok();
    }

    #[test]
    fn test_field_statistics() {
        let storage = create_storage("test_field_stats.owl", 10 * 1024 * 1024).unwrap();
        let manager = StatisticsManager::new();

        let documents = create_test_documents(50);
        for doc in &documents {
            storage.put_document(doc).unwrap();
        }

        let stats = manager
            .collect_table_statistics(storage.as_ref(), "users")
            .unwrap();
        let age_stats = stats.field_stats.get("age").unwrap();

        assert_eq!(age_stats.field_name, "age");
        assert!(age_stats.distinct_count > 0);
        assert_eq!(age_stats.null_count, 0);
        assert!(age_stats.min_value.is_some());
        assert!(age_stats.max_value.is_some());

        std::fs::remove_file("test_field_stats.owl").ok();
    }

    #[test]
    fn test_histogram_generation() {
        let storage = create_storage("test_histogram.owl", 10 * 1024 * 1024).unwrap();
        let manager = StatisticsManager::new();

        let documents = create_test_documents(100);
        for doc in &documents {
            storage.put_document(doc).unwrap();
        }

        let stats = manager
            .collect_table_statistics(storage.as_ref(), "users")
            .unwrap();
        let age_stats = stats.field_stats.get("age").unwrap();

        assert!(age_stats.histogram.is_some());
        let histogram = age_stats.histogram.as_ref().unwrap();
        assert!(histogram.bucket_count > 0);
        assert_eq!(histogram.buckets.len(), histogram.bucket_count);

        std::fs::remove_file("test_histogram.owl").ok();
    }

    #[test]
    fn test_selectivity_estimation() {
        let storage = create_storage("test_selectivity.owl", 10 * 1024 * 1024).unwrap();
        let manager = StatisticsManager::new();

        let documents = create_test_documents(100);
        for doc in &documents {
            storage.put_document(doc).unwrap();
        }

        manager
            .collect_table_statistics(storage.as_ref(), "users")
            .unwrap();

        let selectivity = manager
            .estimate_selectivity("users", "age", &Value::Integer(25))
            .unwrap();

        assert!(selectivity > 0.0);
        assert!(selectivity <= 1.0);

        std::fs::remove_file("test_selectivity.owl").ok();
    }

    #[test]
    fn test_cardinality_estimation() {
        let storage = create_storage("test_cardinality.owl", 10 * 1024 * 1024).unwrap();
        let manager = StatisticsManager::new();

        let documents = create_test_documents(75);
        for doc in &documents {
            storage.put_document(doc).unwrap();
        }

        manager
            .collect_table_statistics(storage.as_ref(), "users")
            .unwrap();

        let cardinality = manager.get_cardinality("users").unwrap();
        assert_eq!(cardinality, 75);

        std::fs::remove_file("test_cardinality.owl").ok();
    }

    #[test]
    fn test_index_statistics() {
        let manager = StatisticsManager::new();

        let index_stats = IndexStatistics {
            index_name: "users_age_idx".to_string(),
            schema: "users".to_string(),
            fields: vec!["age".to_string()],
            entry_count: 1000,
            avg_key_size: 8,
            selectivity: 0.02,
            lookup_count: 500,
            scan_count: 50,
        };

        manager
            .record_index_statistics(index_stats.clone())
            .unwrap();

        let retrieved = manager.get_index_statistics("users_age_idx").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().entry_count, 1000);
    }
}
