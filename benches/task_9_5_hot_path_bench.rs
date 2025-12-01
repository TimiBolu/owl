// Task 9.5 Hot Path Optimization Benchmarks
//
// This benchmark validates the performance improvements from Task 9.5 optimizations:
// 1. Cache-line optimized ObjectHeader layout
// 2. Branch prediction hints
// 3. Memory pool allocators
// 4. String interning
// 5. Field access caching

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use owl_db::document::{Document, Value};
use owl_db::hot_path_optimizations::{
    get_pooled_string, return_pooled_string,
    get_pooled_document_vec, return_pooled_document_vec,
    optimized_get_field, intern_string, get_interned_string,
    CacheLineAligned,
};
use std::collections::HashMap;

fn create_test_document(id: &str) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    fields.insert("age".to_string(), Value::Integer(30));
    fields.insert("active".to_string(), Value::Boolean(true));
    
    Document::with_id(
        id.to_string(),
        "users".to_string(),
        fields,
    )
}

// Benchmark 1: String pooling vs regular allocation
fn bench_string_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_allocation");
    
    group.bench_function("regular_allocation", |b| {
        b.iter(|| {
            let s = String::from("test_string");
            black_box(s);
        });
    });
    
    group.bench_function("pooled_allocation", |b| {
        b.iter(|| {
            let mut s = get_pooled_string();
            s.push_str("test_string");
            black_box(&s);
            return_pooled_string(s);
        });
    });
    
    group.finish();
}

// Benchmark 2: Document vector pooling
fn bench_document_vec_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("document_vec_allocation");
    
    let doc = create_test_document("user1");
    
    group.bench_function("regular_allocation", |b| {
        b.iter(|| {
            let mut vec = Vec::new();
            vec.push(doc.clone());
            vec.push(doc.clone());
            vec.push(doc.clone());
            black_box(&vec);
        });
    });
    
    group.bench_function("pooled_allocation", |b| {
        b.iter(|| {
            let mut vec = get_pooled_document_vec();
            vec.push(doc.clone());
            vec.push(doc.clone());
            vec.push(doc.clone());
            black_box(&vec);
            return_pooled_document_vec(vec);
        });
    });
    
    group.finish();
}

// Benchmark 3: String interning
fn bench_string_interning(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_interning");
    
    let field_names = vec!["name", "email", "age", "active", "created_at", "updated_at"];
    
    group.bench_function("string_comparison", |b| {
        b.iter(|| {
            for name in &field_names {
                let _ = black_box(*name == "email");
            }
        });
    });
    
    group.bench_function("interned_comparison", |b| {
        // Pre-intern the strings
        let interned_ids: Vec<u32> = field_names.iter().map(|s| intern_string(s)).collect();
        let email_id = intern_string("email");
        
        b.iter(|| {
            for &id in &interned_ids {
                let _ = black_box(id == email_id);
            }
        });
    });
    
    group.finish();
}

// Benchmark 4: Field access with caching
fn bench_field_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("field_access");
    
    let doc = create_test_document("user1");
    
    group.bench_function("direct_field_access", |b| {
        b.iter(|| {
            let value = doc.get_field("name");
            black_box(value);
        });
    });
    
    group.bench_function("optimized_field_access", |b| {
        b.iter(|| {
            let value = optimized_get_field(&doc, "name");
            black_box(value);
        });
    });
    
    group.finish();
}

// Benchmark 5: Cache-line aligned data structures
fn bench_cache_line_alignment(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_line_alignment");
    
    group.bench_function("unaligned_access", |b| {
        let data = vec![1u64, 2, 3, 4, 5, 6, 7, 8];
        b.iter(|| {
            let sum: u64 = data.iter().sum();
            black_box(sum);
        });
    });
    
    group.bench_function("aligned_access", |b| {
        let data = CacheLineAligned::new(vec![1u64, 2, 3, 4, 5, 6, 7, 8]);
        b.iter(|| {
            let sum: u64 = data.data.iter().sum();
            black_box(sum);
        });
    });
    
    group.finish();
}

// Benchmark 6: Repeated field access (cache effectiveness)
fn bench_repeated_field_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("repeated_field_access");
    
    let doc = create_test_document("user1");
    
    group.bench_function("direct_repeated_access", |b| {
        b.iter(|| {
            for _ in 0..10 {
                let value = doc.get_field("name");
                black_box(value);
            }
        });
    });
    
    group.bench_function("optimized_repeated_access", |b| {
        b.iter(|| {
            for _ in 0..10 {
                let value = optimized_get_field(&doc, "name");
                black_box(value);
            }
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_string_allocation,
    bench_document_vec_allocation,
    bench_string_interning,
    bench_field_access,
    bench_cache_line_alignment,
    bench_repeated_field_access,
);
criterion_main!(benches);
