use anyhow::{anyhow, Context, Result};
use clap::Parser;
use owl_db::storage::{AutoCheckpointPolicy, ProjectedValueRef, RustStore};
use owl_db::{parse_oql, Document, DocumentStore, FieldType, QueryExecutor, Schema, Value};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::Builder;

const INGEST_BATCH_ROWS: usize = 4_096;
const CONSERVATIVE_PHYSICAL_OVERHEAD_PER_ROW: u64 = 768;

#[derive(Debug, Parser)]
#[command(about = "Run bounded Owl storage scaling experiments")]
struct Args {
    /// Comma-separated row counts. Cases run from smallest to largest.
    #[arg(long, value_delimiter = ',', default_value = "10000,100000,1000000")]
    rows: Vec<usize>,

    /// Approximate user-value bytes per generated record.
    #[arg(long, value_delimiter = ',', default_value = "64,256,1024,4096")]
    record_bytes: Vec<usize>,

    /// Maximum estimated and observed physical bytes for one temporary case.
    #[arg(long, default_value_t = 2.0)]
    max_case_gib: f64,

    /// Warm scan samples retained after one unreported warm-up.
    #[arg(long, default_value_t = 7)]
    samples: usize,

    /// Update this many existing rows after reopen to measure typed-base plus
    /// row-delta scans. The row count and selected values remain unchanged.
    #[arg(long, default_value_t = 0)]
    delta_updates: usize,

    /// Emit the complete machine-readable report at this path as JSON.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct Report {
    generated_at: chrono::DateTime<chrono::Utc>,
    max_case_bytes: u64,
    samples: usize,
    cases: Vec<CaseReport>,
}

#[derive(Debug, Serialize)]
struct CaseReport {
    rows: usize,
    target_record_value_bytes: usize,
    delta_updates: usize,
    estimated_physical_bytes: u64,
    logical_value_bytes: u64,
    logical_identity_bytes: u64,
    ingest_ms: f64,
    ingest_rows_per_second: f64,
    pre_checkpoint_physical_bytes: u64,
    checkpoint_ms: f64,
    checkpoint_physical_bytes: u64,
    peak_observed_physical_bytes: u64,
    physical_to_logical_ratio: f64,
    storage_files: Vec<StorageFile>,
    reopen_ms: f64,
    reopened_physical_bytes: u64,
    typed_base_lsn: u64,
    committed_lsn: u64,
    flush_generations: usize,
    flush_generation_wal_bytes: u64,
    flush_generation_record_bytes: u64,
    flush_generation_index_bytes: u64,
    typed_segments: usize,
    typed_loaded_segments: usize,
    typed_identity_bytes: usize,
    typed_rows: usize,
    typed_authoritative_segments: usize,
    typed_authoritative_rows: usize,
    row_overflow_rows: usize,
    external_id_hash_rows: usize,
    ordered_id_tree_rows: usize,
    objref_locator_rows: usize,
    objref_locator_cached_pages: usize,
    objref_locator_cached_bytes: usize,
    resident_documents: usize,
    lazy_typed_rows: usize,
    typed_delta_rows: usize,
    typed_stale_rows: usize,
    typed_scan_available: bool,
    typed_scan_current: bool,
    first_scan: ScanReport,
    warm_scan: ScanReport,
    warm_value_scan: ScanReport,
    cleanup_verified: bool,
}

#[derive(Debug, Serialize)]
struct StorageFile {
    path: String,
    bytes: u64,
}

