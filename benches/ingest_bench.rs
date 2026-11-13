use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use owl_db::document::{Document, Value};
use owl_db::schema::{FieldType, Schema};
use owl_db::storage::{DocumentStore, RustStore};
use std::collections::HashMap;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    store: RustStore,
    documents: Vec<Document>,
}

fn fixture(document_count: usize) -> Fixture {
    let directory = TempDir::new().expect("benchmark temp directory");
    let store = RustStore::new(
        directory.path().join("ingest-benchmark.owl"),
        256 * 1024 * 1024,
    )
    .expect("benchmark store");
    store
        .put_schema(&Schema::new(
            "events".to_string(),
            HashMap::from([
                ("app".to_string(), FieldType::String),
                ("message".to_string(), FieldType::String),
                ("timestamp".to_string(), FieldType::Integer),
            ]),
        ))
        .expect("schema");
    let documents = (0..document_count)
        .map(|sequence| {
            Document::with_id(
                format!("event-{sequence}"),
                "events".to_string(),
                HashMap::from([
                    ("app".to_string(), Value::String("api".to_string())),
                    (
                        "message".to_string(),
                        Value::String(format!("log payload {sequence}")),
                    ),
                    ("timestamp".to_string(), Value::Integer(sequence as i64)),
                ]),
            )
        })
        .collect();
    Fixture {
        _directory: directory,
        store,
        documents,
    }
}

fn bench_durable_ingest(c: &mut Criterion) {
    let mut group = c.benchmark_group("durable_ingest_batching");
    group.sample_size(20);
    for document_count in [10usize, 100] {
        group.bench_with_input(
            BenchmarkId::new("individual_sync_per_document", document_count),
            &document_count,
            |b, &count| {
                b.iter_batched(
                    || fixture(count),
                    |fixture| {
                        for document in &fixture.documents {
                            fixture.store.put_document(document).expect("single insert");
                        }
                    },
                    BatchSize::SmallInput,
                )
            },
        );
        group.bench_with_input(
            BenchmarkId::new("one_atomic_wal_batch", document_count),
            &document_count,
            |b, &count| {
                b.iter_batched(
                    || fixture(count),
                    |fixture| {
                        fixture
                            .store
                            .put_documents(&fixture.documents)
                            .expect("batch insert");
                    },
                    BatchSize::SmallInput,
                )
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_durable_ingest);
criterion_main!(benches);
