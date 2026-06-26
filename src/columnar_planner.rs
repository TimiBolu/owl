// Task 11.4: Integrate with OQL Query Planner (Rust)
// Detect analytical queries and use columnar storage when beneficial

use crate::columnar_tracker::{HotColumnTracker, ProjectionInvalidator};
use std::sync::Arc;

/// Query type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    /// OLTP query: point lookups, small result sets, navigation
    Transactional,
    /// OLAP query: scans, aggregations, large result sets
    Analytical,
    /// Hybrid: mix of both
    Hybrid,
}

/// Query characteristics for classification
#[derive(Debug, Clone)]
pub struct QueryCharacteristics {
    /// Number of columns accessed
    pub column_count: usize,
    /// Estimated rows to scan
    pub estimated_rows: u64,
    /// Has aggregation (COUNT, SUM, AVG, etc.)
    pub has_aggregation: bool,
    /// Has GROUP BY clause
    pub has_group_by: bool,
    /// Has ORDER BY clause
    pub has_order_by: bool,
    /// Selectivity (estimated rows returned / total rows)
    pub selectivity: f64,
    /// Has reference navigation (.user.orders[0].items)
    pub has_navigation: bool,
    /// Number of joins
    pub join_count: usize,
}

impl QueryCharacteristics {
    /// Classify query type based on characteristics
    pub fn classify(&self) -> QueryType {
        // Analytical query indicators:
        // - High row scan count (> 1000 rows)
        // - Aggregations
        // - GROUP BY
        // - Low selectivity (< 0.1)
        // - Multiple columns accessed

        let analytical_score = {
            let mut score = 0.0;

            if self.estimated_rows > 1000 {
                score += 0.3;
            }

            if self.has_aggregation {
                score += 0.25;
            }

            if self.has_group_by {
                score += 0.2;
            }

            if self.selectivity < 0.1 {
                score += 0.15;
            }

            if self.column_count > 3 {
                score += 0.1;
            }

            score
        };

        // Transactional query indicators:
        // - Reference navigation
        // - High selectivity (> 0.9)
        // - Low row count (< 100)
        // - No aggregations

        let transactional_score = {
            let mut score = 0.0;

            if self.has_navigation {
                score += 0.4;
            }

            if self.selectivity > 0.9 {
                score += 0.3;
            }

            if self.estimated_rows < 100 {
                score += 0.2;
            }

            if !self.has_aggregation && !self.has_group_by {
                score += 0.1;
            }

            score
        };

        if analytical_score > 0.6 {
            QueryType::Analytical
        } else if transactional_score > 0.6 {
            QueryType::Transactional
        } else {
            QueryType::Hybrid
        }
    }
}

/// Execution plan type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPlan {
    /// Use row-oriented storage (standard navigation)
    RowOriented,
    /// Use columnar projection
    ColumnarProjection { projection_id: String },
    /// Use hybrid approach (row + column)
    Hybrid {
        row_columns: Vec<String>,
        columnar_columns: Vec<String>,
    },
}

/// Columnar query planner
#[allow(dead_code)]
pub struct ColumnarQueryPlanner {
    /// Hot column tracker
    tracker: Arc<HotColumnTracker>,
    /// Projection invalidator
    invalidator: Arc<ProjectionInvalidator>,
    /// Cost model parameters
    row_scan_cost: f64,
    columnar_scan_cost: f64,
    materialization_cost: f64,
}

impl ColumnarQueryPlanner {
    pub fn new(tracker: Arc<HotColumnTracker>, invalidator: Arc<ProjectionInvalidator>) -> Self {
        Self {
            tracker,
            invalidator,
            row_scan_cost: 1.0,          // Base cost for row scan
            columnar_scan_cost: 0.1,     // 10x faster for columnar scan
            materialization_cost: 100.0, // One-time cost to materialize
        }
    }

