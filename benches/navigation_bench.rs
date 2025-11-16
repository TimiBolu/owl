// Navigation Performance Benchmark
//
// This benchmark measures the performance of O(1) reference navigation,
// which is the core innovation of OQL.
//
// Target: <50ns per hop for single-level navigation
//         <150ns for 3-level navigation chain

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use owl_db::document::{Document, Value, ObjRef};
use owl_db::storage::{create_storage, DocumentStore};
use std::collections::HashMap;
use std::sync::Arc;

/// Create a test document with a reference field
fn create_document_with_reference(
    schema: &str,
    id: &str,
    ref_field: Option<(&str, ObjRef)>,
) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(format!("Doc {}", id)));
    fields.insert("value".to_string(), Value::Integer(42));
    
    if let Some((field_name, objref)) = ref_field {
        fields.insert(field_name.to_string(), Value::Reference(objref));
    }
    
    Document::with_id(
        id.to_string(),
        schema.to_string(),
        fields,
    )
}

/// Setup: Create a chain of documents for navigation testing
/// 
/// Structure: doc1 -> doc2 -> doc3 -> doc4
fn setup_navigation_chain(storage: &Arc<dyn DocumentStore>) -> (Document, Document, Document, Document) {
    // Create leaf document (no references)
    let doc4 = create_document_with_reference("test", "doc4", None);
    storage.put_document(&doc4).unwrap();
    
    // Create doc3 -> doc4
    let doc4_ref = ObjRef::from_string(&format!("test#{}", doc4.id)).unwrap();
    let doc3 = create_document_with_reference("test", "doc3", Some(("next", doc4_ref)));
    storage.put_document(&doc3).unwrap();
    
    // Create doc2 -> doc3
    let doc3_ref = ObjRef::from_string(&format!("test#{}", doc3.id)).unwrap();
    let doc2 = create_document_with_reference("test", "doc2", Some(("next", doc3_ref)));
    storage.put_document(&doc2).unwrap();
    
    // Create doc1 -> doc2
    let doc2_ref = ObjRef::from_string(&format!("test#{}", doc2.id)).unwrap();
    let doc1 = create_document_with_reference("test", "doc1", Some(("next", doc2_ref)));
    storage.put_document(&doc1).unwrap();
    
    (doc1, doc2, doc3, doc4)
}

/// Benchmark: Single-level navigation (doc1 -> doc2)
/// Target: <50ns per hop
fn bench_single_navigation(c: &mut Criterion) {
    let storage = create_storage("/tmp/nav_bench_single.owl", 100 * 1024 * 1024).unwrap();
    let (doc1, _doc2, _doc3, _doc4) = setup_navigation_chain(&storage);
    
    // Get the reference from doc1
    let next_ref = match doc1.get_field("next") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    c.bench_function("navigate_single_hop", |b| {
        b.iter(|| {
            // This should be <50ns: just pointer arithmetic + memory load
            let result = storage.navigate_reference(black_box(&next_ref));
            black_box(result)
        });
    });
    
    std::fs::remove_file("/tmp/nav_bench_single.owl").ok();
}

/// Benchmark: Two-level navigation (doc1 -> doc2 -> doc3)
/// Target: <100ns (2 hops)
fn bench_two_level_navigation(c: &mut Criterion) {
    let storage = create_storage("/tmp/nav_bench_two.owl", 100 * 1024 * 1024).unwrap();
    let (doc1, _doc2, _doc3, _doc4) = setup_navigation_chain(&storage);
    
    // Get the reference from doc1
    let next_ref = match doc1.get_field("next") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    c.bench_function("navigate_two_hops", |b| {
        b.iter(|| {
            // First hop: doc1 -> doc2
            let doc2 = storage.navigate_reference(black_box(&next_ref)).unwrap().unwrap();
            
            // Second hop: doc2 -> doc3
            let next_ref2 = match doc2.get_field("next") {
                Some(Value::Reference(r)) => *r,
                _ => panic!("Expected reference field"),
            };
            let result = storage.navigate_reference(black_box(&next_ref2));
            black_box(result)
        });
    });
    
    std::fs::remove_file("/tmp/nav_bench_two.owl").ok();
}