#[derive(Debug, Serialize)]
struct ScanReport {
    samples: usize,
    rows: usize,
    selected_value_bytes: u64,
    min_ms: f64,
    median_ms: f64,
    max_ms: f64,
    median_rows_per_second: f64,
    median_selected_megabytes_per_second: f64,
}

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_target(false)
            .try_init();
    }
    let mut args = Args::parse();
    if args.samples == 0 {
        return Err(anyhow!("--samples must be greater than zero"));
    }
    args.rows.sort_unstable();
    args.rows.dedup();
    args.record_bytes.sort_unstable();
    args.record_bytes.dedup();
    let max_case_bytes = gib_to_bytes(args.max_case_gib)?;
    let mut report = Report {
        generated_at: chrono::Utc::now(),
        max_case_bytes,
        samples: args.samples,
        cases: Vec::new(),
    };

    for &record_bytes in &args.record_bytes {
        for &rows in &args.rows {
            let estimated = estimated_physical_bytes(rows, record_bytes);
            if estimated > max_case_bytes {
                println!(
                    "SKIP rows={rows} record_value_bytes={record_bytes} estimated_physical={} budget={}",
                    format_bytes(estimated),
                    format_bytes(max_case_bytes)
                );
                continue;
            }
            let case = run_case(
                rows,
                record_bytes,
                args.samples,
                max_case_bytes,
                args.delta_updates,
            )?;
            print_case(&case);
            report.cases.push(case);
        }
    }

    if let Some(path) = args.output {
        let bytes = serde_json::to_vec_pretty(&report)?;
        fs::write(&path, bytes)
            .with_context(|| format!("write scale report {}", path.display()))?;
        println!("report={}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

fn run_case(
    rows: usize,
    target_record_bytes: usize,
    samples: usize,
    max_case_bytes: u64,
    delta_updates: usize,
) -> Result<CaseReport> {
    let directory = Builder::new().prefix("owl-scale-").tempdir()?;
    let directory_path = directory.path().to_path_buf();
    let database_path = directory_path.join("scale.owl");
    let storage_limit = max_case_bytes.saturating_mul(4).max(256 * 1024 * 1024);
    println!(
        "BUILD rows={rows} record_value_bytes={target_record_bytes} temp={}",
        directory_path.display()
    );

    let store = Arc::new(RustStore::new(&database_path, storage_limit as usize)?);
    store.configure_auto_checkpoint(AutoCheckpointPolicy::disabled())?;
    store.put_schema(&Schema::new(
        "events".into(),
        HashMap::from([
            ("sequence".into(), FieldType::Integer),
            ("message".into(), FieldType::String),
            ("padding".into(), FieldType::String),
        ]),
    ))?;

    let ingest_started = Instant::now();
    let estimated_physical_bytes = estimated_physical_bytes(rows, target_record_bytes);
    let mut peak_observed_physical_bytes = directory_bytes(&directory_path)?;
    let mut logical_value_bytes = 0u64;
    let mut logical_identity_bytes = 0u64;
    let mut batch = Vec::with_capacity(INGEST_BATCH_ROWS.min(rows));
    for sequence in 0..rows {
        let id = format!("event-{sequence:020}");
        let message = format!("event payload {sequence}");
        let fixed_value_bytes = 8usize.saturating_add(message.len());
        let padding = "x".repeat(target_record_bytes.saturating_sub(fixed_value_bytes));
        logical_value_bytes = logical_value_bytes
            .saturating_add(8)
            .saturating_add(message.len() as u64)
            .saturating_add(padding.len() as u64);
        logical_identity_bytes = logical_identity_bytes
            .saturating_add(id.len() as u64)
            .saturating_add("events".len() as u64);
        batch.push(Document::with_id(
            id,
            "events".into(),
            HashMap::from([
                ("sequence".into(), Value::Integer(sequence as i64)),
                ("message".into(), Value::String(message)),
                ("padding".into(), Value::String(padding)),
            ]),
        ));
        if batch.len() == INGEST_BATCH_ROWS {
            store.put_documents(&batch)?;
            batch.clear();
            observe_physical_size(
                &directory_path,
                max_case_bytes,
                &mut peak_observed_physical_bytes,
            )?;
        }
    }
    if !batch.is_empty() {
        store.put_documents(&batch)?;
        observe_physical_size(
            &directory_path,
            max_case_bytes,
            &mut peak_observed_physical_bytes,
        )?;
    }
    let ingest_elapsed = ingest_started.elapsed();
    store.wait_for_maintenance()?;
    let pre_checkpoint_physical_bytes = directory_bytes(&directory_path)?;
    peak_observed_physical_bytes = peak_observed_physical_bytes.max(pre_checkpoint_physical_bytes);

    let checkpoint_started = Instant::now();
    store.checkpoint()?;
    store.wait_for_maintenance()?;
    let checkpoint_elapsed = checkpoint_started.elapsed();
    let storage_files = storage_files(&directory_path)?;
    let checkpoint_physical_bytes = storage_files.iter().map(|file| file.bytes).sum();
    peak_observed_physical_bytes = peak_observed_physical_bytes.max(checkpoint_physical_bytes);

    drop(store);
    let reopen_started = Instant::now();
    let reopened = Arc::new(RustStore::new(&database_path, storage_limit as usize)?);
    let reopen_elapsed = reopen_started.elapsed();
    let reopened_physical_bytes = directory_bytes(&directory_path)?;
    peak_observed_physical_bytes = peak_observed_physical_bytes.max(reopened_physical_bytes);
    let delta_updates = delta_updates.min(rows);
    for sequence in 0..delta_updates {
        let id = format!("event-{sequence:020}");
        let mut document = reopened
            .get_document("events", &id)?
            .ok_or_else(|| anyhow!("delta update target is missing: {id}"))?;
        document.set_field("sequence".into(), Value::Integer(sequence as i64));
        reopened.put_document(&document)?;
    }
    let layout = reopened.storage_layout_stats("events")?;
    let query = parse_oql(r#"events.where { .sequence >= 0 }.select { message }"#)
        .map_err(|error| anyhow!(error.to_string()))?;
    let executor = QueryExecutor::new_with_real_clock(reopened.clone());
    let selected_value_bytes = selected_message_bytes(rows);
    let first_scan = measure_scans(&executor, &query, rows, selected_value_bytes, 1, false)?;
    let warm_scan = measure_scans(&executor, &query, rows, selected_value_bytes, samples, true)?;
    let warm_value_scan =
        measure_value_scans(&executor, &query, rows, selected_value_bytes, samples)?;
    drop(executor);
    drop(reopened);

    let logical_total = logical_value_bytes.saturating_add(logical_identity_bytes);
    let mut case = CaseReport {
        rows,
        target_record_value_bytes: target_record_bytes,
        delta_updates,
        estimated_physical_bytes,
        logical_value_bytes,
        logical_identity_bytes,
        ingest_ms: milliseconds(ingest_elapsed),
        ingest_rows_per_second: rate(rows, ingest_elapsed),
        pre_checkpoint_physical_bytes,
        checkpoint_ms: milliseconds(checkpoint_elapsed),
        checkpoint_physical_bytes,
        peak_observed_physical_bytes,
        physical_to_logical_ratio: checkpoint_physical_bytes as f64 / logical_total.max(1) as f64,
        storage_files,
        reopen_ms: milliseconds(reopen_elapsed),
        reopened_physical_bytes,
        typed_base_lsn: layout.typed_base_lsn,
        committed_lsn: layout.committed_lsn,
        flush_generations: layout.flush_generations,
        flush_generation_wal_bytes: layout.flush_generation_wal_bytes,
        flush_generation_record_bytes: layout.flush_generation_record_bytes,
        flush_generation_index_bytes: layout.flush_generation_index_bytes,
        typed_segments: layout.typed_segments,
        typed_loaded_segments: layout.typed_loaded_segments,
        typed_identity_bytes: layout.typed_identity_bytes,
        typed_rows: layout.typed_rows,
        typed_authoritative_segments: layout.typed_authoritative_segments,
        typed_authoritative_rows: layout.typed_authoritative_rows,
        row_overflow_rows: layout.row_overflow_rows,
        external_id_hash_rows: layout.external_id_hash_rows,
        ordered_id_tree_rows: layout.ordered_id_tree_rows,
        objref_locator_rows: layout.objref_locator_rows,
        objref_locator_cached_pages: layout.objref_locator_cached_pages,
        objref_locator_cached_bytes: layout.objref_locator_cached_bytes,
        resident_documents: layout.resident_documents,
        lazy_typed_rows: layout.lazy_typed_rows,
        typed_delta_rows: layout.typed_delta_rows,
        typed_stale_rows: layout.typed_stale_rows,
        typed_scan_available: layout.typed_scan_available,
        typed_scan_current: layout.typed_scan_current,
        first_scan,
        warm_scan,
        warm_value_scan,
        cleanup_verified: false,
    };

    directory.close().with_context(|| {
        format!(
            "remove temporary scale dataset {}",
            directory_path.display()
        )
    })?;
    case.cleanup_verified = !directory_path.exists();
    if !case.cleanup_verified {
        return Err(anyhow!(
            "temporary scale dataset still exists after cleanup: {}",
            directory_path.display()
        ));
    }
    Ok(case)
}

fn measure_scans(
    executor: &QueryExecutor<RustStore>,
    query: &owl_db::OQLQuery,
    expected_rows: usize,
    selected_value_bytes: u64,
    samples: usize,
    warm_up: bool,
) -> Result<ScanReport> {
    if warm_up {
        let result = executor.execute_query_batches(query)?;
        if result.len() != expected_rows {
            return Err(anyhow!(
                "warm-up returned {} rows, expected {expected_rows}",
                result.len()
            ));
        }
    }
    let mut elapsed = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let result = executor.execute_query_batches(query)?;
        let duration = started.elapsed();
        if result.len() != expected_rows {
            return Err(anyhow!(
                "scan returned {} rows, expected {expected_rows}",
                result.len()
            ));
        }
        elapsed.push(duration);
    }
    elapsed.sort_unstable();
    let median = elapsed[elapsed.len() / 2];
    Ok(ScanReport {
        samples,
        rows: expected_rows,
        selected_value_bytes,
        min_ms: milliseconds(elapsed[0]),
        median_ms: milliseconds(median),
        max_ms: milliseconds(elapsed[elapsed.len() - 1]),
        median_rows_per_second: rate(expected_rows, median),
        median_selected_megabytes_per_second: selected_value_bytes as f64
            / median.as_secs_f64()
            / 1_000_000.0,
    })
}

fn measure_value_scans(
    executor: &QueryExecutor<RustStore>,
    query: &owl_db::OQLQuery,
    expected_rows: usize,
    selected_value_bytes: u64,
    samples: usize,
) -> Result<ScanReport> {
    let warm = executor.execute_query_batches(query)?;
    let (warm_bytes, warm_checksum) = touch_result(&warm);
    if warm.len() != expected_rows || warm_bytes != selected_value_bytes {
        return Err(anyhow!(
            "value warm-up returned rows={} bytes={}, expected rows={expected_rows} bytes={selected_value_bytes}",
            warm.len(),
            warm_bytes
        ));
    }
    std::hint::black_box(warm_checksum);

    let mut elapsed = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let result = executor.execute_query_batches(query)?;
        let (bytes, checksum) = touch_result(&result);
        let duration = started.elapsed();
        if result.len() != expected_rows || bytes != selected_value_bytes {
            return Err(anyhow!(
                "value scan returned rows={} bytes={}, expected rows={expected_rows} bytes={selected_value_bytes}",
                result.len(),
                bytes
            ));
        }
        std::hint::black_box(checksum);
        elapsed.push(duration);
    }
    elapsed.sort_unstable();
    let median = elapsed[elapsed.len() / 2];
    Ok(ScanReport {
        samples,
        rows: expected_rows,
        selected_value_bytes,
        min_ms: milliseconds(elapsed[0]),
        median_ms: milliseconds(median),
        max_ms: milliseconds(elapsed[elapsed.len() - 1]),
        median_rows_per_second: rate(expected_rows, median),
        median_selected_megabytes_per_second: selected_value_bytes as f64
            / median.as_secs_f64()
            / 1_000_000.0,
    })
}

fn touch_result(result: &owl_db::BatchQueryExecution) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut checksum = 0u64;
    for chunk in &result.chunks {
        if let Some(rows) = chunk.projected_rows() {
            for row in rows {
                for (_, value) in row.fields() {
                    let (value_bytes, value_checksum) = touch_projected_value(value);
                    bytes = bytes.saturating_add(value_bytes);
                    checksum = checksum.wrapping_add(value_checksum);
                }
            }
        } else if let Some(documents) = chunk.documents() {
            for document in documents {
                for value in document.fields.values() {
                    let (value_bytes, value_checksum) = touch_value(value);
                    bytes = bytes.saturating_add(value_bytes);
                    checksum = checksum.wrapping_add(value_checksum);
                }
            }
        }
    }
    (bytes, checksum)
}

