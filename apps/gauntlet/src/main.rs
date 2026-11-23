//! Gauntlet: hard queries over a hostile data model, checked and timed
//! against SQLite.
//!
//! Every query in `queries/` is a pair: `name.sql`, which defines the
//! answer, and `name.oql`. An `.oql` file holding only comments marks a
//! query OQL cannot express yet. `check` requires the same rows from both
//! engines; `bench` also times them; `show` prints both answers.

mod columns;
mod data;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use data::{Cell, Row, Table};
use owl_db::storage::DocumentStore;
use owl_db::Value;
use serde_json::Value as Json;
use std::path::PathBuf;
use std::time::Instant;
use storefront::engines::{first_difference, Owl, Sqlite, BATCH};

#[derive(Parser)]
#[command(about = "Hard queries for Owl, checked and timed against SQLite")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Require every query to return the same rows on both engines.
    Check {
        #[arg(long, default_value_t = 2000)]
        people: usize,
        /// Only run queries whose name contains this text.
        #[arg(long)]
        only: Option<String>,
    },
    /// Check every query and time it on both engines.
    Bench {
        #[arg(long, default_value_t = 2000)]
        people: usize,
        #[arg(long)]
        only: Option<String>,
        /// Timed runs per query, after one untimed first run.
        #[arg(long, default_value_t = 5)]
        samples: usize,
    },
    /// Print what each engine answers. An argument is a query name or
    /// prefix, or an OQL statement to run on Owl alone.
    Show {
        #[arg(long, default_value_t = 2000)]
        people: usize,
        /// Rows to print from each answer.
        #[arg(long, default_value_t = 5)]
        rows: usize,
        queries: Vec<String>,
    },
    /// How much memory Owl takes: after loading, after a checkpoint, and
    /// after every query has run. SQLite is not opened, so the figures are
    /// Owl's and the data generator's alone.
    Memory {
        #[arg(long, default_value_t = 2000)]
        people: usize,
        /// Only run queries whose name contains this text.
        #[arg(long)]
        only: Option<String>,
    },
    /// How one database behaves as it grows: how fast it loads, checkpoints,
    /// compacts, reopens, commits one row at a time and reads one row, and
    /// what it holds in memory and on disk. Owl alone.
    Scale {
        #[arg(long, default_value_t = 2000)]
        people: usize,
        /// Keep the database in this directory instead of a temporary
        /// one. When one is already there, only reopen it, `--reopens`
        /// times, which is the way to look closely at opening.
        #[arg(long)]
        at: Option<PathBuf>,
        #[arg(long, default_value_t = 5)]
        reopens: usize,
        /// Load the generated rows this many times over, each copy under
        /// ids of its own. The generator holds every row it makes, so
        /// this is the way to a database larger than memory.
        #[arg(long, default_value_t = 1)]
        copies: usize,
        /// Give each copy ids that fall in between the ids already
        /// loaded, as random ids would, where the default gives each copy
        /// ids that come after them, as ids that grow with time do. Rows
        /// arriving in between existing ones are the harder case: no
        /// stretch of the database is left untouched by them.
        #[arg(long)]
        scattered: bool,
        /// With a database already at `--at`: compact it this many
        /// times once it is open, timing each, which is the way to look
        /// closely at a compaction without loading again.
        #[arg(long, default_value_t = 0)]
        compactions: usize,
    },
    /// An experiment: time two slow questions over rows, over the store's
    /// column segments and over plain arrays.
    Columns {
        #[arg(long, default_value_t = 2000)]
        people: usize,
        #[arg(long, default_value_t = 21)]
        samples: usize,
    },
}

struct Query {
    name: String,
    /// `None` when OQL has no way to say it yet.
    oql: Option<String>,
    sql: String,
}

fn queries(only: Option<&str>) -> Result<Vec<Query>> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("queries");
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .with_context(|| format!("reading {}", directory.display()))?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(".sql").map(str::to_string)
        })
        .filter(|name| only.is_none_or(|text| name.contains(text)))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let read = |extension: &str| {
                let path = directory.join(format!("{name}.{extension}"));
                std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
            };
            let oql = read("oql")?;
            let written = oql.lines().any(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with("//")
            });
            Ok(Query { sql: read("sql")?, oql: written.then_some(oql), name })
        })
        .collect()
}

