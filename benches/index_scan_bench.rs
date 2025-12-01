// Index Scan Benchmarks
// Performance comparison between IndexScan and SeqScan

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use owl_db::{
    document::{Document, Value},
    execution::{IndexScanExec, PhysicalOperatorExec, SeqScanExec},
    parser::{ComparisonOperator, Expression, FieldAccess, Literal},
    planner::types::FilterCondition,
    index_manager::IndexManager,
    storage::Storage,
};
use std::collections::HashMap;
use std::sync::Arc;

fn create_test_documents(count: usize) -> Vec<Document> {
    (0..count)
        .map(|i| {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), Value::Integer(i as i64));
            fields.insert("name".to_string(), Value::String(format!("User{}", i)));
            fields.insert("age".to_string(), Value::Integer(20 + (i % 50) as i64));
            fields.insert("score".to_string(), Value::Integer((i * 7) as i64 % 100));
            
            Document {
                id: format!("user{}", i),
                schema: "users".to_string(),
                fields,
                created_at: 0,
                updated_at: 0,
                version: 1,
            }
        })
        .collect()
}

fn setup_storage_with_index(doc_count: usize) -> (Storage, Arc<IndexManager>) {
    let storage = Storage::new_in_memory();
    let docs = create_test_documents(doc_count);
    
    // Insert documents
    for doc in &docs {
        let _ = storage.insert_document(doc.clone());
    }
    
    // Create and populate index
    let index_manager = Arc::new(IndexManager::new());
    let _ = index_manager.create_index(
        "users",
        vec!["id".to_string()],
        false,
    );
    
    // Populate index
    for doc in &docs {
        if let Some(Value::Integer(id)) = doc.fields.get("id") {
            let mut field_values = HashMap::new();
            field_values.insert("id".to_string(), *id as u64);
            let objref = owl_storage_zig::ObjRef::new((*id as u64) * 1000);
            let _ = index_manager.on_insert("users", objref, &field_values);
        }
    }
    
    (storage, index_manager)
}

fn bench_seq_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("seq_scan");
    
    for size in [100, 1000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let (storage, _) = setup_storage_with_index(size);
            
            b.iter(|| {
                let mut scan = SeqScanExec::new("users".to_string());
                let _ = scan.open(storage.clone());
                
                let mut count = 0;
                while let Ok(Some(_doc)) = scan.next() {
                    count += 1;
                }
                
                let _ = scan.close();
                black_box(count)
            });
        });
    }
    
    group.finish();
}

fn bench_index_scan_equality(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_scan_equality");
    
    for size in [100, 1000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let (storage, _index_manager) = setup_storage_with_index(size);
            
            b.iter(|| {
                let condition = FilterCondition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["id".to_string()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::Integer(50)),
                };
                
                let mut scan = IndexScanExec::new(
                    "users".to_string(),
                    "idx_users_id".to_string(),
                    vec![condition],
                );
                
                // Note: This will use fallback to SeqScan in current implementation
                // Full integration would show true index performance
                let _ = scan.open(storage.clone());
                
                let mut count = 0;
                while let Ok(Some(_doc)) = scan.next() {
                    count += 1;
                }
                
                let _ = scan.close();
                black_box(count)
            });
        });
    }
    
    group.finish();
}

fn bench_index_scan_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_scan_range");
    
    for size in [100, 1000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let (storage, _index_manager) = setup_storage_with_index(size);
            
            b.iter(|| {
                let condition = FilterCondition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["id".to_string()],
                    }),
                    operator: ComparisonOperator::GreaterEqual,
                    right: Expression::Literal(Literal::Integer(size as i64 / 2)),
                };
                
                let mut scan = IndexScanExec::new(
                    "users".to_string(),
                    "idx_users_id".to_string(),
                    vec![condition],
                );
                
                let _ = scan.open(storage.clone());
                
                let mut count = 0;
                while let Ok(Some(_doc)) = scan.next() {
                    count += 1;
                }
                
                let _ = scan.close();
                black_box(count)
            });
        });
    }
    
    group.finish();
}

fn bench_index_scan_selectivity(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_scan_selectivity");
    let size = 10000;
    let (storage, _index_manager) = setup_storage_with_index(size);
    
    // Test different selectivities: 1%, 10%, 50%
    for selectivity in [1, 10, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}%", selectivity)),
            selectivity,
            |b, &selectivity| {
                let threshold = size as i64 * (100 - selectivity) / 100;
                
                b.iter(|| {
                    let condition = FilterCondition::Comparison {
                        left: Expression::FieldAccess(FieldAccess {
                            path: vec!["id".to_string()],
                        }),
                        operator: ComparisonOperator::GreaterEqual,
                        right: Expression::Literal(Literal::Integer(threshold)),
                    };
                    
                    let mut scan = IndexScanExec::new(
                        "users".to_string(),
                        "idx_users_id".to_string(),
                        vec![condition],
                    );
                    
                    let _ = scan.open(storage.clone());
                    
                    let mut count = 0;
                    while let Ok(Some(_doc)) = scan.next() {
                        count += 1;
                    }
                    
                    let _ = scan.close();
                    black_box(count)
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_seq_scan,
    bench_index_scan_equality,
    bench_index_scan_range,
    bench_index_scan_selectivity
);
criterion_main!(benches);
