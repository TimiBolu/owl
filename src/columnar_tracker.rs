// Task 11.1: Hot Column Detection (Rust)
// Track column access patterns in OQL query execution

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Column access statistics
#[derive(Debug, Clone)]
pub struct ColumnAccessStats {
    /// Column name (field path, e.g., "profile.age")
    pub column_name: String,
    /// Schema type tag
    pub type_tag: u16,
    /// Total access count
    pub access_count: u64,
    /// Last access timestamp
    pub last_access: Instant,
    /// Access frequency (accesses per second)
    pub access_frequency: f64,
    /// Average query selectivity (rows returned / total rows)
    pub avg_selectivity: f64,
    /// Total bytes scanned
    pub bytes_scanned: u64,
}

impl ColumnAccessStats {
    pub fn new(column_name: String, type_tag: u16) -> Self {
        Self {
            column_name,
            type_tag,
            access_count: 0,
            last_access: Instant::now(),
            access_frequency: 0.0,
            avg_selectivity: 0.0,
            bytes_scanned: 0,
        }
    }

    /// Record a column access
    pub fn record_access(&mut self, rows_returned: u64, total_rows: u64, bytes: u64) {
        self.access_count += 1;
        self.last_access = Instant::now();
        self.bytes_scanned += bytes;

        // Update average selectivity with exponential moving average
        let selectivity = if total_rows > 0 {
            rows_returned as f64 / total_rows as f64
        } else {
            0.0
        };

        const ALPHA: f64 = 0.2; // Smoothing factor
        self.avg_selectivity = ALPHA * selectivity + (1.0 - ALPHA) * self.avg_selectivity;
    }

    /// Update access frequency based on time window
    pub fn update_frequency(&mut self, window: Duration) {
        let elapsed = self.last_access.elapsed();
        if elapsed < window {
            self.access_frequency = self.access_count as f64 / window.as_secs_f64();
        } else {
            // Decay frequency for inactive columns
            self.access_frequency *= 0.9;
        }
    }

    /// Calculate hotness score (0.0 to 1.0)
    /// Higher score = hotter column = better candidate for materialization
    pub fn hotness_score(&self) -> f64 {
        // Factors:
        // 1. Access frequency (40% weight)
        // 2. Recency (30% weight)
        // 3. Selectivity (20% weight) - lower selectivity = more rows scanned = hotter
        // 4. Bytes scanned (10% weight)

        let frequency_score = (self.access_frequency / 10.0).min(1.0); // Normalize to 10 accesses/sec

        let recency_score = {
            let elapsed = self.last_access.elapsed().as_secs_f64();
            (1.0 / (1.0 + elapsed / 60.0)).min(1.0) // Decay over 1 minute
        };

        let selectivity_score = 1.0 - self.avg_selectivity; // Lower selectivity = higher score

        let bytes_score = (self.bytes_scanned as f64 / 1_000_000.0).min(1.0); // Normalize to 1MB

        0.4 * frequency_score + 0.3 * recency_score + 0.2 * selectivity_score + 0.1 * bytes_score
    }

    /// Check if column should be materialized
    pub fn should_materialize(&self, threshold: f64) -> bool {
        self.hotness_score() >= threshold
    }
}

/// Hot column tracker for automatic materialization
pub struct HotColumnTracker {
    /// Column access statistics by (type_tag, column_name)
    stats: Arc<RwLock<HashMap<(u16, String), ColumnAccessStats>>>,
    /// Materialization threshold (0.0 to 1.0)
    materialization_threshold: f64,
    /// Time window for frequency calculation
    frequency_window: Duration,
}

impl HotColumnTracker {
    /// Create a new hot column tracker
    pub fn new(materialization_threshold: f64) -> Self {
        Self {
            stats: Arc::new(RwLock::new(HashMap::new())),
            materialization_threshold,
            frequency_window: Duration::from_secs(60), // 1 minute window
        }
    }

    /// Record a column access from OQL query execution
    pub fn record_access(
        &self,
        type_tag: u16,
        column_name: &str,
        rows_returned: u64,
        total_rows: u64,
        bytes_scanned: u64,
    ) {
        let mut stats = self.stats.write().unwrap();
        let key = (type_tag, column_name.to_string());

        let entry = stats
            .entry(key.clone())
            .or_insert_with(|| ColumnAccessStats::new(column_name.to_string(), type_tag));

        entry.record_access(rows_returned, total_rows, bytes_scanned);
    }

    /// Get hot columns that should be materialized
    pub fn get_hot_columns(&self, type_tag: Option<u16>) -> Vec<ColumnAccessStats> {
        let mut stats = self.stats.write().unwrap();

        // Update frequencies
        for stat in stats.values_mut() {
            stat.update_frequency(self.frequency_window);
        }

        // Filter and collect hot columns
        stats
            .values()
            .filter(|stat| {
                if let Some(tag) = type_tag {
                    stat.type_tag == tag && stat.should_materialize(self.materialization_threshold)
                } else {
                    stat.should_materialize(self.materialization_threshold)
                }
            })
            .cloned()
            .collect()
    }

    /// Get all column statistics
    pub fn get_all_stats(&self) -> Vec<ColumnAccessStats> {
        let stats = self.stats.read().unwrap();
        stats.values().cloned().collect()
    }

    /// Get statistics for a specific column
    pub fn get_column_stats(&self, type_tag: u16, column_name: &str) -> Option<ColumnAccessStats> {
        let stats = self.stats.read().unwrap();
        stats.get(&(type_tag, column_name.to_string())).cloned()
    }