struct Loaded {
    _directory: tempfile::TempDir,
    owl: Owl,
    sqlite: Sqlite,
}

fn owl_value(cell: &Cell) -> Option<Value> {
    Some(match cell {
        Cell::Text(value) => Value::String(value.clone()),
        Cell::Int(value) => Value::Integer(*value),
        Cell::Float(value) => Value::Float(*value),
        Cell::Absent => return None,
        Cell::Texts(values) => Value::List(values.iter().cloned().map(Value::String).collect()),
        Cell::Rows(rows) => Value::List(rows.iter().map(|row| Value::Object(owl_row(row))).collect()),
    })
}

fn owl_row(row: &Row) -> owl_db::Fields {
    row.iter()
        .filter_map(|(name, cell)| Some(((*name).to_string(), owl_value(cell)?)))
        .collect()
}

fn sql_value(cell: &Cell) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match cell {
        Cell::Text(value) => Sql::Text(value.clone()),
        Cell::Int(value) => Sql::Integer(*value),
        Cell::Float(value) => Sql::Real(*value),
        Cell::Absent | Cell::Texts(_) | Cell::Rows(_) => Sql::Null,
    }
}

/// Insert one table. Scalar fields are the row's columns, in order; a list
/// goes to the child table `<table>_<field>` as (parent, line, columns).
fn load_sqlite(sqlite: &mut Sqlite, table: &Table) -> Result<()> {
    let insert = |name: &str, columns: usize| {
        let marks = vec!["?"; columns].join(", ");
        format!("INSERT INTO {name} VALUES ({marks})")
    };
    for batch in table.rows.chunks(BATCH) {
        let transaction = sqlite.connection().transaction()?;
        for row in batch {
            let scalars: Vec<_> = row
                .iter()
                .filter(|(_, cell)| !matches!(cell, Cell::Texts(_) | Cell::Rows(_)))
                .map(|(_, cell)| sql_value(cell))
                .collect();
            transaction
                .prepare_cached(&insert(table.name, scalars.len()))?
                .execute(rusqlite::params_from_iter(scalars))?;
            let parent = sql_value(&row[0].1);
            for (field, cell) in row {
                let child = format!("{}_{field}", table.name);
                let lines: Vec<Vec<rusqlite::types::Value>> = match cell {
                    Cell::Texts(values) => values
                        .iter()
                        .map(|value| vec![rusqlite::types::Value::Text(value.clone())])
                        .collect(),
                    Cell::Rows(rows) => rows
                        .iter()
                        .map(|row| row.iter().map(|(_, cell)| sql_value(cell)).collect())
                        .collect(),
                    _ => continue,
                };
                for (line, values) in lines.into_iter().enumerate() {
                    let mut statement =
                        transaction.prepare_cached(&insert(&child, values.len() + 2))?;
                    let mut parameters = vec![parent.clone(), (line as i64).into()];
                    parameters.extend(values);
                    statement.execute(rusqlite::params_from_iter(parameters))?;
                }
            }
        }
        transaction.commit()?;
    }
    Ok(())
}

/// Whether to move the rows into column segments after loading, as
/// `OWL_COMPACT=1` asks. Queries then run against the store as it is after
/// compaction: columns on disk, and rows decoded only when read.
fn compacted() -> bool {
    std::env::var("OWL_COMPACT").is_ok_and(|value| value == "1")
}

/// Counts the bytes this process has allocated and not yet freed. What
/// the system reports for a process depends on what else the machine is
/// doing; this does not.
struct Counting;

