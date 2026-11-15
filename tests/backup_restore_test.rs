//! Backup and restore.
//!
//! A backup is the database as of one commit. It is taken while the
//! database is in use, it can be checked without restoring it, and
//! restoring it gives a database that answers every query as the original
//! did at that commit.

use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, QueryExecutor, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

const SIZE: usize = 64 << 20;

fn script(store: &Arc<RustStore>, source: &str) {
    let file = parse_oql_file(source).unwrap_or_else(|error| panic!("{error}\n{source}"));
    let report = execute_oql_file(&file, &QueryExecutor::new_with_real_clock(store.clone())).unwrap();
    assert!(report.errors.is_empty(), "{:?}\n{source}", report.errors);
}

/// A value as JSON, whose objects print with their keys in order.
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

/// The rows of a query, each as JSON text with references shown as ids.
fn rows(store: &Arc<RustStore>, query: &str) -> Vec<String> {
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    executor
        .execute_query(&parse_oql(query).unwrap())
        .unwrap_or_else(|error| panic!("{error}\n{query}"))
        .iter()
        .map(|row| {
            let fields: serde_json::Map<String, serde_json::Value> = row
                .fields
                .iter()
                .map(|(name, value)| (name.clone(), plain(&executor.with_reference_ids(value).unwrap())))
                .collect();
            serde_json::Value::Object(fields).to_string()
        })
        .collect()
}

fn try_run(store: &Arc<RustStore>, query: &str) -> Result<usize, String> {
    QueryExecutor::new_with_real_clock(store.clone())
        .execute_query(&parse_oql(query).unwrap())
        .map(|rows| rows.len())
        .map_err(|error| error.to_string())
}

/// A database with schemas, indexes, references, nested values, a
/// checkpoint behind it and commits after the checkpoint.
fn shop(directory: &Path) -> Arc<RustStore> {
    let store = Arc::new(RustStore::new(directory.join("shop.owl"), SIZE).unwrap());
    script(
        &store,
        r#"
        schema customers { id: String, name: String, email: String unique, joined: DateTime, index(.name) }
        schema orders {
          id: String,
          customer: Ref(customers),
          total: Float,
          status: String,
          items: List({ sku: String, qty: Integer }),
          index(.status, .total)
        }
        customers.insert [
          { id: "c1", name: "Ada", email: "ada@x.org", joined: "2026-01-05" },
          { id: "c2", name: "Bo", email: "bo@x.org", joined: "2026-02-01" }
        ]
        "#,
    );
    let mut orders = String::from("orders.insert [\n");
    for index in 0..300 {
        orders.push_str(&format!(
            "  {{ id: \"o{index:03}\", customer: \"c{}\", total: {}.5, status: \"{}\", items: [{{ sku: \"s{}\", qty: {} }}] }},\n",
            1 + index % 2,
            index,
            ["paid", "open", "void"][index % 3],
            index % 7,
            1 + index % 4
        ));
    }
    orders.truncate(orders.len() - 2);
    orders.push_str("\n]");
    script(&store, &orders);
    store.checkpoint().unwrap();
    // Work the checkpoint does not hold: it is in the log.
    script(
        &store,
        r#"
        orders.where { .id == "o000" }.update { total: 1000.0 }
        orders.where { .id == "o299" }.delete
        customers.insert { id: "c3", name: "Cy", email: "cy@x.org", joined: "2026-03-01" }
        "#,
    );
    store
}

const QUESTIONS: [&str; 9] = [
    "customers.sort { .id.asc }",
    "orders.count()",
    "orders.sort { .id.asc }.limit(5)",
    "orders.where { .status == \"paid\" && .total > 200 }.sort { .total.desc, .id.asc }.limit(10).select { id, total }",
    "orders.group_by { .status }.select { status, n: count(), total: sum(.total) }",
    "orders.group_by { .customer.name }.select { name, n: count() }",
    "customers.sort { .id.asc }.select { id, orders: .orders.count(), spent: .orders.sum(.total) }",
    "orders.where { .id == \"o000\" }.select { id, total, who: .customer.name, items }",
    "customers.where { matches(.name, \"^B\") }.select { id }",
];

fn answers(store: &Arc<RustStore>) -> Vec<Vec<String>> {
    QUESTIONS.iter().map(|question| rows(store, question)).collect()
}

