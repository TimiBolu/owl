//! A compaction carries a column segment into the new checkpoint as it is
//! when none of its rows has changed and nothing new falls among them.
//! Whatever it carries and whatever it builds again, the result must be
//! the same database: every row, every index entry, before and after it
//! is reopened.

use owl_db::document::Fields;
use owl_db::schema::{FieldType, Schema};
use owl_db::storage::{DocumentStore, ScalarIndexDefinition};
use owl_db::storage_engine::{AutoCheckpointPolicy, RustStore, WriteOperation};
use owl_db::{Document, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

fn item(id: &str, value: i64) -> Document {
    Document::with_id(
        id.to_string(),
        "items".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(value)),
            ("name".to_string(), Value::String(format!("name of {id}"))),
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

/// The store holds exactly these rows, and its index on `n` finds each of
/// them under its value and nothing else.
fn agrees(store: &RustStore, rows: &BTreeMap<String, i64>, when: &str) {
    assert_eq!(store.scan_documents("items").unwrap().len(), rows.len(), "row count {when}");
    for (id, value) in rows.iter().step_by(37) {
        let document = store
            .get_document("items", id)
            .unwrap()
            .unwrap_or_else(|| panic!("{id} is missing {when}"));
        assert_eq!(document.fields.get("n"), Some(&Value::Integer(*value)), "{id} {when}");
    }
    let mut by_value = HashMap::<i64, usize>::new();
    for value in rows.values() {
        *by_value.entry(*value).or_default() += 1;
    }
    for value in -3..1_003 {
        let found = store
            .lookup_scalar_index("items", "by_n", &[Value::Integer(value)])
            .unwrap()
            .len();
        assert_eq!(
            found,
            by_value.get(&value).copied().unwrap_or(0),
            "rows the index finds under n = {value} {when}"
        );
    }
    store.verify_integrity().unwrap_or_else(|error| panic!("integrity {when}: {error}"));
}

#[test]
fn a_compaction_that_carries_segments_over_leaves_the_same_database() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("carry.owl");
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

    // Enough rows for several segments.
    let mut rows = BTreeMap::new();
    const LOADED: usize = 200_000;
    for start in (0..LOADED).step_by(2_000) {
        let batch = (start..start + 2_000)
            .map(|number| {
                let id = format!("item-{number:08}");
                rows.insert(id.clone(), (number % 1_000) as i64);
                WriteOperation::PutDocument(item(&id, (number % 1_000) as i64))
            })
            .collect();
        store.write_batch(batch).unwrap();
    }
    store.compact().unwrap();
    agrees(&store, &rows, "after the first compaction");

    // Nothing has changed: every segment can be carried.
    store.compact().unwrap();
    agrees(&store, &rows, "after a compaction with nothing to do");

    // One row of the first segment is written again, one of a later
    // segment is deleted, a new row falls inside another, and new rows
    // follow the last. The segments in between are untouched.
    let mut batch = Vec::new();
    rows.insert("item-00000010".to_string(), -1);
    batch.push(WriteOperation::PutDocument(item("item-00000010", -1)));
    rows.remove("item-00090000");
    batch.push(WriteOperation::DeleteDocument {
        schema: "items".to_string(),
        id: "item-00090000".to_string(),
    });
    rows.insert("item-00150000-between".to_string(), -2);
    batch.push(WriteOperation::PutDocument(item("item-00150000-between", -2)));
    for number in LOADED..LOADED + 500 {
        let id = format!("item-{number:08}");
        rows.insert(id.clone(), 1_001);
        batch.push(WriteOperation::PutDocument(item(&id, 1_001)));
    }
    store.write_batch(batch).unwrap();
    agrees(&store, &rows, "after the changes");
    store.compact().unwrap();
    agrees(&store, &rows, "after compacting the changes");

    // Through an incremental checkpoint first, then a compaction.
    rows.insert("item-00000020".to_string(), -3);
    store
        .write_batch(vec![WriteOperation::PutDocument(item("item-00000020", -3))])
        .unwrap();
    store.checkpoint().unwrap();
    rows.remove("item-00199999");
    store
        .write_batch(vec![WriteOperation::DeleteDocument {
            schema: "items".to_string(),
            id: "item-00199999".to_string(),
        }])
        .unwrap();
    store.compact().unwrap();
    agrees(&store, &rows, "after a checkpoint and a compaction");

    drop(store);
    let store = open(&path);
    agrees(&store, &rows, "once reopened");
    store.compact().unwrap();
    agrees(&store, &rows, "after compacting the reopened database");
}
