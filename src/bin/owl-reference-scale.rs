use anyhow::{anyhow, Context, Result};
use clap::Parser;
use owl_db::storage::{AutoCheckpointPolicy, RustStore};
use owl_db::{Document, DocumentStore, FieldType, Schema, Value};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::Builder;

const BATCH_ROWS: usize = 4_096;

#[derive(Debug, Parser)]
#[command(about = "Run bounded Owl reverse-reference scaling experiments")]
struct Args {
    #[arg(long, value_delimiter = ',', default_value = "100000,1000000")]
    sources: Vec<usize>,
    #[arg(long, default_value_t = 1_000)]
    targets: usize,
    #[arg(long, default_value_t = 64)]
    record_bytes: usize,
    #[arg(long, default_value_t = 100)]
    mutations: usize,
    /// Add a second reference path to every Nth source; zero disables it.
    #[arg(long, default_value_t = 4)]
    mixed_reference_stride: usize,
    #[arg(long, default_value_t = 7)]
    samples: usize,
    #[arg(long, default_value_t = 5)]
    reopen_samples: usize,
    /// Full-checkpoint and lazily reopen after this many ingested sources.
    /// Zero retains the one-shot ingestion behavior.
    #[arg(long, default_value_t = 0)]
    ingest_checkpoint_rows: usize,
    #[arg(long, default_value_t = 2.0)]
    max_case_gib: f64,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct Report {
    generated_at: chrono::DateTime<chrono::Utc>,
    cases: Vec<CaseReport>,
}

#[derive(Debug, Serialize)]
struct CaseReport {
    sources: usize,
    ingest_checkpoints: usize,
    targets: usize,
    edges: usize,
    reference_paths: usize,
    mixed_reference_stride: usize,
    record_bytes: usize,
    mutations: usize,
    ingest_ms: f64,
    checkpoint_ms: f64,
    reopen_ms: f64,
    reopen_first_ms: f64,
    reopen_min_ms: f64,
    reopen_max_ms: f64,
    physical_bytes: u64,
    peak_observed_bytes: u64,
    typed_loaded_segments_at_open: usize,
    resident_id_bytes_at_open: usize,
    locator_cached_pages_at_open: usize,
    locator_cached_bytes_at_open: usize,
    locator_cached_pages_after_cold: usize,
    locator_cached_bytes_after_cold: usize,
    resident_documents_at_open: usize,
    cold_lookup_ms: f64,
    warm_lookup_median_ms: f64,
    warm_lookup_min_ms: f64,
    warm_lookup_max_ms: f64,
    postings_per_target: usize,
    target_sweep_ms: f64,
    mutation_ms: f64,
    mutated_target_postings: usize,
    post_mutation_checkpoint_ms: f64,
    post_mutation_physical_bytes: u64,
    cleanup_verified: bool,
}

fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_target(false)
            .try_init();
    }
    let args = Args::parse();
    if args.targets == 0 || args.samples == 0 || args.reopen_samples == 0 {
        return Err(anyhow!(
            "--targets, --samples, and --reopen-samples must be greater than zero"
        ));
    }
    let max_bytes = (args.max_case_gib * 1024.0 * 1024.0 * 1024.0) as u64;
    let mut cases = Vec::new();
    for sources in args.sources {
        cases.push(run_case(
            sources,
            args.targets.min(sources.max(1)),
            args.record_bytes,
            args.mutations.min(sources),
            args.mixed_reference_stride,
            args.samples,
            args.reopen_samples,
            args.ingest_checkpoint_rows,
            max_bytes,
        )?);
    }
    let report = Report {
        generated_at: chrono::Utc::now(),
        cases,
    };
    if let Some(path) = args.output {
        fs::write(&path, serde_json::to_vec_pretty(&report)?)
            .with_context(|| format!("write report {}", path.display()))?;
        println!("report={}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

fn run_case(
    sources: usize,
    targets: usize,
    record_bytes: usize,
    mutations: usize,
    mixed_reference_stride: usize,
    samples: usize,
    reopen_samples: usize,
    ingest_checkpoint_rows: usize,
    max_bytes: u64,
) -> Result<CaseReport> {
    let directory = Builder::new().prefix("owl-reference-scale-").tempdir()?;
    let directory_path = directory.path().to_path_buf();
    let database_path = directory_path.join("references.owl");
    println!(
        "BUILD sources={sources} targets={targets} temp={}",
        directory_path.display()
    );
    let mut store = RustStore::new(&database_path, max_bytes.saturating_mul(4) as usize)?;
    store.configure_auto_checkpoint(AutoCheckpointPolicy::disabled())?;
    store.put_schema(&Schema::new(
        "nodes".into(),
        HashMap::from([
            ("sequence".into(), FieldType::Integer),
            ("message".into(), FieldType::String),
            (
                "parent".into(),
                FieldType::Optional(Box::new(FieldType::Reference(owl_db::ReferenceType::new(
                    "nodes".into(),
                )))),
            ),
            (
                "related".into(),
                FieldType::Optional(Box::new(FieldType::Reference(owl_db::ReferenceType::new(
                    "nodes".into(),
                )))),
            ),
        ]),
    ))?;

    let ingest_started = Instant::now();
    let mut ingest_checkpoint_ms = 0.0;
    let mut ingest_checkpoints = 0usize;
    let mut checkpointed_sources = 0usize;
    let mut peak = directory_bytes(&directory_path)?;
    let mut batch = Vec::with_capacity(BATCH_ROWS);
    for target in 0..targets {
        batch.push(Document::with_id(
            target_id(target),
            "nodes".into(),
            HashMap::from([
                ("sequence".into(), Value::Integer(target as i64)),
                (
                    "message".into(),
                    Value::String(padded_message("target", target, record_bytes)),
                ),
            ]),
        ));
        flush_batch(&store, &mut batch, &directory_path, max_bytes, &mut peak)?;
    }
    if !batch.is_empty() {
        store.put_documents(&batch)?;
        batch.clear();
    }
    let mut target_refs = Vec::with_capacity(targets);
    for target in 0..targets {
        target_refs.push(
            store
                .get_objref("nodes", &target_id(target))?
                .ok_or_else(|| anyhow!("target identity is missing"))?,
        );
    }
    for source in 0..sources {
        let mut fields = HashMap::from([
            ("sequence".into(), Value::Integer(source as i64)),
            (
                "message".into(),
                Value::String(padded_message("source", source, record_bytes)),
            ),
            (
                "parent".into(),
                Value::Reference(target_refs[source % targets]),
            ),
        ]);
        if mixed_reference_stride != 0 && source % mixed_reference_stride == 0 {
            fields.insert(
                "related".into(),
                Value::Reference(target_refs[(source + 1) % targets]),
            );
        }
        batch.push(Document::with_id(source_id(source), "nodes".into(), fields));
        flush_batch(&store, &mut batch, &directory_path, max_bytes, &mut peak)?;
        let ingested_sources = source + 1;
        if ingest_checkpoint_rows != 0 && ingested_sources % ingest_checkpoint_rows == 0 {
            if !batch.is_empty() {
                store.put_documents(&batch)?;
                batch.clear();
            }
            observe_size(&directory_path, max_bytes, &mut peak)?;
            let checkpoint_started = Instant::now();
            store.checkpoint()?;
            store.wait_for_maintenance()?;
            ingest_checkpoint_ms += checkpoint_started.elapsed().as_secs_f64() * 1_000.0;
            ingest_checkpoints += 1;
            checkpointed_sources = ingested_sources;
            observe_size(&directory_path, max_bytes, &mut peak)?;
            drop(store);
            store = RustStore::new(&database_path, max_bytes.saturating_mul(4) as usize)?;
            store.configure_auto_checkpoint(AutoCheckpointPolicy::disabled())?;
            println!(
                "INGEST_CHECKPOINT sources={ingested_sources} checkpoints={ingest_checkpoints}"
            );
        }
    }
    if !batch.is_empty() {
        store.put_documents(&batch)?;
    }
    let ingest_ms = ingest_started.elapsed().as_secs_f64() * 1_000.0 - ingest_checkpoint_ms;
    observe_size(&directory_path, max_bytes, &mut peak)?;

    if checkpointed_sources != sources {
        let checkpoint_started = Instant::now();
        store.checkpoint()?;
        store.wait_for_maintenance()?;
        ingest_checkpoint_ms += checkpoint_started.elapsed().as_secs_f64() * 1_000.0;
        ingest_checkpoints += 1;
    }
    let checkpoint_ms = ingest_checkpoint_ms;
    let physical_bytes = directory_bytes(&directory_path)?;
    peak = peak.max(physical_bytes);
    drop(store);

    let mut reopen_timings = Vec::with_capacity(reopen_samples);
    for _ in 0..reopen_samples.saturating_sub(1) {
        let reopen_started = Instant::now();
        let sample = RustStore::new(&database_path, max_bytes.saturating_mul(4) as usize)?;
        sample.configure_auto_checkpoint(AutoCheckpointPolicy::disabled())?;
        reopen_timings.push(reopen_started.elapsed().as_secs_f64() * 1_000.0);
        drop(sample);
    }
    let reopen_started = Instant::now();
    let reopened = RustStore::new(&database_path, max_bytes.saturating_mul(4) as usize)?;
    reopened.configure_auto_checkpoint(AutoCheckpointPolicy::disabled())?;
    reopen_timings.push(reopen_started.elapsed().as_secs_f64() * 1_000.0);
    let reopen_first_ms = reopen_timings[0];
    reopen_timings.sort_by(f64::total_cmp);
    let reopen_ms = reopen_timings[reopen_timings.len() / 2];
    let reopen_min_ms = reopen_timings[0];
    let reopen_max_ms = *reopen_timings.last().unwrap();
    let layout = reopened.storage_layout_stats("nodes")?;

    let expected_postings = sources.div_ceil(targets);
    let cold_started = Instant::now();
    let cold =
        reopened.list_references_to("nodes", &target_id(0), Some("nodes"), Some("parent"))?;
    let cold_lookup_ms = cold_started.elapsed().as_secs_f64() * 1_000.0;
    if cold.len() != expected_postings {
        return Err(anyhow!(
            "cold reverse lookup returned {} postings, expected {expected_postings}",
            cold.len()
        ));
    }
    let locator_after_cold = reopened.storage_layout_stats("nodes")?;
    let mut warm = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let rows =
            reopened.list_references_to("nodes", &target_id(0), Some("nodes"), Some("parent"))?;
        if rows.len() != expected_postings {
            return Err(anyhow!("warm reverse lookup changed its result"));
        }
        warm.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    warm.sort_by(f64::total_cmp);
    let sweep_started = Instant::now();
    for target in (0..targets).step_by((targets / 32).max(1)) {
        let _ = reopened.list_references_to("nodes", &target_id(target), None, None)?;
    }
    let target_sweep_ms = sweep_started.elapsed().as_secs_f64() * 1_000.0;

    let mutation_started = Instant::now();
    let destination = targets - 1;
    let mut updates = Vec::with_capacity(mutations);
    let mut already_destination = 0usize;
    for source in 0..mutations {
        let mut document = reopened
            .get_document("nodes", &source_id(source))?
            .ok_or_else(|| anyhow!("mutation source is missing"))?;
        if source % targets == destination {
            already_destination += 1;
        }
        document.set_field("parent".into(), Value::Reference(target_refs[destination]));
        updates.push(document);
    }
    reopened.put_documents(&updates)?;
    let mutation_ms = mutation_started.elapsed().as_secs_f64() * 1_000.0;
    let mutated_target_postings = reopened
        .list_references_to("nodes", &target_id(destination), None, Some("parent"))?
        .len();
    let expected_mutated =
        sources / targets + usize::from(destination < sources % targets) + mutations
            - already_destination;
    if mutated_target_postings != expected_mutated {
        return Err(anyhow!(
            "mutated target has {mutated_target_postings} postings, expected {expected_mutated}"
        ));
    }
    let post_mutation_checkpoint_started = Instant::now();
    reopened.checkpoint()?;
    reopened.wait_for_maintenance()?;
    let post_mutation_checkpoint_ms =
        post_mutation_checkpoint_started.elapsed().as_secs_f64() * 1_000.0;
    let post_mutation_physical_bytes = directory_bytes(&directory_path)?;
    peak = peak.max(post_mutation_physical_bytes);
    if post_mutation_physical_bytes > max_bytes {
        return Err(anyhow!(
            "post-mutation checkpoint used {}, above the {} byte case budget",
            format_bytes(post_mutation_physical_bytes),
            format_bytes(max_bytes)
        ));
    }
    let compacted_target_postings = reopened
        .list_references_to("nodes", &target_id(destination), None, Some("parent"))?
        .len();
    if compacted_target_postings != expected_mutated {
        return Err(anyhow!(
            "post-compaction target has {compacted_target_postings} postings, expected {expected_mutated}"
        ));
    }

    drop(reopened);
    directory.close()?;
    let cleanup_verified = !directory_path.exists();
    if !cleanup_verified {
        return Err(anyhow!("temporary reference dataset was not removed"));
    }
    let report = CaseReport {
        sources,
        ingest_checkpoints,
        targets,
        edges: sources
            + if mixed_reference_stride == 0 {
                0
            } else {
                sources.div_ceil(mixed_reference_stride)
            },
        reference_paths: 1 + usize::from(mixed_reference_stride != 0),
        mixed_reference_stride,
        record_bytes,
        mutations,
        ingest_ms,
        checkpoint_ms,
        reopen_ms,
        reopen_first_ms,
        reopen_min_ms,
        reopen_max_ms,
        physical_bytes,
        peak_observed_bytes: peak,
        typed_loaded_segments_at_open: layout.typed_loaded_segments,
        resident_id_bytes_at_open: layout.typed_identity_bytes,
        locator_cached_pages_at_open: layout.objref_locator_cached_pages,
        locator_cached_bytes_at_open: layout.objref_locator_cached_bytes,
        locator_cached_pages_after_cold: locator_after_cold.objref_locator_cached_pages,
        locator_cached_bytes_after_cold: locator_after_cold.objref_locator_cached_bytes,
        resident_documents_at_open: layout.resident_documents,
        cold_lookup_ms,
        warm_lookup_median_ms: warm[warm.len() / 2],
        warm_lookup_min_ms: warm[0],
        warm_lookup_max_ms: *warm.last().unwrap(),
        postings_per_target: expected_postings,
        target_sweep_ms,
        mutation_ms,
        mutated_target_postings,
        post_mutation_checkpoint_ms,
        post_mutation_physical_bytes,
        cleanup_verified,
    };
    println!(
        "RESULT sources={} targets={} reopen_median={:.3}ms reopen_range={:.3}..{:.3}ms cold_reverse={:.3}ms warm_reverse={:.3}ms mutation={:.3}ms post_mutation_checkpoint={:.3}ms physical={} cleanup={}",
        report.sources,
        report.targets,
        report.reopen_ms,
        report.reopen_min_ms,
        report.reopen_max_ms,
        report.cold_lookup_ms,
        report.warm_lookup_median_ms,
        report.mutation_ms,
        report.post_mutation_checkpoint_ms,
        format_bytes(report.physical_bytes),
        report.cleanup_verified
    );
    Ok(report)
}

fn flush_batch(
    store: &RustStore,
    batch: &mut Vec<Document>,
    directory: &Path,
    max_bytes: u64,
    peak: &mut u64,
) -> Result<()> {
    if batch.len() < BATCH_ROWS {
        return Ok(());
    }
    store.put_documents(batch)?;
    batch.clear();
    observe_size(directory, max_bytes, peak)
}

fn observe_size(directory: &Path, max_bytes: u64, peak: &mut u64) -> Result<()> {
    let bytes = directory_bytes(directory)?;
    *peak = (*peak).max(bytes);
    if bytes > max_bytes {
        return Err(anyhow!(
            "case exceeded byte budget: {} > {}",
            format_bytes(bytes),
            format_bytes(max_bytes)
        ));
    }
    Ok(())
}

fn directory_bytes(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn target_id(index: usize) -> String {
    format!("target-{index:020}")
}

fn source_id(index: usize) -> String {
    format!("source-{index:020}")
}

fn padded_message(kind: &str, index: usize, bytes: usize) -> String {
    let prefix = format!("{kind} payload {index}");
    format!("{prefix}{}", "x".repeat(bytes.saturating_sub(prefix.len())))
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:.2}MiB", bytes as f64 / MIB)
}
