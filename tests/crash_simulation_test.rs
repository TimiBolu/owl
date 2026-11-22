//! Crashes the storage engine at arbitrary points and checks what it
//! promises about them.
//!
//! Each run takes a seed. From the seed comes a workload (batches,
//! transactions, deletes, checkpoints, compactions, reopenings) and the
//! faults: at some steps the simulated machine loses power in the middle
//! of a chosen disk operation, keeping an arbitrary part of what was not
//! synced. Beside the store runs a plain map of what has been
//! acknowledged. After every crash the store is reopened and must hold
//! exactly that map, give or take the one step that was under way, which
//! must be there whole or not at all.
//!
//! A failure names its seed. `OWL_CRASH_SEED=<seed>` runs that seed alone
//! and prints each step; `OWL_CRASH_SEEDS=<n>` sets how many seeds the
//! sweep runs.

use owl_db::document::Fields;
use owl_db::schema::{FieldType, Schema};
use owl_db::simulation::SimulatedFileSystem;
use owl_db::storage::DocumentStore;
use owl_db::storage_engine::{AutoCheckpointPolicy, RustStore, WriteOperation};
use owl_db::{Document, Value};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

const DATABASE: &str = "crash.owl";
/// One collection with no schema and one with, since the two are stored
/// differently.
const COLLECTIONS: [&str; 2] = ["notes", "items"];
const IDS: usize = 24;

/// What a row holds, as far as the check is concerned.
type Row = (i64, String);
/// Every row that should be there: collection and id to contents.
type Rows = BTreeMap<(String, String), Row>;

#[derive(Clone, Debug)]
enum Change {
    Put(String, String, Row),
    Delete(String, String),
}

fn apply(rows: &mut Rows, changes: &[Change]) {
    for change in changes {
        match change {
            Change::Put(collection, id, row) => {
                rows.insert((collection.clone(), id.clone()), row.clone());
            }
            Change::Delete(collection, id) => {
                rows.remove(&(collection.clone(), id.clone()));
            }
        }
    }
}

fn document(collection: &str, id: &str, row: &Row) -> Document {
    Document::with_id(
        id.to_string(),
        collection.to_string(),
        Fields::from_iter([
            ("n".to_string(), Value::Integer(row.0)),
            ("pad".to_string(), Value::String(row.1.clone())),
        ]),
    )
}

fn operation(change: &Change) -> WriteOperation {
    match change {
        Change::Put(collection, id, row) => {
            WriteOperation::PutDocument(document(collection, id, row))
        }
        Change::Delete(collection, id) => WriteOperation::DeleteDocument {
            schema: collection.clone(),
            id: id.clone(),
        },
    }
}

fn open(filesystem: &Arc<SimulatedFileSystem>) -> owl_db::OQLResult<RustStore> {
    let store = RustStore::new_with_fs(Path::new(DATABASE), 1 << 30, filesystem.clone())?;
    // Every checkpoint is asked for by the workload, on this thread, so
    // that the disk sees the same operations in the same order each run.
    store.configure_auto_checkpoint(AutoCheckpointPolicy::disabled())?;
    Ok(store)
}

fn read(store: &RustStore) -> Result<Rows, String> {
    let mut rows = Rows::new();
    for collection in COLLECTIONS {
        let documents = store
            .scan_documents(collection)
            .map_err(|error| format!("cannot scan {collection}: {error}"))?;
        for document in documents {
            let n = match document.fields.get("n") {
                Some(Value::Integer(n)) => *n,
                other => return Err(format!("{collection}/{} has n = {other:?}", document.id)),
            };
            let pad = match document.fields.get("pad") {
                Some(Value::String(pad)) => pad.clone(),
                other => return Err(format!("{collection}/{} has pad = {other:?}", document.id)),
            };
            if rows
                .insert((collection.to_string(), document.id.clone()), (n, pad))
                .is_some()
            {
                return Err(format!("{collection}/{} is there twice", document.id));
            }
        }
    }
    // Looking a row up by its id must agree with the scan.
    for collection in COLLECTIONS {
        for number in 0..IDS {
            let id = format!("r{number}");
            let found = store
                .get_document(collection, &id)
                .map_err(|error| format!("cannot get {collection}/{id}: {error}"))?
                .map(|document| match document.fields.get("n") {
                    Some(Value::Integer(n)) => *n,
                    _ => i64::MIN,
                });
            let scanned = rows
                .get(&(collection.to_string(), id.clone()))
                .map(|row| row.0);
            if found != scanned {
                return Err(format!(
                    "{collection}/{id}: a lookup gives {found:?}, a scan gives {scanned:?}"
                ));
            }
        }
    }
    Ok(rows)
}

