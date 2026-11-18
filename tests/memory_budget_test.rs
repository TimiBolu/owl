//! A budget on decoded documents.
//!
//! A row with a list, an object or a vector in it lives outside the column
//! segments. Without a budget it is held decoded for as long as the
//! database is open. With one, only so many are, and the rest are kept as
//! their records and decoded when read. That changes how much memory the
//! database takes and how fast it reads. It must not change one answer.

use owl_db::storage::{AutoCheckpointPolicy, DocumentStore, RustStore};
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, QueryExecutor, Value};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

const SIZE: usize = 64 << 20;
const ORDERS: usize = 400;

fn script(store: &Arc<RustStore>, source: &str) {
    let file = parse_oql_file(source).unwrap_or_else(|error| panic!("{error}\n{source}"));
    let report = execute_oql_file(&file, &QueryExecutor::new_with_real_clock(store.clone())).unwrap();
    assert!(report.errors.is_empty(), "{:?}\n{source}", report.errors);
}

fn plain(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(value) => serde_json::json!(value),
        Value::Integer(value) => serde_json::json!(value),
        Value::Float(value) => serde_json::json!(value),
        Value::String(value) => serde_json::json!(value),
        Value::DateTime(value) => serde_json::json!(value.to_rfc3339()),
        Value::List(items) => serde_json::Value::Array(items.iter().map(plain).collect()),
        Value::Object(fields) => serde_json::Value::Object(
            fields.iter().map(|(name, value)| (name.clone(), plain(value))).collect(),
        ),
        other => panic!("a reference should have become an id: {other:?}"),
    }
}

fn rows(store: &Arc<RustStore>, query: &str) -> Result<Vec<String>, String> {
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    Ok(executor
        .execute_query(&parse_oql(query).unwrap())
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| {
            let fields: serde_json::Map<String, serde_json::Value> = row
                .fields
                .iter()
                .map(|(name, value)| (name.clone(), plain(&executor.with_reference_ids(value).unwrap())))
                .collect();
            serde_json::Value::Object(fields).to_string()
        })
        .collect())
}

/// Customers (plain rows, which a compaction moves into column segments)
/// and orders (rows with a list of line items, which stay outside them).
fn load(store: &Arc<RustStore>) {
    script(
        store,
        r#"
        schema customers { id: String, name: String, tier: String, index(.tier) }
        schema orders {
          id: String,
          customer: Ref(customers),
          total: Float,
          status: String,
          items: List({ sku: String, qty: Integer }),
          note: Optional(String),
          index(.status, .total)
        }
        "#,
    );
    let mut customers = String::from("customers.insert [\n");
    for index in 0..40 {
        customers.push_str(&format!(
            "  {{ id: \"c{index:02}\", name: \"Customer {index}\", tier: \"{}\" }},\n",
            ["gold", "plain"][index % 2]
        ));
    }
    customers.truncate(customers.len() - 2);
    customers.push_str("\n]");
    script(store, &customers);
    let mut orders = String::from("orders.insert [\n");
    for index in 0..ORDERS {
        orders.push_str(&format!(
            "  {{ id: \"o{index:03}\", customer: \"c{:02}\", total: {}.25, status: \"{}\", items: [{{ sku: \"s{}\", qty: {} }}, {{ sku: \"t{}\", qty: 1 }}]{} }},\n",
            index % 40,
            index,
            ["paid", "open", "void"][index % 3],
            index % 7,
            1 + index % 4,
            index % 5,
            if index % 6 == 0 { ", note: \"n\"" } else { "" },
        ));
    }
    orders.truncate(orders.len() - 2);
    orders.push_str("\n]");
    script(store, &orders);
}

const QUESTIONS: [&str; 14] = [
    "orders.count()",
    "orders.sort { .id.asc }.limit(7)",
    "orders.where { .id == \"o123\" }",
    "orders.where { .status == \"paid\" && .total > 200 }.sort { .total.desc, .id.asc }.limit(10).select { id, total }",
    "orders.group_by { .status }.select { status, n: count(), total: sum(.total) }",
    "orders.group_by { .customer.tier }.select { tier, n: count(), noted: count(.note) }",
    "orders.where { .customer.tier == \"gold\" && .total > 350 }.sort { .id.asc }.select { id, who: .customer.name }",
    "orders.where { .status == \"void\" }.items.group_by { .sku }.select { sku, units: sum(.qty) }",
    "orders.where { .total > 390 }.select { id, items, note }",
    "customers.sort { .id.asc }.select { id, orders: .orders.count(), spent: .orders.sum(.total), any_void: .orders.where { .status == \"void\" }.exists() }",
    "customers.where { .tier == \"gold\" }.sort { .id.asc }.limit(5).select { id, latest: .orders.sort { .total.desc }.limit(2).select { id, total } }",
    "orders.where { matches(.status, \"^(paid|open)$\") && .note != null }.count()",
    "orders.select { lines: sum(.items.count()), biggest: max(.total) }",
    "customers.where { !.orders.where { .total > 395 }.exists() }.count()",
];

