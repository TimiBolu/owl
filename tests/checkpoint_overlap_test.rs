//! A checkpoint is built and published while writes go on, so the rows it
//! holds are those of an earlier commit than the one the store has reached
//! by the time it is done. Every row must stay readable through that:
//! those the checkpoint moved, and those written while it was built.

use owl_db::document::Fields;
use owl_db::schema::{FieldType, Schema};
use owl_db::storage::DocumentStore;
use owl_db::storage_engine::{AutoCheckpointPolicy, RustStore, WriteOperation};
use owl_db::{Document, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Rows below this number are written before the checkpoints start and
/// never touched again.
const SETTLED: usize = 4_000;

/// A row of plain values, which a compaction moves into column segments.
fn item(number: usize, value: i64) -> Document {
    Document::with_id(
        format!("item-{number:08}"),
        "items".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(value)),
            ("name".to_string(), Value::String(format!("name {number}"))),
        ]),
    )
}

/// A row with a list in it, which stays out of the column segments.
fn note(number: usize, value: i64) -> Document {
    Document::with_id(
        format!("note-{number:08}"),
        "notes".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(value)),
            (
                "tags".to_string(),
                Value::List(vec![Value::String(format!("tag-{}", number % 7))]),
            ),
        ]),
    )
}

fn row(collection: &str, number: usize, value: i64) -> Document {
    match collection {
        "items" => item(number, value),
        _ => note(number, value),
    }
}

fn open(path: &Path) -> RustStore {
    let store = RustStore::new(path, 1 << 30).unwrap();
    store
        .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
        .unwrap();
    store
}

fn load(store: &RustStore, collection: &str, numbers: std::ops::Range<usize>) {
    let numbers: Vec<usize> = numbers.collect();
    for batch in numbers.chunks(1_000) {
        store
            .write_batch(
                batch
                    .iter()
                    .map(|number| WriteOperation::PutDocument(row(collection, *number, *number as i64)))
                    .collect(),
            )
            .unwrap();
    }
}

fn value_of(store: &RustStore, collection: &str, id: &str) -> Option<i64> {
    let document = store
        .get_document(collection, &id.to_string())
        .unwrap_or_else(|error| panic!("reading {id}: {error}"))?;
    match document.fields.get("n") {
        Some(Value::Integer(value)) => Some(*value),
        other => panic!("{id} has n = {other:?}"),
    }
}

/// What the store should hold: the value of each row, by collection and id.
type Model = BTreeMap<(&'static str, usize), i64>;

fn id_of(collection: &str, number: usize) -> String {
    format!("{}-{number:08}", &collection[..4])
}

fn agrees(store: &RustStore, model: &Model, deleted: &[(&'static str, usize)], when: &str) {
    for ((collection, number), value) in model {
        let id = id_of(collection, *number);
        assert_eq!(value_of(store, collection, &id), Some(*value), "{id} {when}");
    }
    for (collection, number) in deleted {
        if !model.contains_key(&(*collection, *number)) {
            let id = id_of(collection, *number);
            assert_eq!(value_of(store, collection, &id), None, "{id} {when}");
        }
    }
    for collection in ["items", "notes"] {
        let expected = model.keys().filter(|(name, _)| *name == collection).count();
        assert_eq!(
            store.scan_documents(collection).unwrap().len(),
            expected,
            "rows of {collection} {when}"
        );
    }
}

/// Rows flushed by an incremental checkpoint are records in its files and
/// nowhere else. A compaction moves them into column segments. If a write
/// lands while it does, the store must still find them.
#[test]
fn rows_flushed_before_a_compaction_stay_readable_when_a_write_overlaps_it() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("overlap.owl");
    let store = Arc::new(open(&path));
    store
        .write_batch(vec![WriteOperation::PutSchema(Schema::new(
            "items".to_string(),
            HashMap::from([
                ("n".to_string(), FieldType::Integer),
                ("name".to_string(), FieldType::String),
            ]),
        ))])
        .unwrap();
    const BASE: usize = 60_000;
    const FLUSHED: usize = 20_000;
    load(&store, "items", 0..BASE);
    store.checkpoint().unwrap();
    load(&store, "items", BASE..BASE + FLUSHED);
    store.checkpoint().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (store, stop) = (store.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut number = BASE + FLUSHED;
            while !stop.load(Ordering::Relaxed) {
                load(&store, "items", number..number + 1);
                number += 1;
            }
            number
        })
    };
    store.compact().unwrap();
    stop.store(true, Ordering::Relaxed);
    let written = writer.join().unwrap();
    assert!(
        written > BASE + FLUSHED,
        "no write overlapped the compaction, so this run tested nothing"
    );

    for number in 0..written {
        let id = id_of("items", number);
        assert_eq!(value_of(&store, "items", &id), Some(number as i64), "{id}");
    }
    assert_eq!(store.scan_documents("items").unwrap().len(), written);
    store.verify_integrity().unwrap();
}

