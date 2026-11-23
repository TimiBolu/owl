//! What an open database holds in memory must not grow with its rows.
//!
//! The memory budget covers what the store keeps so as not to read it
//! again. These tests are about the rest: what is held simply because a
//! row exists. They open the same kind of database at two sizes under a
//! small budget, before anything is asked of it, and look at how much more
//! the larger one holds for each row it has more.
//!
//! The process's allocations are counted directly, so the figures do not
//! depend on what the store says about itself.

use owl_db::document::Fields;
use owl_db::schema::{FieldType, Schema};
use owl_db::storage_engine::{AutoCheckpointPolicy, RustStore, WriteOperation};
use owl_db::{Document, Value};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes straight to the system allocator; the count
// beside it does not touch the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        LIVE.fetch_add(size, Ordering::Relaxed);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The count is for the whole process, so one measurement runs at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The budget every database here is opened under.
const BUDGET: usize = 4 * 1024 * 1024;

/// A row with a list in it stays out of the column segments, and is kept
/// as a record of its own.
fn listed(number: usize) -> Document {
    Document::with_id(
        format!("row-{number:08}"),
        "notes".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(number as i64)),
            (
                "tags".to_string(),
                Value::List(vec![
                    Value::String("one".to_string()),
                    Value::String(format!("tag-{}", number % 7)),
                ]),
            ),
        ]),
    )
}

/// A row of plain values, which a compaction moves into column segments.
fn plain(number: usize) -> Document {
    Document::with_id(
        format!("item-{number:08}"),
        "items".to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(number as i64)),
            ("name".to_string(), Value::String(format!("name {number}"))),
        ]),
    )
}

fn build(path: &Path, rows: usize, row: fn(usize) -> Document) {
    let store = RustStore::new(path, 1 << 30).unwrap();
    store
        .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
        .unwrap();
    store
        .write_batch(vec![WriteOperation::PutSchema(Schema::new(
            "items".to_string(),
            HashMap::from([
                ("n".to_string(), FieldType::Integer),
                ("name".to_string(), FieldType::String),
            ]),
        ))])
        .unwrap();
    for start in (0..rows).step_by(1_000) {
        let batch = (start..(start + 1_000).min(rows))
            .map(|number| WriteOperation::PutDocument(row(number)))
            .collect();
        store.write_batch(batch).unwrap();
    }
    store.compact_and_reclaim().unwrap();
}

/// The bytes an open database of this many rows holds before it is asked
/// anything.
fn held_once_open(rows: usize, row: fn(usize) -> Document) -> usize {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("bound.owl");
    build(&path, rows, row);
    let before = LIVE.load(Ordering::Relaxed);
    let store = RustStore::new(&path, 1 << 30).unwrap();
    let held = LIVE.load(Ordering::Relaxed).saturating_sub(before);
    drop(store);
    held
}

/// How many more bytes are held for each row the larger database has more.
fn bytes_a_row(row: fn(usize) -> Document) -> f64 {
    // The budget is read once, when the first database is opened.
    std::env::set_var("OWL_DECODED_BYTES", BUDGET.to_string());
    const SMALL: usize = 20_000;
    const LARGE: usize = 80_000;
    let small = held_once_open(SMALL, row);
    let large = held_once_open(LARGE, row);
    let each = large.saturating_sub(small) as f64 / (LARGE - SMALL) as f64;
    println!(
        "{SMALL} rows hold {:.1} MB once open, {LARGE} rows hold {:.1} MB: {each:.1} bytes for each row more",
        small as f64 / 1e6,
        large as f64 / 1e6
    );
    each
}

/// Rows in column segments are found through what the segments say about
/// themselves, and cost nothing each.
#[test]
fn rows_in_column_segments_cost_nothing_each_once_open() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let each = bytes_a_row(plain);
    assert!(
        each < 4.0,
        "an open database holds {each:.1} bytes for each row in its column segments"
    );
}

/// Nor does a row with a list in it, which stays out of the column
/// segments: it is found through the index of rows by id and the record
/// directory, both of which stay in the checkpoint's files.
#[test]
fn rows_outside_column_segments_cost_nothing_each_once_open() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let each = bytes_a_row(listed);
    assert!(
        each < 4.0,
        "an open database holds {each:.1} bytes for each row outside its column segments"
    );
}
