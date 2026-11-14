//! Storefront: a commerce workload for Owl, checked and timed against SQLite.
//!
//! `check` loads the same data into both engines and requires every query
//! to return the same rows. `bench` times loading and each query.

mod data;
mod engines;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use engines::{first_difference, Owl, Sqlite};
use serde_json::{json, Value as Json};
use std::path::Path;
use std::time::{Duration, Instant};

macro_rules! queries {
    ($($name:literal),* $(,)?) => {
        [$((
            $name,
            include_str!(concat!("../queries/", $name, ".oql")),
            include_str!(concat!("../queries/", $name, ".sql")),
        )),*]
    };
}

/// Every query, as (name, OQL, SQL).
const QUERIES: [(&str, &str, &str); 16] = queries![
    "q01_order_detail",
    "q02_recent_paid",
    "q03_large_orders_by_country",
    "q04_top_spenders",
    "q05_latest_orders_per_customer",
    "q06_status_summary",
    "q07_monthly_revenue",
    "q08_revenue_by_country",
    "q09_units_by_product",
    "q10_paid_without_tickets",
    "q11_above_own_average",
    "q12_category_tree",
    "q13_customers_by_country",
    "q14_buying_countries",
    "q15_payments_by_method",
    "q16_best_reviewed",
];

#[derive(Parser)]
#[command(about = "A commerce workload for Owl, checked and timed against SQLite")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load both engines with the same data and require identical answers.
    Check {
        /// Number of orders to generate; the other collections scale with it.
        #[arg(long, default_value_t = 2000)]
        orders: usize,
    },
    /// Time ad-hoc OQL statements against a loaded dataset.
    Time {
        #[arg(long, default_value_t = 10000)]
        orders: usize,
        /// Statements to time, each run once and then five times.
        queries: Vec<String>,
    },
    /// Time loading and every query on both engines.
    Bench {
        /// Order counts to run, smallest first.
        #[arg(long, value_delimiter = ',', default_value = "10000")]
        orders: Vec<usize>,
        /// Timed runs per query, after one untimed first run.
        #[arg(long, default_value_t = 5)]
        samples: usize,
        /// Only run queries whose name contains this text.
        #[arg(long)]
        only: Option<String>,
        /// Give up on a query's remaining samples once one run exceeds this.
        #[arg(long, default_value_t = 30.0)]
        max_seconds: f64,
        /// Write the full report here as JSON.
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
}

fn main() -> Result<()> {
    match Arguments::parse().command {
        Command::Check { orders } => check(orders),
        Command::Time { orders, queries } => {
            let loaded = load(orders)?;
            for query in queries {
                match time(5, 30.0, || Ok(loaded.owl.query(&query)?.len())) {
                    Ok((first, median, rows)) => println!(
                        "{:>10.3} ms first {:>10.3} ms median {:>7} rows  [{}]  {}",
                        first,
                        median.unwrap_or(first),
                        rows,
                        loaded.owl.explain(&query).unwrap_or_default(),
                        query
                    ),
                    Err(error) => println!("failed: {error}  {query}"),
                }
            }
            Ok(())
        }
        Command::Bench {
            orders,
            samples,
            only,
            max_seconds,
            output,
        } => bench(&orders, samples, only.as_deref(), max_seconds, output.as_deref()),
    }
}

struct Loaded {
    _directory: tempfile::TempDir,
    owl: Owl,
    sqlite: Sqlite,
    rows: usize,
    owl_load: Duration,
    sqlite_load: Duration,
    owl_bytes: u64,
    sqlite_bytes: u64,
}

fn load(orders: usize) -> Result<Loaded> {
    let data = data::generate(orders);
    let directory = tempfile::TempDir::new()?;
    let owl_directory = directory.path().join("owl");
    let sqlite_directory = directory.path().join("sqlite");
    std::fs::create_dir_all(&owl_directory)?;
    std::fs::create_dir_all(&sqlite_directory)?;

    let owl = Owl::open(&owl_directory.join("storefront.owl"))?;
    let started = Instant::now();
    owl.load(&data).context("loading Owl")?;
    // STOREFRONT_SKIP_CHECKPOINT leaves Owl's data in its in-memory
    // overlay, to separate the cost of reading checkpointed records.
    if std::env::var_os("STOREFRONT_SKIP_CHECKPOINT").is_none() {
        owl.checkpoint()?;
    }
    let owl_load = started.elapsed();

    let mut sqlite = Sqlite::open(&sqlite_directory.join("storefront.sqlite"))?;
    let started = Instant::now();
    sqlite.load(&data).context("loading SQLite")?;
    sqlite.checkpoint()?;
    let sqlite_load = started.elapsed();

    Ok(Loaded {
        owl_bytes: directory_bytes(&owl_directory)?,
        sqlite_bytes: directory_bytes(&sqlite_directory)?,
        rows: data.rows(),
        _directory: directory,
        owl,
        sqlite,
        owl_load,
        sqlite_load,
    })
}

