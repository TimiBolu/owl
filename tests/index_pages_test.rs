//! How many index pages a compaction writes.
//!
//! New rows whose ids fall in between the ids of rows already compacted
//! leave no column segment untouched: every segment has a new row inside
//! its span and is built again. The rows in those segments have not
//! changed, though, so their index entries have not either, and only the
//! pages the new rows' own entries land on should be written.
//!
//! The counts are of the whole process, so this file holds one test.

use owl_db::document::Fields;
use owl_db::schema::{FieldType, Schema};
use owl_db::storage::{DocumentStore, ScalarIndexDefinition};
use owl_db::storage_engine::{AutoCheckpointPolicy, RustStore, WriteOperation};
use owl_db::{Document, Value};
use std::collections::HashMap;

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

#[test]
fn rows_arriving_between_others_do_not_have_every_index_page_written_again() {
    let directory = tempfile::TempDir::new().unwrap();
    let store = RustStore::new(directory.path().join("pages.owl"), 1 << 30).unwrap();
    store
        .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
        .unwrap();
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
    const LOADED: usize = 60_000;
    for start in (0..LOADED).step_by(2_000) {
        let batch = (start..start + 2_000)
            .map(|number| WriteOperation::PutDocument(item(format!("item-{number:08}"), number as i64)))
            .collect();
        store.write_batch(batch).unwrap();
    }
    store.compact().unwrap();
    store.compact().unwrap();

    // Two hundred new rows, each with an id just after that of an
    // existing row, spread evenly: one lands inside every segment. Their
    // values of `n` are past every other, so in the index on `n` they
    // belong together at the end.
    let before = RustStore::index_pages_carried_and_written();
    let batch = (0..200)
        .map(|new| {
            let beside = new * (LOADED / 200);
            WriteOperation::PutDocument(item(format!("item-{beside:08}-new"), (LOADED + new) as i64))
        })
        .collect();
    store.write_batch(batch).unwrap();
    store.compact().unwrap();
    let after = RustStore::index_pages_carried_and_written();
    let (carried, written) = (after.0 - before.0, after.1 - before.1);
    println!("that compaction carried {carried} index pages and wrote {written}");
    assert!(
        written * 4 < carried,
        "a compaction of 200 new rows among {LOADED} wrote {written} index pages and carried {carried}"
    );

    assert_eq!(store.scan_documents("items").unwrap().len(), LOADED + 200);
    for value in [0, 31_337, LOADED as i64 - 1, LOADED as i64, LOADED as i64 + 199] {
        assert_eq!(
            store
                .lookup_scalar_index("items", "by_n", &[Value::Integer(value)])
                .unwrap()
                .len(),
            1,
            "rows the index finds under n = {value}"
        );
    }
    store.verify_integrity().unwrap();
    assert_eq!(RustStore::index_page_carries().1, 0);
}