fn touch_projected_value(value: ProjectedValueRef<'_>) -> (u64, u64) {
    match value {
        ProjectedValueRef::Value(value) => touch_value(value),
        ProjectedValueRef::Null => (0, 0),
        ProjectedValueRef::String(value) => touch_bytes(value.as_bytes()),
        ProjectedValueRef::Integer(value) => (8, value as u64),
        ProjectedValueRef::Float(value) => (8, value.to_bits()),
        ProjectedValueRef::Boolean(value) => (1, u64::from(value)),
        ProjectedValueRef::DateTime(value) => (12, value.timestamp() as u64),
        ProjectedValueRef::Reference(value) => (8, value.packed()),
    }
}

fn touch_value(value: &Value) -> (u64, u64) {
    match value {
        Value::String(value) => touch_bytes(value.as_bytes()),
        Value::Integer(value) => (8, *value as u64),
        Value::Float(value) => (8, value.to_bits()),
        Value::Boolean(value) => (1, u64::from(*value)),
        Value::DateTime(value) => (12, value.timestamp() as u64),
        Value::Reference(value) => (8, value.packed()),
        Value::GlobalReference(value) => touch_bytes(value.to_string_id().as_bytes()),
        Value::List(values) => values.iter().fold((0u64, 0u64), |total, value| {
            let next = touch_value(value);
            (total.0.saturating_add(next.0), total.1.wrapping_add(next.1))
        }),
        Value::Object(values) => values.values().fold((0u64, 0u64), |total, value| {
            let next = touch_value(value);
            (total.0.saturating_add(next.0), total.1.wrapping_add(next.1))
        }),
        Value::Null => (0, 0),
    }
}

