// Query Optimizer Module
//
// This module provides query optimization capabilities including:
// - Statistics collection for cost estimation
// - Histogram generation for value distribution analysis
// - Cost-based query optimization
// - Query plan optimization rules

pub mod cost_model;
pub mod histogram;
pub mod rules;
pub mod statistics;

// Re-export commonly used types
pub use statistics::{
    FieldStatistics, Histogram, HistogramBucket, HistogramType, IndexStatistics, StatisticsConfig,
    StatisticsManager, TableStatistics,
};

pub use histogram::{
    compare_values, estimate_range_selectivity, estimate_selectivity_from_histogram,
    interpolate_value, HistogramBuilder,
};

pub use cost_model::{CostEstimate, CostModel, CostParameters};

pub use rules::{OptimizationStats, QueryOptimizer};
