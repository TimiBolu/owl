//! How much of its indexes a store keeps decoded.
//!
//! A store with a budget in bytes gives a sixteenth of it to index pages
//! read from its files, across all its indexes, from the moment the
//! budget is set: reading through a whole index must not leave the whole
//! index in memory, whether or not any row has yet been let go.

use owl_db::document::Fields;
use owl_db::schema::{FieldType, Schema};
use owl_db::storage::ScalarIndexDefinition;
use owl_db::storage_engine::{AutoCheckpointPolicy, RustStore, WriteOperation};
use owl_db::{Document, Value};
use std::collections::HashMap;
use std::path::Path;

fn item(number: usize) -> Document {
    Document::with_id(
        format!("item-{number:08}"),
        "items".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(number as i64)),
            ("m".to_string(), Value::Integer((number * 7 % 60_000) as i64)),
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
fn index_pages_held_keep_within_their_share_of_the_budget() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("pages.owl");
    const LOADED: usize = 60_000;
    {
        let store = open(&path);
        let index = |name: &str, field: &str| {
            WriteOperation::PutScalarIndex(ScalarIndexDefinition {
                name: name.to_string(),
                schema: "items".to_string(),
                fields: vec![field.to_string()],
                unique: false,
            })
        };
        store
            .write_batch(vec![
                WriteOperation::PutSchema(Schema::new(
                    "items".to_string(),
                    HashMap::from([
                        ("n".to_string(), FieldType::Integer),
                        ("m".to_string(), FieldType::Integer),
                    ]),
                )),
                index("by_n", "n"),
                index("by_m", "m"),
            ])
            .unwrap();
        for start in (0..LOADED).step_by(2_000) {
            let batch = (start..start + 2_000)
                .map(|number| WriteOperation::PutDocument(item(number)))
                .collect();
            store.write_batch(batch).unwrap();
        }
        store.compact().unwrap();
    }

    let store = open(&path);
    // Far more than the few hundred rows read below take, so no row is
    // let go and the store never comes to think memory short.
    const BUDGET: usize = 16 << 20;
    store.set_decoded_bytes_budget(Some(BUDGET)).unwrap();
    let (_, most) = store.index_page_bytes_held().unwrap();
    assert_eq!(most, BUDGET / 16);

    // A value from every stretch of both indexes, so that every page of
    // each is read.
    let mut most_held = 0;
    for value in (0..LOADED).step_by(150) {
        for name in ["by_n", "by_m"] {
            let found = store
                .lookup_scalar_index("items", name, &[Value::Integer(value as i64)])
                .unwrap();
            assert_eq!(found.len(), 1, "rows under {name} = {value}");
        }
        let (held, _) = store.index_page_bytes_held().unwrap();
        most_held = most_held.max(held);
    }
    println!("index pages held at most {most_held} bytes of {most} allowed");
    assert!(most_held <= most, "index pages held {most_held} bytes, over the {most} allowed");
    // The indexes are larger than their share, so the share was used.
    assert!(most_held > most / 2, "index pages held only {most_held} bytes");

    // New indexes replace these at a compaction: what the old ones held
    // is no longer counted.
    store.write_batch(vec![WriteOperation::PutDocument(item(LOADED))]).unwrap();
    store.compact().unwrap();
    let (held, _) = store.index_page_bytes_held().unwrap();
    assert!(held <= most, "index pages held {held} bytes after a compaction");
    store.verify_integrity().unwrap();
}
