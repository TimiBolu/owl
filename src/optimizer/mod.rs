// Query Optimizer Module
//
// This module provides query optimization capabilities including:
// - Statistics collection for cost estimation
// - Histogram generation for value distribution analysis
// - Cost-based query optimization
// - Query plan optimization rules

pub mod statistics;
pub mod histogram;
pub mod cost_model;
pub mod rules;

// Re-export commonly used types
pub use statistics::{
    StatisticsManager, TableStatistics, FieldStatistics, IndexStatistics,
    StatisticsConfig, Histogram, HistogramBucket, HistogramType,
};

pub use histogram::{
    HistogramBuilder, compare_values, interpolate_value,
    estimate_selectivity_from_histogram, estimate_range_selectivity,
};

pub use cost_model::{
    CostModel, CostParameters, CostEstimate,
};

pub use rules::{
    QueryOptimizer, OptimizationStats,
};
