// Parallel Execution Tests
// Tests for parallel query execution operators

use owl_db::document::{Document, Value};
use owl_db::execution::{
    ParallelSeqScanExec, ParallelFilterExec, ParallelAggregateExec,
    PhysicalOperatorExec, SeqScanExec, ThreadPool,
};
use owl_db::parser::FieldAccess;
use owl_db::planner::types::{AggregateFunction, AggregateSpec};
use owl_db::storage::create_storage;
use std::collections::HashMap;
use std::sync::Arc;

fn create_test_document(schema: &str, id: &str, value: i64, category: &str) -> Document {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), Value::Integer(value));
    fields.insert("category".to_string(), Value::String(category.to_string()));
    
    Document::with_id(
        id.to_string(),
        schema.to_string(),
        fields,
    )
}

#[test]
fn test_parallel_seq_scan_basic() {
    let storage = create_storage("test_parallel_seq_scan_basic.owl", 10 * 1024 * 1024).unwrap();
    
    // Create test documents
    for i in 0..50 {
        let doc = create_test_document("items", &format!("item{}", i), i, "A");
        storage.put_document(&doc).unwrap();
    }
    
    // Create parallel scan with 4 workers
    let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
    let mut scan = ParallelSeqScanExec::new("items".to_string(), thread_pool, 10);
    
    // Execute
    scan.open(storage.clone()).unwrap();
    
    let mut count = 0;
    let mut sum = 0i64;
    while let Some(doc) = scan.next().unwrap() {
        if let Some(Value::Integer(v)) = doc.fields.get("value") {
            sum += v;
        }
        count += 1;
    }
    
    assert_eq!(count, 50);
    assert_eq!(sum, (0..50).sum::<i64>());
    
    scan.close().unwrap();
    std::fs::remove_file("test_parallel_seq_scan_basic.owl").ok();
}

#[test]
fn test_parallel_seq_scan_large() {
    let storage = create_storage("test_parallel_seq_scan_large.owl", 50 * 1024 * 1024).unwrap();
    
    // Create many test documents
    for i in 0..1000 {
        let doc = create_test_document("items", &format!("item{}", i), i, "B");
        storage.put_document(&doc).unwrap();
    }
    
    // Create parallel scan with auto chunk size
    let thread_pool = Arc::new(ThreadPool::new(8).unwrap());
    let mut scan = ParallelSeqScanExec::new("items".to_string(), thread_pool, 0);
    
    // Execute
    scan.open(storage.clone()).unwrap();
    
    let mut count = 0;
    while let Some(_doc) = scan.next().unwrap() {
        count += 1;
    }
    
    assert_eq!(count, 1000);
    
    scan.close().unwrap();
    std::fs::remove_file("test_parallel_seq_scan_large.owl").ok();
}

#[test]
fn test_parallel_filter() {
    let storage = create_storage("test_parallel_filter.owl", 10 * 1024 * 1024).unwrap();
    
    // Create test documents
    for i in 0..100 {
        let category = if i % 3 == 0 { "A" } else { "B" };
        let doc = create_test_document("items", &format!("item{}", i), i, category);
        storage.put_document(&doc).unwrap();
    }
    
    // Create parallel filter (keep only category A)
    let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
    let input = Box::new(SeqScanExec::new("items".to_string()));
    let mut filter = ParallelFilterExec::new(
        input,
        |doc: &Document| {
            if let Some(Value::String(cat)) = doc.fields.get("category") {
                cat == "A"
            } else {
                false
            }
        },
        thread_pool,
        20,
    );
    
    // Execute
    filter.open(storage.clone()).unwrap();
    
    let mut count = 0;
    while let Some(doc) = filter.next().unwrap() {
        if let Some(Value::String(cat)) = doc.fields.get("category") {
            assert_eq!(cat, "A");
        }
        count += 1;
    }
    
    // Should have 34 documents (0, 3, 6, ..., 99)
    assert_eq!(count, 34);
    
    filter.close().unwrap();
    std::fs::remove_file("test_parallel_filter.owl").ok();
}

#[test]
fn test_parallel_aggregate_count() {
    let storage = create_storage("test_parallel_agg_count.owl", 10 * 1024 * 1024).unwrap();
    
    // Create test documents
    for i in 0..200 {
        let doc = create_test_document("items", &format!("item{}", i), i, "C");
        storage.put_document(&doc).unwrap();
    }
    
    // Create parallel aggregate
    let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
    let input = Box::new(SeqScanExec::new("items".to_string()));
    let aggregates = vec![AggregateSpec {
        function: AggregateFunction::Count,
        field: None,
        alias: Some("total_count".to_string()),
    }];
    let mut agg = ParallelAggregateExec::new(aggregates, input, thread_pool, 50);
    
    // Execute
    agg.open(storage.clone()).unwrap();
    
    let result = agg.next().unwrap().unwrap();
    assert_eq!(result.fields.get("total_count"), Some(&Value::Integer(200)));
    
    agg.close().unwrap();
    std::fs::remove_file("test_parallel_agg_count.owl").ok();
}

