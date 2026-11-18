//! A function defined in OQL belongs to the database.
//!
//! It is written as any document is, so it is there after the database is
//! closed and opened again, after a checkpoint and after a compaction; and
//! a transaction that is rolled back does not keep the definitions made
//! inside it.

use owl_db::storage::{DocumentStore, RustStore};
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, QueryExecutor, Value};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

const SIZE: usize = 16 << 20;

fn open(path: &Path) -> Arc<RustStore> {
    Arc::new(RustStore::new(path, SIZE).unwrap())
}

/// Run a script on a connection of its own and give back what went wrong.
fn script(store: &Arc<RustStore>, source: &str) -> Vec<String> {
    let file = parse_oql_file(source).unwrap_or_else(|error| panic!("{error}\n{source}"));
    execute_oql_file(&file, &QueryExecutor::new_with_real_clock(store.clone()))
        .unwrap()
        .errors
}

fn ask(store: &Arc<RustStore>, query: &str) -> Result<Vec<Value>, String> {
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    Ok(executor
        .execute_query(&parse_oql(query).unwrap())
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| row.fields.get("answer").cloned().unwrap_or(Value::Null))
        .collect())
}

fn load(store: &Arc<RustStore>) {
    let errors = script(
        store,
        r#"
        schema staff { id: String, pay: Integer }
        staff.insert [
          { id: "a", pay: 10 },
          { id: "b", pay: 20 },
          { id: "c", pay: 30 }
        ]
        fn with_bonus(pay, rate = 2) { pay * rate + 1 }
        "#,
    );
    assert!(errors.is_empty(), "{errors:?}");
}

const BONUSES: &str = "staff.sort { .id.asc }.select { answer: with_bonus(.pay) }";

fn integers(values: &[i64]) -> Vec<Value> {
    values.iter().map(|value| Value::Integer(*value)).collect()
}

#[test]
fn a_function_is_there_after_the_database_is_opened_again() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("owl");
    {
        let store = open(&path);
        load(&store);
        assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
    }
    // From the log alone.
    {
        let store = open(&path);
        assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
        store.checkpoint().unwrap();
    }
    // From a checkpoint.
    {
        let store = open(&path);
        assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
        store.compact().unwrap();
        assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
    }
    // From a compacted database.
    let store = open(&path);
    assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
    assert_eq!(ask(&store, "staff.select { answer: count() }").unwrap(), integers(&[3]));
}

#[test]
fn the_latest_definition_is_the_one_kept() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("owl");
    {
        let store = open(&path);
        load(&store);
        assert!(script(&store, "fn with_bonus(pay) { pay + 5 }").is_empty());
        // A function of the standard library may be replaced, and stays
        // replaced.
        assert!(script(&store, "fn variance(values) { 7 }").is_empty());
    }
    let store = open(&path);
    assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[15, 25, 35]));
    assert_eq!(
        ask(&store, "staff.limit(1).select { answer: variance([10, 20, 30]) }").unwrap(),
        integers(&[7])
    );
}

#[test]
fn a_transaction_rolled_back_keeps_none_of_its_definitions() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("owl");
    {
        let store = open(&path);
        load(&store);
        let errors = script(
            &store,
            r#"
            transaction {
              fn with_bonus(pay) { pay + 1000 }
              fn brand_new(pay) { pay }
              fn variance(values) { 7 }
              staff.insert { id: "d", pay: "not a number" }
            }
            "#,
        );
        assert!(!errors.is_empty(), "the transaction should have failed");
        // On this connection the earlier definition is back, the new one
        // is gone, and the standard library's is its own again.
        assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
        let error = ask(&store, "staff.select { answer: brand_new(.pay) }").unwrap_err();
        assert!(error.contains("Unknown function"), "{error}");
        assert_eq!(
            ask(&store, "staff.limit(1).select { answer: variance([10, 20, 30]) }").unwrap(),
            vec![Value::Float(100.0)]
        );
    }
    // And the same after a restart.
    let store = open(&path);
    assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
    let error = ask(&store, "staff.select { answer: brand_new(.pay) }").unwrap_err();
    assert!(error.contains("Unknown function"), "{error}");
    assert_eq!(
        ask(&store, "staff.limit(1).select { answer: variance([10, 20, 30]) }").unwrap(),
        vec![Value::Float(100.0)]
    );
}

#[test]
fn a_transaction_committed_keeps_its_definitions() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("owl");
    {
        let store = open(&path);
        load(&store);
        let errors = script(
            &store,
            r#"
            transaction {
              fn doubled(pay) { pay * 2 }
              staff.insert { id: "d", pay: 40 }
            }
            "#,
        );
        assert!(errors.is_empty(), "{errors:?}");
    }
    let store = open(&path);
    assert_eq!(
        ask(&store, "staff.sort { .id.asc }.select { answer: doubled(.pay) }").unwrap(),
        integers(&[20, 40, 60, 80])
    );
}

#[test]
fn a_stored_function_that_cannot_be_read_says_so() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("owl");
    {
        let store = open(&path);
        load(&store);
        // What a later, different version of Owl might have left behind.
        let mut damaged = owl_db::functions::stored_function(
            &match parse_oql_file("fn from_the_future(pay) { pay }").unwrap().statements.remove(0) {
                owl_db::parser::OQLFileStatement::FunctionDef(definition) => definition,
                _ => unreachable!(),
            },
        )
        .unwrap();
        damaged
            .fields
            .insert("definition".to_string(), Value::String("{ \"not\": \"a function\" }".to_string()));
        store.put_document(&damaged).unwrap();
    }
    let store = open(&path);
    // Everything else still works.
    assert_eq!(ask(&store, BONUSES).unwrap(), integers(&[21, 41, 61]));
    let error = ask(&store, "staff.select { answer: from_the_future(.pay) }").unwrap_err();
    assert!(error.contains("stored in this database but cannot be used"), "{error}");
    // Defining it again puts it right.
    assert!(script(&store, "fn from_the_future(pay) { pay - 1 }").is_empty());
    assert_eq!(
        ask(&store, "staff.sort { .id.asc }.select { answer: from_the_future(.pay) }").unwrap(),
        integers(&[9, 19, 29])
    );
}

#[test]
fn the_functions_are_not_a_collection_of_the_users() {
    let directory = TempDir::new().unwrap();
    let store = open(&directory.path().join("owl"));
    load(&store);
    assert_eq!(store.list_schemas().unwrap(), vec!["staff".to_string()]);
}

#[test]
fn every_function_of_the_standard_library_can_be_written_down_and_read_back() {
    let library = parse_oql_file(include_str!("../src/std.oql")).unwrap();
    assert!(!library.statements.is_empty());
    for statement in library.statements {
        let owl_db::parser::OQLFileStatement::FunctionDef(definition) = statement else {
            panic!("the standard library holds only functions");
        };
        let stored = owl_db::functions::stored_function(&definition).unwrap();
        assert_eq!(owl_db::functions::function_of_stored(&stored).unwrap(), definition);
    }
}