    /// Clear statistics for a type (called when schema changes)
    pub fn clear_type(&self, type_tag: u16) {
        let mut stats = self.stats.write().unwrap();
        stats.retain(|(tag, _), _| *tag != type_tag);
    }

    /// Clear all statistics
    pub fn clear_all(&self) {
        let mut stats = self.stats.write().unwrap();
        stats.clear();
    }

    /// Set materialization threshold
    pub fn set_threshold(&mut self, threshold: f64) {
        self.materialization_threshold = threshold.clamp(0.0, 1.0);
    }

    /// Get current materialization threshold
    pub fn get_threshold(&self) -> f64 {
        self.materialization_threshold
    }
}

/// Materialization trigger for automatic projection creation
pub struct MaterializationTrigger {
    tracker: Arc<HotColumnTracker>,
    check_interval: Duration,
}

impl MaterializationTrigger {
    pub fn new(tracker: Arc<HotColumnTracker>, check_interval: Duration) -> Self {
        Self {
            tracker,
            check_interval,
        }
    }

    /// Check for columns that need materialization
    /// Returns list of (type_tag, column_names) that should be materialized
    pub fn check_materialization_needed(&self) -> Vec<(u16, Vec<String>)> {
        let hot_columns = self.tracker.get_hot_columns(None);

        // Group by type_tag
        let mut by_type: HashMap<u16, Vec<String>> = HashMap::new();
        for stat in hot_columns {
            by_type
                .entry(stat.type_tag)
                .or_insert_with(Vec::new)
                .push(stat.column_name);
        }

        by_type.into_iter().collect()
    }

    /// Get check interval
    pub fn get_check_interval(&self) -> Duration {
        self.check_interval
    }
}

/// Invalidation logic for columnar projections
pub struct ProjectionInvalidator {
    /// Track which projections exist for each type
    projections: Arc<RwLock<HashMap<u16, Vec<String>>>>,
}

impl ProjectionInvalidator {
    pub fn new() -> Self {
        Self {
            projections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a projection
    pub fn register_projection(&self, type_tag: u16, column_names: Vec<String>) {
        let mut projections = self.projections.write().unwrap();
        projections.insert(type_tag, column_names);
    }

    /// Invalidate projections for a type (called on insert/update/delete)
    pub fn invalidate_type(&self, type_tag: u16) -> Vec<String> {
        let projections = self.projections.read().unwrap();
        projections.get(&type_tag).cloned().unwrap_or_default()
    }

    /// Check if a projection exists for a type
    pub fn has_projection(&self, type_tag: u16) -> bool {
        let projections = self.projections.read().unwrap();
        projections.contains_key(&type_tag)
    }

    /// Get all projection column names for a type
    pub fn get_projection_columns(&self, type_tag: u16) -> Option<Vec<String>> {
        let projections = self.projections.read().unwrap();
        projections.get(&type_tag).cloned()
    }

    /// Remove projection registration
    pub fn remove_projection(&self, type_tag: u16) {
        let mut projections = self.projections.write().unwrap();
        projections.remove(&type_tag);
    }
}

impl Default for ProjectionInvalidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_access_stats_creation() {
        let stats = ColumnAccessStats::new("profile.age".to_string(), 1);
        assert_eq!(stats.column_name, "profile.age");
        assert_eq!(stats.type_tag, 1);
        assert_eq!(stats.access_count, 0);
    }

    #[test]
    fn test_record_access() {
        let mut stats = ColumnAccessStats::new("profile.age".to_string(), 1);
        stats.record_access(50, 100, 1000);

        assert_eq!(stats.access_count, 1);
        assert_eq!(stats.bytes_scanned, 1000);
        assert!(stats.avg_selectivity > 0.0);
    }

    #[test]
    fn test_hotness_score() {
        let mut stats = ColumnAccessStats::new("profile.age".to_string(), 1);

        // Record multiple accesses
        for _ in 0..10 {
            stats.record_access(10, 100, 1000);
        }

        stats.update_frequency(Duration::from_secs(60));

        let score = stats.hotness_score();
        assert!(score > 0.0 && score <= 1.0);
    }

    #[test]
    fn test_hot_column_tracker() {
        let tracker = HotColumnTracker::new(0.5);

        // Record accesses
        for _ in 0..20 {
            tracker.record_access(1, "profile.age", 10, 100, 1000);
        }

        let hot_columns = tracker.get_hot_columns(Some(1));
        assert!(!hot_columns.is_empty());
    }

    #[test]
    fn test_materialization_trigger() {
        let tracker = Arc::new(HotColumnTracker::new(0.3));
        let trigger = MaterializationTrigger::new(tracker.clone(), Duration::from_secs(10));

        // Record many accesses to make column hot
        for _ in 0..50 {
            tracker.record_access(1, "profile.age", 10, 100, 1000);
        }

        let needed = trigger.check_materialization_needed();
        assert!(!needed.is_empty());
    }

    #[test]
    fn test_projection_invalidator() {
        let invalidator = ProjectionInvalidator::new();

        invalidator.register_projection(1, vec!["age".to_string(), "name".to_string()]);

        assert!(invalidator.has_projection(1));

        let columns = invalidator.invalidate_type(1);
        assert_eq!(columns.len(), 2);

        invalidator.remove_projection(1);
        assert!(!invalidator.has_projection(1));
    }

    #[test]
    fn test_clear_type() {
        let tracker = HotColumnTracker::new(0.5);

        tracker.record_access(1, "col1", 10, 100, 1000);
        tracker.record_access(2, "col2", 10, 100, 1000);

        tracker.clear_type(1);

        let stats = tracker.get_all_stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].type_tag, 2);
    }
}
