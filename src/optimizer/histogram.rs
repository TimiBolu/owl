// Histogram utilities for statistics collection
//
// This module provides histogram-related utilities for value distribution analysis.

use crate::document::Value;
use crate::error::{OQLError, OQLResult};

pub use crate::optimizer::statistics::{Histogram, HistogramBucket, HistogramType};

/// Histogram builder for creating histograms from value distributions
pub struct HistogramBuilder {
    bucket_count: usize,
    histogram_type: HistogramType,
}

impl HistogramBuilder {
    /// Create a new histogram builder
    pub fn new(bucket_count: usize, histogram_type: HistogramType) -> Self {
        Self {
            bucket_count,
            histogram_type,
        }
    }
    
    /// Build a histogram from values
    pub fn build(&self, values: &[Value]) -> OQLResult<Histogram> {
        if values.is_empty() {
            return Err(OQLError::generic("Cannot build histogram from empty values"));
        }
        
        match self.histogram_type {
            HistogramType::EqualWidth => self.build_equal_width(values),
            HistogramType::EqualDepth => self.build_equal_depth(values),
        }
    }
    
    /// Build an equal-width histogram
    fn build_equal_width(&self, values: &[Value]) -> OQLResult<Histogram> {
        let mut sorted_values = values.to_vec();
        sorted_values.sort_by(|a, b| compare_values(a, b).cmp(&0));
        
        let min = &sorted_values[0];
        let max = &sorted_values[sorted_values.len() - 1];
        
        let bucket_count = self.bucket_count.min(sorted_values.len());
        let mut buckets = Vec::with_capacity(bucket_count);
        
        for i in 0..bucket_count {
            let lower_bound = if i == 0 {
                min.clone()
            } else {
                interpolate_value(min, max, i as f64 / bucket_count as f64)
            };
            
            let upper_bound = if i == bucket_count - 1 {
                max.clone()
            } else {
                interpolate_value(min, max, (i + 1) as f64 / bucket_count as f64)
            };
            
            let count = sorted_values.iter()
                .filter(|v| {
                    compare_values(v, &lower_bound) >= 0
                        && (i == bucket_count - 1 || compare_values(v, &upper_bound) < 0)
                })
                .count() as u64;
            
            let distinct_values: std::collections::HashSet<String> = sorted_values.iter()
                .filter(|v| {
                    compare_values(v, &lower_bound) >= 0
                        && (i == bucket_count - 1 || compare_values(v, &upper_bound) < 0)
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
    fn build_equal_depth(&self, values: &[Value]) -> OQLResult<Histogram> {
        let mut sorted_values = values.to_vec();
        sorted_values.sort_by(|a, b| compare_values(a, b).cmp(&0));
        
        let bucket_count = self.bucket_count.min(sorted_values.len());
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
            let distinct_values: std::collections::HashSet<String> = bucket_values.iter()
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
            bucket_count: buckets.len(),
            buckets,
            histogram_type: HistogramType::EqualDepth,
        })
    }
}

/// Compare two values for ordering
pub fn compare_values(a: &Value, b: &Value) -> i32 {
    match (a, b) {
        (Value::Integer(a), Value::Integer(b)) => a.cmp(b) as i32,
        (Value::Float(a), Value::Float(b)) => {
            if a < b { -1 } else if a > b { 1 } else { 0 }
        }
        (Value::String(a), Value::String(b)) => a.cmp(b) as i32,
        (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b) as i32,
        _ => 0,
    }
}

/// Interpolate a value between min and max
pub fn interpolate_value(min: &Value, max: &Value, ratio: f64) -> Value {
    match (min, max) {
        (Value::Integer(min_val), Value::Integer(max_val)) => {
            let range = (*max_val - *min_val) as f64;
            Value::Integer(*min_val + (range * ratio) as i64)
        }
        (Value::Float(min_val), Value::Float(max_val)) => {
            Value::Float(*min_val + (*max_val - *min_val) * ratio)
        }
        _ => min.clone(),
    }
}

/// Estimate selectivity from a histogram for a given value
pub fn estimate_selectivity_from_histogram(histogram: &Histogram, value: &Value) -> f64 {
    for bucket in &histogram.buckets {
        if compare_values(value, &bucket.lower_bound) >= 0
            && compare_values(value, &bucket.upper_bound) < 0
        {
            if bucket.distinct_count > 0 {
                return (bucket.count as f64 / bucket.distinct_count as f64)
                    / histogram.buckets.iter().map(|b| b.count).sum::<u64>() as f64;
            }
        }
    }
    
    0.01 // Value not in histogram range - very selective
}

/// Estimate range selectivity from a histogram
pub fn estimate_range_selectivity(
    histogram: &Histogram,
    lower: Option<&Value>,
    upper: Option<&Value>,
) -> f64 {
    let total_count: u64 = histogram.buckets.iter().map(|b| b.count).sum();
    if total_count == 0 {
        return 0.0;
    }
    
    let mut matching_count = 0u64;
    
    for bucket in &histogram.buckets {
        let bucket_matches = match (lower, upper) {
            (Some(l), Some(u)) => {
                // Range query: lower <= value < upper
                compare_values(&bucket.upper_bound, l) > 0
                    && compare_values(&bucket.lower_bound, u) < 0
            }
            (Some(l), None) => {
                // Lower bound only: value >= lower
                compare_values(&bucket.upper_bound, l) > 0
            }
            (None, Some(u)) => {
                // Upper bound only: value < upper
                compare_values(&bucket.lower_bound, u) < 0
            }
            (None, None) => true, // No bounds - all values match
        };
        
        if bucket_matches {
            matching_count += bucket.count;
        }
    }
    
    matching_count as f64 / total_count as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_histogram_builder_equal_width() {
        let values: Vec<Value> = (0..100).map(|i| Value::Integer(i)).collect();
        let builder = HistogramBuilder::new(10, HistogramType::EqualWidth);
        
        let histogram = builder.build(&values).unwrap();
        
        assert_eq!(histogram.bucket_count, 10);
        assert_eq!(histogram.buckets.len(), 10);
        assert_eq!(histogram.histogram_type, HistogramType::EqualWidth);
    }

    #[test]
    fn test_histogram_builder_equal_depth() {
        let values: Vec<Value> = (0..100).map(|i| Value::Integer(i)).collect();
        let builder = HistogramBuilder::new(10, HistogramType::EqualDepth);
        
        let histogram = builder.build(&values).unwrap();
        
        assert_eq!(histogram.bucket_count, 10);
        assert_eq!(histogram.buckets.len(), 10);
        assert_eq!(histogram.histogram_type, HistogramType::EqualDepth);
        
        // Each bucket should have roughly the same count
        for bucket in &histogram.buckets {
            assert!(bucket.count >= 8 && bucket.count <= 12); // Allow some variance
        }
    }

    #[test]
    fn test_compare_values() {
        assert_eq!(compare_values(&Value::Integer(5), &Value::Integer(10)), -1);
        assert_eq!(compare_values(&Value::Integer(10), &Value::Integer(5)), 1);
        assert_eq!(compare_values(&Value::Integer(5), &Value::Integer(5)), 0);
        
        assert_eq!(compare_values(&Value::Float(5.5), &Value::Float(10.5)), -1);
        assert_eq!(compare_values(&Value::String("a".to_string()), &Value::String("b".to_string())), -1);
    }

    #[test]
    fn test_interpolate_value() {
        let min = Value::Integer(0);
        let max = Value::Integer(100);
        
        let mid = interpolate_value(&min, &max, 0.5);
        assert_eq!(mid, Value::Integer(50));
        
        let quarter = interpolate_value(&min, &max, 0.25);
        assert_eq!(quarter, Value::Integer(25));
    }

    #[test]
    fn test_estimate_selectivity() {
        let values: Vec<Value> = (0..100).map(|i| Value::Integer(i)).collect();
        let builder = HistogramBuilder::new(10, HistogramType::EqualDepth);
        let histogram = builder.build(&values).unwrap();
        
        let selectivity = estimate_selectivity_from_histogram(&histogram, &Value::Integer(50));
        assert!(selectivity > 0.0);
        assert!(selectivity <= 1.0);
    }

    #[test]
    fn test_estimate_range_selectivity() {
        let values: Vec<Value> = (0..100).map(|i| Value::Integer(i)).collect();
        let builder = HistogramBuilder::new(10, HistogramType::EqualDepth);
        let histogram = builder.build(&values).unwrap();
        
        // Range: 25 <= value < 75 (should be ~50% selective)
        let selectivity = estimate_range_selectivity(
            &histogram,
            Some(&Value::Integer(25)),
            Some(&Value::Integer(75)),
        );
        assert!(selectivity > 0.4 && selectivity < 0.6);
        
        // Lower bound only: value >= 50 (should be ~50% selective)
        let selectivity = estimate_range_selectivity(
            &histogram,
            Some(&Value::Integer(50)),
            None,
        );
        assert!(selectivity > 0.4 && selectivity < 0.6);
    }
}