#[test]
fn a_restored_backup_answers_as_the_original_did() {
    let home = TempDir::new().unwrap();
    let store = shop(home.path());
    // A transaction that never commits is not part of any backup.
    let open = store.begin_transaction().unwrap();
    {
        let _bound = store.bind_transaction(open).unwrap();
        try_run(&store, r#"customers.insert { id: "c9", name: "Ghost", email: "g@x.org", joined: "2026-01-01" }"#).unwrap();
    }
    let expected = answers(&store);
    let committed = store.committed_lsn();

    let backup = home.path().join("backups/first");
    let info = store.backup(&backup).unwrap();
    assert_eq!(info.database, "shop.owl");
    assert_eq!(info.committed_lsn, committed);
    assert!(info.bytes() > 0 && !info.files.is_empty());
    assert_eq!(RustStore::verify_backup(&backup).unwrap(), info);

    // The original carries on, and what it does next is not in the backup.
    script(&store, r#"orders.where { .status == "void" }.delete"#);
    assert_ne!(answers(&store), expected);

    // Restored beside it, under another name.
    let restored = Arc::new(RustStore::restore(&backup, home.path().join("elsewhere/copy.owl"), SIZE).unwrap());
    assert_eq!(answers(&restored), expected);
    assert_eq!(restored.committed_lsn(), committed);
    assert_eq!(rows(&restored, r#"customers.where { .id == "c9" }.count()"#), vec![r#"{"count":0}"#]);

    // It is a working database: its indexes serve queries, its constraints
    // hold, and it takes writes of its own.
    let plan = rows(&restored, r#"orders.where { .status == "paid" && .total > 200 }.select { id }.explain()"#);
    assert!(plan[0].contains(r#""uses_index":true"#), "{plan:?}");
    let refused = try_run(&restored, r#"customers.insert { id: "c8", name: "Dup", email: "ada@x.org", joined: "2026-01-01" }"#).unwrap_err();
    assert!(refused.contains("unique"), "{refused}");
    assert!(try_run(&restored, r#"orders.insert { id: "n1", customer: "nobody", total: 1.0, status: "open", items: [] }"#).is_err());
    try_run(&restored, r#"orders.insert { id: "n1", customer: "c3", total: 1.0, status: "open", items: [] }"#).unwrap();
    assert_eq!(rows(&restored, r#"customers.where { .id == "c3" }.select { n: .orders.count() }"#), vec![r#"{"n":1}"#]);

    // The backup itself was not touched by being restored or used.
    assert_eq!(RustStore::verify_backup(&backup).unwrap(), info);
    // And it restores again, to the same thing, after the first copy moved on.
    drop(restored);
    let again = Arc::new(RustStore::restore(&backup, home.path().join("again.owl"), SIZE).unwrap());
    assert_eq!(answers(&again), expected);

    // Reopening a restored database later finds the same.
    drop(again);
    let reopened = Arc::new(RustStore::new(home.path().join("again.owl"), SIZE).unwrap());
    assert_eq!(answers(&reopened), expected);
}

#[test]
fn a_backup_taken_during_writes_is_one_moment_of_the_database() {
    let home = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(home.path().join("bank.owl"), SIZE).unwrap());
    script(
        &store,
        r#"
        schema accounts { id: String, balance: Integer }
        accounts.insert [ { id: "a", balance: 1000 }, { id: "b", balance: 1000 }, { id: "c", balance: 1000 } ]
        "#,
    );
    // Transfers run the whole time: each moves money between two accounts
    // in one transaction, so the total is 3000 at every commit and at no
    // moment in between.
    let stop = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let (store, stop) = (store.clone(), stop.clone());
            std::thread::spawn(move || {
                let executor = QueryExecutor::new_with_real_clock(store.clone());
                let accounts = ["a", "b", "c"];
                let mut round = worker;
                while !stop.load(Ordering::Acquire) {
                    round += 1;
                    let (from, to) = (accounts[round % 3], accounts[(round + 1) % 3]);
                    store
                        .in_transaction(1_000, || {
                            executor.execute_query(&parse_oql(&format!(
                                "accounts.where {{ .id == \"{from}\" }}.update {{ balance: .balance - 7 }}"
                            )).unwrap())?;
                            executor.execute_query(&parse_oql(&format!(
                                "accounts.where {{ .id == \"{to}\" }}.update {{ balance: .balance + 7 }}"
                            )).unwrap())?;
                            Ok(())
                        })
                        .unwrap();
                }
            })
        })
        .collect();

    let mut backups = Vec::new();
    for index in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(30));
        let directory = home.path().join(format!("backup-{index}"));
        backups.push((directory.clone(), store.backup(&directory).unwrap()));
    }
    stop.store(true, Ordering::Release);
    for worker in workers {
        worker.join().unwrap();
    }

    let mut seen = Vec::new();
    for (index, (directory, info)) in backups.iter().enumerate() {
        let restored = Arc::new(
            RustStore::restore(directory, home.path().join(format!("restored-{index}.owl")), SIZE).unwrap(),
        );
        assert_eq!(restored.committed_lsn(), info.committed_lsn);
        assert_eq!(rows(&restored, "accounts.sum(.balance)"), vec![r#"{"sum":3000}"#], "backup {index}");
        seen.push(info.committed_lsn);
    }
    // The backups were taken at different commits, each later than the last.
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "{seen:?}");
}

#[test]
fn a_backup_that_is_damaged_or_incomplete_is_refused() {
    let home = TempDir::new().unwrap();
    let store = shop(home.path());
    let backup = home.path().join("backup");
    let info = store.backup(&backup).unwrap();
    let target = |name: &str| home.path().join("targets").join(name);
    let refused = |backup: &Path, name: &str| -> String {
        let path = target(name);
        let error = RustStore::restore(backup, &path, SIZE).err().expect("the restore should be refused").to_string();
        // Refusing leaves nothing where the database would have been.
        assert!(!path.exists(), "{name} left {}", path.display());
        error
    };
    let copy = |name: &str| {
        let directory = home.path().join(name);
        std::fs::create_dir_all(&directory).unwrap();
        for entry in std::fs::read_dir(&backup).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), directory.join(entry.file_name())).unwrap();
        }
        directory
    };
    let largest = info.files.iter().max_by_key(|file| file.bytes).unwrap();

    // One byte changed.
    let damaged = copy("damaged");
    let mut bytes = std::fs::read(damaged.join(&largest.name)).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x40;
    std::fs::write(damaged.join(&largest.name), bytes).unwrap();
    assert!(RustStore::verify_backup(&damaged).is_err());
    assert!(refused(&damaged, "damaged.owl").contains("does not match the manifest"));

    // A file cut short, a file missing, a file that does not belong.
    let short = copy("short");
    let bytes = std::fs::read(short.join(&largest.name)).unwrap();
    std::fs::write(short.join(&largest.name), &bytes[..bytes.len() - 1]).unwrap();
    assert!(refused(&short, "short.owl").contains("does not match the manifest"));

    let missing = copy("missing");
    std::fs::remove_file(missing.join(&largest.name)).unwrap();
    assert!(RustStore::verify_backup(&missing).is_err());
    refused(&missing, "missing.owl");

    let extra = copy("extra");
    std::fs::write(extra.join("shop.owl.stray"), b"not part of it").unwrap();
    assert!(refused(&extra, "extra.owl").contains("does not list"));

    // No manifest: the copy never finished.
    let unfinished = copy("unfinished");
    std::fs::remove_file(unfinished.join("owl-backup.json")).unwrap();
    assert!(refused(&unfinished, "unfinished.owl").contains("no manifest"));
    assert!(refused(&home.path().join("no-such-backup"), "none.owl").contains("no manifest"));

    // A manifest that points outside the backup.
    let escaping = copy("escaping");
    let manifest = std::fs::read_to_string(escaping.join("owl-backup.json")).unwrap();
    std::fs::write(
        escaping.join("owl-backup.json"),
        manifest.replacen(
            &format!("\"name\": \"{}\"", info.files[0].name),
            "\"name\": \"../shop.owl\"",
            1,
        ),
    )
    .unwrap();
    assert_ne!(manifest, std::fs::read_to_string(escaping.join("owl-backup.json")).unwrap());
    let error = RustStore::verify_backup(&escaping).unwrap_err().to_string();
    assert!(error.contains("not a file of the backup"), "{error}");

    // The good backup is still good, and restores.
    assert_eq!(RustStore::verify_backup(&backup).unwrap(), info);
    let restored = Arc::new(RustStore::restore(&backup, target("good.owl"), SIZE).unwrap());
    assert_eq!(answers(&restored), answers(&store));
}

#[test]
fn neither_a_backup_nor_a_restore_overwrites_anything() {
    let home = TempDir::new().unwrap();
    let store = shop(home.path());
    let backup = home.path().join("backup");
    store.backup(&backup).unwrap();

    // A second backup into the same directory is refused, as is one into
    // any directory that already holds something.
    let error = store.backup(&backup).unwrap_err().to_string();
    assert!(error.contains("not empty"), "{error}");
    let occupied = home.path().join("occupied");
    std::fs::create_dir_all(&occupied).unwrap();
    std::fs::write(occupied.join("notes.txt"), b"mine").unwrap();
    assert!(store.backup(&occupied).is_err());
    assert_eq!(std::fs::read(occupied.join("notes.txt")).unwrap(), b"mine");
    // An empty directory is fine.
    let empty = home.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    store.backup(&empty).unwrap();

    // Restoring over the live database, or over any database, is refused
    // and leaves it as it was.
    let before = answers(&store);
    let error = RustStore::restore(&backup, home.path().join("shop.owl"), SIZE).err().unwrap().to_string();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(answers(&store), before);
    script(&store, r#"customers.insert { id: "c4", name: "Di", email: "di@x.org", joined: "2026-04-01" }"#);
    assert_eq!(rows(&store, "customers.count()"), vec![r#"{"count":4}"#]);
}