fn touch_bytes(value: &[u8]) -> (u64, u64) {
    (
        value.len() as u64,
        value.iter().fold(0u64, |checksum, byte| {
            checksum.wrapping_mul(16_777_619).wrapping_add(*byte as u64)
        }),
    )
}

fn storage_files(root: &Path) -> Result<Vec<StorageFile>> {
    let mut files = Vec::new();
    collect_storage_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_storage_files(root: &Path, path: &Path, files: &mut Vec<StorageFile>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_storage_files(root, &entry_path, files)?;
        } else if file_type.is_file() {
            files.push(StorageFile {
                path: entry_path
                    .strip_prefix(root)
                    .unwrap_or(&entry_path)
                    .display()
                    .to_string(),
                bytes: entry.metadata()?.len(),
            });
        }
    }
    Ok(())
}

fn directory_bytes(path: &Path) -> Result<u64> {
    Ok(storage_files(path)?
        .into_iter()
        .map(|file| file.bytes)
        .sum())
}

fn selected_message_bytes(rows: usize) -> u64 {
    (0..rows)
        .map(|sequence| format!("event payload {sequence}").len() as u64)
        .sum()
}

fn print_case(case: &CaseReport) {
    println!(
        "RESULT rows={} record_value_bytes={} delta_updates={} logical={} physical={} peak_observed={} ratio={:.2}x ingest={:.2}ms reopen={:.2}ms flush_generations={} generation_wal_bytes={} generation_record_bytes={} generation_index_bytes={} typed_segments={} typed_loaded_segments={} typed_identity_bytes={} typed_rows={} authoritative_rows={} overflow_rows={} external_id_hash_rows={} ordered_id_tree_rows={} objref_locator_rows={} resident_documents={} lazy_typed_rows={} typed_delta_rows={} typed_stale_rows={} typed_available={} typed_current={} first_scan={:.3}ms warm_batch={:.3}ms warm_values={:.3}ms rows_per_second={:.2}M cleanup={}",
        case.rows,
        case.target_record_value_bytes,
        case.delta_updates,
        format_bytes(case.logical_value_bytes + case.logical_identity_bytes),
        format_bytes(case.checkpoint_physical_bytes),
        format_bytes(case.peak_observed_physical_bytes),
        case.physical_to_logical_ratio,
        case.ingest_ms,
        case.reopen_ms,
        case.flush_generations,
        case.flush_generation_wal_bytes,
        case.flush_generation_record_bytes,
        case.flush_generation_index_bytes,
        case.typed_segments,
        case.typed_loaded_segments,
        case.typed_identity_bytes,
        case.typed_rows,
        case.typed_authoritative_rows,
        case.row_overflow_rows,
        case.external_id_hash_rows,
        case.ordered_id_tree_rows,
        case.objref_locator_rows,
        case.resident_documents,
        case.lazy_typed_rows,
        case.typed_delta_rows,
        case.typed_stale_rows,
        case.typed_scan_available,
        case.typed_scan_current,
        case.first_scan.median_ms,
        case.warm_scan.median_ms,
        case.warm_value_scan.median_ms,
        case.warm_value_scan.median_rows_per_second / 1_000_000.0,
        case.cleanup_verified
    );
}

fn observe_physical_size(root: &Path, budget: u64, peak: &mut u64) -> Result<()> {
    let current = directory_bytes(root)?;
    *peak = (*peak).max(current);
    if current > budget {
        return Err(anyhow!(
            "temporary dataset exceeded physical budget: current={} budget={} path={}",
            format_bytes(current),
            format_bytes(budget),
            root.display()
        ));
    }
    Ok(())
}

fn estimated_physical_bytes(rows: usize, record_value_bytes: usize) -> u64 {
    (rows as u64).saturating_mul(
        (record_value_bytes as u64).saturating_add(CONSERVATIVE_PHYSICAL_OVERHEAD_PER_ROW),
    )
}

fn rate(rows: usize, elapsed: Duration) -> f64 {
    rows as f64 / elapsed.as_secs_f64()
}

fn milliseconds(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1_000.0
}

fn gib_to_bytes(gib: f64) -> Result<u64> {
    if !gib.is_finite() || gib <= 0.0 {
        return Err(anyhow!("--max-case-gib must be finite and positive"));
    }
    Ok((gib * 1024.0 * 1024.0 * 1024.0) as u64)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2}{}", UNITS[unit])
}
