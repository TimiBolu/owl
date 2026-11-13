use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use owl_db::document::{Document, ObjRef, Value};
use owl_db::schema::{FieldType, ReferenceType, Schema};
use owl_db::storage::{DocumentStore, RustStore};
use std::collections::HashMap;
use tempfile::TempDir;

struct ChainFixture {
    _directory: TempDir,
    store: RustStore,
    start: ObjRef,
}

fn node_schema() -> Schema {
    Schema::new(
        "nodes".to_string(),
        HashMap::from([
            ("name".to_string(), FieldType::String),
            (
                "next".to_string(),
                FieldType::Reference(ReferenceType::optional("nodes".to_string())),
            ),
        ]),
    )
}

fn chain_fixture(hops: usize) -> ChainFixture {
    assert!(hops > 0);
    let directory = TempDir::new().expect("navigation benchmark directory");
    let store = RustStore::new(directory.path().join("navigation.owl"), 64 * 1024 * 1024)
        .expect("navigation benchmark store");
    store.put_schema(&node_schema()).expect("node schema");

    let mut next = None;
    for index in (0..hops).rev() {
        let mut fields =
            HashMap::from([("name".to_string(), Value::String(format!("node-{index}")))]);
        if let Some(object_ref) = next {
            fields.insert("next".to_string(), Value::Reference(object_ref));
        }
        let document = Document::with_id(format!("node-{index}"), "nodes".to_string(), fields);
        store.put_document(&document).expect("chain node");
        next = store
            .get_objref("nodes", &document.id)
            .expect("chain object lookup");
    }

    ChainFixture {
        _directory: directory,
        store,
        start: next.expect("non-empty chain"),
    }
}

fn follow_chain(store: &RustStore, start: ObjRef, hops: usize) -> ObjRef {
    let mut current = start;
    for _ in 0..hops {
        let document = store
            .navigate_reference(&current)
            .expect("reference navigation")
            .expect("live chain object");
        current = match document.fields.get("next") {
            Some(Value::Reference(object_ref)) => *object_ref,
            _ => ObjRef::null(),
        };
    }
    current
}

fn bench_forward_navigation(c: &mut Criterion) {
    let mut group = c.benchmark_group("rust_store_forward_navigation");
    for hops in [1usize, 3, 8] {
        let fixture = chain_fixture(hops);
        assert_eq!(
            follow_chain(&fixture.store, fixture.start, hops),
            ObjRef::null()
        );
        group.bench_with_input(
            BenchmarkId::new("owned_document", hops),
            &hops,
            |b, hops| {
                b.iter(|| {
                    black_box(follow_chain(
                        black_box(&fixture.store),
                        black_box(fixture.start),
                        *hops,
                    ))
                });
            },
        );
    }
    group.finish();
}

fn bench_projected_navigation(c: &mut Criterion) {
    let fixture = chain_fixture(1);
    let projection = vec!["name".to_string()];
    c.bench_function("rust_store_projected_navigation/one_field", |b| {
        b.iter(|| {
            black_box(
                fixture
                    .store
                    .navigate_reference_projection(
                        black_box(&fixture.start),
                        black_box(&projection),
                    )
                    .expect("projected navigation"),
            )
        });
    });
}

struct FanoutFixture {
    _directory: TempDir,
    store: RustStore,
    target_id: String,
    object_refs: Vec<ObjRef>,
    expected: usize,
}

fn fanout_fixture(fanout: usize) -> FanoutFixture {
    let directory = TempDir::new().expect("fanout benchmark directory");
    let store =
        RustStore::new(directory.path().join("fanout.owl"), 128 * 1024 * 1024).expect("store");
    store.put_schema(&node_schema()).expect("node schema");

    let target_id = "target".to_string();
    let target = Document::with_id(
        target_id.clone(),
        "nodes".to_string(),
        HashMap::from([("name".to_string(), Value::String("target".to_string()))]),
    );
    store.put_document(&target).expect("target");
    let target_ref = store
        .get_objref("nodes", &target_id)
        .expect("target lookup")
        .expect("target reference");

    let sources = (0..fanout)
        .map(|index| {
            Document::with_id(
                format!("source-{index}"),
                "nodes".to_string(),
                HashMap::from([
                    ("name".to_string(), Value::String(format!("source-{index}"))),
                    ("next".to_string(), Value::Reference(target_ref)),
                ]),
            )
        })
        .collect::<Vec<_>>();
    store.put_documents(&sources).expect("fanout sources");
    let object_refs = sources
        .iter()
        .map(|document| {
            store
                .get_objref("nodes", &document.id)
                .expect("source lookup")
                .expect("source reference")
        })
        .collect();

    FanoutFixture {
        _directory: directory,
        store,
        target_id,
        object_refs,
        expected: fanout,
    }
}

fn bench_projected_navigation_batches(c: &mut Criterion) {
    let mut group = c.benchmark_group("rust_store_projected_navigation_batch");
    let projection = vec!["name".to_string()];
    for rows in [16usize, 256, 1024] {
        let fixture = fanout_fixture(rows);
        fixture
            .store
            .checkpoint()
            .expect("checkpoint projected navigation fixture");
        group.bench_with_input(BenchmarkId::new("scalar", rows), &rows, |b, _| {
            b.iter(|| {
                black_box(
                    fixture
                        .object_refs
                        .iter()
                        .map(|object_ref| {
                            fixture
                                .store
                                .navigate_reference_projection(
                                    black_box(object_ref),
                                    black_box(&projection),
                                )
                                .expect("scalar projected navigation")
                        })
                        .collect::<Vec<_>>(),
                )
            });
        });
        group.bench_with_input(BenchmarkId::new("batch", rows), &rows, |b, _| {
            b.iter(|| {
                black_box(
                    fixture
                        .store
                        .navigate_references_projection(
                            black_box(&fixture.object_refs),
                            black_box(&projection),
                        )
                        .expect("batch projected navigation"),
                )
            });
        });
    }
    group.finish();
}

fn bench_reverse_navigation(c: &mut Criterion) {
    let mut group = c.benchmark_group("rust_store_reverse_navigation");
    for fanout in [16usize, 256, 1024] {
        let fixture = fanout_fixture(fanout);
        let references = fixture
            .store
            .list_references_to("nodes", &fixture.target_id, Some("nodes"), Some("next"))
            .expect("reverse references");
        assert_eq!(references.len(), fixture.expected);

        group.bench_with_input(BenchmarkId::new("edge_ids", fanout), &fanout, |b, _| {
            b.iter(|| {
                black_box(
                    fixture
                        .store
                        .list_references_to(
                            black_box("nodes"),
                            black_box(&fixture.target_id),
                            Some("nodes"),
                            Some("next"),
                        )
                        .expect("reverse references"),
                )
            });
        });
        group.bench_with_input(
            BenchmarkId::new("owned_documents", fanout),
            &fanout,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .store
                            .scan_referencing_documents(
                                black_box("nodes"),
                                black_box(&fixture.target_id),
                                Some("nodes"),
                                Some("next"),
                            )
                            .expect("reverse documents"),
                    )
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_forward_navigation,
    bench_projected_navigation,
    bench_projected_navigation_batches,
    bench_reverse_navigation
);
criterion_main!(benches);
