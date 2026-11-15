//! A budget on decoded documents.
//!
//! A row with a list, an object or a vector in it lives outside the column
//! segments. Without a budget it is held decoded for as long as the
//! database is open. With one, only so many are, and the rest are kept as
//! their records and decoded when read. That changes how much memory the
//! database takes and how fast it reads. It must not change one answer.

use owl_db::storage::RustStore;
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
