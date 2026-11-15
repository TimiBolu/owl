//! Transactions that run at the same time.
//!
//! Each transaction has its own writes, which nobody else sees until it
//! commits. A commit is refused when a document it wrote was changed by
//! someone else after the transaction began; the transaction is then gone
//! and its work is retried from the start.

use owl_db::error::OQLError;
use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, Document, QueryExecutor, Value};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

struct Db {
    _directory: TempDir,
    store: Arc<RustStore>,
}

impl Db {
    fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
        let db = Self {
            _directory: directory,
            store,
        };
        db.script(
            r#"
            schema accounts { id: String, owner: String, balance: Integer }
            accounts.insert [
              { id: "a", owner: "Ada", balance: 100 },
              { id: "b", owner: "Bo", balance: 100 },
              { id: "c", owner: "Cy", balance: 100 }
            ]
            "#,
        );
        db
    }

    fn executor(&self) -> QueryExecutor<RustStore> {
        QueryExecutor::new_with_real_clock(self.store.clone())
    }

    fn script(&self, source: &str) {
        let file = parse_oql_file(source).unwrap_or_else(|error| panic!("{error}\n{source}"));
        let report = execute_oql_file(&file, &self.executor()).unwrap();
        assert!(report.errors.is_empty(), "{:?}\n{source}", report.errors);
    }

    /// Run one statement outside any transaction.
    fn run(&self, query: &str) -> Result<Vec<Document>, OQLError> {
        self.executor().execute_query(&parse_oql(query).unwrap())
    }

    /// Run one statement as part of a transaction.
    fn run_in(&self, transaction: u64, query: &str) -> Result<Vec<Document>, OQLError> {
        let _bound = self.store.bind_transaction(transaction)?;
        self.executor().execute_query(&parse_oql(query).unwrap())
    }

    fn balance(&self, id: &str) -> i64 {
        Self::balance_of(&self.run(&format!("accounts.where {{ .id == \"{id}\" }}")).unwrap())
    }

    fn balance_in(&self, transaction: u64, id: &str) -> i64 {
        Self::balance_of(
            &self
                .run_in(transaction, &format!("accounts.where {{ .id == \"{id}\" }}"))
                .unwrap(),
        )
    }

    fn balance_of(rows: &[Document]) -> i64 {
        match rows.first().and_then(|row| row.get_field("balance")) {
            Some(Value::Integer(balance)) => *balance,
            other => panic!("no balance: {other:?}"),
        }
    }

    fn count(&self, query: &str) -> i64 {
        match self.run(query).unwrap().first().and_then(|row| row.get_field("count")) {
            Some(Value::Integer(count)) => *count,
            other => panic!("no count: {other:?}"),
        }
    }
}

fn set_balance(id: &str, balance: i64) -> String {
    format!("accounts.where {{ .id == \"{id}\" }}.update {{ balance: {balance} }}")
}

