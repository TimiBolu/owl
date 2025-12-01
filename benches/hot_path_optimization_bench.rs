// Hot Path Optimization Benchmark
//
// Measures the performance improvements from hot path optimizations:
// 1. Memory pool allocations vs standard allocations
// 2. String interning vs repeated string allocations
// 3. Field access caching vs direct HashMap lookups
// 4. Cache-line optimized data structures
//
// Requirements: 12, 19

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use owl_db::document::{Document, Value};
use owl_db::hot_path_optimizations::{
    StringInterner, MemoryPool, FieldAccessCache, 
    get_pooled_string, return_pooled_string,
    get_pooled_document_vec, return_pooled_document_vec,
    optimized_get_field, intern_string, get_interned_string,
    OptimizedHashMap, CacheLineAligned, HotPathProfiler,
    simd_ops
};
use std::collections::HashMap;
use std::time::Instant;

/// Create a test document with many fields
fn create_test_document(id: &str, field_count: usize) -> Document {
    let mut fields = HashMap::new();
    
    // Add common fields that would benefit from interning
    fields.insert("id".to_string(), Value::String(id.to_string()));
    fields.insert("name".to_string(), Value::String(format!("Document {}", id)));
    fields.insert("email".to_string(), Value::String(format!("{}@example.com", id)));
    fields.insert("status".to_string(), Value::String("active".to_string()));
    fields.insert("created_at".to_string(), Value::DateTime(chrono::Utc::now()));
    fields.insert("updated_at".to_string(), Value::DateTime(chrono::Utc::now()));
    
    // Add additional fields to test scaling
    for i in 0..field_count {
        fields.insert(format!("field_{}", i), Value::Integer(i as i64));
    }
    
    Document::with_id(
        id.to_string(),
        "test_docs".to_string(),
        fields,
    )
}

/// Benchmark string interning vs regular string operations
fn bench_string_interning(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_interning");
    
    let field_names = vec![
        "id", "name", "email", "status", "created_at", "updated_at",
        "user_id", "order_id", "product_id", "customer_id", "total",
        "amount", "price", "quantity", "description", "title"
    ];
    
    // Benchmark regular string operations
    group.bench_function("regular_strings", |b| {
        b.iter(|| {
            let mut strings = Vec::new();
            for _ in 0..1000 {
                for field_name in &field_names {
                    strings.push(field_name.to_string());
                }
            }
            black_box(strings);
        });
    });
    
    // Benchmark string interning
    group.bench_function("interned_strings", |b| {
        b.iter(|| {
            let mut interner = StringInterner::new();
            interner.preload_common_fields();
            
            let mut ids = Vec::new();
            for _ in 0..1000 {
                for field_name in &field_names {
                    ids.push(interner.intern(field_name));
                }
            }
            black_box(ids);
        });
    });
    
    group.finish();
}

/// Benchmark memory pool vs standard allocation
fn bench_memory_pools(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_pools");
    
    // Benchmark standard Vec allocation
    group.bench_function("standard_allocation", |b| {
        b.iter(|| {
            let mut vecs = Vec::new();
            for _ in 0..1000 {
                let mut v = Vec::<String>::new();
                for i in 0..10 {
                    v.push(format!("item_{}", i));
                }
                vecs.push(v);
            }
            black_box(vecs);
        });
    });
    
    // Benchmark pooled allocation
    group.bench_function("pooled_allocation", |b| {
        b.iter(|| {
            let mut vecs = Vec::new();
            for _ in 0..1000 {
                let mut v = get_pooled_document_vec();
                for i in 0..10 {
                    let doc = create_test_document(&format!("doc_{}", i), 5);
                    v.push(doc);
                }
                vecs.push(v.clone());
                return_pooled_document_vec(v);
            }
            black_box(vecs);
        });
    });
    
    group.finish();
}

/// Benchmark field access caching
fn bench_field_access_caching(c: &mut Criterion) {
    let mut group = c.benchmark_group("field_access_caching");
    
    let documents: Vec<_> = (0..100)
        .map(|i| create_test_document(&format!("doc_{}", i), 20))
        .collect();
    
    let field_names = vec!["id", "name", "email", "status", "field_5", "field_10"];
    
    // Benchmark direct field access
    group.bench_function("direct_access", |b| {
        b.iter(|| {
            let mut results = Vec::new();
            for _ in 0..100 {
                for doc in &documents {
                    for field_name in &field_names {
                        if let Some(value) = doc.get_field(field_name) {
                            results.push(value.clone());
                        }
                    }
                }
            }
            black_box(results);
        });
    });
    
    // Benchmark cached field access
    group.bench_function("cached_access", |b| {
        b.iter(|| {
            let mut results = Vec::new();
            for _ in 0..100 {
                for doc in &documents {
                    for field_name in &field_names {
                        if let Some(value) = optimized_get_field(doc, field_name) {
                            results.push(value);
                        }
                    }
                }
            }
            black_box(results);
        });
    });
    
    group.finish();
}

/// Benchmark optimized HashMap vs standard HashMap
fn bench_optimized_hashmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("optimized_hashmap");
    
    let keys: Vec<String> = (0..1000).map(|i| format!("key_{}", i)).collect();
    let values: Vec<String> = (0..1000).map(|i| format!("value_{}", i)).collect();
    
    // Benchmark standard HashMap
    group.bench_function("standard_hashmap", |b| {
        b.iter(|| {
            let mut map = HashMap::new();
            
            // Insert
            for (key, value) in keys.iter().zip(values.iter()) {
                map.insert(key.clone(), value.clone());
            }
            
            // Lookup
            let mut results = Vec::new();
            for key in &keys {
                if let Some(value) = map.get(key) {
                    results.push(value.clone());
                }
            }
            
            black_box(results);
        });
    });
    
    // Benchmark optimized HashMap
    group.bench_function("optimized_hashmap", |b| {
        b.iter(|| {
            let mut map = OptimizedHashMap::new();
            
            // Insert
            for (key, value) in keys.iter().zip(values.iter()) {
                map.insert(key.clone(), value.clone());
            }
            
            // Lookup
            let mut results = Vec::new();
            for key in &keys {
                if let Some(value) = map.get(key) {
                    results.push(value.clone());
                }
            }
            
            black_box(results);
        });
    });
    
    group.finish();
}