static LIVE_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The most that was held at once since it was last asked for.
static PEAK_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// SAFETY: every call is passed straight to the system allocator; the count
// beside it does not touch the memory.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let now = LIVE_BYTES.fetch_add(layout.size(), std::sync::atomic::Ordering::Relaxed) + layout.size();
        PEAK_BYTES.fetch_max(now, std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        let now = LIVE_BYTES.fetch_add(size, std::sync::atomic::Ordering::Relaxed) + size;
        PEAK_BYTES.fetch_max(now, std::sync::atomic::Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What this process has allocated and still holds, in megabytes.
fn resident_megabytes() -> f64 {
    LIVE_BYTES.load(std::sync::atomic::Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
}

fn memory(people: usize, only: Option<&str>) -> Result<()> {
    let queries = queries(only)?;
    let start = resident_megabytes();
    let directory = tempfile::TempDir::new()?;
    let path = directory.path().join("gauntlet.owl");
    let owl = Owl::open_with(&path, include_str!("../schema.oql"))?;
    let mut rows = 0usize;
    let mut source_bytes = 0usize;
    {
        let tables = data::generate(people);
        for table in &tables {
            rows += table.rows.len();
            for batch in table.rows.chunks(BATCH) {
                owl.insert_rows(table.name, batch.iter().map(owl_row).collect())?;
            }
            source_bytes += table
                .rows
                .iter()
                .map(|row| serde_json::to_string(&row.iter().map(|(name, cell)| (name, format!("{cell:?}"))).collect::<Vec<_>>()).map_or(0, |text| text.len()))
                .sum::<usize>();
        }
    }
    let report = |when: &str, base: f64| {
        let now = resident_megabytes();
        // The most held at once since the last line, which is what the
        // work in between needed on top of what it left behind.
        let peak = PEAK_BYTES.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / (1024.0 * 1024.0);
        println!(
            "{when:<36} {:>7.0} MB held {:>7.0} bytes a row {:>8.0} MB at most meanwhile",
            now - base,
            (now - base) * 1024.0 * 1024.0 / rows as f64,
            peak - base
        );
        now
    };
    println!("{rows} rows ({people} people); written out as text they are about {:.0} MB\n", source_bytes as f64 / 1e6);
    println!(
        "a value is {} bytes, a row before its fields {} bytes\n",
        std::mem::size_of::<Value>(),
        std::mem::size_of::<owl_db::Document>()
    );
    report("after loading", start);
    owl.checkpoint()?;
    report("after a checkpoint", start);
    let files_of = |directory: &std::path::Path| -> Vec<(String, u64)> {
        let mut files: Vec<(String, u64)> = std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                Some((entry.file_name().to_string_lossy().into_owned(), entry.metadata().ok()?.len()))
            })
            .collect();
        files.sort();
        files
    };
    let files = files_of(directory.path());
    let on_disk: u64 = files.iter().map(|(_, size)| size).sum();
    println!("{:<44} {:>8.1} MB", "the database's files on disk", on_disk as f64 / 1e6);
    for (name, size) in &files {
        println!("    {name:<44} {:>8.1} MB", *size as f64 / 1e6);
    }
    owl.compact()?;
    report("after compaction", start);
    let files = files_of(directory.path());
    let compacted: u64 = files.iter().map(|(_, size)| size).sum();
    println!("{:<44} {:>8.1} MB", "the same after compaction", compacted as f64 / 1e6);
    for (name, size) in &files {
        println!("    {name:<44} {:>8.1} MB", *size as f64 / 1e6);
    }
    owl.compact_and_reclaim()?;
    report("after compacting again", start);
    let reclaimed: u64 = files_of(directory.path()).iter().map(|(_, size)| size).sum();
    println!(
        "{:<44} {:>8.1} MB",
        "with the copy it replaced removed",
        reclaimed as f64 / 1e6
    );
    // The same data in SQLite, with the indexes its schema declares.
    {
        let other = tempfile::TempDir::new()?;
        let mut sqlite = Sqlite::open_with(&other.path().join("gauntlet.sqlite"), include_str!("../schema.sql"))?;
        for table in &data::generate(people) {
            load_sqlite(&mut sqlite, table)?;
        }
        sqlite.checkpoint()?;
        drop(sqlite);
        let size: u64 = files_of(other.path()).iter().map(|(_, size)| size).sum();
        println!("{:<44} {:>8.1} MB", "SQLite's files for the same data", size as f64 / 1e6);
    }
    // Loading SQLite generated the data again, which is not Owl's doing.
    PEAK_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
    // Close the database and open it again: what a restart holds before a
    // single question is asked, and what the questions then bring in.
    drop(owl);
    report("closed", start);
    let owl = Owl::open_with(&path, include_str!("../schema.oql"))?;
    report("opened again", start);
    for round in 1..=2 {
        for query in &queries {
            if let Some(oql) = &query.oql {
                owl.query(oql)?;
            }
        }
        report(&format!("after every query, round {round}"), start);
    }
    // Which queries need the most on top of what is held.
    let mut needs: Vec<(f64, &str)> = Vec::new();
    for query in &queries {
        if let Some(oql) = &query.oql {
            let before = resident_megabytes();
            PEAK_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
            owl.query(oql)?;
            let peak = PEAK_BYTES.load(std::sync::atomic::Ordering::Relaxed) as f64 / (1024.0 * 1024.0);
            needs.push((peak - before, query.name.as_str()));
        }
    }
    needs.sort_by(|left, right| right.0.total_cmp(&left.0));
    println!("\nthe queries that need the most while they run:");
    for (megabytes, name) in needs.iter().take(8) {
        println!("  {name:<34} {megabytes:>7.0} MB");
    }
    println!("\nwhat the store holds, part by part (estimated):");
    for (name, entries, bytes) in owl.store().memory_breakdown().map_err(|error| anyhow!("{error}"))? {
        println!("  {name:<46} {entries:>8} entries {:>7.1} MB", bytes as f64 / 1e6);
    }
    for collection in ["people", "tasks", "events"] {
        let (live, resident, lazy) = owl.residency(collection)?;
        println!("{collection}: {live} rows; store-wide {resident} held decoded, {lazy} only in column segments");
    }
    Ok(())
}

/// Rows held back from the load, to be committed one at a time.
const SINGLE_COMMITS: usize = 2000;
/// Rows looked up by id.
const LOOKUPS: usize = 20_000;

/// Whether each copy's ids fall in between those already loaded.
static SCATTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The id as one copy of the rows has it. The first copy keeps the ids
/// as they were generated. A later copy's ids sort after every earlier
/// copy's, or, when scattered, in between them.
fn id_in_copy(id: &str, copy: usize) -> String {
    match (copy, SCATTERED.load(std::sync::atomic::Ordering::Relaxed)) {
        (0, _) => id.to_string(),
        (copy, true) => format!("{id}-{copy}"),
        // Past any generated id, which starts with a lower-case letter,
        // and in the order of the copies.
        (copy, false) => format!("z{copy:04}-{id}"),
    }
}

/// A row as one copy has it: every text that is the id of a generated
/// row, whether the row's own or a reference to another, names that
/// copy's row.
fn row_in_copy(row: &Row, copy: usize, ids: &std::collections::HashSet<String>) -> Row {
    let text = |value: &String| match ids.contains(value) {
        true => id_in_copy(value, copy),
        false => value.clone(),
    };
    row.iter()
        .map(|(name, cell)| {
            let cell = match cell {
                Cell::Text(value) => Cell::Text(text(value)),
                Cell::Texts(values) => Cell::Texts(values.iter().map(text).collect()),
                Cell::Rows(rows) => Cell::Rows(rows.iter().map(|row| row_in_copy(row, copy, ids)).collect()),
                Cell::Int(value) => Cell::Int(*value),
                Cell::Float(value) => Cell::Float(*value),
                Cell::Absent => Cell::Absent,
            };
            (*name, cell)
        })
        .collect()
}

fn scale(
    people: usize,
    at: Option<PathBuf>,
    reopens: usize,
    copies: usize,
    compactions: usize,
) -> Result<()> {
    let megabytes = |bytes: u64| bytes as f64 / 1e6;
    let size_of = |directory: &std::path::Path| -> u64 {
        std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .filter_map(|entry| Some(entry.ok()?.metadata().ok()?.len()))
            .sum()
    };
    let base = resident_megabytes();
    let held = || {
        let peak = PEAK_BYTES.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / (1024.0 * 1024.0);
        (resident_megabytes() - base, peak - base)
    };
    let line = |what: &str, seconds: f64, note: String| {
        let (now, peak) = held();
        println!("{what:<34} {seconds:>8.2} s {now:>7.0} MB held {peak:>7.0} MB at most   {note}");
    };

    let temporary = tempfile::TempDir::new()?;
    let directory = at.unwrap_or_else(|| temporary.path().to_path_buf());
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("gauntlet.owl");
    if path.exists() {
        for _ in 0..reopens {
            held();
            let started = Instant::now();
            let owl = Owl::open_with(&path, include_str!("../schema.oql"))?;
            line("reopen", started.elapsed().as_secs_f64(), String::new());
            drop(owl);
        }
        // What an open database holds before anything is asked of it.
        let owl = Owl::open_with(&path, include_str!("../schema.oql"))?;
        println!("\nwhat the store holds once open, part by part (estimated):");
        for (name, entries, bytes) in owl.store().memory_breakdown().map_err(|error| anyhow!("{error}"))? {
            println!("  {name:<46} {entries:>9} entries {:>7.1} MB", bytes as f64 / 1e6);
        }
        for _ in 0..compactions {
            held();
            let started = Instant::now();
            owl.compact()?;
            line(
                "compaction",
                started.elapsed().as_secs_f64(),
                format!("{:.0} MB on disk", megabytes(size_of(&directory))),
            );
        }
        // Left open for a while when asked, so a tool can look at it.
        if let Some(seconds) = std::env::var("OWL_HOLD_OPEN").ok().and_then(|value| value.parse().ok()) {
            std::thread::sleep(std::time::Duration::from_secs(seconds));
        }
        drop(owl);
        return Ok(());
    }
    let owl = Owl::open_with(&path, include_str!("../schema.oql"))?;

    let started = Instant::now();
    let mut tables = data::generate(people);
    let generated = started.elapsed().as_secs_f64();
    // The generator's rows are not Owl's doing; count from here.
    let generator = resident_megabytes() - base;
    let mut kept_back: Vec<Row> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for table in &mut tables {
        if table.name == "events" && table.rows.len() > 2 * SINGLE_COMMITS {
            kept_back = table.rows.split_off(table.rows.len() - SINGLE_COMMITS);
        }
        if table.name == "people" {
            ids = table
                .rows
                .iter()
                .filter_map(|row| match &row[0].1 {
                    Cell::Text(id) => Some(id.clone()),
                    _ => None,
                })
                .collect();
        }
    }
    let rows: usize = tables.iter().map(|table| table.rows.len()).sum::<usize>() * copies;
    println!(
        "{rows} rows ({people} people, {copies} time(s) over); generating them took {generated:.1} s and holds {generator:.0} MB of its own\n"
    );

    let started = Instant::now();
    if copies == 1 {
        for table in tables {
            for batch in table.rows.chunks(BATCH) {
                owl.insert_rows(table.name, batch.iter().map(owl_row).collect())?;
            }
            // Each table is let go once it is in, so the generator's rows
            // do not sit beside Owl's for the whole load.
        }
    } else {
        let generated_ids: std::collections::HashSet<String> = tables
            .iter()
            .flat_map(|table| &table.rows)
            .filter_map(|row| match row.first() {
                Some(("id", Cell::Text(id))) => Some(id.clone()),
                _ => None,
            })
            .collect();
        for copy in 0..copies {
            let copy_started = Instant::now();
            for table in &tables {
                for batch in table.rows.chunks(BATCH) {
                    owl.insert_rows(
                        table.name,
                        batch
                            .iter()
                            .map(|row| owl_row(&row_in_copy(row, copy, &generated_ids)))
                            .collect(),
                    )?;
                }
            }
            let each = rows / copies;
            eprintln!(
                "  copy {} of {copies}: {:.0} rows a second, {:.0} MB held",
                copy + 1,
                each as f64 / copy_started.elapsed().as_secs_f64(),
                resident_megabytes() - base
            );
        }
        drop(tables);
    }
    let seconds = started.elapsed().as_secs_f64();
    line("load", seconds, format!("{:.0} rows a second", rows as f64 / seconds));

    let started = Instant::now();
    owl.checkpoint()?;
    line(
        "checkpoint",
        started.elapsed().as_secs_f64(),
        format!("{:.0} MB on disk", megabytes(size_of(&directory))),
    );

    let started = Instant::now();
    owl.compact()?;
    line(
        "compaction",
        started.elapsed().as_secs_f64(),
        format!("{:.0} MB on disk", megabytes(size_of(&directory))),
    );

    let started = Instant::now();
    owl.compact_and_reclaim()?;
    line(
        "compaction again, old copy removed",
        started.elapsed().as_secs_f64(),
        format!("{:.0} MB on disk", megabytes(size_of(&directory))),
    );

    let started = Instant::now();
    let commits = kept_back.len();
    for row in &kept_back {
        owl.insert_rows("events", vec![owl_row(row)])?;
    }
    let seconds = started.elapsed().as_secs_f64();
    line(
        "commits of one row each",
        seconds,
        format!("{commits} commits, {:.0} a second", commits as f64 / seconds.max(1e-9)),
    );

    drop(owl);
    held();
    let started = Instant::now();
    let owl = Owl::open_with(&path, include_str!("../schema.oql"))?;
    line("reopen", started.elapsed().as_secs_f64(), String::new());

    // Every so-manyth id, so the lookups are spread over the collection.
    let step = (ids.len() / LOOKUPS.min(ids.len()).max(1)).max(1);
    let wanted: Vec<String> = ids
        .iter()
        .step_by(step)
        .enumerate()
        .map(|(position, id)| id_in_copy(id, position % copies))
        .collect();
    let started = Instant::now();
    let mut found = 0usize;
    for id in &wanted {
        if owl
            .store()
            .get_document("people", id)
            .map_err(|error| anyhow!("{error}"))?
            .is_some()
        {
            found += 1;
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    line(
        "lookups by id",
        seconds,
        format!(
            "{found} found, {:.1} microseconds each",
            seconds * 1e6 / wanted.len().max(1) as f64
        ),
    );

    for round in ["first", "second"] {
        let started = Instant::now();
        owl.query("events.count()")?;
        line(&format!("count every event, {round} time"), started.elapsed().as_secs_f64(), String::new());
    }
    Ok(())
}

fn load(people: usize) -> Result<Loaded> {
    let tables = data::generate(people);
    let directory = tempfile::TempDir::new()?;
    let owl = Owl::open_with(&directory.path().join("gauntlet.owl"), include_str!("../schema.oql"))?;
    let mut sqlite = Sqlite::open_with(
        &directory.path().join("gauntlet.sqlite"),
        include_str!("../schema.sql"),
    )?;
    let mut rows = 0;
    for table in &tables {
        rows += table.rows.len();
        for batch in table.rows.chunks(BATCH) {
            owl.insert_rows(table.name, batch.iter().map(owl_row).collect())?;
        }
        load_sqlite(&mut sqlite, table).with_context(|| format!("loading {} into SQLite", table.name))?;
    }
    owl.checkpoint()?;
    if compacted() {
        owl.compact()?;
        println!("compacted: rows are in column segments");
    }
    sqlite.checkpoint()?;
    println!("{rows} rows loaded into each engine ({people} people)\n");
    Ok(Loaded { _directory: directory, owl, sqlite })
}

enum Outcome {
    Same(usize),
    /// OQL cannot say it.
    Missing,
    Failed(String),
    Differ(String),
}

fn compare(loaded: &Loaded, query: &Query) -> Result<Outcome> {
    let sqlite = loaded
        .sqlite
        .query(&query.sql)
        .with_context(|| format!("{}.sql", query.name))?;
    let Some(oql) = &query.oql else {
        return Ok(Outcome::Missing);
    };
    Ok(match loaded.owl.query(oql) {
        Err(error) => Outcome::Failed(error.to_string()),
        Ok(owl) => match first_difference(&owl, &sqlite) {
            None => Outcome::Same(owl.len()),
            Some(difference) => Outcome::Differ(difference),
        },
    })
}

/// The median of `samples` runs after one untimed run, in milliseconds.
fn median_ms(samples: usize, mut run: impl FnMut() -> Result<()>) -> Result<f64> {
    run()?;
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples.max(1) {
        let started = Instant::now();
        run()?;
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    Ok(timings[timings.len() / 2])
}

fn run(people: usize, only: Option<&str>, samples: Option<usize>) -> Result<()> {
    let queries = queries(only)?;
    let loaded = load(people)?;
    let (mut same, mut missing, mut wrong) = (0, 0, 0);
    if samples.is_some() {
        println!("{:<34} {:>10} {:>10} {:>8}", "query", "Owl ms", "SQLite ms", "ratio");
    }
    for query in &queries {
        let outcome = compare(&loaded, query)?;
        let timing = match (&outcome, samples) {
            (Outcome::Same(_), Some(samples)) => {
                let oql = query.oql.as_deref().unwrap_or_default();
                let owl = median_ms(samples, || loaded.owl.query(oql).map(|_| ()))?;
                let sqlite = median_ms(samples, || loaded.sqlite.query(&query.sql).map(|_| ()))?;
                format!("{owl:>10.3} {sqlite:>10.3} {:>7.1}x", owl / sqlite.max(1e-6))
            }
            _ => String::new(),
        };
        match outcome {
            Outcome::Same(rows) if samples.is_some() => {
                same += 1;
                println!("{:<34} {timing}  ({rows} row(s))", query.name);
            }
            Outcome::Same(rows) => {
                same += 1;
                println!("  same    {}  ({rows} row(s))", query.name);
            }
            Outcome::Missing => {
                missing += 1;
                println!("  NO OQL  {}", query.name);
            }
            Outcome::Failed(problem) => {
                wrong += 1;
                println!("  FAILED  {}\n          {problem}", query.name);
            }
            Outcome::Differ(problem) => {
                wrong += 1;
                println!("  DIFFER  {}\n          {problem}", query.name);
            }
        }
    }
    println!(
        "\n{same} of {} the same, {wrong} wrong or failing, {missing} OQL cannot express.",
        queries.len()
    );
    if wrong > 0 {
        return Err(anyhow!("{wrong} queries are wrong or failing"));
    }
    Ok(())
}

fn show(people: usize, rows: usize, wanted: &[String]) -> Result<()> {
    let known = queries(None)?;
    let loaded = load(people)?;
    let print = |engine: &str, answer: Result<Vec<Json>>| match answer {
        Ok(answer) => {
            println!("{engine}: {} row(s)", answer.len());
            for row in answer.iter().take(rows) {
                println!("  {row}");
            }
        }
        Err(error) => println!("{engine}: failed: {error}"),
    };
    for wanted in wanted {
        match known.iter().find(|query| query.name.starts_with(wanted.as_str())) {
            Some(query) => {
                println!("== {}", query.name);
                match &query.oql {
                    Some(oql) => print("Owl", loaded.owl.query(oql)),
                    None => println!("Owl: no OQL"),
                }
                print("SQLite", loaded.sqlite.query(&query.sql));
                if let Ok(Outcome::Differ(difference)) = compare(&loaded, query) {
                    println!("first difference: {difference}");
                }
            }
            None => {
                println!("== {wanted}");
                let started = Instant::now();
                let answer = loaded.owl.query(wanted);
                println!("{:.3} ms", started.elapsed().as_secs_f64() * 1000.0);
                print("Owl", answer);
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    match Arguments::parse().command {
        Command::Check { people, only } => run(people, only.as_deref(), None),
        Command::Bench { people, only, samples } => run(people, only.as_deref(), Some(samples)),
        Command::Show { people, rows, queries } => show(people, rows, &queries),
        Command::Columns { people, samples } => columns::run(&load(people)?.owl, samples),
        Command::Memory { people, only } => memory(people, only.as_deref()),
        Command::Scale { people, at, reopens, copies, compactions, scattered } => {
            SCATTERED.store(scattered, std::sync::atomic::Ordering::Relaxed);
            scale(people, at, reopens, copies.max(1), compactions)
        }
    }
}
