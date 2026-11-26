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
use owl_db::storage_engine::{
    AutoCheckpointPolicy, MemoryObjectStore, ObjectFileSystem, ObjectStore, RustStore, S3Config, S3Faults,
    S3ObjectStore, SimulatedS3, WriteOperation,
};
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

/// An object store that is reached from one machine and cannot be
/// reached once that machine has lost power: a dead machine puts and
/// deletes nothing. What is already in the store stays.
struct Tethered {
    store: Arc<dyn ObjectStore>,
    machine: Arc<SimulatedFileSystem>,
}

impl Tethered {
    fn up(&self) -> std::io::Result<()> {
        match self.machine.is_down() {
            true => Err(std::io::Error::other("the machine is down")),
            false => Ok(()),
        }
    }
}

impl ObjectStore for Tethered {
    fn put(&self, key: &str, source: &mut dyn std::io::Read, len: u64) -> std::io::Result<()> {
        self.up()?;
        self.store.put(key, source, len)
    }

    fn read_at(&self, key: &str, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.up()?;
        self.store.read_at(key, offset, buffer)
    }

    fn len(&self, key: &str) -> std::io::Result<Option<u64>> {
        self.up()?;
        self.store.len(key)
    }

    fn delete(&self, key: &str) -> std::io::Result<()> {
        self.up()?;
        self.store.delete(key)
    }

    fn list(&self, prefix: &str) -> std::io::Result<Vec<String>> {
        self.up()?;
        self.store.list(prefix)
    }

    fn version(&self, key: &str) -> std::io::Result<Option<String>> {
        self.up()?;
        self.store.version(key)
    }

    fn put_if(
        &self,
        key: &str,
        source: &mut dyn std::io::Read,
        len: u64,
        expected: Option<&str>,
    ) -> std::io::Result<Option<String>> {
        self.up()?;
        self.store.put_if(key, source, len, expected)
    }