fn difference(expected: &Rows, found: &Rows) -> String {
    let mut lines = Vec::new();
    for (key, row) in expected {
        match found.get(key) {
            None => lines.push(format!("{}/{} is missing (n = {})", key.0, key.1, row.0)),
            Some(other) if other != row => lines.push(format!(
                "{}/{} has n = {}, expected {}",
                key.0, key.1, other.0, row.0
            )),
            _ => {}
        }
    }
    for (key, row) in found {
        if !expected.contains_key(key) {
            lines.push(format!("{}/{} should not be there (n = {})", key.0, key.1, row.0));
        }
    }
    lines.join("; ")
}

struct Run {
    seed: u64,
    rng: StdRng,
    filesystem: Arc<SimulatedFileSystem>,
    /// What has been acknowledged.
    rows: Rows,
    next: i64,
    /// One line for each step, for replaying a seed by eye.
    trace: Vec<String>,
    crashes: usize,
    io_faults: bool,
}

/// What may go wrong with the disk, besides losing power.
#[derive(Clone, Copy)]
struct Faults {
    /// Writes and syncs fail now and then.
    io_faults: bool,
    /// A crash may keep a later unsynced write and lose an earlier one.
    reorder: bool,
}

/// What a step did, as the workload saw it.
enum Outcome {
    /// It was acknowledged; these changes are now promised.
    Done(Vec<Change>),
    /// It failed or was cut off; these changes may be there, whole, or not.
    Unknown(Vec<Change>, String),
}

impl Run {
    fn new(seed: u64, faults: Faults) -> Self {
        let Faults { io_faults, reorder } = faults;
        Self {
            seed,
            rng: StdRng::seed_from_u64(seed),
            filesystem: {
                let filesystem = SimulatedFileSystem::new(seed ^ 0x9e37_79b9_7f4a_7c15);
                filesystem.set_in_order_only(!reorder);
                Arc::new(filesystem)
            },
            rows: Rows::new(),
            next: 0,
            trace: Vec::new(),
            crashes: 0,
            io_faults,
        }
    }

    fn fail(&self, message: String) -> String {
        let files: Vec<String> = self
            .filesystem
            .files()
            .into_iter()
            .map(|(path, length)| format!("{} ({length})", path.display()))
            .collect();
        format!(
            "seed {}: {message}\n  replay with OWL_CRASH_SEED={}\n  files:\n    {}\n  steps:\n    {}",
            self.seed,
            self.seed,
            files.join("\n    "),
            self.trace.join("\n    ")
        )
    }

    fn changes(&mut self, most: usize) -> Vec<Change> {
        let count = self.rng.gen_range(1..=most);
        let mut changes = Vec::new();
        for _ in 0..count {
            let collection = COLLECTIONS[self.rng.gen_range(0..COLLECTIONS.len())].to_string();
            let id = format!("r{}", self.rng.gen_range(0..IDS));
            // Only a row that is there can be deleted.
            let mut after = self.rows.clone();
            apply(&mut after, &changes);
            let there = after.contains_key(&(collection.clone(), id.clone()));
            if there && self.rng.gen_bool(0.4) {
                changes.push(Change::Delete(collection, id));
            } else {
                self.next += 1;
                // Mostly small rows, now and then one that spans sectors.
                let length = if self.rng.gen_bool(0.1) {
                    self.rng.gen_range(600..3000)
                } else {
                    self.rng.gen_range(0..40)
                };
                let pad = "x".repeat(length);
                changes.push(Change::Put(collection, id, (self.next, pad)));
            }
        }
        changes
    }