#[test]
fn a_transaction_sees_its_own_writes_and_nobody_else_does() {
    let db = Db::new();
    let first = db.store.begin_transaction().unwrap();
    let second = db.store.begin_transaction().unwrap();

    db.run_in(first, &set_balance("a", 10)).unwrap();
    db.run_in(first, r#"accounts.insert { id: "d", owner: "Di", balance: 5 }"#)
        .unwrap();
    db.run_in(second, &set_balance("b", 20)).unwrap();
    db.run_in(second, r#"accounts.where { .id == "c" }.delete"#)
        .unwrap();

    // Each sees what it wrote.
    assert_eq!(db.balance_in(first, "a"), 10);
    assert_eq!(db.run_in(first, "accounts.count()").unwrap()[0].get_field("count"), Some(&Value::Integer(4)));
    assert_eq!(db.balance_in(second, "b"), 20);
    assert_eq!(db.run_in(second, "accounts.count()").unwrap()[0].get_field("count"), Some(&Value::Integer(2)));

    // Neither sees the other's, and a reader outside both sees neither.
    assert_eq!(db.balance_in(first, "b"), 100);
    assert_eq!(db.balance_in(second, "a"), 100);
    assert_eq!(db.balance("a"), 100);
    assert_eq!(db.balance("b"), 100);
    assert_eq!(db.count("accounts.count()"), 3);

    // The same from another thread, which is bound to no transaction.
    let store = db.store.clone();
    std::thread::spawn(move || {
        let executor = QueryExecutor::new_with_real_clock(store);
        let rows = executor
            .execute_query(&parse_oql("accounts.sum(.balance)").unwrap())
            .unwrap();
        assert_eq!(rows[0].get_field("sum"), Some(&Value::Integer(300)));
    })
    .join()
    .unwrap();

    db.store.commit_transaction(first).unwrap();
    assert_eq!(db.balance("a"), 10);
    assert_eq!(db.count("accounts.count()"), 4);
    // Committed work is visible to a transaction still open.
    assert_eq!(db.balance_in(second, "a"), 10);

    db.store.commit_transaction(second).unwrap();
    assert_eq!(db.balance("b"), 20);
    assert_eq!(db.count("accounts.count()"), 3);
    assert_eq!(db.count(r#"accounts.where { .id == "c" }.count()"#), 0);
}

#[test]
fn a_write_outside_a_transaction_is_not_swept_into_one() {
    let db = Db::new();
    let transaction = db.store.begin_transaction().unwrap();
    db.run_in(transaction, &set_balance("a", 1)).unwrap();

    // Another caller writes while the transaction is open.
    db.run(&set_balance("b", 2)).unwrap();
    assert_eq!(db.balance("b"), 2);

    db.store.rollback_transaction(transaction).unwrap();
    // Rolling back undoes the transaction's work and nothing else.
    assert_eq!(db.balance("a"), 100);
    assert_eq!(db.balance("b"), 2);
}

#[test]
fn the_second_to_commit_a_change_to_the_same_document_is_refused() {
    let db = Db::new();
    let first = db.store.begin_transaction().unwrap();
    let second = db.store.begin_transaction().unwrap();

    db.run_in(first, &set_balance("a", 1)).unwrap();
    db.run_in(second, &set_balance("a", 2)).unwrap();
    db.run_in(second, &set_balance("b", 2)).unwrap();

    db.store.commit_transaction(first).unwrap();
    let refused = db.store.commit_transaction(second).unwrap_err();
    assert!(refused.is_transaction_conflict(), "{refused}");

    // The first stands, and none of the second's work was applied.
    assert_eq!(db.balance("a"), 1);
    assert_eq!(db.balance("b"), 100);

    // A refused transaction is over.
    assert!(db.run_in(second, "accounts.count()").is_err());
    assert!(db.store.commit_transaction(second).is_err());
    assert!(db.store.rollback_transaction(second).is_err());
}

#[test]
fn a_change_made_outside_a_transaction_also_refuses_it() {
    let db = Db::new();
    let transaction = db.store.begin_transaction().unwrap();
    let seen = db.balance_in(transaction, "a");

    // Someone else changes the document the transaction is about to
    // overwrite with a value computed from what it read.
    db.run(&set_balance("a", 500)).unwrap();
    db.run_in(transaction, &set_balance("a", seen - 30)).unwrap();

    let refused = db.store.commit_transaction(transaction).unwrap_err();
    assert!(refused.is_transaction_conflict(), "{refused}");
    assert_eq!(db.balance("a"), 500);
}

#[test]
fn transactions_that_touch_different_documents_both_commit() {
    let db = Db::new();
    let first = db.store.begin_transaction().unwrap();
    let second = db.store.begin_transaction().unwrap();
    db.run_in(first, &set_balance("a", 1)).unwrap();
    db.run_in(second, &set_balance("b", 2)).unwrap();
    db.run_in(second, r#"accounts.insert { id: "d", owner: "Di", balance: 4 }"#)
        .unwrap();
    db.store.commit_transaction(second).unwrap();
    db.store.commit_transaction(first).unwrap();
    assert_eq!(db.balance("a"), 1);
    assert_eq!(db.balance("b"), 2);
    assert_eq!(db.balance("d"), 4);
}

#[test]
fn two_transactions_cannot_both_create_the_same_document() {
    let db = Db::new();
    let first = db.store.begin_transaction().unwrap();
    let second = db.store.begin_transaction().unwrap();
    db.run_in(first, r#"accounts.insert { id: "d", owner: "One", balance: 1 }"#)
        .unwrap();
    db.run_in(second, r#"accounts.insert { id: "d", owner: "Two", balance: 2 }"#)
        .unwrap();
    db.store.commit_transaction(first).unwrap();
    assert!(db.store.commit_transaction(second).is_err());
    assert_eq!(db.balance("d"), 1);
    assert_eq!(db.count("accounts.count()"), 4);
}

#[test]
fn rolling_back_leaves_nothing_and_the_transaction_is_over() {
    let db = Db::new();
    let transaction = db.store.begin_transaction().unwrap();
    db.run_in(transaction, &set_balance("a", 1)).unwrap();
    db.run_in(transaction, r#"accounts.where { .id == "b" }.delete"#)
        .unwrap();
    db.store.rollback_transaction(transaction).unwrap();
    assert_eq!(db.balance("a"), 100);
    assert_eq!(db.count("accounts.count()"), 3);
    assert!(db.store.commit_transaction(transaction).is_err());
    assert!(db.store.bind_transaction(transaction).is_err());
    // An id that was never handed out is refused too.
    assert!(db.store.bind_transaction(987_654).is_err());
}

#[test]
fn a_statement_that_fails_does_not_end_the_transaction() {
    let db = Db::new();
    let transaction = db.store.begin_transaction().unwrap();
    db.run_in(transaction, &set_balance("a", 1)).unwrap();
    assert!(db
        .run_in(transaction, r#"accounts.insert { id: "a", owner: "Dup", balance: 0 }"#)
        .is_err());
    db.run_in(transaction, &set_balance("b", 2)).unwrap();
    db.store.commit_transaction(transaction).unwrap();
    assert_eq!(db.balance("a"), 1);
    assert_eq!(db.balance("b"), 2);
}

#[test]
fn a_binding_ends_with_its_guard() {
    let db = Db::new();
    let transaction = db.store.begin_transaction().unwrap();
    {
        let _bound = db.store.bind_transaction(transaction).unwrap();
        db.executor()
            .execute_query(&parse_oql(&set_balance("a", 1)).unwrap())
            .unwrap();
    }
    // No longer bound: this write commits on its own, at once.
    db.run(&set_balance("b", 2)).unwrap();
    assert_eq!(db.balance("b"), 2);
    assert_eq!(db.balance("a"), 100);
    db.store.commit_transaction(transaction).unwrap();
    assert_eq!(db.balance("a"), 1);
}

#[test]
fn a_script_transaction_still_commits_or_rolls_back_as_one() {
    let db = Db::new();
    db.script(
        r#"
        transaction {
          accounts.where { .id == "a" }.update { balance: 70 }
          accounts.where { .id == "b" }.update { balance: 130 }
        }
        "#,
    );
    assert_eq!(db.balance("a"), 70);
    assert_eq!(db.balance("b"), 130);

    let file = parse_oql_file(
        r#"
        transaction {
          accounts.where { .id == "a" }.update { balance: 0 }
          accounts.insert { id: "a", owner: "Dup", balance: 0 }
        }
        "#,
    )
    .unwrap();
    let report = execute_oql_file(&file, &db.executor()).unwrap();
    assert!(!report.errors.is_empty());
    assert_eq!(db.balance("a"), 70);
}

/// The transfer every bank example uses: read two balances, write two
/// balances. Whatever interleaving the threads get, no money is made or
/// lost, because a transfer that read a stale balance is refused and run
/// again.
#[test]
fn concurrent_transfers_conserve_money() {
    let db = Arc::new(Db::new());
    let threads = 6;
    let transfers = 40;
    let start = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|worker| {
            let db = db.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                let accounts = ["a", "b", "c"];
                start.wait();
                let mut retried = 0u32;
                for round in 0..transfers {
                    let from = accounts[(worker + round) % 3];
                    let to = accounts[(worker + round + 1) % 3];
                    loop {
                        let transaction = db.store.begin_transaction().unwrap();
                        let from_balance = db.balance_in(transaction, from);
                        let to_balance = db.balance_in(transaction, to);
                        db.run_in(transaction, &set_balance(from, from_balance - 1))
                            .unwrap();
                        db.run_in(transaction, &set_balance(to, to_balance + 1))
                            .unwrap();
                        match db.store.commit_transaction(transaction) {
                            Ok(()) => break,
                            Err(error) if error.is_transaction_conflict() => retried += 1,
                            Err(error) => panic!("{error}"),
                        }
                    }
                }
                retried
            })
        })
        .collect();
    let retried: u32 = workers.into_iter().map(|worker| worker.join().unwrap()).sum();

    let total = db.balance("a") + db.balance("b") + db.balance("c");
    assert_eq!(total, 300, "money was made or lost ({retried} retries)");
    // Every thread moved one unit round the ring the same number of times.
    assert_eq!(db.count("accounts.count()"), 3);
}

/// A counter incremented by many threads, each reading it inside a
/// transaction: the lost-update test.
#[test]
fn concurrent_increments_are_all_counted() {
    let db = Arc::new(Db::new());
    let threads = 8;
    let increments = 25;
    let start = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let db = db.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                for _ in 0..increments {
                    loop {
                        let transaction = db.store.begin_transaction().unwrap();
                        let balance = db.balance_in(transaction, "a");
                        db.run_in(transaction, &set_balance("a", balance + 1)).unwrap();
                        match db.store.commit_transaction(transaction) {
                            Ok(()) => break,
                            Err(error) if error.is_transaction_conflict() => {}
                            Err(error) => panic!("{error}"),
                        }
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(db.balance("a"), 100 + (threads * increments) as i64);
}

/// The same counter, written the way a Rust caller would: hand the work to
/// the store and let it retry.
#[test]
fn work_handed_to_the_store_is_retried_until_it_commits() {
    let db = Arc::new(Db::new());
    let threads = 6;
    let increments = 20;
    let start = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let db = db.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                let executor = db.executor();
                start.wait();
                for _ in 0..increments {
                    db.store
                        .in_transaction(1_000, || {
                            let rows = executor
                                .execute_query(&parse_oql(r#"accounts.where { .id == "a" }"#).unwrap())?;
                            let balance = Db::balance_of(&rows);
                            executor.execute_query(&parse_oql(&set_balance("a", balance + 1)).unwrap())?;
                            Ok(())
                        })
                        .unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(db.balance("a"), 100 + (threads * increments) as i64);

    // An error from the work rolls back and comes out unchanged, and a
    // single attempt that loses is reported as the conflict it is.
    let executor = db.executor();
    let failed = db.store.in_transaction(3, || {
        executor.execute_query(&parse_oql(&set_balance("b", 1)).unwrap())?;
        executor.execute_query(&parse_oql(r#"accounts.insert { id: "a", owner: "Dup", balance: 0 }"#).unwrap())?;
        Ok(())
    });
    assert!(failed.is_err_and(|error| !error.is_transaction_conflict()));
    assert_eq!(db.balance("b"), 100);

    let lost = db.store.in_transaction(1, || {
        executor.execute_query(&parse_oql(&set_balance("b", 1)).unwrap())?;
        // Someone else commits to the same document meanwhile.
        let other = db.store.clone();
        std::thread::spawn(move || {
            QueryExecutor::new_with_real_clock(other)
                .execute_query(&parse_oql(&set_balance("b", 77)).unwrap())
                .unwrap();
        })
        .join()
        .unwrap();
        Ok(())
    });
    assert!(lost.is_err_and(|error| error.is_transaction_conflict()));
    assert_eq!(db.balance("b"), 77);
}

#[test]
fn committed_transactions_survive_a_reopen() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("db.owl");
    {
        let store = Arc::new(RustStore::new(&path, 16 << 20).unwrap());
        let executor = QueryExecutor::new_with_real_clock(store.clone());
        let file = parse_oql_file(
            r#"
            schema accounts { id: String, owner: String, balance: Integer }
            accounts.insert { id: "a", owner: "Ada", balance: 100 }
            "#,
        )
        .unwrap();
        assert!(execute_oql_file(&file, &executor).unwrap().errors.is_empty());

        let kept = store.begin_transaction().unwrap();
        let dropped = store.begin_transaction().unwrap();
        {
            let _bound = store.bind_transaction(kept).unwrap();
            executor
                .execute_query(&parse_oql(r#"accounts.insert { id: "b", owner: "Bo", balance: 7 }"#).unwrap())
                .unwrap();
        }
        {
            let _bound = store.bind_transaction(dropped).unwrap();
            executor
                .execute_query(&parse_oql(r#"accounts.insert { id: "c", owner: "Cy", balance: 9 }"#).unwrap())
                .unwrap();
        }
        store.commit_transaction(kept).unwrap();
        // `dropped` is never committed.
    }
    let store = Arc::new(RustStore::new(&path, 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let rows = executor
        .execute_query(&parse_oql("accounts.sort { .id.asc }.select { id }").unwrap())
        .unwrap();
    let ids: Vec<_> = rows.iter().map(|row| row.get_field("id").cloned()).collect();
    assert_eq!(
        ids,
        vec![
            Some(Value::String("a".to_string())),
            Some(Value::String("b".to_string()))
        ]
    );
}

/// Run `work` on several threads at once and collect what each returns.
fn together<T: Send + 'static>(
    db: &Arc<Db>,
    threads: usize,
    work: impl Fn(&Db, usize) -> T + Send + Sync + 'static,
) -> Vec<T> {
    let start = Arc::new(Barrier::new(threads));
    let work = Arc::new(work);
    let workers: Vec<_> = (0..threads)
        .map(|worker| {
            let (db, start, work) = (db.clone(), start.clone(), work.clone());
            std::thread::spawn(move || {
                start.wait();
                work(&db, worker)
            })
        })
        .collect();
    workers.into_iter().map(|worker| worker.join().unwrap()).collect()
}

// From here on nobody opens a transaction: each statement is on its own,
// and has to be one indivisible step by itself.

#[test]
fn an_update_computed_from_the_row_is_never_lost() {
    let db = Arc::new(Db::new());
    together(&db, 8, |db, _| {
        for _ in 0..25 {
            let rows = db
                .run(r#"accounts.where { .id == "a" }.update { balance: .balance + 1 }"#)
                .unwrap();
            assert_eq!(rows.len(), 1);
        }
    });
    assert_eq!(db.balance("a"), 300);
}

#[test]
fn a_guarded_update_never_takes_more_than_there_is() {
    let db = Arc::new(Db::new());
    // 100 in the account, 8 x 20 = 160 attempts to take one.
    let taken: usize = together(&db, 8, |db, _| {
        (0..20)
            .map(|_| {
                db.run(r#"accounts.where { .id == "a" && .balance > 0 }.update { balance: .balance - 1 }"#)
                    .unwrap()
                    .len()
            })
            .sum::<usize>()
    })
    .into_iter()
    .sum();
    assert_eq!(taken, 100);
    assert_eq!(db.balance("a"), 0);
}

#[test]
fn only_one_of_many_inserts_of_an_id_succeeds() {
    let db = Arc::new(Db::new());
    let outcomes = together(&db, 8, |db, worker| {
        db.run(&format!(
            r#"accounts.insert {{ id: "new", owner: "W{worker}", balance: {worker} }}"#
        ))
        .map(|rows| rows.len())
        .map_err(|error| error.to_string())
    });
    let inserted: Vec<_> = outcomes.iter().filter(|outcome| outcome.is_ok()).collect();
    assert_eq!(inserted.len(), 1, "{outcomes:?}");
    for refused in outcomes.iter().filter_map(|outcome| outcome.as_ref().err()) {
        assert!(refused.contains("already exists"), "{refused}");
    }
    assert_eq!(db.count("accounts.count()"), 4);

    // The same through the prepared insert.
    let outcomes = together(&db, 8, |db, worker| {
        db.executor()
            .insert_documents(
                "accounts",
                vec![[
                    ("id".to_string(), Value::String("given".to_string())),
                    ("owner".to_string(), Value::String(format!("W{worker}"))),
                    ("balance".to_string(), Value::Integer(worker as i64)),
                ]
                .into_iter()
                .collect()],
            )
            .is_ok()
    });
    assert_eq!(outcomes.iter().filter(|inserted| **inserted).count(), 1);
    assert_eq!(db.count("accounts.count()"), 5);
}

#[test]
fn a_condition_and_its_update_are_a_compare_and_set() {
    let db = Arc::new(Db::new());
    // Everyone tries to claim the account from its current owner.
    let claims = together(&db, 8, |db, worker| {
        db.run(&format!(
            r#"accounts.where {{ .id == "a" && .owner == "Ada" }}.update {{ owner: "W{worker}" }}"#
        ))
        .unwrap()
        .len()
    });
    assert_eq!(claims.iter().sum::<usize>(), 1, "{claims:?}");
    let winner = claims.iter().position(|claimed| *claimed == 1).unwrap();
    let rows = db.run(r#"accounts.where { .id == "a" }.select { owner }"#).unwrap();
    assert_eq!(
        rows[0].get_field("owner"),
        Some(&Value::String(format!("W{winner}")))
    );
}

#[test]
fn updates_to_many_rows_at_once_do_not_tear() {
    let db = Arc::new(Db::new());
    // Each statement moves every balance by the same amount, so whatever
    // the interleaving the three stay equal.
    together(&db, 6, |db, worker| {
        for round in 0..15 {
            let step = if (worker + round) % 2 == 0 { 3 } else { -2 };
            db.run(&format!("accounts.update {{ balance: .balance + {step} }}"))
                .unwrap();
        }
    });
    let (a, b, c) = (db.balance("a"), db.balance("b"), db.balance("c"));
    assert_eq!((a, b), (b, c));
    let ups: i64 = (0..6).map(|worker| (0..15).filter(|round| (worker + round) % 2 == 0).count() as i64).sum();
    assert_eq!(a, 100 + 3 * ups - 2 * (90 - ups));
}

#[test]
fn a_delete_and_an_update_of_the_same_row_do_not_resurrect_it() {
    for _ in 0..20 {
        let db = Arc::new(Db::new());
        together(&db, 2, |db, worker| {
            if worker == 0 {
                db.run(r#"accounts.where { .id == "a" }.delete"#).unwrap();
            } else {
                db.run(r#"accounts.where { .id == "a" }.update { balance: .balance + 1 }"#)
                    .unwrap();
            }
        });
        // Whichever ran first, the delete is the last word.
        assert_eq!(db.count(r#"accounts.where { .id == "a" }.count()"#), 0);
        assert_eq!(db.count("accounts.count()"), 2);
    }
}

#[test]
fn statements_inside_a_transaction_wait_for_its_commit() {
    let db = Db::new();
    let transaction = db.store.begin_transaction().unwrap();
    db.run_in(transaction, r#"accounts.where { .id == "a" }.update { balance: .balance + 1 }"#)
        .unwrap();
    db.run_in(transaction, r#"accounts.where { .id == "a" }.update { balance: .balance + 1 }"#)
        .unwrap();
    assert_eq!(db.balance_in(transaction, "a"), 102);
    assert_eq!(db.balance("a"), 100);
    db.store.commit_transaction(transaction).unwrap();
    assert_eq!(db.balance("a"), 102);
}

/// Commits that arrive together share one sync of the log. Whatever they
/// share, a commit that was acknowledged is on disk: after a crash that
/// loses everything unsynced, each one is still there.
#[test]
fn every_acknowledged_commit_survives_a_crash_however_many_shared_a_sync() {
    use owl_db::simulation::SimulatedFileSystem;
    use owl_db::storage::DocumentStore;
    use std::collections::HashMap;

    let filesystem = Arc::new(SimulatedFileSystem::new(7));
    let path = "crash/db.owl";
    let threads = 8;
    let each = 40;
    {
        let store = Arc::new(RustStore::new_with_fs(path, 32 << 20, filesystem.clone()).unwrap());
        let start = Arc::new(Barrier::new(threads));
        let workers: Vec<_> = (0..threads)
            .map(|worker| {
                let (store, start) = (store.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    for round in 0..each {
                        let document = Document::with_id(
                            format!("w{worker}-{round:03}"),
                            "events".to_string(),
                            HashMap::from([("n".to_string(), Value::Integer(round as i64))]),
                        );
                        // Returning is the acknowledgement.
                        store.put_document(&document).unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(store.scan_documents("events").unwrap().len(), threads * each);
        // No clean shutdown: the store is simply gone.
        std::mem::forget(store);
    }
    filesystem.crash();
    filesystem.recover();
    let reopened = RustStore::new_with_fs(path, 32 << 20, filesystem).unwrap();
    let survived = reopened.scan_documents("events").unwrap();
    assert_eq!(survived.len(), threads * each);
    for worker in 0..threads {
        for round in 0..each {
            let id = format!("w{worker}-{round:03}");
            assert!(
                reopened.get_document("events", &id).unwrap().is_some(),
                "{id} was acknowledged and is gone"
            );
        }
    }
}
