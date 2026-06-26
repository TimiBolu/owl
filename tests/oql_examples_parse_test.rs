use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

#[test]
fn oql_examples_parse_as_query_or_script() {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.push("oql");

    let mut failures = Vec::new();
    let entries = fs::read_dir(&dir).expect("read oql dir");
    for entry in entries {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("oql") {
            continue;
        }
        let raw = fs::read_to_string(&path).expect("read oql file");
        let query_result = owl_db::parser::parse_oql(&raw);
        let script_result = owl_db::parser::parse_oql_file(&raw);
        let query_ok = query_result.is_ok();
        let script_ok = script_result.is_ok();
        if !query_ok && !script_ok {
            let query_err = query_result
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            let script_err = script_result
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            failures.push(format!(
                "{}\n  query: {}\n  script: {}",
                path.display(),
                query_err,
                script_err
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "Some .oql examples failed to parse as query or script:\n{}",
        failures.join("\n")
    );
}

#[test]
fn oql_examples_execute_script_smoke() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let oql_dir = manifest_dir.join("oql");

    let candidates = [
        "phase1_acceptance.oql",
        "functions_litmus_test.oql",
        "improved_syntax.oql",
    ];

    let mut failures = Vec::new();
    for file_name in candidates {
        let path = oql_dir.join(file_name);
        let raw = fs::read_to_string(&path).expect("read oql file");
        let file = match owl_db::parse_oql_file(&raw) {
            Ok(file) => file,
            Err(e) => {
                failures.push(format!("{} parse failed: {}", path.display(), e));
                continue;
            }
        };

        let temp_dir = TempDir::new().expect("temp dir");
        let db_path = temp_dir
            .path()
            .join(format!("{}.owl", file_name.replace(".oql", "")));
        let store = match owl_db::storage::zig_adapter::ZigStore::new(
            db_path.to_str().expect("db path str"),
            10 * 1024 * 1024,
        ) {
            Ok(store) => Arc::new(store),
            Err(e) => {
                failures.push(format!("{} store init failed: {}", path.display(), e));
                continue;
            }
        };
        let executor = owl_db::QueryExecutor::new_with_real_clock(store);

        match owl_db::execute_oql_file(&file, &executor) {
            Ok(result) => {
                if !result.errors.is_empty() {
                    failures.push(format!(
                        "{} execution errors: {:?}",
                        path.display(),
                        result.errors
                    ));
                }
            }
            Err(e) => failures.push(format!("{} execute failed: {}", path.display(), e)),
        }
    }

    assert!(
        failures.is_empty(),
        "Some smoke script examples failed to execute:\n{}",
        failures.join("\n")
    );
}