    /// Plan query execution based on characteristics
    pub fn plan_query(
        &self,
        type_tag: u16,
        characteristics: &QueryCharacteristics,
        columns: &[String],
    ) -> ExecutionPlan {
        let query_type = characteristics.classify();

        match query_type {
            QueryType::Transactional => {
                // Always use row-oriented for transactional queries
                ExecutionPlan::RowOriented
            }
            QueryType::Analytical => {
                // Check if columnar projection exists
                if let Some(proj_columns) = self.invalidator.get_projection_columns(type_tag) {
                    // Check if projection covers requested columns
                    let covered = columns.iter().all(|col| proj_columns.contains(col));

                    if covered {
                        return ExecutionPlan::ColumnarProjection {
                            projection_id: format!("proj_{}_{}", type_tag, proj_columns.join("_")),
                        };
                    }
                }

                // No projection available - use row-oriented
                ExecutionPlan::RowOriented
            }
            QueryType::Hybrid => {
                // Use cost-based optimization
                self.cost_based_plan(type_tag, characteristics, columns)
            }
        }
    }

    /// Cost-based optimization for hybrid queries
    fn cost_based_plan(
        &self,
        type_tag: u16,
        characteristics: &QueryCharacteristics,
        columns: &[String],
    ) -> ExecutionPlan {
        // Calculate cost of row-oriented execution
        let row_cost = self.row_scan_cost * characteristics.estimated_rows as f64;

        // Calculate cost of columnar execution (if projection exists)
        if let Some(proj_columns) = self.invalidator.get_projection_columns(type_tag) {
            let covered_columns: Vec<_> = columns
                .iter()
                .filter(|col| proj_columns.contains(col))
                .cloned()
                .collect();

            let uncovered_columns: Vec<_> = columns
                .iter()
                .filter(|col| !proj_columns.contains(col))
                .cloned()
                .collect();

            if !covered_columns.is_empty() {
                let columnar_cost = self.columnar_scan_cost * characteristics.estimated_rows as f64;
                let row_cost_for_uncovered =
                    self.row_scan_cost * characteristics.estimated_rows as f64 * 0.5; // Partial scan

                let hybrid_cost = columnar_cost + row_cost_for_uncovered;

                if hybrid_cost < row_cost {
                    return ExecutionPlan::Hybrid {
                        row_columns: uncovered_columns,
                        columnar_columns: covered_columns,
                    };
                }
            }
        }

        // Default to row-oriented
        ExecutionPlan::RowOriented
    }

    /// Estimate speedup from using columnar storage
    pub fn estimate_speedup(
        &self,
        type_tag: u16,
        characteristics: &QueryCharacteristics,
        columns: &[String],
    ) -> f64 {
        let plan = self.plan_query(type_tag, characteristics, columns);

        match plan {
            ExecutionPlan::RowOriented => 1.0, // No speedup
            ExecutionPlan::ColumnarProjection { .. } => {
                // Columnar is ~10x faster for analytical queries
                self.row_scan_cost / self.columnar_scan_cost
            }
            ExecutionPlan::Hybrid { .. } => {
                // Hybrid is ~3-5x faster
                3.0
            }
        }
    }

    /// Check if query should trigger materialization
    pub fn should_materialize(
        &self,
        type_tag: u16,
        characteristics: &QueryCharacteristics,
        columns: &[String],
    ) -> bool {
        // Only materialize for analytical queries
        if characteristics.classify() != QueryType::Analytical {
            return false;
        }

        // Check if projection already exists
        if self.invalidator.has_projection(type_tag) {
            return false;
        }

        // Check if columns are hot
        let hot_columns = self.tracker.get_hot_columns(Some(type_tag));
        let hot_column_names: Vec<_> = hot_columns.iter().map(|s| s.column_name.as_str()).collect();

        // Materialize if at least 50% of requested columns are hot
        let hot_count = columns
            .iter()
            .filter(|col| hot_column_names.contains(&col.as_str()))
            .count();

        hot_count >= columns.len() / 2
    }