#[test]
fn test_parallel_aggregate_sum_avg() {
    let storage = create_storage("test_parallel_agg_sum_avg.owl", 10 * 1024 * 1024).unwrap();
    
    // Create test documents (values 1 to 100)
    for i in 1..=100 {
        let doc = create_test_document("items", &format!("item{}", i), i, "D");
        storage.put_document(&doc).unwrap();
    }
    
    // Create parallel aggregate
    let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
    let input = Box::new(SeqScanExec::new("items".to_string()));
    let aggregates = vec![
        AggregateSpec {
            function: AggregateFunction::Sum,
            field: Some(FieldAccess {
                path: vec!["value".to_string()],
            }),
            alias: Some("total_sum".to_string()),
        },
        AggregateSpec {
            function: AggregateFunction::Avg,
            field: Some(FieldAccess {
                path: vec!["value".to_string()],
            }),
            alias: Some("avg_value".to_string()),
        },
    ];
    let mut agg = ParallelAggregateExec::new(aggregates, input, thread_pool, 25);
    
    // Execute
    agg.open(storage.clone()).unwrap();
    
    let result = agg.next().unwrap().unwrap();
    
    // Sum of 1..100 = 5050
    assert_eq!(result.fields.get("total_sum"), Some(&Value::Float(5050.0)));
    
    // Average = 5050 / 100 = 50.5
    assert_eq!(result.fields.get("avg_value"), Some(&Value::Float(50.5)));
    
    agg.close().unwrap();
    std::fs::remove_file("test_parallel_agg_sum_avg.owl").ok();
}

#[test]
fn test_parallel_aggregate_min_max() {
    let storage = create_storage("test_parallel_agg_minmax.owl", 10 * 1024 * 1024).unwrap();
    
    // Create test documents with random-ish values
    let values = vec![42, 7, 99, 13, 88, 5, 67, 23, 91, 15];
    for (i, &val) in values.iter().enumerate() {
        let doc = create_test_document("items", &format!("item{}", i), val, "E");
        storage.put_document(&doc).unwrap();
    }
    
    // Create parallel aggregate
    let thread_pool = Arc::new(ThreadPool::new(2).unwrap());
    let input = Box::new(SeqScanExec::new("items".to_string()));
    let aggregates = vec![
        AggregateSpec {
            function: AggregateFunction::Min,
            field: Some(FieldAccess {
                path: vec!["value".to_string()],
            }),
            alias: Some("min_value".to_string()),
        },
        AggregateSpec {
            function: AggregateFunction::Max,
            field: Some(FieldAccess {
                path: vec!["value".to_string()],
            }),
            alias: Some("max_value".to_string()),
        },
    ];
    let mut agg = ParallelAggregateExec::new(aggregates, input, thread_pool, 3);
    
    // Execute
    agg.open(storage.clone()).unwrap();
    
    let result = agg.next().unwrap().unwrap();
    
    assert_eq!(result.fields.get("min_value"), Some(&Value::Integer(5)));
    assert_eq!(result.fields.get("max_value"), Some(&Value::Integer(99)));
    
    agg.close().unwrap();
    std::fs::remove_file("test_parallel_agg_minmax.owl").ok();
}

#[test]
fn test_thread_pool_workers() {
    // Test with different worker counts
    for num_workers in [1, 2, 4, 8] {
        let pool = ThreadPool::new(num_workers).unwrap();
        assert_eq!(pool.num_workers(), num_workers);
    }
}

#[test]
fn test_parallel_execution_correctness() {
    // Verify parallel execution produces same results as sequential
    let storage = create_storage("test_parallel_correctness.owl", 10 * 1024 * 1024).unwrap();
    
    // Create test documents
    for i in 0..100 {
        let doc = create_test_document("items", &format!("item{}", i), i, "F");
        storage.put_document(&doc).unwrap();
    }
    
    // Sequential aggregate
    let input_seq = Box::new(SeqScanExec::new("items".to_string()));
    let aggregates_seq = vec![
        AggregateSpec {
            function: AggregateFunction::Count,
            field: None,
            alias: Some("count".to_string()),
        },
        AggregateSpec {
            function: AggregateFunction::Sum,
            field: Some(FieldAccess {
                path: vec!["value".to_string()],
            }),
            alias: Some("sum".to_string()),
        },
    ];
    let mut agg_seq = owl_db::execution::AggregateExec::new(aggregates_seq, input_seq);
    agg_seq.open(storage.clone()).unwrap();
    let result_seq = agg_seq.next().unwrap().unwrap();
    agg_seq.close().unwrap();
    
    // Parallel aggregate
    let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
    let input_par = Box::new(SeqScanExec::new("items".to_string()));
    let aggregates_par = vec![
        AggregateSpec {
            function: AggregateFunction::Count,
            field: None,
            alias: Some("count".to_string()),
        },
        AggregateSpec {
            function: AggregateFunction::Sum,
            field: Some(FieldAccess {
                path: vec!["value".to_string()],
            }),
            alias: Some("sum".to_string()),
        },
    ];
    let mut agg_par = ParallelAggregateExec::new(aggregates_par, input_par, thread_pool, 20);
    agg_par.open(storage.clone()).unwrap();
    let result_par = agg_par.next().unwrap().unwrap();
    agg_par.close().unwrap();
    
    // Results should match
    assert_eq!(
        result_seq.fields.get("count"),
        result_par.fields.get("count")
    );
    assert_eq!(
        result_seq.fields.get("sum"),
        result_par.fields.get("sum")
    );
    
    std::fs::remove_file("test_parallel_correctness.owl").ok();
}