    /// Run one step of the workload against an open store.
    fn step(&mut self, store: &RustStore) -> Outcome {
        let pick = self.rng.gen_range(0..100);
        let (name, changes, result): (String, Vec<Change>, Result<(), String>) = if pick < 45 {
            let changes = self.changes(5);
            let result = store
                .write_batch(changes.iter().map(operation).collect())
                .map(|_| ())
                .map_err(|error| error.to_string());
            (format!("batch of {}", changes.len()), changes, result)
        } else if pick < 60 {
            let changes = self.changes(4);
            let commit = self.rng.gen_bool(0.75);
            let result = (|| {
                let id = store
                    .begin_local_transaction()
                    .map_err(|error| error.to_string())?;
                let written = changes.iter().try_for_each(|change| match change {
                    Change::Put(collection, id, row) => {
                        store.put_document(&document(collection, id, row))
                    }
                    Change::Delete(collection, id) => {
                        store.delete_document(collection, id).map(|_| ())
                    }
                });
                let ended = match written {
                    Ok(()) if commit => store.commit_local_transaction(id),
                    Ok(()) => store.rollback_local_transaction(id),
                    Err(error) => Err(error),
                };
                if ended.is_err() {
                    let _ = store.rollback_local_transaction(id);
                }
                ended.map_err(|error| error.to_string())
            })();
            if commit {
                (format!("transaction of {}", changes.len()), changes, result)
            } else {
                ("transaction rolled back".to_string(), Vec::new(), result)
            }
        } else if pick < 75 {
            let result = store.checkpoint().map(|_| ()).map_err(|e| e.to_string());
            ("checkpoint".to_string(), Vec::new(), result)
        } else if pick < 85 {
            let result = store.compact().map(|_| ()).map_err(|e| e.to_string());
            ("compact".to_string(), Vec::new(), result)
        } else if pick < 93 {
            let result = store
                .compact_and_reclaim()
                .map(|_| ())
                .map_err(|e| e.to_string());
            ("compact and reclaim".to_string(), Vec::new(), result)
        } else {
            let result = store
                .reclaim_obsolete_files()
                .map(|_| ())
                .map_err(|e| e.to_string());
            ("reclaim".to_string(), Vec::new(), result)
        };
        let name = if changes.is_empty() {
            name
        } else {
            let listed: Vec<String> = changes
                .iter()
                .map(|change| match change {
                    Change::Put(collection, id, row) => format!("{collection}/{id}={}", row.0),
                    Change::Delete(collection, id) => format!("{collection}/{id} deleted"),
                })
                .collect();
            format!("{name} [{}]", listed.join(", "))
        };
        match result {
            Ok(()) => {
                self.trace.push(format!("{name}: done"));
                Outcome::Done(changes)
            }
            Err(error) => {
                self.trace.push(format!("{name}: {error}"));
                Outcome::Unknown(changes, error)
            }
        }
    }

    /// Open the store after a crash, sometimes crashing again during the
    /// recovery itself.
    fn reopen(&mut self) -> Result<RustStore, String> {
        for _ in 0..8 {
            self.filesystem.recover();
            let again = self.rng.gen_bool(0.2);
            if again {
                let after = self.rng.gen_range(1..=12);
                self.filesystem.crash_after(after, true);
            }
            let opened = open(&self.filesystem);
            self.filesystem.disarm();
            match opened {
                Ok(store) if !self.filesystem.is_down() => return Ok(store),
                Ok(store) => {
                    drop(store);
                    self.crashes += 1;
                    self.trace.push("crashed during recovery".to_string());
                }
                Err(_) if self.filesystem.is_down() => {
                    self.crashes += 1;
                    self.trace.push("crashed during recovery".to_string());
                }
                Err(error) => return Err(self.fail(format!("cannot reopen: {error}"))),
            }
        }
        self.filesystem.recover();
        open(&self.filesystem).map_err(|error| self.fail(format!("cannot reopen: {error}")))
    }