/// Writes, updates and deletes of both kinds of row go on while
/// checkpoints and compactions are taken one after another. The store
/// must agree with what was written throughout, with its own files at the
/// end, and again once reopened.
#[test]
fn a_store_agrees_with_what_was_written_while_checkpoints_overlap_writes() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("overlap.owl");
    let store = Arc::new(open(&path));
    store
        .write_batch(vec![WriteOperation::PutSchema(Schema::new(
            "items".to_string(),
            HashMap::from([
                ("n".to_string(), FieldType::Integer),
                ("name".to_string(), FieldType::String),
            ]),
        ))])
        .unwrap();
    const LOADED: usize = 30_000;
    load(&store, "items", 0..LOADED);
    load(&store, "notes", 0..LOADED);

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (store, stop) = (store.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut model: Model = ["items", "notes"]
                .into_iter()
                .flat_map(|collection| (0..LOADED).map(move |number| ((collection, number), number as i64)))
                .collect();
            let mut deleted = Vec::new();
            let mut next = LOADED;
            let mut seed = 0x9E37_79B9_7F4A_7C15u64;
            let mut random = move |below: usize| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed % below as u64) as usize
            };
            let mut step = 0i64;
            while !stop.load(Ordering::Relaxed) {
                step += 1;
                let collection = ["items", "notes"][random(2)];
                let batch = (0..1 + random(4))
                    .map(|_| match random(4) {
                        0 => {
                            next += 1;
                            model.insert((collection, next), step);
                            WriteOperation::PutDocument(row(collection, next, step))
                        }
                        1 | 2 => {
                            // Any row past the settled ones, written again
                            // whether or not it has been deleted.
                            let number = SETTLED + random(next - SETTLED);
                            model.insert((collection, number), step);
                            WriteOperation::PutDocument(row(collection, number, step))
                        }
                        _ => {
                            let number = SETTLED + random(next - SETTLED);
                            match model.remove(&(collection, number)) {
                                Some(_) => {
                                    deleted.push((collection, number));
                                    WriteOperation::DeleteDocument {
                                        schema: collection.to_string(),
                                        id: id_of(collection, number),
                                    }
                                }
                                None => {
                                    model.insert((collection, number), step);
                                    WriteOperation::PutDocument(row(collection, number, step))
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                store.write_batch(batch).unwrap();
            }
            (model, deleted)
        })
    };

    for round in 0..12 {
        match round % 3 {
            2 => store.compact().map(|_| ()),
            _ => store.checkpoint().map(|_| ()),
        }
        .unwrap();
        // The settled rows are never written, so they can be checked
        // while the writer runs.
        for number in (0..SETTLED).step_by(7) {
            for collection in ["items", "notes"] {
                let id = id_of(collection, number);
                assert_eq!(
                    value_of(&store, collection, &id),
                    Some(number as i64),
                    "{id} after round {round}"
                );
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    let (model, deleted) = writer.join().unwrap();

    agrees(&store, &model, &deleted, "after the checkpoints");
    store.verify_integrity().unwrap();
    store.compact().unwrap();
    agrees(&store, &model, &deleted, "after a quiet compaction");
    store.verify_integrity().unwrap();
    drop(store);
    let store = open(&path);
    agrees(&store, &model, &deleted, "once reopened");
    store.verify_integrity().unwrap();
    // No compaction here found the index pages it built over to be other
    // than it expected.
    assert_eq!(RustStore::index_page_carries().1, 0);
}