    fn delete_if(&self, key: &str, expected: &str) -> std::io::Result<bool> {
        self.up()?;
        self.store.delete_if(key, expected)
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
    /// The object store the database's parts are kept in, when the run
    /// is of a database that keeps them there.
    objects: Option<Arc<dyn ObjectStore>>,
    /// The file system the open store was given, which is what publishes
    /// and what drops local parts.
    layer: Option<Arc<ObjectFileSystem>>,
    /// What a reader may see: the rows of the last publication, or, while
    /// a publication was cut off, of either that one or the one before.
    /// `None` is a database never published.
    published: Vec<Option<Rows>>,
}

/// What may go wrong with the disk, besides losing power.
#[derive(Clone, Copy)]
struct Faults {
    /// Writes and syncs fail now and then.
    io_faults: bool,
    /// A crash may keep a later unsynced write and lose an earlier one.
    reorder: bool,
    /// Whether the database keeps its parts in an object store, publishes
    /// for readers, and drops its local parts now and then; and in what.
    objects: Objects,
}

/// Where a run's database keeps its parts besides its own disk.
#[derive(Clone, Copy, PartialEq)]
enum Objects {
    /// Nowhere.
    No,
    /// An object store that does as it is asked.
    Memory,
    /// S3, through the S3 client, where the S3 is a simulated one that
    /// loses requests and answers, fails, and cuts answers short.
    S3,
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
        let Faults {
            io_faults,
            reorder,
            objects,
        } = faults;
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
            objects: match objects {
                Objects::No => None,
                Objects::Memory => Some(Arc::new(MemoryObjectStore::new())),
                Objects::S3 => {
                    let s3 = SimulatedS3::new("owl", true, seed ^ 0x0b1e_c7ed).with_page(5);
                    s3.set_faults(S3Faults {
                        lost_request: 0.03,
                        error: 0.03,
                        lost_answer: 0.06,
                        cut_short: 0.06,
                    });
                    let config = S3Config {
                        bucket: "owl".to_string(),
                        region: "us-east-1".to_string(),
                        access_key: "AKIDEXAMPLE".to_string(),
                        secret_key: "secret".to_string(),
                        endpoint: "s3.test".to_string(),
                        path_style: true,
                        prefix: "crash/".to_string(),
                        // Small, so that parts and logs go up in parts.
                        part_bytes: 700,
                        // Enough that a request failing every time is
                        // not chance.
                        attempts: 40,
                    };
                    let clock = Arc::new(|| 1_700_000_000);
                    Some(Arc::new(S3ObjectStore::new(config, Arc::new(s3), clock, u128::from(seed))))
                }
            },
            layer: None,
            published: vec![None],
        }
    }

    /// Open the store on the run's machine: on its disk alone, or as the
    /// writer of a database whose parts are in the object store.
    fn open(&mut self) -> Result<RustStore, String> {
        let Some(objects) = &self.objects else {
            return open(&self.filesystem).map_err(|error| error.to_string());
        };
        let tethered = Arc::new(Tethered {
            store: objects.clone(),
            machine: self.filesystem.clone(),
        });
        let layer = ObjectFileSystem::writer(self.filesystem.clone(), tethered, "", DATABASE, 4 << 20)
            .map_err(|error| format!("cannot claim the database: {error}"))?;
        let store =
            RustStore::new_with_fs(Path::new(DATABASE), 1 << 30, layer.clone()).map_err(|error| error.to_string())?;
        store
            .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
            .map_err(|error| error.to_string())?;
        self.layer = Some(layer);
        Ok(store)
    }

    /// A reader on a machine of its own must see a publication that was
    /// made: the last one, or, if the last was cut off, that or the one
    /// before. And what it opens must be sound.
    fn check_reader(&mut self) -> Result<(), String> {
        let Some(objects) = self.objects.clone() else {
            return Ok(());
        };
        let machine = Arc::new(SimulatedFileSystem::new(self.seed ^ 0x51ed_270b));
        let found = match ObjectFileSystem::reader(machine, objects, "", DATABASE, 1 << 20) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(self.fail(format!("a reader cannot fetch the publication: {error}"))),
            Ok((filesystem, _)) => {
                let reader = RustStore::new_with_fs(Path::new(DATABASE), 1 << 30, filesystem)
                    .map_err(|error| self.fail(format!("a reader cannot open the publication: {error}")))?;
                reader
                    .verify_integrity()
                    .map_err(|error| self.fail(format!("a reader's integrity check fails: {error}")))?;
                Some(read(&reader).map_err(|error| self.fail(format!("a reader: {error}")))?)
            }
        };
        if !self.published.contains(&found) {
            let message = match (&found, self.published.last().cloned().flatten()) {
                (Some(found), Some(last)) => {
                    format!("a reader sees rows that were not published: {}", difference(&last, found))
                }
                (None, _) => "a reader finds nothing published, though a publication was made".to_string(),
                (Some(_), None) => "a reader sees a publication that was never made".to_string(),
            };
            return Err(self.fail(message));
        }
        self.published = vec![found];
        Ok(())
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
        // A database with its parts in an object store publishes and
        // drops its local parts as part of its work. These draw from the
        // seed only in such a run, so the others are the runs they were.
        if let Some(layer) = self.layer.clone() {
            let pick = self.rng.gen_range(0..100);
            if pick < 20 {
                let (name, result) = if pick < 13 {
                    // What a reader may see from here on is these rows,
                    // or, if this is cut off, still the rows before.
                    self.published.push(Some(self.rows.clone()));
                    let result = store.publish(&layer).map(|_| ()).map_err(|error| error.to_string());
                    if result.is_ok() {
                        self.published = vec![Some(self.rows.clone())];
                    }
                    ("publish", result)
                } else {
                    ("drop local parts", layer.drop_local_parts().map(|_| ()).map_err(|error| error.to_string()))
                };
                return match result {
                    Ok(()) => {
                        self.trace.push(format!("{name}: done"));
                        Outcome::Done(Vec::new())
                    }
                    Err(error) => {
                        self.trace.push(format!("{name}: {error}"));
                        Outcome::Unknown(Vec::new(), error)
                    }
                };
            }
        }
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
            let opened = self.open();
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
        self.open().map_err(|error| self.fail(format!("cannot reopen: {error}")))
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
            // (Not where the parts are in an object store: a second
            // open would claim the database from the first.)
            let again = match self.objects {
                Some(_) => Err("not opened afresh".to_string()),
                None => open(&self.filesystem).map_err(|e| e.to_string()).and_then(|again| read(&again)),
            };
            let lost = match again {
                Ok(again) if again == self.rows => "a fresh open still holds every row".to_string(),
                Ok(again) => format!("a fresh open has lost rows: {}", difference(&self.rows, &again)),
                Err(error) => format!("a fresh open fails: {error}"),
            };
            return Err(self.fail(format!("integrity check fails: {error}; {lost}")));
        }
        self.check_reader()
    }

    fn run(&mut self, steps: usize) -> Result<(), String> {
        let mut store = self.open().map_err(|error| self.fail(format!("cannot open: {error}")))?;
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
                store = self
                    .open()
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
        let store = self
            .open()
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
    // Every opening audits its indexes both against their pages and
    // entry by entry, and fails if the two disagree.
    RustStore::audit_opens_both_ways(true);
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
    // A compaction that finds the index pages it builds over to be other
    // than it expected falls back to building from every row, which
    // hides nothing but should not happen.
    let (over_pages, refused) = RustStore::index_page_carries();
    println!("{over_pages} compactions built their indexes over pages, {refused} could not");
    println!(
        "{} openings audited their indexes against pages and entry by entry, and the two agreed",
        RustStore::opens_audited_against_pages()
    );
    assert_eq!(refused, 0, "compactions that could not build their indexes over pages");
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
        objects: Objects::No,
    };
    sweep(1, seeds(300), 60, faults);
}