    /// The store must hold what was acknowledged, with the step that was
    /// under way either wholly there or wholly absent.
    fn check(&mut self, store: &RustStore, under_way: &[Change]) -> Result<(), String> {
        let found = read(store).map_err(|error| self.fail(error))?;
        if found != self.rows {
            let mut with = self.rows.clone();
            apply(&mut with, under_way);
            if under_way.is_empty() || found != with {
                let message = format!(
                    "after a crash the store does not hold what was acknowledged: {}{}",
                    difference(&self.rows, &found),
                    if under_way.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " (nor that with the step under way: {})",
                            difference(&with, &found)
                        )
                    }
                );
                return Err(self.fail(message));
            }
            self.trace.push("the step under way survived".to_string());
            self.rows = with;
        }
        if let Err(error) = store.verify_integrity() {
            // Say whether the disk has really lost something, by opening
            // it afresh beside the store that is already open.
            let lost = match open(&self.filesystem).map_err(|e| e.to_string()).and_then(|again| read(&again)) {
                Ok(again) if again == self.rows => "a fresh open still holds every row".to_string(),
                Ok(again) => format!("a fresh open has lost rows: {}", difference(&self.rows, &again)),
                Err(error) => format!("a fresh open fails: {error}"),
            };
            return Err(self.fail(format!("integrity check fails: {error}; {lost}")));
        }
        Ok(())
    }

    fn run(&mut self, steps: usize) -> Result<(), String> {
        let mut store =
            open(&self.filesystem).map_err(|error| self.fail(format!("cannot open: {error}")))?;
        let fields = HashMap::from([
            ("n".to_string(), FieldType::Integer),
            ("pad".to_string(), FieldType::String),
        ]);
        store
            .write_batch(vec![WriteOperation::PutSchema(Schema::new(
                "items".to_string(),
                fields,
            ))])
            .map_err(|error| self.fail(format!("cannot declare the schema: {error}")))?;

        for _ in 0..steps {
            // Now and then, close and open again with nothing wrong.
            if self.rng.gen_bool(0.05) {
                drop(store);
                store = open(&self.filesystem)
                    .map_err(|error| self.fail(format!("cannot reopen cleanly: {error}")))?;
                self.trace.push("reopened".to_string());
                self.check(&store, &[])?;
            }

            let armed = self.rng.gen_bool(0.3);
            if armed {
                // Usually early in the step, sometimes deep into it.
                let after = if self.rng.gen_bool(0.6) {
                    self.rng.gen_range(1..=6)
                } else {
                    self.rng.gen_range(1..=120)
                };
                self.filesystem.crash_after(after, true);
                self.trace.push(format!("armed: crash at operation {after}"));
            }
            if self.io_faults {
                let rate = if self.rng.gen_bool(0.15) { 0.05 } else { 0.0 };
                self.filesystem.set_io_failure_rate(rate);
            }
            let outcome = self.step(&store);
            // A commit asks the maintenance thread to look whether a
            // checkpoint is due. Let it finish looking before going on, so
            // that nothing touches the disk behind the workload's back.
            let _ = store.wait_for_maintenance();
            self.filesystem.disarm();
            self.filesystem.set_io_failure_rate(0.0);

            let down = self.filesystem.is_down();
            match outcome {
                Outcome::Done(changes) => {
                    // It was acknowledged, so it is promised even if the
                    // machine died a moment later.
                    apply(&mut self.rows, &changes);
                    if down {
                        drop(store);
                        self.crashes += 1;
                        store = self.reopen()?;
                        self.check(&store, &[])?;
                    }
                }
                Outcome::Unknown(changes, error) => {
                    if !down && !self.io_faults {
                        return Err(self.fail(format!(
                            "a step failed though nothing was wrong with the disk: {error}"
                        )));
                    }
                    if !down {
                        // A failed write or sync leaves the store unsure of
                        // itself; the answer is to start again from disk.
                        self.filesystem.crash_torn();
                    }
                    drop(store);
                    self.crashes += 1;
                    store = self.reopen()?;
                    self.check(&store, &changes)?;
                }
            }
        }

        // At the end, with nothing wrong: a clean close and open.
        let found = read(&store).map_err(|error| self.fail(error))?;
        if found != self.rows {
            return Err(self.fail(format!(
                "the open store drifted from what was acknowledged: {}",
                difference(&self.rows, &found)
            )));
        }
        drop(store);
        let store = open(&self.filesystem)
            .map_err(|error| self.fail(format!("cannot reopen at the end: {error}")))?;
        self.check(&store, &[])
    }
}