/// Benchmark: Three-level navigation (doc1 -> doc2 -> doc3 -> doc4)
/// Target: <150ns (3 hops)
fn bench_three_level_navigation(c: &mut Criterion) {
    let storage = create_storage("/tmp/nav_bench_three.owl", 100 * 1024 * 1024).unwrap();
    let (doc1, _doc2, _doc3, _doc4) = setup_navigation_chain(&storage);
    
    // Get the reference from doc1
    let next_ref = match doc1.get_field("next") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    c.bench_function("navigate_three_hops", |b| {
        b.iter(|| {
            // First hop: doc1 -> doc2
            let doc2 = storage.navigate_reference(black_box(&next_ref)).unwrap().unwrap();
            
            // Second hop: doc2 -> doc3
            let next_ref2 = match doc2.get_field("next") {
                Some(Value::Reference(r)) => *r,
                _ => panic!("Expected reference field"),
            };
            let doc3 = storage.navigate_reference(black_box(&next_ref2)).unwrap().unwrap();
            
            // Third hop: doc3 -> doc4
            let next_ref3 = match doc3.get_field("next") {
                Some(Value::Reference(r)) => *r,
                _ => panic!("Expected reference field"),
            };
            let result = storage.navigate_reference(black_box(&next_ref3));
            black_box(result)
        });
    });
    
    std::fs::remove_file("/tmp/nav_bench_three.owl").ok();
}

/// Benchmark: Navigation with varying chain lengths
fn bench_navigation_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("navigation_scaling");
    
    for chain_length in [1, 2, 3, 5, 10].iter() {
        let storage = create_storage(
            &format!("/tmp/nav_bench_scale_{}.owl", chain_length),
            100 * 1024 * 1024
        ).unwrap();
        
        // Create a chain of specified length
        let mut docs = Vec::new();
        let mut prev_ref = None;
        
        for i in (0..*chain_length).rev() {
            let doc = create_document_with_reference(
                "test",
                &format!("doc{}", i),
                prev_ref.map(|r| ("next", r)),
            );
            storage.put_document(&doc).unwrap();
            prev_ref = Some(ObjRef::from_string(&format!("test#{}", doc.id)).unwrap());
            docs.push(doc);
        }
        
        docs.reverse();
        let first_ref = match docs[0].get_field("next") {
            Some(Value::Reference(r)) => *r,
            _ => ObjRef::null(),
        };
        
        group.bench_with_input(
            BenchmarkId::from_parameter(chain_length),
            chain_length,
            |b, &length| {
                b.iter(|| {
                    let mut current_ref = first_ref;
                    for _ in 0..length {
                        if current_ref.is_null() {
                            break;
                        }
                        match storage.navigate_reference(black_box(&current_ref)) {
                            Ok(Some(doc)) => {
                                current_ref = match doc.get_field("next") {
                                    Some(Value::Reference(r)) => *r,
                                    _ => ObjRef::null(),
                                };
                            }
                            _ => break,
                        }
                    }
                    black_box(current_ref)
                });
            },
        );
        
        std::fs::remove_file(&format!("/tmp/nav_bench_scale_{}.owl", chain_length)).ok();
    }
    
    group.finish();
}

/// Benchmark: Compare navigation vs traditional lookup
fn bench_navigation_vs_lookup(c: &mut Criterion) {
    let storage = create_storage("/tmp/nav_bench_compare.owl", 100 * 1024 * 1024).unwrap();
    let (doc1, doc2, _doc3, _doc4) = setup_navigation_chain(&storage);
    
    let next_ref = match doc1.get_field("next") {
        Some(Value::Reference(r)) => *r,
        _ => panic!("Expected reference field"),
    };
    
    let mut group = c.benchmark_group("navigation_vs_lookup");
    
    // Benchmark O(1) navigation
    group.bench_function("o1_navigation", |b| {
        b.iter(|| {
            let result = storage.navigate_reference(black_box(&next_ref));
            black_box(result)
        });
    });
    
    // Benchmark traditional lookup (by schema + ID)
    group.bench_function("traditional_lookup", |b| {
        b.iter(|| {
            let result = storage.get_document(black_box("test"), black_box(&doc2.id));
            black_box(result)
        });
    });
    
    group.finish();
    
    std::fs::remove_file("/tmp/nav_bench_compare.owl").ok();
}

criterion_group!(
    benches,
    bench_single_navigation,
    bench_two_level_navigation,
    bench_three_level_navigation,
    bench_navigation_scaling,
    bench_navigation_vs_lookup,
);

criterion_main!(benches);
