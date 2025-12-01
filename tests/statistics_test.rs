// Statistics Collection Tests
//
// Tests for the statistics collection module including:
// - Table statistics collection
// - Field statistics
// - Histogram generation
// - Selectivity estimation
// - Cardinality estimation

use owl_db::document::{Document, Value};
use owl_db::optimizer::{
    StatisticsManager, StatisticsConfig, HistogramType, HistogramBuilder,
    IndexStatistics, estimate_selectivity_from_histogram, estimate_range_selectivity,
};
use owl_db::storage::create_storage;
use std::collections::HashMap;

fn create_test_documents(count: usize) -> Vec<Document> {
    (0..count)
        .map(|i| {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), Value::Integer(i as i64));
            fields.insert("name".to_string(), Value::String(format!("User {}", i)));
            fields.insert("age".to_string(), Value::Integer(20 + (i % 50) as i64));
            fields.insert("score".to_string(), Value::Float((i % 100) as f64));
            fields.insert("active".to_string(), Value::Boolean(i % 2 == 0));
            
            Document::with_id(
                format!("user_{}", i),
                "users".to_string(),
                fields,
            )
        })
        .collect()
}

#[test]
fn test_statistics_manager_creation() {
    let manager = StatisticsManager::new();
    assert!(manager.get_table_statistics("users").unwrap().is_none());
}

#[test]
fn test_statistics_manager_with_config() {
    let config = StatisticsConfig {
        histogram_buckets: 50,
        histogram_type: HistogramType::EqualWidth,
        sample_size: 5000,
        auto_update_threshold: 500,
    };
    
    let manager = StatisticsManager::with_config(config);
    assert!(manager.get_table_statistics("users").unwrap().is_none());
}

