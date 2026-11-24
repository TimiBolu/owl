//! What opening a database costs when its last checkpoint was not a
//! compaction.
//!
//! Such a database is a compacted base and the generations written since.
//! Opening it checks that the indexes, once the generations are applied,
//! cover every row exactly once. That check should cost in proportion to
//! what the generations hold, not to the database: a few rows written
//! since the last compaction should not have every index page read.
//!
//! The count is of the whole process, so this file holds one test.

use owl_db::document::Fields;
use owl_db::schema::{FieldType, Schema};
use owl_db::storage::{DocumentStore, ScalarIndexDefinition};
use owl_db::storage_engine::{AutoCheckpointPolicy, RustStore, WriteOperation};
use owl_db::{Document, Value};
use std::collections::HashMap;
use std::path::Path;

fn item(id: String, value: i64) -> Document {
    Document::with_id(
        id,
        "items".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(value)),
            ("name".to_string(), Value::String(format!("name {value}"))),
        ]),
    )
}

fn open(path: &Path) -> RustStore {
    let store = RustStore::new(path, 1 << 30).unwrap();
    store
        .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
        .unwrap();
    store
}

#[test]
fn opening_after_a_small_checkpoint_does_not_read_every_index_page() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("reopen.owl");
    const LOADED: usize = 60_000;
    {
        let store = open(&path);
        store
            .write_batch(vec![
                WriteOperation::PutSchema(Schema::new(
                    "items".to_string(),
                    HashMap::from([
                        ("n".to_string(), FieldType::Integer),
                        ("name".to_string(), FieldType::String),
                    ]),
                )),
                WriteOperation::PutScalarIndex(ScalarIndexDefinition {
                    name: "by_n".to_string(),
                    schema: "items".to_string(),
                    fields: vec!["n".to_string()],
                    unique: false,
                }),
            ])
            .unwrap();
        for start in (0..LOADED).step_by(2_000) {
            let batch = (start..start + 2_000)
                .map(|number| WriteOperation::PutDocument(item(format!("item-{number:08}"), number as i64)))
                .collect();
            store.write_batch(batch).unwrap();
        }
        store.compact().unwrap();
        store.compact().unwrap();

        // Since the compaction: fifty new rows, fifty rows whose indexed
        // value changes, fifty whose other field changes, fifty deleted.
        // The rows changed lie near one another, so what they change in
        // the index falls on a few of its pages.
        let mut batch = Vec::new();
        for new in 0..50 {
            batch.push(WriteOperation::PutDocument(item(format!("new-{new:04}"), (LOADED + new) as i64)));
        }
        for moved in 0..50 {
            let number = 1_000 + moved * 7;
            batch.push(WriteOperation::PutDocument(item(format!("item-{number:08}"), -(number as i64) - 1)));
        }
        for renamed in 0..50 {
            let number = 1_001 + renamed * 7;
            let mut document = item(format!("item-{number:08}"), number as i64);
            document.fields.insert("name".to_string(), Value::String("renamed".to_string()));
            batch.push(WriteOperation::PutDocument(document));
        }
        for deleted in 0..50 {
            let number = 1_002 + deleted * 7;
            batch.push(WriteOperation::DeleteDocument {
                schema: "items".to_string(),
                id: format!("item-{number:08}"),
            });
        }
        store.write_batch(batch).unwrap();
        store.checkpoint().unwrap();
    }

    // Audit both ways, so that the opening fails if the audit against
    // pages passes what the audit of every entry would not.
    RustStore::audit_opens_both_ways(true);
    drop(open(&path));
    RustStore::audit_opens_both_ways(false);

    let before = RustStore::index_pages_read();
    let started = std::time::Instant::now();
    let store = open(&path);
    let read = RustStore::index_pages_read() - before;
    println!(
        "opening read {read} index pages and took {:.0} ms",
        started.elapsed().as_secs_f64() * 1e3
    );
    assert!(read <= 10, "opening read {read} index pages");

    assert_eq!(store.scan_documents("items").unwrap().len(), LOADED);
    let found = |value: i64| {
        store
            .lookup_scalar_index("items", "by_n", &[Value::Integer(value)])
            .unwrap()
            .len()
    };
    assert_eq!(found(LOADED as i64 + 7), 1, "a new row");
    assert_eq!(found(1_000), 0, "a row's old value");
    assert_eq!(found(-1_001), 1, "a row's new value");
    assert_eq!(found(1_001), 1, "a row changed elsewhere");
    assert_eq!(found(1_002), 0, "a deleted row");
    assert_eq!(found(31_337), 1, "an untouched row");
    store.verify_integrity().unwrap();
}