fn answers(store: &Arc<RustStore>) -> Vec<Result<Vec<String>, String>> {
    QUESTIONS.iter().map(|question| rows(store, question)).collect()
}

/// Compare two sets of answers, naming the first question that differs.
fn same(actual: &[Result<Vec<String>, String>], expected: &[Result<Vec<String>, String>], when: &str) {
    for ((question, actual), expected) in QUESTIONS.iter().zip(actual).zip(expected) {
        if actual != expected {
            let (short_actual, short_expected) = (format!("{actual:?}"), format!("{expected:?}"));
            let differ = short_actual
                .chars()
                .zip(short_expected.chars())
                .position(|(left, right)| left != right)
                .unwrap_or(0);
            let from = differ.saturating_sub(80);
            panic!(
                "{when}: {question}\n  actual:   ..{}\n  expected: ..{}",
                short_actual.chars().skip(from).take(200).collect::<String>(),
                short_expected.chars().skip(from).take(200).collect::<String>()
            );
        }
    }
}

/// What an unbudgeted database answers, for comparison.
fn reference(directory: &Path) -> (Arc<RustStore>, Vec<Result<Vec<String>, String>>) {
    let store = Arc::new(RustStore::new(directory.join("reference.owl"), SIZE).unwrap());
    load(&store);
    let expected = answers(&store);
    for (question, answer) in QUESTIONS.iter().zip(&expected) {
        assert!(answer.as_ref().is_ok_and(|rows| !rows.is_empty()), "{question}: {answer:?}");
    }
    (store, expected)
}

#[test]
fn a_budget_bounds_what_is_decoded_and_changes_no_answer() {
    let home = TempDir::new().unwrap();
    let (unbounded, expected) = reference(home.path());
    assert!(unbounded.decoded_documents() >= ORDERS);

    for budget in [0, 1, 25, 10_000] {
        let path = home.path().join(format!("budget-{budget}.owl"));
        let store = Arc::new(RustStore::with_decoded_budget(&path, SIZE, budget).unwrap());
        load(&store);
        assert!(store.decoded_documents() <= budget, "budget {budget}: {}", store.decoded_documents());
        assert_eq!(answers(&store), expected, "budget {budget}, as loaded");
        assert!(store.decoded_documents() <= budget);

        // The same once a checkpoint has the rows on disk, and once a
        // compaction has moved the plain ones into column segments.
        store.checkpoint().unwrap();
        assert_eq!(answers(&store), expected, "budget {budget}, checkpointed");
        store.compact().unwrap();
        assert_eq!(answers(&store), expected, "budget {budget}, compacted");
        assert!(store.decoded_documents() <= budget);
        store.verify_integrity().unwrap();

        // And after closing and opening again under the same budget, where
        // nothing beyond it is decoded on the way in.
        drop(store);
        let store = Arc::new(RustStore::with_decoded_budget(&path, SIZE, budget).unwrap());
        assert!(store.decoded_documents() <= budget, "budget {budget}, reopened");
        assert_eq!(answers(&store), expected, "budget {budget}, reopened");
        assert!(store.decoded_documents() <= budget);
    }
}

