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

/// A row with a list in it, which is kept as a record.
fn note(id: &str, value: i64) -> Document {
    Document::with_id(
        id.to_string(),
        "notes".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(value)),
            (
                "tags".to_string(),
                Value::List(vec![Value::String(format!("tag of {id}")), Value::String("x".repeat(200))]),
            ),
        ]),
    )
}

/// The sizes of the part files of the database at `path`, by part number.
fn parts_on_disk(path: &Path) -> BTreeMap<u64, u64> {
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let part = entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(&format!("{name}.part."))?
                .parse()
                .ok()?;
            Some((part, entry.metadata().ok()?.len()))
        })
        .collect()
}

/// A compaction writes what has changed, and lists the rest from the
/// parts that already hold it. One with nothing to do writes almost
/// nothing; and when rows keep changing, the parts they leave mostly
/// unused are let go, so the database does not grow without end.
#[test]
fn a_compaction_writes_what_changed_and_the_database_stays_its_size() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("listed.owl");
    let store = open(&path);
    store
        .write_batch(vec![WriteOperation::PutSchema(Schema::new(
            "items".to_string(),
            HashMap::from([
                ("n".to_string(), FieldType::Integer),
                ("name".to_string(), FieldType::String),
            ]),
        ))])
        .unwrap();
    const ITEMS: usize = 60_000;
    const NOTES: usize = 20_000;
    for start in (0..ITEMS).step_by(2_000) {
        let batch = (start..start + 2_000)
            .map(|number| WriteOperation::PutDocument(item(&format!("item-{number:08}"), number as i64)))
            .collect();
        store.write_batch(batch).unwrap();
    }
    let mut notes = BTreeMap::new();
    for start in (0..NOTES).step_by(2_000) {
        let batch = (start..start + 2_000)
            .map(|number| {
                notes.insert(format!("note-{number:08}"), number as i64);
                WriteOperation::PutDocument(note(&format!("note-{number:08}"), number as i64))
            })
            .collect();
        store.write_batch(batch).unwrap();
    }
    store.compact_and_reclaim().unwrap();
    let whole: u64 = parts_on_disk(&path).values().sum();
    assert!(whole > 4_000_000, "the database is {whole} bytes, too small to tell anything");

    // Nothing has changed: the new part holds the header, the directory,
    // the indexes and the row locator, and none of the rows.
    store.compact().unwrap();
    let parts = parts_on_disk(&path);
    let newest = *parts.values().last().unwrap();
    assert!(
        newest * 3 < whole,
        "a compaction with nothing to do wrote a part of {newest} bytes for a database of {whole}"
    );
    store.verify_integrity().unwrap();

    // Rows keep changing. Each round rewrites a different twentieth of
    // the notes and compacts.
    for round in 0..24 {
        let batch = (0..NOTES)
            .filter(|number| number % 20 == round % 20)
            .map(|number| {
                let id = format!("note-{number:08}");
                notes.insert(id.clone(), -(round as i64) - 1);
                WriteOperation::PutDocument(note(&id, -(round as i64) - 1))
            })
            .collect();
        store.write_batch(batch).unwrap();
        store.compact().unwrap();
    }
    let after: u64 = parts_on_disk(&path).values().sum();
    assert!(
        after < whole * 3,
        "after every note was rewritten the parts take {after} bytes, and took {whole} to begin with"
    );
    store.verify_integrity().unwrap();

    let check = |store: &RustStore, when: &str| {
        assert_eq!(store.scan_documents("items").unwrap().len(), ITEMS, "items {when}");
        assert_eq!(store.scan_documents("notes").unwrap().len(), NOTES, "notes {when}");
        for (id, value) in notes.iter().step_by(53) {
            let document = store.get_document("notes", id).unwrap().unwrap();
            assert_eq!(document.fields.get("n"), Some(&Value::Integer(*value)), "{id} {when}");
        }
    };
    check(&store, "after the rounds");
    drop(store);
    let store = open(&path);
    check(&store, "once reopened");
    store.verify_integrity().unwrap();
}