    /// Record query execution for tracking
    pub fn record_query_execution(
        &self,
        type_tag: u16,
        columns: &[String],
        rows_returned: u64,
        total_rows: u64,
        bytes_scanned: u64,
    ) {
        for column in columns {
            self.tracker
                .record_access(type_tag, column, rows_returned, total_rows, bytes_scanned);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columnar_tracker::HotColumnTracker;

    #[test]
    fn test_query_classification_analytical() {
        let chars = QueryCharacteristics {
            column_count: 5,
            estimated_rows: 10000,
            has_aggregation: true,
            has_group_by: true,
            has_order_by: false,
            selectivity: 0.05,
            has_navigation: false,
            join_count: 0,
        };

        assert_eq!(chars.classify(), QueryType::Analytical);
    }

    #[test]
    fn test_query_classification_transactional() {
        let chars = QueryCharacteristics {
            column_count: 2,
            estimated_rows: 10,
            has_aggregation: false,
            has_group_by: false,
            has_order_by: false,
            selectivity: 0.95,
            has_navigation: true,
            join_count: 0,
        };

        assert_eq!(chars.classify(), QueryType::Transactional);
    }

    #[test]
    fn test_query_classification_hybrid() {
        let chars = QueryCharacteristics {
            column_count: 3,
            estimated_rows: 500,
            has_aggregation: false,
            has_group_by: false,
            has_order_by: true,
            selectivity: 0.5,
            has_navigation: false,
            join_count: 1,
        };

        assert_eq!(chars.classify(), QueryType::Hybrid);
    }

    #[test]
    fn test_planner_transactional_query() {
        let tracker = Arc::new(HotColumnTracker::new(0.5));
        let invalidator = Arc::new(ProjectionInvalidator::new());
        let planner = ColumnarQueryPlanner::new(tracker, invalidator);

        let chars = QueryCharacteristics {
            column_count: 2,
            estimated_rows: 10,
            has_aggregation: false,
            has_group_by: false,
            has_order_by: false,
            selectivity: 0.95,
            has_navigation: true,
            join_count: 0,
        };

        let plan = planner.plan_query(1, &chars, &["id".to_string(), "name".to_string()]);
        assert_eq!(plan, ExecutionPlan::RowOriented);
    }

    #[test]
    fn test_planner_analytical_query_with_projection() {
        let tracker = Arc::new(HotColumnTracker::new(0.5));
        let invalidator = Arc::new(ProjectionInvalidator::new());

        // Register a projection
        invalidator.register_projection(1, vec!["age".to_string(), "salary".to_string()]);

        let planner = ColumnarQueryPlanner::new(tracker, invalidator);

        let chars = QueryCharacteristics {
            column_count: 2,
            estimated_rows: 10000,
            has_aggregation: true,
            has_group_by: false,
            has_order_by: false,
            selectivity: 0.1,
            has_navigation: false,
            join_count: 0,
        };

        let plan = planner.plan_query(1, &chars, &["age".to_string(), "salary".to_string()]);

        match plan {
            ExecutionPlan::ColumnarProjection { .. } => {
                // Success
            }
            _ => panic!("Expected columnar projection plan"),
        }
    }

    #[test]
    fn test_estimate_speedup() {
        let tracker = Arc::new(HotColumnTracker::new(0.5));
        let invalidator = Arc::new(ProjectionInvalidator::new());

        invalidator.register_projection(1, vec!["age".to_string()]);

        let planner = ColumnarQueryPlanner::new(tracker, invalidator);

        let chars = QueryCharacteristics {
            column_count: 1,
            estimated_rows: 10000,
            has_aggregation: true,
            has_group_by: false,
            has_order_by: false,
            selectivity: 0.1,
            has_navigation: false,
            join_count: 0,
        };

        let speedup = planner.estimate_speedup(1, &chars, &["age".to_string()]);
        assert!(speedup > 1.0);
    }

    #[test]
    fn test_should_materialize() {
        let tracker = Arc::new(HotColumnTracker::new(0.3));
        let invalidator = Arc::new(ProjectionInvalidator::new());

        // Make columns hot
        for _ in 0..50 {
            tracker.record_access(1, "age", 100, 10000, 10000);
            tracker.record_access(1, "salary", 100, 10000, 10000);
        }

        let planner = ColumnarQueryPlanner::new(tracker, invalidator);

        let chars = QueryCharacteristics {
            column_count: 2,
            estimated_rows: 10000,
            has_aggregation: true,
            has_group_by: false,
            has_order_by: false,
            selectivity: 0.1,
            has_navigation: false,
            join_count: 0,
        };

        let should_mat =
            planner.should_materialize(1, &chars, &["age".to_string(), "salary".to_string()]);

        assert!(should_mat);
    }
}