#[test]
fn acknowledged_writes_survive_failing_writes_and_syncs() {
    let faults = Faults {
        io_faults: true,
        reorder: false,
        objects: Objects::No,
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
        objects: Objects::No,
    };
    for seed in [101_261, 100_722] {
        let mut run = Run::new(seed, faults);
        if let Err(message) = run.run(60) {
            panic!("{message}");
        }
    }
}

/// A disk may keep a later unsynced write and lose an earlier one, which
/// leaves a hole in the log with bytes after it. Each frame says how much
/// of the log was on disk when it was written, so recovery can see that
/// nothing after the hole was written with the hole's bytes synced, and
/// reads the hole as the end of the log. Nothing acknowledged is past it.
#[test]
fn acknowledged_writes_survive_a_crash_that_reorders_writes() {
    let faults = Faults {
        io_faults: false,
        reorder: true,
        objects: Objects::No,
    };
    sweep(200_000, seeds(300), 60, faults);
}

/// The same promises for a database that keeps its parts in an object
/// store: it claims the database each time it opens, publishes for
/// readers, and now and then drops its local parts so that they are read
/// back from the object store. The object store keeps what it was given
/// when the machine dies, and a dead machine gives it nothing more. After
/// every crash a reader on a machine of its own must also see a
/// publication that was made, whole.
#[test]
fn acknowledged_writes_survive_a_crash_with_parts_in_an_object_store() {
    let faults = Faults {
        io_faults: false,
        reorder: false,
        objects: Objects::Memory,
    };
    sweep(300_000, seeds(300), 60, faults);
}

/// The same, on a disk that may keep a later unsynced write and lose an
/// earlier one.
#[test]
fn acknowledged_writes_survive_a_reordering_crash_with_parts_in_an_object_store() {
    let faults = Faults {
        io_faults: false,
        reorder: true,
        objects: Objects::Memory,
    };
    sweep(400_000, seeds(300), 60, faults);
}

/// The same, on a disk whose writes and syncs fail now and then.
#[test]
fn acknowledged_writes_survive_failing_writes_with_parts_in_an_object_store() {
    let faults = Faults {
        io_faults: true,
        reorder: false,
        objects: Objects::Memory,
    };
    sweep(500_000, seeds(300), 60, faults);
}

/// The same again through the S3 client, against an S3 that loses about
/// one request or answer in six: a request never arrives, the service
/// fails, a request is applied and its answer lost, a read is answered
/// short. The store, its claim, its publications and its readers must
/// come out as they do against an object store that never errs.
#[test]
fn acknowledged_writes_survive_a_crash_with_parts_in_a_failing_s3() {
    let faults = Faults {
        io_faults: false,
        reorder: false,
        objects: Objects::S3,
    };
    sweep(600_000, seeds(300), 60, faults);
}

/// And with the disk reordering its unsynced writes as well.
#[test]
fn acknowledged_writes_survive_a_reordering_crash_with_parts_in_a_failing_s3() {
    let faults = Faults {
        io_faults: false,
        reorder: true,
        objects: Objects::S3,
    };
    sweep(700_000, seeds(300), 60, faults);
}

#[test]
fn one_seed_gives_one_run() {
    let faults = Faults {
        io_faults: false,
        reorder: false,
        objects: Objects::No,
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