fn directory_bytes(path: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() {
            directory_bytes(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}

fn check(orders: usize) -> Result<()> {
    let loaded = load(orders)?;
    println!("{} rows loaded into each engine\n", loaded.rows);
    let mut failures = 0;
    for (name, oql, sql) in QUERIES {
        let outcome = loaded
            .owl
            .query(oql)
            .map_err(|error| format!("Owl failed: {error}"))
            .and_then(|owl| {
                let sqlite = loaded
                    .sqlite
                    .query(sql)
                    .map_err(|error| format!("SQLite failed: {error}"))?;
                match first_difference(&owl, &sqlite) {
                    None => Ok(owl.len()),
                    Some(difference) => Err(difference),
                }
            });
        match outcome {
            Ok(rows) => println!("  same   {name}  ({rows} row(s))"),
            Err(problem) => {
                failures += 1;
                println!("  DIFFER {name}\n         {problem}");
            }
        }
    }
    if failures > 0 {
        return Err(anyhow!("{failures} of {} queries differ", QUERIES.len()));
    }
    println!("\nAll {} queries return the same rows on both engines.", QUERIES.len());
    Ok(())
}

/// The first run and the median of the timed runs, in milliseconds. `None`
/// for the median when a run was too slow to repeat.
fn time(
    samples: usize,
    max_seconds: f64,
    mut run: impl FnMut() -> Result<usize>,
) -> Result<(f64, Option<f64>, usize)> {
    let started = Instant::now();
    let rows = run()?;
    let first = started.elapsed().as_secs_f64() * 1000.0;
    if first > max_seconds * 1000.0 {
        return Ok((first, None, rows));
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        run()?;
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    Ok((first, timings.get(timings.len() / 2).copied(), rows))
}

fn bench(
    scales: &[usize],
    samples: usize,
    only: Option<&str>,
    max_seconds: f64,
    output: Option<&Path>,
) -> Result<()> {
    let mut report = Vec::new();
    for &orders in scales {
        let loaded = load(orders)?;
        let rate = |duration: Duration| loaded.rows as f64 / duration.as_secs_f64();
        println!("\n== {orders} orders, {} rows in total ==", loaded.rows);
        println!(
            "load     Owl {:>8.2}s ({:>8.0} rows/s, {:>6.1} MiB)   SQLite {:>8.2}s ({:>8.0} rows/s, {:>6.1} MiB)",
            loaded.owl_load.as_secs_f64(),
            rate(loaded.owl_load),
            loaded.owl_bytes as f64 / 1_048_576.0,
            loaded.sqlite_load.as_secs_f64(),
            rate(loaded.sqlite_load),
            loaded.sqlite_bytes as f64 / 1_048_576.0,
        );
        println!(
            "\n{:<32} {:>11} {:>11} {:>8}  {}",
            "query", "Owl ms", "SQLite ms", "ratio", "Owl path"
        );
        let mut results = Vec::new();
        for (name, oql, sql) in QUERIES {
            if only.is_some_and(|text| !name.contains(text)) {
                continue;
            }
            let path = loaded.owl.explain(oql).unwrap_or_else(|error| error.to_string());
            let owl = time(samples, max_seconds, || Ok(loaded.owl.query(oql)?.len()));
            let sqlite = time(samples, max_seconds, || Ok(loaded.sqlite.query(sql)?.len()))?;
            let sqlite_ms = sqlite.1.unwrap_or(sqlite.0);
            match owl {
                Ok((first, median, rows)) => {
                    let owl_ms = median.unwrap_or(first);
                    println!(
                        "{:<32} {:>11.3} {:>11.3} {:>7.1}x  {}{}",
                        name,
                        owl_ms,
                        sqlite_ms,
                        owl_ms / sqlite_ms.max(0.000_001),
                        path,
                        if median.is_none() { "  (single run)" } else { "" }
                    );
                    results.push(json!({
                        "query": name, "rows": rows, "owl_path": path,
                        "owl_first_ms": first, "owl_median_ms": median,
                        "sqlite_first_ms": sqlite.0, "sqlite_median_ms": sqlite.1,
                    }));
                }
                Err(error) => {
                    println!("{name:<32} failed: {error}");
                    results.push(json!({ "query": name, "error": error.to_string() }));
                }
            }
        }
        report.push(json!({
            "orders": orders,
            "rows": loaded.rows,
            "load": {
                "owl_seconds": loaded.owl_load.as_secs_f64(),
                "sqlite_seconds": loaded.sqlite_load.as_secs_f64(),
                "owl_bytes": loaded.owl_bytes,
                "sqlite_bytes": loaded.sqlite_bytes,
            },
            "queries": results,
        }));
    }
    if let Some(path) = output {
        std::fs::write(path, serde_json::to_string_pretty(&Json::Array(report))?)?;
        println!("\nreport written to {}", path.display());
    }
    Ok(())
}
