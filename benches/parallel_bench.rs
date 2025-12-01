// Parallel Execution Benchmarks
// Benchmarks comparing sequential vs parallel execution

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use owl_db::document::{Document, Value};
use owl_db::execution::{
    ParallelSeqScanExec, ParallelAggregateExec, SeqScanExec, AggregateExec,
    PhysicalOperatorExec, ThreadPool,
};
use owl_db::parser::FieldAccess;
use owl_db::planner::types::{AggregateFunction, AggregateSpec};
use owl_db::storage::create_storage;
use std::collections::HashMap;
use std::sync::Arc;

fn create_test_document(schema: &str, id: &str, value: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), Value::Integer(value));
    fields.insert("score".to_string(), Value::Float(value as f64 * 1.5));
    
    Document::with_id(
        id.to_string(),
        schema.to_string(),
        fields,
    )
}

fn setup_test_data(num_docs: usize) -> owl_db::storage::Storage {
    let storage = create_storage(
        &format!("bench_parallel_{}.owl", num_docs),
        100 * 1024 * 1024
    ).unwrap();
    
    for i in 0..num_docs {
        let doc = create_test_document("items", &format!("item{}", i), i as i64);
        storage.put_document(&doc).unwrap();
    }
    
    storage
}

fn bench_sequential_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan");
    
    for size in [1000, 5000, 10000].iter() {
        let storage = setup_test_data(*size);
        
        group.bench_with_input(BenchmarkId::new("sequential", size), size, |b, _| {
            b.iter(|| {
                let mut scan = SeqScanExec::new("items".to_string());
                scan.open(storage.clone()).unwrap();
                
                let mut count = 0;
                while let Some(_doc) = scan.next().unwrap() {
                    count += 1;
                }
                
                scan.close().unwrap();
                black_box(count);
            });
        });
        
        std::fs::remove_file(format!("bench_parallel_{}.owl", size)).ok();
    }
    
    group.finish();
}

fn bench_parallel_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan");
    
    for size in [1000, 5000, 10000].iter() {
        let storage = setup_test_data(*size);
        let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
        
        group.bench_with_input(BenchmarkId::new("parallel", size), size, |b, _| {
            b.iter(|| {
                let mut scan = ParallelSeqScanExec::new(
                    "items".to_string(),
                    Arc::clone(&thread_pool),
                    0
                );
                scan.open(storage.clone()).unwrap();
                
                let mut count = 0;
                while let Some(_doc) = scan.next().unwrap() {
                    count += 1;
                }
                
                scan.close().unwrap();
                black_box(count);
            });
        });
        
        std::fs::remove_file(format!("bench_parallel_{}.owl", size)).ok();
    }
    
    group.finish();
}

fn bench_sequential_aggregate(c: &mut Criterion) {
    let mut group = c.benchmark_group("aggregate");
    
    for size in [1000, 5000, 10000].iter() {
        let storage = setup_test_data(*size);
        
        group.bench_with_input(BenchmarkId::new("sequential", size), size, |b, _| {
            b.iter(|| {
                let input = Box::new(SeqScanExec::new("items".to_string()));
                let aggregates = vec![
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
                    AggregateSpec {
                        function: AggregateFunction::Avg,
                        field: Some(FieldAccess {
                            
                            path: vec!["score".to_string()],
                        }),
                        alias: Some("avg".to_string()),
                    },
                ];
                let mut agg = AggregateExec::new(aggregates, input);
                agg.open(storage.clone()).unwrap();
                
                let result = agg.next().unwrap();
                
                agg.close().unwrap();
                black_box(result);
            });
        });
        
        std::fs::remove_file(format!("bench_parallel_{}.owl", size)).ok();
    }
    
    group.finish();
}

fn bench_parallel_aggregate(c: &mut Criterion) {
    let mut group = c.benchmark_group("aggregate");
    
    for size in [1000, 5000, 10000].iter() {
        let storage = setup_test_data(*size);
        let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
        
        group.bench_with_input(BenchmarkId::new("parallel", size), size, |b, _| {
            b.iter(|| {
                let input = Box::new(SeqScanExec::new("items".to_string()));
                let aggregates = vec![
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
                    AggregateSpec {
                        function: AggregateFunction::Avg,
                        field: Some(FieldAccess {
                            
                            path: vec!["score".to_string()],
                        }),
                        alias: Some("avg".to_string()),
                    },
                ];
                let mut agg = ParallelAggregateExec::new(
                    aggregates,
                    input,
                    Arc::clone(&thread_pool),
                    0
                );
                agg.open(storage.clone()).unwrap();
                
                let result = agg.next().unwrap();
                
                agg.close().unwrap();
                black_box(result);
            });
        });
        
        std::fs::remove_file(format!("bench_parallel_{}.owl", size)).ok();
    }
    
    group.finish();
}

fn bench_worker_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("worker_scaling");
    let size = 10000;
    let storage = setup_test_data(size);
    
    for num_workers in [1, 2, 4, 8].iter() {
        let thread_pool = Arc::new(ThreadPool::new(*num_workers).unwrap());
        
        group.bench_with_input(
            BenchmarkId::new("aggregate", num_workers),
            num_workers,
            |b, _| {
                b.iter(|| {
                    let input = Box::new(SeqScanExec::new("items".to_string()));
                    let aggregates = vec![
                        AggregateSpec {
                            function: AggregateFunction::Sum,
                            field: Some(FieldAccess {
                                
                                path: vec!["value".to_string()],
                            }),
                            alias: Some("sum".to_string()),
                        },
                    ];
                    let mut agg = ParallelAggregateExec::new(
                        aggregates,
                        input,
                        Arc::clone(&thread_pool),
                        100
                    );
                    agg.open(storage.clone()).unwrap();
                    
                    let result = agg.next().unwrap();
                    
                    agg.close().unwrap();
                    black_box(result);
                });
            }
        );
    }
    
    std::fs::remove_file(format!("bench_parallel_{}.owl", size)).ok();
    group.finish();
}

criterion_group!(
    benches,
    bench_sequential_scan,
    bench_parallel_scan,
    bench_sequential_aggregate,
    bench_parallel_aggregate,
    bench_worker_scaling
);
criterion_main!(benches);
