use owl_db::document::{Document, Value};
use owl_db::simulation::SimulatedFileSystem;
use owl_db::storage::{create_storage, create_storage_with_fs, DocumentStore};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

fn percentile_nanos(samples: &mut [u128], pct: f64) -> u128 {
    samples.sort_unstable();
    let idx = ((samples.len() as f64 - 1.0) * pct).round() as usize;
    samples[idx]
}

fn make_doc(schema: &str, id: &str, i: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), Value::Integer(i));
    fields.insert("tag".to_string(), Value::String(format!("t{}", i % 10)));
    Document::with_id(id.to_string(), schema.to_string(), fields)
}

#[test]
#[ignore]
fn characterize_write_read_latency() {
    let temp = tempfile::TempDir::new().unwrap();
    let data_path = temp.path().join("characterization.owl");
    let path = data_path.to_string_lossy().to_string();
    let storage = create_storage(&path, 64 * 1024 * 1024).unwrap();

    let n = 2_000usize;
    let mut write_ns = Vec::with_capacity(n);
    for i in 0..n {
        let doc = make_doc("bench_users", &format!("u{}", i), i as i64);
        let start = Instant::now();
        storage.put_document(&doc).unwrap();
        write_ns.push(start.elapsed().as_nanos());
    }

    let mut read_ns = Vec::with_capacity(n);
    for i in 0..n {
        let start = Instant::now();
        let _ = storage
            .get_document("bench_users", &format!("u{}", i))
            .unwrap()
            .unwrap();
        read_ns.push(start.elapsed().as_nanos());
    }

    let mut write_sorted = write_ns.clone();
    let mut read_sorted = read_ns.clone();
    let write_p50 = percentile_nanos(&mut write_sorted, 0.50);
    let write_p95 = percentile_nanos(&mut write_sorted, 0.95);
    let read_p50 = percentile_nanos(&mut read_sorted, 0.50);
    let read_p95 = percentile_nanos(&mut read_sorted, 0.95);

    let total_write_ns: u128 = write_ns.iter().copied().sum();
    let total_read_ns: u128 = read_ns.iter().copied().sum();
    let write_ops_sec = (n as f64) / (total_write_ns as f64 / 1_000_000_000.0);
    let read_ops_sec = (n as f64) / (total_read_ns as f64 / 1_000_000_000.0);

    println!(
        "write p50={}ns p95={}ns throughput={:.2} ops/s",
        write_p50, write_p95, write_ops_sec
    );
    println!(
        "read  p50={}ns p95={}ns throughput={:.2} ops/s",
        read_p50, read_p95, read_ops_sec
    );
}

#[test]
#[ignore]
fn characterize_crash_recovery_durability() {
    let fs = Arc::new(SimulatedFileSystem::new(42));
    let path = "char_durable.owl";

    {
        let store = create_storage_with_fs(path, 32 * 1024 * 1024, fs.clone()).unwrap();
        let doc = make_doc("durability", "d1", 1);
        store.put_document(&doc).unwrap();
    }

    fs.crash();
    fs.recover();

    let restarted = create_storage_with_fs(path, 32 * 1024 * 1024, fs.clone()).unwrap();
    let recovered = restarted
        .get_document("durability", &"d1".to_string())
        .unwrap();
    assert!(recovered.is_some());
}
