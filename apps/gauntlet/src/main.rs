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
    }
}
