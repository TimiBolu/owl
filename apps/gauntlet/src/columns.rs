//! An experiment, not a feature: what would reading fields from columns
//! gain over reading them from rows?
//!
//! Two questions the gauntlet is slow at are asked several ways over the
//! same loaded data, each timed, each required to give the same answer:
//!
//! - through the engine, as a query runs today;
//! - over the stored rows, reading each field from the row's own map,
//!   which is the least the row layout can cost;
//! - over the store's column segments, through the interface it already has;
//! - over plain arrays, one per field, which is the most a column layout
//!   could give.

use anyhow::{anyhow, Result};
use owl_db::storage::{DocumentStore, ProjectedRowBatch, ProjectedValueRef};
use owl_db::Value;
use std::time::Instant;
use storefront::engines::Owl;

/// A quick hash for short names, standing in for any map that does not pay
/// for resistance to chosen keys.
#[derive(Default)]
struct QuickHasher(u64);

impl std::hash::Hasher for QuickHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*byte)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
}

type QuickMap = std::collections::HashMap<String, Value, std::hash::BuildHasherDefault<QuickHasher>>;

fn median_ms<T: PartialEq + std::fmt::Debug>(
    samples: usize,
    expect: &T,
    mut run: impl FnMut() -> Result<T>,
) -> Result<f64> {
    let first = run()?;
    if first != *expect {
        return Err(anyhow!("a way gave {first:?}, not {expect:?}"));
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples.max(1) {
        let started = Instant::now();
        std::hint::black_box(run()?);
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    Ok(timings[timings.len() / 2])
}

fn line(way: &str, ms: f64, rows: usize, base: f64) {
    println!(
        "  {way:<44} {ms:>8.3} ms {:>7.0} ns/row {:>6.1}x",
        ms * 1e6 / rows as f64,
        base / ms
    );
}

fn batches(owl: &Owl, collection: &str, fields: &[&str]) -> Result<Vec<ProjectedRowBatch>> {
    let fields: Vec<String> = fields.iter().map(|field| field.to_string()).collect();
    owl.store()
        .scan_projected_batches(collection, &[], &fields, 4096)
        .map_err(|error| anyhow!("{error}"))?
        .ok_or_else(|| anyhow!("the store gave no column batches for {collection}"))
}

/// `tasks.where { .assignee == null && .parent != null && .closed == null }.count()`
fn filter(owl: &Owl, samples: usize) -> Result<()> {
    let store = owl.store();
    let rows = store
        .scan_documents_shared("tasks")
        .map_err(|error| anyhow!("{error}"))?
        .ok_or_else(|| anyhow!("tasks are not all held as rows"))?;
    let total = rows.len();
    let null = |value: Option<&Value>| matches!(value, None | Some(Value::Null));
    let over_rows = |rows: &[std::sync::Arc<owl_db::Document>]| {
        rows.iter()
            .filter(|row| {
                null(row.fields.get("assignee"))
                    && !null(row.fields.get("parent"))
                    && null(row.fields.get("closed"))
            })
            .count()
    };
    let expect = over_rows(&rows);
    let over_batches = |batches: &[ProjectedRowBatch]| {
        let mut count = 0;
        for batch in batches {
            for row in batch.rows() {
                let (mut assignee, mut parent, mut closed) = (true, true, true);
                for (name, value) in row.fields() {
                    let is_null = matches!(value, ProjectedValueRef::Null | ProjectedValueRef::Value(Value::Null));
                    match name {
                        "assignee" => assignee = is_null,
                        "parent" => parent = is_null,
                        _ => closed = is_null,
                    }
                }
                count += usize::from(assignee && !parent && closed);
            }
        }
        count
    };
    let fields = ["assignee", "parent", "closed"];
    let held = batches(owl, "tasks", &fields)?;
    let arrays: [Vec<bool>; 3] = fields.map(|field| rows.iter().map(|row| null(row.fields.get(field))).collect());

    println!("A filter and a count over {total} tasks, three fields read ({expect} pass)");
    let engine = median_ms(samples, &expect, || {
        let answer = owl.query("tasks.where { .assignee == null && .parent != null && .closed == null }.count()")?;
        Ok(answer[0]["count"].as_u64().unwrap_or(0) as usize)
    })?;
    line("the engine today", engine, total, engine);
    owl_db::executor::set_column_aggregation(false);
    let without = median_ms(samples, &expect, || {
        let answer = owl.query("tasks.where { .assignee == null && .parent != null && .closed == null }.count()")?;
        Ok(answer[0]["count"].as_u64().unwrap_or(0) as usize)
    });
    owl_db::executor::set_column_aggregation(true);
    line("the engine with its column path turned off", without?, total, engine);
    let scan = median_ms(samples, &total, || {
        Ok(store.scan_documents_shared("tasks").map_err(|error| anyhow!("{error}"))?.map_or(0, |rows| rows.len()))
    })?;
    line("  of which: asking the store for the rows", scan, total, engine);
    line("rows, each field from the row's map", median_ms(samples, &expect, || Ok(over_rows(&rows)))?, total, engine);
    // The same rows copied afresh, so that what the next two lines gain
    // from sitting side by side in memory can be told from what they gain
    // from their layout.
    let fresh: Vec<Box<std::collections::HashMap<String, Value>>> =
        rows.iter()
            .map(|row| Box::new(row.fields.iter().map(|(name, value)| (name.clone(), value.clone())).collect()))
            .collect();
    line(
        "rows, the same map, copied afresh",
        median_ms(samples, &expect, || {
            Ok(fresh
                .iter()
                .filter(|row| null(row.get("assignee")) && !null(row.get("parent")) && null(row.get("closed")))
                .count())
        })?,
        total,
        engine,
    );
    // The same rows with a quicker hash for their field names.
    let quick: Vec<Box<QuickMap>> = rows
        .iter()
        .map(|row| Box::new(row.fields.iter().map(|(name, value)| (name.clone(), value.clone())).collect()))
        .collect();
    line(
        "rows, a quicker hash for field names",
        median_ms(samples, &expect, || {
            Ok(quick
                .iter()
                .filter(|row| null(row.get("assignee")) && !null(row.get("parent")) && null(row.get("closed")))
                .count())
        })?,
        total,
        engine,
    );
    // The same rows holding their fields in the schema's order, so a field
    // is found by its place and nothing is hashed.
    let mut names: Vec<&String> = rows.iter().flat_map(|row| row.fields.keys()).collect();
    names.sort();
    names.dedup();
    let place = |name: &str| names.iter().position(|known| *known == name).unwrap_or(0);
    let slots: Vec<Box<[Value]>> = rows
        .iter()
        .map(|row| names.iter().map(|name| row.fields.get(*name).cloned().unwrap_or(Value::Null)).collect())
        .collect();
    let (assignee, parent, closed) = (place("assignee"), place("parent"), place("closed"));
    let is_null = |value: &Value| matches!(value, Value::Null);
    line(
        "rows, fields held in schema order",
        median_ms(samples, &expect, || {
            Ok(slots
                .iter()
                .filter(|row| is_null(&row[assignee]) && !is_null(&row[parent]) && is_null(&row[closed]))
                .count())
        })?,
        total,
        engine,
    );
    line(
        "the store's columns, asked for and read",
        median_ms(samples, &expect, || Ok(over_batches(&batches(owl, "tasks", &fields)?)))?,
        total,
        engine,
    );
    line("the store's columns, read only", median_ms(samples, &expect, || Ok(over_batches(&held)))?, total, engine);
    line(
        "plain arrays, one per field",
        median_ms(samples, &expect, || {
            Ok((0..total).filter(|at| arrays[0][*at] && !arrays[1][*at] && arrays[2][*at]).count())
        })?,
        total,
        engine,
    );
    Ok(())
}

/// `events.sort { .at.asc, .id.asc }.limit(500).select { id }`
fn sort(owl: &Owl, samples: usize) -> Result<()> {
    const KEEP: usize = 500;
    let store = owl.store();
    let rows = store
        .scan_documents_shared("events")
        .map_err(|error| anyhow!("{error}"))?
        .ok_or_else(|| anyhow!("events are not all held as rows"))?;
    let total = rows.len();
    fn first<'a, Key: Ord + Copy>(keys: &[(Key, &'a str)]) -> Vec<&'a str> {
        let mut order: Vec<u32> = (0..keys.len() as u32).collect();
        let by = |left: &u32, right: &u32| keys[*left as usize].cmp(&keys[*right as usize]);
        if keys.len() > KEEP {
            order.select_nth_unstable_by(KEEP - 1, by);
            order.truncate(KEEP);
        }
        order.sort_unstable_by(by);
        order.into_iter().map(|at| keys[at as usize].1).collect()
    }
    let over_rows = |rows: &[std::sync::Arc<owl_db::Document>]| -> Result<Vec<String>> {
        let mut keys = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(Value::DateTime(at)) = row.fields.get("at") else {
                return Err(anyhow!("an event has no time"));
            };
            keys.push((*at, row.id.as_str()));
        }
        Ok(first(&keys).into_iter().map(str::to_string).collect())
    };
    let expect = over_rows(&rows)?;
    let over_batches = |batches: &[ProjectedRowBatch]| -> Result<Vec<String>> {
        let mut keys = Vec::with_capacity(total);
        for batch in batches {
            for row in batch.rows() {
                let at = match row.fields().next() {
                    Some((_, ProjectedValueRef::DateTime(at))) => at,
                    Some((_, ProjectedValueRef::Value(Value::DateTime(at)))) => *at,
                    other => return Err(anyhow!("an event has no time: {other:?}")),
                };
                keys.push((at, row.id()));
            }
        }
        Ok(first(&keys).into_iter().map(str::to_string).collect())
    };
    let held = batches(owl, "events", &["at"])?;
    let times: Vec<i64> = rows
        .iter()
        .map(|row| match row.fields.get("at") {
            Some(Value::DateTime(at)) => at.timestamp_micros(),
            _ => 0,
        })
        .collect();
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();

    println!("\nThe first {KEEP} of {total} events in time order, one field and the id read");
    let engine = median_ms(samples, &expect, || {
        let answer = owl.query("events.sort { .at.asc, .id.asc }.limit(500).select { id }")?;
        Ok(answer.iter().map(|row| row["id"].as_str().unwrap_or("").to_string()).collect())
    })?;
    line("the engine today", engine, total, engine);
    line("rows, each field from the row's map", median_ms(samples, &expect, || over_rows(&rows))?, total, engine);
    line(
        "the store's columns, asked for and read",
        median_ms(samples, &expect, || over_batches(&batches(owl, "events", &["at"])?))?,
        total,
        engine,
    );
    line("the store's columns, read only", median_ms(samples, &expect, || over_batches(&held))?, total, engine);
    line(
        "plain arrays, one per field",
        median_ms(samples, &expect, || {
            let keys: Vec<(i64, &str)> = times.iter().copied().zip(ids.iter().copied()).collect();
            Ok(first(&keys).into_iter().map(str::to_string).collect())
        })?,
        total,
        engine,
    );
    Ok(())
}