#[test]
fn rows_held_as_records_are_written_to_like_any_other() {
    let home = TempDir::new().unwrap();
    let (unbounded, _) = reference(home.path());
    let path = home.path().join("bounded.owl");
    let bounded = Arc::new(RustStore::with_decoded_budget(&path, SIZE, 10).unwrap());
    load(&bounded);
    bounded.checkpoint().unwrap();

    let changes = [
        r#"orders.where { .id == "o005" }.update { total: 9999.5, note: "changed" }"#,
        r#"orders.where { .status == "void" && .total > 300 }.delete"#,
        r#"orders.where { .customer == "c07" }.update { status: "paid" }"#,
        r#"orders.insert { id: "o900", customer: "c01", total: 1.5, status: "open", items: [{ sku: "new", qty: 3 }] }"#,
        r#"orders.where { .id == "o010" }.update { items: [{ sku: "only", qty: 8 }] }"#,
        r#"customers.where { .id == "c39" }.update { tier: "gold" }"#,
    ];
    for (step, change) in changes.iter().enumerate() {
        let (left, right) = (rows(&bounded, change), rows(&unbounded, change));
        assert_eq!(left.map(|rows| rows.len()), right.map(|rows| rows.len()), "{change}");
        assert_eq!(answers(&bounded), answers(&unbounded), "after {change}");
        assert!(bounded.decoded_documents() <= 10);
        if step % 2 == 1 {
            bounded.checkpoint().unwrap();
        }
    }
    // What is refused is refused: a reference to nothing, a taken id.
    for refused in [
        r#"orders.insert { id: "o901", customer: "nobody", total: 1.0, status: "open", items: [] }"#,
        r#"orders.insert { id: "o005", customer: "c01", total: 1.0, status: "open", items: [] }"#,
        r#"customers.where { .id == "c01" }.delete"#,
    ] {
        assert_eq!(rows(&bounded, refused).is_err(), rows(&unbounded, refused).is_err(), "{refused}");
        assert!(rows(&bounded, refused).is_err(), "{refused}");
    }

    // A transaction sees its own writes over rows held as records.
    let transaction = bounded.begin_transaction().unwrap();
    {
        let _bound = bounded.bind_transaction(transaction).unwrap();
        rows(&bounded, r#"orders.where { .id == "o020" }.update { total: 5.0 }"#).unwrap();
        assert_eq!(
            rows(&bounded, r#"orders.where { .id == "o020" }.select { total }"#).unwrap(),
            vec![r#"{"total":5.0}"#]
        );
    }
    bounded.rollback_transaction(transaction).unwrap();
    assert_eq!(answers(&bounded), answers(&unbounded));

    // All of it survives being closed, opened without a budget, and
    // opened with one again.
    let expected = answers(&unbounded);
    drop(bounded);
    let reopened = Arc::new(RustStore::new(&path, SIZE).unwrap());
    assert_eq!(answers(&reopened), expected);
    reopened.verify_integrity().unwrap();
    drop(reopened);
    let reopened = Arc::new(RustStore::with_decoded_budget(&path, SIZE, 3).unwrap());
    assert_eq!(answers(&reopened), expected);
    assert!(reopened.decoded_documents() <= 3);
}

#[test]
fn the_budget_of_an_open_database_can_be_changed() {
    let home = TempDir::new().unwrap();
    let (store, expected) = reference(home.path());
    store.checkpoint().unwrap();
    let held = store.decoded_documents();
    assert!(held >= ORDERS);

    // Tightened: documents are let go at once, including ones whose
    // records a checkpoint had already taken out of memory.
    store.set_decoded_budget(Some(20)).unwrap();
    assert!(store.decoded_documents() <= 20);
    assert_eq!(answers(&store), expected);
    store.set_decoded_budget(Some(0)).unwrap();
    assert_eq!(store.decoded_documents(), 0);
    assert_eq!(answers(&store), expected);
    store.checkpoint().unwrap();
    assert_eq!(answers(&store), expected);

    // Lifted: reading brings documents back and they stay.
    store.set_decoded_budget(None).unwrap();
    assert_eq!(answers(&store), expected);
    rows(&store, "customers.select { id, n: .orders.count() }").unwrap();
    assert!(store.decoded_documents() > 20);
    assert_eq!(answers(&store), expected);
    store.verify_integrity().unwrap();
}

#[test]
fn a_backup_of_a_budgeted_database_restores_whole() {
    let home = TempDir::new().unwrap();
    let (_, expected) = reference(home.path());
    let store = Arc::new(RustStore::with_decoded_budget(home.path().join("small.owl"), SIZE, 5).unwrap());
    load(&store);
    store.checkpoint().unwrap();
    rows(&store, r#"orders.where { .id == "o001" }.update { note: "after the checkpoint" }"#).unwrap();
    let after = answers(&store);
    assert_ne!(after, expected);

    let backup = home.path().join("backup");
    store.backup(&backup).unwrap();
    let restored = Arc::new(RustStore::restore(&backup, home.path().join("restored.owl"), SIZE).unwrap());
    assert_eq!(answers(&restored), after);
}

/// Under a budget, a writer that outruns the background checkpoints waits
/// for them, so what has been written but not checkpointed stays small. The
/// checkpoints then happen as the data is loaded, with no one asking, and
/// the rows they cover are read back from the files.
#[test]
fn a_bounded_store_checkpoints_as_it_is_written_to() {
    let home = TempDir::new().unwrap();
    let (_unbounded, expected) = reference(home.path());
    let path = home.path().join("bounded.owl");
    let store = Arc::new(RustStore::with_decoded_budget(&path, SIZE, 0).unwrap());
    // A checkpoint is due after a few kilobytes of log.
    store
        .configure_auto_checkpoint(AutoCheckpointPolicy { wal_bytes: 8 * 1024, commits: 0 })
        .unwrap();
    load(&store);
    let status = store.maintenance_status().unwrap();
    assert!(status.checkpoint_sequence > 0, "no checkpoint ran: {status:?}");
    assert!(status.last_error.is_none(), "{status:?}");
    assert_eq!(store.decoded_documents(), 0);
    assert_eq!(answers(&store), expected);

    // More writes over rows that now live only in the files.
    for change in [
        r#"orders.where { .status == "open" && .total < 50 }.update { note: "low" }"#,
        r#"orders.where { .id == "o399" }.delete"#,
        r#"orders.insert { id: "o901", customer: "c03", total: 2.5, status: "open", items: [{ sku: "z", qty: 1 }] }"#,
    ] {
        rows(&store, change).unwrap();
    }
    store.wait_for_maintenance().unwrap();
    let after = answers(&store);
    assert_ne!(after, expected);
    store.verify_integrity().unwrap();
    store.compact().unwrap();
    assert_eq!(answers(&store), after);

    // The same from the files alone, with and without a budget.
    drop(store);
    let reopened = Arc::new(RustStore::with_decoded_budget(&path, SIZE, 0).unwrap());
    assert_eq!(answers(&reopened), after);
    assert_eq!(reopened.decoded_documents(), 0);
    drop(reopened);
    let reopened = Arc::new(RustStore::new(&path, SIZE).unwrap());
    assert_eq!(answers(&reopened), after);
    reopened.verify_integrity().unwrap();
}

/// Declaring a schema again, as an application does each time it starts,
/// leaves its indexes as they are: the same answers, through the same
/// indexes, and nothing written for an index that did not change.
#[test]
fn declaring_an_unchanged_schema_again_rebuilds_nothing() {
    let home = TempDir::new().unwrap();
    let path = home.path().join("declared.owl");
    let (_unbounded, expected) = reference(home.path());
    let store = Arc::new(RustStore::with_decoded_budget(&path, SIZE, 5).unwrap());
    load(&store);
    same(&answers(&store), &expected, "loaded");
    store.compact().unwrap();
    same(&answers(&store), &expected, "compacted");
    drop(store);

    let schema = r#"
        schema customers { id: String, name: String, tier: String, index(.tier) }
        schema orders {
          id: String,
          customer: Ref(customers),
          total: Float,
          status: String,
          items: List({ sku: String, qty: Integer }),
          note: Optional(String),
          index(.status, .total)
        }
    "#;
    let store = Arc::new(RustStore::with_decoded_budget(&path, SIZE, 5).unwrap());
    let indexes = store.list_scalar_indexes(None).unwrap();
    same(&answers(&store), &expected, "reopened");
    script(&store, schema);
    same(&answers(&store), &expected, "declared once");
    script(&store, schema);
    assert_eq!(store.list_scalar_indexes(None).unwrap(), indexes);
    same(&answers(&store), &expected, "declared twice");
    store.verify_integrity().unwrap();

    // A declaration that does change an index still takes effect.
    script(
        &store,
        &schema.replace("index(.status, .total)", "index(.status, .total),\n index(.total)"),
    );
    assert_eq!(store.list_scalar_indexes(None).unwrap().len(), indexes.len() + 1);
    same(&answers(&store), &expected, "an index added");
    drop(store);
    let reopened = Arc::new(RustStore::new(&path, SIZE).unwrap());
    same(&answers(&reopened), &expected, "reopened without a budget");
    reopened.verify_integrity().unwrap();
}

/// What is kept between queries to answer them faster (a collection's rows
/// listed, its fields as columns) must not hold on to rows the budget has
/// let go. A row the store dropped is then still in memory, uncounted.
#[test]
fn nothing_kept_between_queries_outlives_the_budget() {
    let home = TempDir::new().unwrap();
    let store = Arc::new(RustStore::with_decoded_budget(home.path().join("kept.owl"), SIZE, 30).unwrap());
    script(
        &store,
        "schema alpha { id: String, n: Integer, tags: List(String) }
         schema beta { id: String, n: Integer, tags: List(String) }",
    );
    let insert = |collection: &str| {
        let rows: Vec<String> = (0..20)
            .map(|row| format!("{{ id: \"{collection}{row:02}\", n: {row}, tags: [\"t\"] }}"))
            .collect();
        script(&store, &format!("{collection}.insert [ {} ]", rows.join(", ")));
    };
    insert("alpha");
    let every = |collection: &str| rows(&store, &format!("{collection}.where {{ .n >= 0 }}.sort {{ .id.asc }}")).unwrap();
    let count = |collection: &str| rows(&store, &format!("{collection}.where {{ .n > 5 }}.count()")).unwrap();

    // All of the first collection is held. Ask the same question of it
    // several times, which is what gets things kept.
    for _ in 0..4 {
        assert_eq!(count("alpha"), [r#"{"count":14}"#]);
    }
    let Some(held) = store.scan_documents_shared("alpha").unwrap() else {
        panic!("the first collection should all be held after writing it");
    };
    let one = held[0].clone();
    drop(held);

    // Writing the second collection pushes rows of the first out of the
    // budget. Nothing was written to the first, so what was kept of it
    // would otherwise stay.
    insert("beta");
    for _ in 0..4 {
        assert_eq!(count("beta"), [r#"{"count":14}"#]);
    }
    assert!(store.decoded_documents() <= 30, "{} held", store.decoded_documents());
    assert_eq!(
        Arc::strong_count(&one),
        1,
        "a row the budget let go is still held somewhere"
    );
    // And the answers are still right, asked more than once.
    for _ in 0..3 {
        assert_eq!(count("alpha"), [r#"{"count":14}"#]);
        assert_eq!(every("alpha").len(), 20);
        assert_eq!(count("beta"), [r#"{"count":14}"#]);
    }
    assert!(store.decoded_documents() <= 30, "{} held", store.decoded_documents());
}

/// A row whose record is in a checkpoint file is decoded when read. It must
/// then be held, within the budget, so that reading it again does not
/// decode it again; and a write to it must still be seen.
#[test]
fn a_row_read_from_a_checkpoint_file_is_held_within_the_budget() {
    let home = TempDir::new().unwrap();
    let store = Arc::new(RustStore::with_decoded_budget(home.path().join("held.owl"), SIZE, 10).unwrap());
    load(&store);
    store.checkpoint().unwrap();
    // The budget is far below the rows written, so most are on file only.
    store.set_decoded_budget(Some(0)).unwrap();
    assert_eq!(store.decoded_documents(), 0);
    store.set_decoded_budget(Some(10)).unwrap();

    let one = r#"orders.where { .id == "o007" }"#;
    let before = rows(&store, one).unwrap();
    assert_eq!(before.len(), 1);
    assert!(store.decoded_documents() >= 1, "the row read was not held");
    assert!(store.decoded_documents() <= 10);
    assert_eq!(rows(&store, one).unwrap(), before);

    // A write is seen, by this row and by a query over all of them.
    rows(&store, r#"orders.where { .id == "o007" }.update { note: "changed since" }"#).unwrap();
    let after = rows(&store, one).unwrap();
    assert_ne!(after, before);
    assert!(after[0].contains("changed since"), "{after:?}");
    assert_eq!(rows(&store, r#"orders.where { .note == "changed since" }.count()"#).unwrap(), [r#"{"count":1}"#]);
    assert!(store.decoded_documents() <= 10);
}

/// A compaction leaves the copy it replaced on disk, as something to fall
/// back to. Asked to, it removes that copy once the new one has been read
/// back and checked. The database is then about half the size on disk, and
/// answers and reopens exactly as before.
#[test]
fn a_compaction_can_remove_the_copy_it_replaced() {
    let home = TempDir::new().unwrap();
    let (_, expected) = reference(home.path());
    let path = home.path().join("light.owl");
    let size_on_disk = || -> u64 {
        std::fs::read_dir(home.path())
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("light.owl")
                    .then(|| entry.metadata().ok().map(|metadata| metadata.len()))
                    .flatten()
            })
            .sum()
    };
    let store = Arc::new(RustStore::new(&path, SIZE).unwrap());
    load(&store);
    store.checkpoint().unwrap();
    store.compact().unwrap();
    let kept = size_on_disk();
    same(&answers(&store), &expected, "after a compaction");

    let (_, reclaimed) = store.compact_and_reclaim().unwrap();
    assert!(reclaimed.files_removed >= 1, "{reclaimed:?}");
    let light = size_on_disk();
    assert!(light * 10 < kept * 7, "{light} bytes after reclaiming, {kept} before");
    same(&answers(&store), &expected, "after reclaiming");
    assert!(store.verify_integrity().is_ok());

    // A write afterwards, and the database reopened.
    rows(&store, r#"orders.where { .id == "o001" }.update { note: "after reclaiming" }"#).unwrap();
    let after = answers(&store);
    drop(store);
    let reopened = Arc::new(RustStore::new(&path, SIZE).unwrap());
    same(&answers(&reopened), &after, "reopened after reclaiming");
}

/// Column segments are decoded from their files when read. With a limit on
/// how much of them is held, the ones decoded longest ago are let go and
/// decoded again when wanted, and no answer changes: here the limit is so
/// small that only the segment in use is ever held.
#[test]
fn column_segments_are_let_go_and_read_again() {
    let home = TempDir::new().unwrap();
    let (_, expected) = reference(home.path());
    let path = home.path().join("segments.owl");
    let store = Arc::new(RustStore::with_decoded_budget(&path, SIZE, 10).unwrap());
    load(&store);
    store.checkpoint().unwrap();
    store.compact().unwrap();
    RustStore::set_column_segment_budget(Some(1));
    for round in 0..3 {
        same(&answers(&store), &expected, &format!("round {round} with one segment held"));
    }
    rows(&store, r#"orders.where { .id == "o003" }.update { note: "between rounds" }"#).unwrap();
    let after = answers(&store);
    RustStore::set_column_segment_budget(None);
    same(&answers(&store), &after, "with no limit again");
    drop(store);
    let reopened = Arc::new(RustStore::new(&path, SIZE).unwrap());
    same(&answers(&reopened), &after, "reopened");
}

/// A budget in bytes bounds roughly how much memory the decoded rows take,
/// whatever the size of each. Answers are the same within it, across
/// writes, when it is removed, and after reopening.
#[test]
fn a_budget_in_bytes_bounds_memory_and_changes_no_answer() {
    let home = TempDir::new().unwrap();
    let (_, expected) = reference(home.path());
    let path = home.path().join("bytes.owl");
    let store = Arc::new(RustStore::new(&path, SIZE).unwrap());
    load(&store);
    store.checkpoint().unwrap();
    // Everything fits at first, and a database told nothing has a budget
    // far above this one's size, so every row is held.
    let unbounded = store.decoded_bytes();
    assert!(unbounded > 100_000, "{unbounded} bytes held");
    same(&answers(&store), &expected, "with the default budget");

    for budget in [unbounded / 2, 20_000, 0] {
        store.set_decoded_bytes_budget(Some(budget)).unwrap();
        assert!(store.decoded_bytes() <= budget, "{} held under {budget}", store.decoded_bytes());
        for round in 0..2 {
            same(&answers(&store), &expected, &format!("round {round} under {budget} bytes"));
            assert!(store.decoded_bytes() <= budget, "{} held under {budget}", store.decoded_bytes());
        }
    }

    // Writes are seen, and held within the budget.
    store.set_decoded_bytes_budget(Some(20_000)).unwrap();
    rows(&store, r#"orders.where { .id == "o002" }.update { note: "under a bytes budget" }"#).unwrap();
    let after = answers(&store);
    assert_ne!(after, expected);
    assert!(store.decoded_bytes() <= 20_000);
    same(&answers(&store), &after, "asked again");

    // With the budget removed everything may be held again.
    store.set_decoded_bytes_budget(None).unwrap();
    same(&answers(&store), &after, "with no budget");
    drop(store);
    let reopened = Arc::new(RustStore::new(&path, SIZE).unwrap());
    same(&answers(&reopened), &after, "reopened");
}