/// Benchmark SIMD operations
fn bench_simd_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_operations");
    
    let integers: Vec<i64> = (0..10000).collect();
    let strings: Vec<String> = (0..1000).map(|i| format!("test_string_{}", i)).collect();
    
    // Benchmark SIMD integer comparison
    group.bench_function("simd_integer_comparison", |b| {
        b.iter(|| {
            let results = simd_ops::simd_compare_integers(
                black_box(&integers), 
                black_box(5000), 
                black_box(simd_ops::ComparisonOp::GreaterThan)
            );
            black_box(results);
        });
    });
    
    // Benchmark scalar integer comparison
    group.bench_function("scalar_integer_comparison", |b| {
        b.iter(|| {
            let mut results = Vec::new();
            for &value in black_box(&integers) {
                results.push(value > black_box(5000));
            }
            black_box(results);
        });
    });
    
    // Benchmark SIMD string prefix matching
    group.bench_function("simd_string_prefix", |b| {
        b.iter(|| {
            let results = simd_ops::simd_string_prefix_match(
                black_box(&strings), 
                black_box("test_string_1")
            );
            black_box(results);
        });
    });
    
    // Benchmark scalar string prefix matching
    group.bench_function("scalar_string_prefix", |b| {
        b.iter(|| {
            let mut results = Vec::new();
            let prefix = black_box("test_string_1");
            for string in black_box(&strings) {
                results.push(string.starts_with(prefix));
            }
            black_box(results);
        });
    });
    
    group.finish();
}

/// Benchmark cache-line alignment effects
fn bench_cache_line_alignment(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_line_alignment");
    
    // Create aligned and unaligned data structures
    let aligned_data: Vec<CacheLineAligned<u64>> = (0..1000)
        .map(|i| CacheLineAligned::new(i as u64))
        .collect();
    
    let unaligned_data: Vec<u64> = (0..1000).collect();
    
    // Benchmark aligned access
    group.bench_function("aligned_access", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for item in black_box(&aligned_data) {
                sum += item.data;
            }
            black_box(sum);
        });
    });
    
    // Benchmark unaligned access
    group.bench_function("unaligned_access", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for &item in black_box(&unaligned_data) {
                sum += item;
            }
            black_box(sum);
        });
    });
    
    group.finish();
}

/// Benchmark profiling overhead
fn bench_profiling_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("profiling_overhead");
    
    // Benchmark without profiling
    group.bench_function("no_profiling", |b| {
        b.iter(|| {
            let mut sum = 0;
            for i in 0..1000 {
                sum += i * 2;
            }
            black_box(sum);
        });
    });
    
    // Benchmark with profiling
    group.bench_function("with_profiling", |b| {
        let mut profiler = HotPathProfiler::new();
        profiler.set_enabled(true);
        
        b.iter(|| {
            let start = Instant::now();
            let mut sum = 0;
            for i in 0..1000 {
                sum += i * 2;
            }
            let duration = start.elapsed().as_nanos() as u64;
            profiler.record_call("test_function", duration);
            black_box(sum);
        });
    });
    
    group.finish();
}

/// Benchmark overall query execution with optimizations
fn bench_optimized_query_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("optimized_query_execution");
    
    let documents: Vec<_> = (0..1000)
        .map(|i| create_test_document(&format!("doc_{}", i), 10))
        .collect();
    
    // Simulate query execution with standard operations
    group.bench_function("standard_execution", |b| {
        b.iter(|| {
            let mut results = Vec::new();
            
            // Filter documents
            for doc in &documents {
                if let Some(Value::String(status)) = doc.get_field("status") {
                    if status == "active" {
                        results.push(doc.clone());
                    }
                }
            }
            
            // Project fields
            let mut projected = Vec::new();
            for doc in &results {
                let mut fields = HashMap::new();
                if let Some(id) = doc.get_field("id") {
                    fields.insert("id".to_string(), id.clone());
                }
                if let Some(name) = doc.get_field("name") {
                    fields.insert("name".to_string(), name.clone());
                }
                projected.push(fields);
            }
            
            black_box(projected);
        });
    });
    
    // Simulate query execution with optimizations
    group.bench_function("optimized_execution", |b| {
        b.iter(|| {
            let mut results = get_pooled_document_vec();
            
            // Filter documents with optimized field access
            for doc in &documents {
                if let Some(Value::String(status)) = optimized_get_field(doc, "status") {
                    if status == "active" {
                        results.push(doc.clone());
                    }
                }
            }
            
            // Project fields with optimized access
            let mut projected = Vec::new();
            for doc in &results {
                let mut fields = OptimizedHashMap::new();
                if let Some(id) = optimized_get_field(doc, "id") {
                    fields.insert("id".to_string(), id);
                }
                if let Some(name) = optimized_get_field(doc, "name") {
                    fields.insert("name".to_string(), name);
                }
                projected.push(fields);
            }
            
            return_pooled_document_vec(results);
            black_box(projected);
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_string_interning,
    bench_memory_pools,
    bench_field_access_caching,
    bench_optimized_hashmap,
    bench_simd_operations,
    bench_cache_line_alignment,
    bench_profiling_overhead,
    bench_optimized_query_execution
);

criterion_main!(benches);