#[test]
fn test_collect_table_statistics_basic() {
    let storage = create_storage("test_stats_basic.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    // Insert test documents
    let documents = create_test_documents(100);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    // Collect statistics
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    assert_eq!(stats.schema, "users");
    assert_eq!(stats.row_count, 100);
    assert!(stats.avg_row_size > 0);
    assert!(stats.total_size > 0);
    assert!(stats.last_updated > 0);
    
    // Check field statistics exist
    assert!(stats.field_stats.contains_key("id"));
    assert!(stats.field_stats.contains_key("name"));
    assert!(stats.field_stats.contains_key("age"));
    assert!(stats.field_stats.contains_key("score"));
    assert!(stats.field_stats.contains_key("active"));
    
    std::fs::remove_file("test_stats_basic.owl").ok();
}

#[test]
fn test_collect_table_statistics_empty() {
    let storage = create_storage("test_stats_empty.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    // Collect statistics on empty collection
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    assert_eq!(stats.schema, "users");
    assert_eq!(stats.row_count, 0);
    assert_eq!(stats.avg_row_size, 0);
    assert_eq!(stats.total_size, 0);
    assert!(stats.field_stats.is_empty());
    
    std::fs::remove_file("test_stats_empty.owl").ok();
}

#[test]
fn test_field_statistics_integer() {
    let storage = create_storage("test_field_stats_int.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    let documents = create_test_documents(50);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    let age_stats = stats.field_stats.get("age").unwrap();
    
    assert_eq!(age_stats.field_name, "age");
    assert!(age_stats.distinct_count > 0);
    assert!(age_stats.distinct_count <= 50); // At most 50 distinct values
    assert_eq!(age_stats.null_count, 0);
    assert!(age_stats.min_value.is_some());
    assert!(age_stats.max_value.is_some());
    assert!(age_stats.avg_size > 0);
    
    // Check min/max values
    if let Some(Value::Integer(min)) = age_stats.min_value {
        assert!(min >= 20);
    }
    if let Some(Value::Integer(max)) = age_stats.max_value {
        assert!(max < 70);
    }
    
    std::fs::remove_file("test_field_stats_int.owl").ok();
}

#[test]
fn test_field_statistics_string() {
    let storage = create_storage("test_field_stats_str.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    let documents = create_test_documents(30);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    let name_stats = stats.field_stats.get("name").unwrap();
    
    assert_eq!(name_stats.field_name, "name");
    assert_eq!(name_stats.distinct_count, 30); // All names are unique
    assert_eq!(name_stats.null_count, 0);
    assert!(name_stats.avg_size > 0);
    
    std::fs::remove_file("test_field_stats_str.owl").ok();
}

#[test]
fn test_field_statistics_boolean() {
    let storage = create_storage("test_field_stats_bool.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    let documents = create_test_documents(40);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    let active_stats = stats.field_stats.get("active").unwrap();
    
    assert_eq!(active_stats.field_name, "active");
    assert_eq!(active_stats.distinct_count, 2); // Only true/false
    assert_eq!(active_stats.null_count, 0);
    
    std::fs::remove_file("test_field_stats_bool.owl").ok();
}

#[test]
fn test_histogram_generation_equal_width() {
    let storage = create_storage("test_histogram_ew.owl", 10 * 1024 * 1024).unwrap();
    let config = StatisticsConfig {
        histogram_buckets: 10,
        histogram_type: HistogramType::EqualWidth,
        sample_size: 0, // Full scan
        auto_update_threshold: 1000,
    };
    let manager = StatisticsManager::with_config(config);
    
    let documents = create_test_documents(100);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    let age_stats = stats.field_stats.get("age").unwrap();
    
    assert!(age_stats.histogram.is_some());
    let histogram = age_stats.histogram.as_ref().unwrap();
    assert_eq!(histogram.histogram_type, HistogramType::EqualWidth);
    assert!(histogram.bucket_count > 0);
    assert_eq!(histogram.buckets.len(), histogram.bucket_count);
    
    // Verify buckets have data
    let total_count: u64 = histogram.buckets.iter().map(|b| b.count).sum();
    assert_eq!(total_count, 100);
    
    std::fs::remove_file("test_histogram_ew.owl").ok();
}

#[test]
fn test_histogram_generation_equal_depth() {
    let storage = create_storage("test_histogram_ed.owl", 10 * 1024 * 1024).unwrap();
    let config = StatisticsConfig {
        histogram_buckets: 10,
        histogram_type: HistogramType::EqualDepth,
        sample_size: 0,
        auto_update_threshold: 1000,
    };
    let manager = StatisticsManager::with_config(config);
    
    let documents = create_test_documents(100);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    let age_stats = stats.field_stats.get("age").unwrap();
    
    assert!(age_stats.histogram.is_some());
    let histogram = age_stats.histogram.as_ref().unwrap();
    assert_eq!(histogram.histogram_type, HistogramType::EqualDepth);
    assert!(histogram.bucket_count > 0);
    
    // Equal depth means buckets should have roughly equal counts
    let avg_count = 100 / histogram.bucket_count as u64;
    for bucket in &histogram.buckets {
        // Allow some variance due to duplicate values
        assert!(bucket.count >= avg_count - 5 && bucket.count <= avg_count + 5);
    }
    
    std::fs::remove_file("test_histogram_ed.owl").ok();
}

#[test]
fn test_selectivity_estimation_with_histogram() {
    let storage = create_storage("test_selectivity_hist.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    let documents = create_test_documents(100);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    // Test selectivity for different values
    let selectivity_25 = manager.estimate_selectivity(
        "users",
        "age",
        &Value::Integer(25),
    ).unwrap();
    
    let selectivity_50 = manager.estimate_selectivity(
        "users",
        "age",
        &Value::Integer(50),
    ).unwrap();
    
    assert!(selectivity_25 > 0.0);
    assert!(selectivity_25 <= 1.0);
    assert!(selectivity_50 > 0.0);
    assert!(selectivity_50 <= 1.0);
    
    std::fs::remove_file("test_selectivity_hist.owl").ok();
}

#[test]
fn test_selectivity_estimation_without_histogram() {
    let storage = create_storage("test_selectivity_no_hist.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    // Create documents with string field (no histogram for strings)
    let documents = create_test_documents(50);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    // Estimate selectivity for string field (uses distinct count)
    let selectivity = manager.estimate_selectivity(
        "users",
        "name",
        &Value::String("User 10".to_string()),
    ).unwrap();
    
    assert!(selectivity > 0.0);
    assert!(selectivity <= 1.0);
    
    std::fs::remove_file("test_selectivity_no_hist.owl").ok();
}

#[test]
fn test_cardinality_estimation() {
    let storage = create_storage("test_cardinality.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    let documents = create_test_documents(75);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    let cardinality = manager.get_cardinality("users").unwrap();
    assert_eq!(cardinality, 75);
    
    std::fs::remove_file("test_cardinality.owl").ok();
}

#[test]
fn test_cardinality_estimation_empty() {
    let storage = create_storage("test_cardinality_empty.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    let cardinality = manager.get_cardinality("users").unwrap();
    assert_eq!(cardinality, 0);
    
    std::fs::remove_file("test_cardinality_empty.owl").ok();
}

#[test]
fn test_index_statistics_recording() {
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
    
    manager.record_index_statistics(index_stats.clone()).unwrap();
    
    let retrieved = manager.get_index_statistics("users_age_idx").unwrap();
    assert!(retrieved.is_some());
    
    let retrieved_stats = retrieved.unwrap();
    assert_eq!(retrieved_stats.index_name, "users_age_idx");
    assert_eq!(retrieved_stats.schema, "users");
    assert_eq!(retrieved_stats.fields, vec!["age".to_string()]);
    assert_eq!(retrieved_stats.entry_count, 1000);
    assert_eq!(retrieved_stats.avg_key_size, 8);
    assert_eq!(retrieved_stats.selectivity, 0.02);
    assert_eq!(retrieved_stats.lookup_count, 500);
    assert_eq!(retrieved_stats.scan_count, 50);
}

#[test]
fn test_index_statistics_multiple() {
    let manager = StatisticsManager::new();
    
    let index1 = IndexStatistics {
        index_name: "users_age_idx".to_string(),
        schema: "users".to_string(),
        fields: vec!["age".to_string()],
        entry_count: 1000,
        avg_key_size: 8,
        selectivity: 0.02,
        lookup_count: 500,
        scan_count: 50,
    };
    
    let index2 = IndexStatistics {
        index_name: "users_name_idx".to_string(),
        schema: "users".to_string(),
        fields: vec!["name".to_string()],
        entry_count: 1000,
        avg_key_size: 20,
        selectivity: 0.001,
        lookup_count: 1000,
        scan_count: 10,
    };
    
    manager.record_index_statistics(index1).unwrap();
    manager.record_index_statistics(index2).unwrap();
    
    assert!(manager.get_index_statistics("users_age_idx").unwrap().is_some());
    assert!(manager.get_index_statistics("users_name_idx").unwrap().is_some());
}

#[test]
fn test_histogram_builder_equal_width() {
    let values: Vec<Value> = (0..100).map(|i| Value::Integer(i)).collect();
    let builder = HistogramBuilder::new(10, HistogramType::EqualWidth);
    
    let histogram = builder.build(&values).unwrap();
    
    assert_eq!(histogram.bucket_count, 10);
    assert_eq!(histogram.buckets.len(), 10);
    assert_eq!(histogram.histogram_type, HistogramType::EqualWidth);
    
    // Verify total count
    let total: u64 = histogram.buckets.iter().map(|b| b.count).sum();
    assert_eq!(total, 100);
}

#[test]
fn test_histogram_builder_equal_depth() {
    let values: Vec<Value> = (0..100).map(|i| Value::Integer(i)).collect();
    let builder = HistogramBuilder::new(10, HistogramType::EqualDepth);
    
    let histogram = builder.build(&values).unwrap();
    
    assert_eq!(histogram.bucket_count, 10);
    assert_eq!(histogram.buckets.len(), 10);
    assert_eq!(histogram.histogram_type, HistogramType::EqualDepth);
    
    // Each bucket should have roughly 10 values
    for bucket in &histogram.buckets {
        assert!(bucket.count >= 8 && bucket.count <= 12);
    }
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
    
    // Upper bound only: value < 50 (should be ~50% selective)
    let selectivity = estimate_range_selectivity(
        &histogram,
        None,
        Some(&Value::Integer(50)),
    );
    assert!(selectivity > 0.4 && selectivity < 0.6);
    
    // No bounds: all values (should be 100% selective)
    let selectivity = estimate_range_selectivity(
        &histogram,
        None,
        None,
    );
    assert_eq!(selectivity, 1.0);
}

#[test]
fn test_statistics_persistence() {
    let storage = create_storage("test_stats_persist.owl", 10 * 1024 * 1024).unwrap();
    let manager = StatisticsManager::new();
    
    let documents = create_test_documents(50);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    // Collect and store statistics
    manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    // Retrieve statistics
    let stats = manager.get_table_statistics("users").unwrap();
    assert!(stats.is_some());
    
    let stats = stats.unwrap();
    assert_eq!(stats.schema, "users");
    assert_eq!(stats.row_count, 50);
    
    std::fs::remove_file("test_stats_persist.owl").ok();
}

#[test]
fn test_sampling() {
    let storage = create_storage("test_sampling.owl", 10 * 1024 * 1024).unwrap();
    let config = StatisticsConfig {
        histogram_buckets: 10,
        histogram_type: HistogramType::EqualDepth,
        sample_size: 50, // Sample only 50 documents
        auto_update_threshold: 1000,
    };
    let manager = StatisticsManager::with_config(config);
    
    // Insert 200 documents
    let documents = create_test_documents(200);
    for doc in &documents {
        storage.put_document(doc).unwrap();
    }
    
    // Collect statistics (should sample)
    let stats = manager.collect_table_statistics(storage.as_ref(), "users").unwrap();
    
    assert_eq!(stats.row_count, 200); // Full count
    assert!(stats.avg_row_size > 0); // Estimated from sample
    
    std::fs::remove_file("test_sampling.owl").ok();
}
