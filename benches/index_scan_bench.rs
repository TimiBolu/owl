use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use owl_db::document::{Document, DocumentId, Value};
use owl_db::error::OQLResult;
use owl_db::executor::QueryExecutor;
use owl_db::parser::{parse_oql, ComparisonOperator, OQLQuery};
use owl_db::schema::{FieldType, Schema};
use owl_db::storage::{
    DocumentStore, RustStore, ScalarIndexDefinition, ScanPredicate, WriteOperation,
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    store: Arc<RustStore>,
    executor: QueryExecutor<RustStore>,
    unfused_executor: QueryExecutor<UnfusedStore>,
    indexed_query: OQLQuery,
    scan_oracle_query: OQLQuery,
    index_union_query: OQLQuery,
    ordered_index_union_query: OQLQuery,
    projected_seq_scan_query: OQLQuery,
    broad_projected_seq_scan_query: OQLQuery,
    or_seq_scan_query: OQLQuery,
}

struct UnfusedStore {
    inner: Arc<RustStore>,
}

impl DocumentStore for UnfusedStore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>> {
        self.inner.get_document(schema, id)
    }

    fn put_document(&self, document: &Document) -> OQLResult<()> {
        self.inner.put_document(document)
    }

    fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool> {
        self.inner.delete_document(schema, id)
    }

    fn scan_documents(&self, schema: &str) -> OQLResult<Vec<Document>> {
        self.inner.scan_documents(schema)
    }

    fn scan_documents_projection(
        &self,
        schema: &str,
        fields: &[String],
    ) -> OQLResult<Vec<Document>> {
        self.inner.scan_documents_projection(schema, fields)
    }

    fn scan_documents_filtered_projection(
        &self,
        schema: &str,
        filters: &[owl_db::storage::ScanPredicate],
        fields: Option<&[String]>,
    ) -> OQLResult<Option<Vec<Document>>> {
        self.inner
            .scan_documents_filtered_projection(schema, filters, fields)
    }

    fn get_schema(&self, name: &str) -> OQLResult<Option<Schema>> {
        self.inner.get_schema(name)
    }

    fn put_schema(&self, schema: &Schema) -> OQLResult<()> {
        self.inner.put_schema(schema)
    }

    fn list_schemas(&self) -> OQLResult<Vec<String>> {
        self.inner.list_schemas()
    }

    fn list_scalar_indexes(&self, schema: Option<&str>) -> OQLResult<Vec<ScalarIndexDefinition>> {
        self.inner.list_scalar_indexes(schema)
    }
}

fn fixture(document_count: usize) -> Fixture {
    let directory = TempDir::new().expect("benchmark temp directory");
    let store = Arc::new(
        RustStore::new(
            directory.path().join("index-benchmark.owl"),
            256 * 1024 * 1024,
        )
        .expect("benchmark store"),
    );
    store
        .put_schema(&Schema::new(
            "events".to_string(),
            HashMap::from([
                ("level".to_string(), FieldType::String),
                ("message".to_string(), FieldType::String),
                ("payload".to_string(), FieldType::String),
                ("sequence".to_string(), FieldType::Integer),
            ]),
        ))
        .expect("schema");
    store
        .upsert_scalar_index(ScalarIndexDefinition {
            name: "events_by_level_sequence".to_string(),
            schema: "events".to_string(),
            fields: vec!["level".to_string(), "sequence".to_string()],
            unique: false,
        })
        .expect("index");

    let mut writes = Vec::with_capacity(document_count);
    for sequence in 0..document_count {
        let level = if sequence % 100 == 0 {
            "rare"
        } else {
            "common"
        };
        writes.push(WriteOperation::PutDocument(Document::with_id(
            format!("event-{sequence}"),
            "events".to_string(),
            HashMap::from([
                ("level".to_string(), Value::String(level.to_string())),
                (
                    "message".to_string(),
                    Value::String(format!("event payload {sequence}")),
                ),
                ("payload".to_string(), Value::String("x".repeat(1024))),
                ("sequence".to_string(), Value::Integer(sequence as i64)),
            ]),
        )));
    }
    store.write_batch(writes).expect("document batch");

    let unfused_store = Arc::new(UnfusedStore {
        inner: store.clone(),
    });
    Fixture {
        _directory: directory,
        store: store.clone(),
        executor: QueryExecutor::new_with_real_clock(store),
        unfused_executor: QueryExecutor::new_with_real_clock(unfused_store),
        indexed_query: parse_oql(
            r#"events.where { .level == "rare" }.sort { .sequence.desc }.select { message }.limit 10"#,
        )
        .expect("indexed query"),
        // The equivalent OR predicate deliberately stays on the scan fallback,
        // providing an execution-level oracle rather than a synthetic SeqScan.
        scan_oracle_query: parse_oql(
            r#"events.where { .level == "rare" or .payload == "never" }.sort { .sequence.desc }.select { message }.limit 10"#,
        )
        .expect("scan query"),
        index_union_query: parse_oql(
            r#"events.where { .level == "rare" or .level == "never" }.select { message }"#,
        )
        .expect("index union query"),
        ordered_index_union_query: parse_oql(
            r#"events.where { .level == "rare" or .level == "common" }.sort { .sequence.desc }.select { message }.limit 10"#,
        )
        .expect("ordered index union query"),
        projected_seq_scan_query: parse_oql(
            r#"events.where { .message == "event payload 500" }.select { sequence }"#,
        )
        .expect("projected sequential scan query"),
        broad_projected_seq_scan_query: parse_oql(
            r#"events.where { .sequence >= 0 }.select { message }"#,
        )
        .expect("broad projected sequential scan query"),
        // Neither branch has an index, so this exercises the safe projected
        // SeqScan path while retaining overlap/deduplication semantics.
        or_seq_scan_query: parse_oql(
            r#"events.where { .message == "event payload 500" or .message == "event payload 500" }.select { sequence }"#,
        )
        .expect("OR sequential scan query"),
    }
}