fn seeds(default: u64) -> u64 {
    std::env::var("OWL_CRASH_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn sweep(first: u64, count: u64, steps: usize, faults: Faults) {
    if let Some(seed) = std::env::var("OWL_CRASH_SEED")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        let mut run = Run::new(seed, faults);
        let result = run.run(steps);
        for line in &run.trace {
            println!("{line}");
        }
        if let Err(message) = result {
            panic!("{message}");
        }
        return;
    }
    let mut crashes = 0;
    let mut failures = Vec::new();
    for seed in first..first + count {
        let mut run = Run::new(seed, faults);
        if let Err(message) = run.run(steps) {
            failures.push(message);
        }
        crashes += run.crashes;
    }
    println!("{count} seeds, {crashes} crashes, {} failures", failures.len());
    // Failures by what went wrong, most common first.
    let mut kinds: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for failure in &failures {
        let line = failure.lines().next().unwrap_or_default();
        let (seed, what) = line.split_once(": ").unwrap_or(("", line));
        let seed = seed.trim_start_matches("seed ").parse().unwrap_or(0);
        let what: String = what.chars().take(110).collect();
        let kind = kinds.entry(what).or_insert((0, seed));
        kind.0 += 1;
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (what, (times, seed)) in kinds {
        println!("  {times:4} x (first seed {seed}) {what}");
    }
    if !failures.is_empty() {
        let shown: Vec<&String> = failures.iter().take(3).collect();
        panic!(
            "{} of {count} seeds failed. The first:\n{}",
            failures.len(),
            shown
                .iter()
                .map(|failure| failure.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        );
    }
}

#[test]
fn acknowledged_writes_survive_a_crash_at_any_point() {
    let faults = Faults {
        io_faults: false,
        reorder: false,
    };
    sweep(1, seeds(300), 60, faults);
}

#[test]
fn acknowledged_writes_survive_failing_writes_and_syncs() {
    let faults = Faults {
        io_faults: true,
        reorder: false,
    };
    sweep(100_000, seeds(300), 60, faults);
}

/// Two seeds the larger sweep found, kept for what they show.
///
/// A checkpoint writes its files, then its manifest. When writing the
/// manifest fails without a crash, the bytes may still have reached the
/// disk. The checkpoint used to be tried again in place: an incremental
/// one emptied its generation files to write them afresh, and a full one
/// could pick the standing base segment to write over. Power lost at that
/// moment left a manifest that was whole beside files that were not, and
/// the database would not open.
#[test]
fn the_database_opens_after_a_checkpoint_is_retried_and_power_is_lost() {
    let faults = Faults {
        io_faults: true,
        reorder: false,
    };
    for seed in [101_261, 100_722] {
        let mut run = Run::new(seed, faults);
        if let Err(message) = run.run(60) {
            panic!("{message}");
        }
    }
}

/// Not yet true, and left out of the usual run for that reason.
///
/// The log is read on the understanding that an interrupted append leaves
/// the file ending inside a frame and nothing else. A disk that keeps a
/// later unsynced write and loses an earlier one leaves a hole with bytes
/// after it instead, and recovery takes that for corruption and refuses
/// to open ("invalid WAL frame magic"). Nothing acknowledged is lost, but
/// the database stays shut until someone repairs it. Whether to read such
/// a hole as the end of the log is a decision about what a damaged log
/// means, and it has not been made.
#[test]
#[ignore = "recovery refuses a log whose unsynced writes were reordered"]
fn acknowledged_writes_survive_a_crash_that_reorders_writes() {
    let faults = Faults {
        io_faults: false,
        reorder: true,
    };
    sweep(200_000, seeds(300), 60, faults);
}

#[test]
fn one_seed_gives_one_run() {
    let faults = Faults {
        io_faults: false,
        reorder: false,
    };
    let once = {
        let mut run = Run::new(7, faults);
        let _ = run.run(60);
        (run.trace, run.filesystem.operations())
    };
    let twice = {
        let mut run = Run::new(7, faults);
        let _ = run.run(60);
        (run.trace, run.filesystem.operations())
    };
    assert_eq!(once.1, twice.1, "the disk saw a different number of operations");
    assert_eq!(once.0, twice.0);
}

// What the sweeps found, each pinned down to the smallest case that shows it.

fn note(store: &RustStore, id: &str, n: i64) {
    store
        .put_document(&document("notes", id, &(n, "x".to_string())))
        .unwrap();
}

fn item(store: &RustStore, id: &str, n: i64) {
    store
        .put_document(&document("items", id, &(n, "x".to_string())))
        .unwrap();
}

fn declare_items(store: &RustStore) {
    let fields = HashMap::from([
        ("n".to_string(), FieldType::Integer),
        ("pad".to_string(), FieldType::String),
    ]);
    store
        .write_batch(vec![WriteOperation::PutSchema(Schema::new(
            "items".to_string(),
            fields,
        ))])
        .unwrap();
}

fn n_of(store: &RustStore, collection: &str, id: &str) -> Option<i64> {
    read(store)
        .unwrap()
        .get(&(collection.to_string(), id.to_string()))
        .map(|row| row.0)
}

/// A row whose newest version is in a generation file, written by an
/// incremental checkpoint, and nowhere newer. A compaction then dies after
/// sealing the log. Recovery checkpoints what it recovered; it used to
/// take that row's record from the base file, skipping the generation, so
/// the open store answered 2 and the next open answered 1.
#[test]
fn a_row_last_written_before_an_incremental_checkpoint_survives_a_crashed_compaction() {
    for at in 1..40 {
        let filesystem = Arc::new(SimulatedFileSystem::new(1));
        let store = open(&filesystem).unwrap();
        note(&store, "a", 1);
        note(&store, "b", 1);
        store.compact().unwrap();
        note(&store, "a", 2);
        store.checkpoint().unwrap();
        filesystem.crash_after(at, false);
        let _ = store.compact();
        filesystem.disarm();
        if !filesystem.is_down() {
            continue;
        }
        drop(store);
        filesystem.recover();

        let store = open(&filesystem).unwrap();
        assert_eq!(n_of(&store, "notes", "a"), Some(2), "crash at operation {at}");
        store
            .verify_integrity()
            .unwrap_or_else(|error| panic!("crash at operation {at}: {error}"));
        drop(store);
        let store = open(&filesystem).unwrap();
        assert_eq!(
            n_of(&store, "notes", "a"),
            Some(2),
            "crash at operation {at}, opened a second time"
        );
    }
}

/// After recovering from an interrupted checkpoint, the open store kept
/// the rows of a collection with a schema both in memory and in the
/// checkpoint it had just written, and its own integrity check said the
/// disk did not match it.
#[test]
fn a_collection_with_a_schema_passes_its_integrity_check_after_an_interrupted_checkpoint() {
    for at in 1..40 {
        let filesystem = Arc::new(SimulatedFileSystem::new(1));
        let store = open(&filesystem).unwrap();
        declare_items(&store);
        item(&store, "a", 1);
        filesystem.crash_after(at, false);
        let _ = store.checkpoint();
        filesystem.disarm();
        if !filesystem.is_down() {
            continue;
        }
        drop(store);
        filesystem.recover();

        let store = open(&filesystem).unwrap();
        store
            .verify_integrity()
            .unwrap_or_else(|error| panic!("crash at operation {at}: {error}"));
        item(&store, "b", 2);
        store.checkpoint().unwrap();
        store
            .verify_integrity()
            .unwrap_or_else(|error| panic!("crash at operation {at}, after more work: {error}"));
        assert_eq!(n_of(&store, "items", "a"), Some(1));
        assert_eq!(n_of(&store, "items", "b"), Some(2));
    }
}

/// An incremental checkpoint writes its list of generations, then its
/// manifest. Dying between the two left the list behind, and the
/// checkpoint recovery makes took the same sequence number, so the list
/// passed for its own and named generation files recovery had removed.
/// The database could not be opened again.
#[test]
fn the_database_opens_after_an_incremental_checkpoint_dies_before_its_manifest() {
    for at in 1..40 {
        let filesystem = Arc::new(SimulatedFileSystem::new(1));
        let store = open(&filesystem).unwrap();
        note(&store, "a", 1);
        store.compact().unwrap();
        note(&store, "a", 2);
        store.checkpoint().unwrap();
        note(&store, "a", 3);
        filesystem.crash_after(at, false);
        let _ = store.checkpoint();
        filesystem.disarm();
        if !filesystem.is_down() {
            continue;
        }
        drop(store);
        filesystem.recover();

        let store = open(&filesystem)
            .unwrap_or_else(|error| panic!("crash at operation {at}: cannot reopen: {error}"));
        assert_eq!(n_of(&store, "notes", "a"), Some(3), "crash at operation {at}");
        drop(store);
        let store = open(&filesystem).unwrap_or_else(|error| {
            panic!("crash at operation {at}: cannot reopen a second time: {error}")
        });
        assert_eq!(n_of(&store, "notes", "a"), Some(3), "crash at operation {at}");
    }
}

/// A compaction of a collection with a schema writes a small journal
/// before its sort runs. Dying while writing it left a journal that was
/// not all there, and opening the database failed on it.
#[test]
fn the_database_opens_after_a_compaction_dies_writing_its_build_journal() {
    for at in 1..60 {
        let filesystem = Arc::new(SimulatedFileSystem::new(at));
        filesystem.set_in_order_only(true);
        let store = open(&filesystem).unwrap();
        declare_items(&store);
        item(&store, "a", 1);
        filesystem.crash_after(at, true);
        let _ = store.compact();
        filesystem.disarm();
        if !filesystem.is_down() {
            continue;
        }
        drop(store);
        filesystem.recover();

        let store = open(&filesystem)
            .unwrap_or_else(|error| panic!("crash at operation {at}: cannot reopen: {error}"));
        assert_eq!(n_of(&store, "items", "a"), Some(1), "crash at operation {at}");
    }
}

/// An incremental checkpoint that fails is kept to be finished later.
/// Finishing it took a lock and, when it failed again, asked for the same
/// lock to put it back: the thread waited on itself for ever, and closing
/// the database waited on the thread.
#[test]
fn a_checkpoint_that_fails_twice_gives_an_error_both_times() {
    for at in 1..40 {
        let filesystem = Arc::new(SimulatedFileSystem::new(1));
        let store = open(&filesystem).unwrap();
        note(&store, "a", 1);
        store.compact().unwrap();
        note(&store, "a", 2);
        filesystem.crash_after(at, false);
        let first = store.checkpoint();
        filesystem.disarm();
        if !filesystem.is_down() {
            continue;
        }
        assert!(first.is_err());
        // The disk is still down, so the second attempt fails too. It must
        // say so rather than never come back.
        let (done, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let second = store.checkpoint().is_err();
            drop(store);
            let _ = done.send(second);
        });
        let second = finished
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap_or_else(|_| panic!("crash at operation {at}: the second checkpoint hangs"));
        assert!(second, "crash at operation {at}");
        worker.join().unwrap();
    }
}