/// The same count with a write before each query: to another collection,
/// which leaves what is kept of this one alone, and to this collection,
/// after which its columns are built again.
fn mixed(owl: &Owl, samples: usize) -> Result<()> {
    let question = "tasks.where { .assignee == null && .parent != null && .closed == null }.count()";
    let ask = || -> Result<usize> { Ok(owl.query(question)?[0]["count"].as_u64().unwrap_or(0) as usize) };
    let expect = ask()?;
    let total = owl.query("tasks.count()")?[0]["count"].as_u64().unwrap_or(1) as usize;
    let event = owl.query("events.limit(1).select { id }")?[0]["id"].as_str().unwrap_or("").to_string();
    let turn = std::cell::Cell::new(0u64);
    let write = |collection: &str, id: &str, field: &str| -> Result<()> {
        turn.set(turn.get() + 1);
        owl.query(&format!(
            "{collection}.where {{ .id == \"{id}\" }}.update {{ {field}: {} }}",
            turn.get() % 7
        ))?;
        Ok(())
    };
    println!("\nThe same count over {total} tasks, with a write before each query");
    let alone = median_ms(samples, &expect, ask)?;
    line("no write, the query alone", alone, total, alone);
    let other_write = median_ms(samples, &(), || write("events", &event, "amount"))?;
    line("a write to another collection, alone", other_write, total, alone);
    line(
        "a write to another collection, then the query",
        median_ms(samples, &expect, || {
            write("events", &event, "amount")?;
            ask()
        })?,
        total,
        alone,
    );
    let own_write = median_ms(samples, &(), || write("tasks", "t0002717", "points"))?;
    line("a write to this collection, alone", own_write, total, alone);
    line(
        "a write to this collection, then the query",
        median_ms(samples, &expect, || {
            write("tasks", "t0002717", "points")?;
            ask()
        })?,
        total,
        alone,
    );
    owl_db::executor::set_column_aggregation(false);
    let by_rows = median_ms(samples, &expect, || {
        write("tasks", "t0002717", "points")?;
        ask()
    });
    owl_db::executor::set_column_aggregation(true);
    line("the same, with columns turned off", by_rows?, total, alone);
    // Writes wait for the next sync of the log, which comes at intervals,
    // so a write and a query timed together say little about the query.
    // Here only the query is timed: the first after a write, and the next.
    let mut first = Vec::new();
    let mut second = Vec::new();
    for _ in 0..samples.max(1) {
        write("tasks", "t0002717", "points")?;
        for timings in [&mut first, &mut second] {
            let started = Instant::now();
            std::hint::black_box(ask()?);
            timings.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    for (way, timings) in [
        ("the first query after a write to this collection", &mut first),
        ("the second query after it", &mut second),
    ] {
        timings.sort_by(f64::total_cmp);
        line(way, timings[timings.len() / 2], total, alone);
    }
    Ok(())
}

pub fn run(owl: &Owl, samples: usize) -> Result<()> {
    for collection in ["tasks", "events"] {
        let (live, resident, lazy) = owl.residency(collection)?;
        println!("{collection}: {live} rows, {resident} held as rows, {lazy} only in column segments");
    }
    println!("\nEach line: the time, the time per row, and how many times faster than the engine today.\n");
    filter(owl, samples)?;
    sort(owl, samples)?;
    mixed(owl, samples)
}