fn bench_oql_index_against_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("oql_native_index_vs_scan");
    for document_count in [1_000usize, 10_000] {
        let fixture = fixture(document_count);
        let explain = fixture
            .executor
            .explain_indexed_read(&fixture.indexed_query)
            .expect("explain")
            .expect("ordered native plan");
        assert!(!explain.contains("Sort keys="), "{explain}");
        assert!(explain.contains("direction=Descending"), "{explain}");
        assert!(explain.contains("pushed_limit=Some(10)"), "{explain}");
        assert!(fixture
            .executor
            .explain_indexed_read(&fixture.scan_oracle_query)
            .expect("explain")
            .is_none());
        assert!(fixture
            .executor
            .explain_indexed_read(&fixture.index_union_query)
            .expect("explain")
            .is_some_and(|plan| plan.contains("IndexUnion branches=2")));
        let ordered_union = fixture
            .executor
            .explain_indexed_read(&fixture.ordered_index_union_query)
            .expect("explain")
            .expect("ordered union");
        assert!(ordered_union.contains("order_fields=[\"sequence\"]"));
        assert!(ordered_union.contains("pushed_limit=Some(10)"));
        assert!(!ordered_union.contains("Sort keys="));
        let projected = fixture
            .executor
            .execute_query_with_plan(&fixture.projected_seq_scan_query)
            .expect("projected plan");
        assert!(projected
            .physical_plan
            .as_deref()
            .is_some_and(|plan| plan.contains("StorageScan")));
        assert!(fixture
            .executor
            .execute_query_with_plan(&fixture.or_seq_scan_query)
            .expect("OR sequential plan")
            .physical_plan
            .is_some_and(|plan| plan.contains("StorageScan")));

        group.bench_with_input(
            BenchmarkId::new("native_ordered_top_k", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.indexed_query))
                            .expect("indexed execution"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("scan_oracle", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.scan_oracle_query))
                            .expect("scan execution"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("native_index_union", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.index_union_query))
                            .expect("index union execution"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("native_ordered_union_top_k", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.ordered_index_union_query))
                            .expect("ordered index union execution"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("projected_seq_scan", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.projected_seq_scan_query))
                            .expect("projected sequential scan"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("projected_seq_scan_unfused", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .unfused_executor
                            .execute_query(black_box(&fixture.projected_seq_scan_query))
                            .expect("unfused projected sequential scan"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("plan_projected_seq_scan", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .explain_indexed_read(black_box(&fixture.projected_seq_scan_query))
                            .expect("fused scan planning"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("plan_projected_seq_scan_unfused", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .unfused_executor
                            .explain_indexed_read(black_box(&fixture.projected_seq_scan_query))
                            .expect("unfused scan planning"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("broad_projected_batch_query", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query_batches(black_box(
                                &fixture.broad_projected_seq_scan_query,
                            ))
                            .expect("broad projected batch query"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("broad_projected_document_compat", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.broad_projected_seq_scan_query))
                            .expect("broad projected document compatibility query"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("broad_projected_seq_scan_unfused", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .unfused_executor
                            .execute_query(black_box(&fixture.broad_projected_seq_scan_query))
                            .expect("unfused broad projected sequential scan"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("or_projected_seq_scan", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.or_seq_scan_query))
                            .expect("OR sequential scan"),
                    )
                })
            },
        );
        let broad_filter = [ScanPredicate::Comparison {
            field: "sequence".to_string(),
            operator: ComparisonOperator::GreaterEqual,
            value: Value::Integer(0),
        }];
        let broad_fields = ["message".to_string()];
        group.bench_with_input(
            BenchmarkId::new("storage_columnar_scan", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .store
                            .scan_projected_batches(
                                "events",
                                black_box(&broad_filter),
                                black_box(&broad_fields),
                                1024,
                            )
                            .expect("columnar storage scan")
                            .expect("native columnar batches"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("columnar_document_materialization", document_count),
            &document_count,
            |b, _| {
                b.iter_batched(
                    || {
                        fixture
                            .store
                            .scan_projected_batches("events", &broad_filter, &broad_fields, 1024)
                            .expect("columnar storage scan")
                            .expect("native columnar batches")
                    },
                    |batches| {
                        black_box(
                            batches
                                .into_iter()
                                .flat_map(|batch| batch.into_documents())
                                .collect::<Vec<_>>(),
                        )
                    },
                    criterion::BatchSize::SmallInput,
                )
            },
        );

        fixture.store.checkpoint().expect("benchmark checkpoint");
        group.bench_with_input(
            BenchmarkId::new("checkpointed_projected_seq_scan", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.projected_seq_scan_query))
                            .expect("checkpointed projected sequential scan"),
                    )
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("checkpointed_broad_projected_seq_scan", document_count),
            &document_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        fixture
                            .executor
                            .execute_query(black_box(&fixture.broad_projected_seq_scan_query))
                            .expect("checkpointed broad projected sequential scan"),
                    )
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_oql_index_against_scan);
criterion_main!(benches);
