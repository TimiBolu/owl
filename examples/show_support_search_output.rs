use owl_db::parser::{
    parse_oql_file, resolve_query_with_bindings, resolve_query_with_value_bindings,
    resolve_value_with_bindings, schema_definition_to_schema, Expression, OQLFileStatement,
    OQLQuery,
};
use owl_db::schema::SchemaRegistry;
use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, DocumentStore, QueryExecutor};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn describe_query(query: &OQLQuery) -> String {
    let ops = query
        .operations
        .iter()
        .map(|op| format!("{:?}", op))
        .collect::<Vec<_>>()
        .join(" -> ");
    format!("{}.{}", query.collection, ops)
}

fn print_documents(docs: &[owl_db::Document], max_docs: usize) {
    println!("documents returned: {}", docs.len());
    for (i, doc) in docs.iter().take(max_docs).enumerate() {
        println!("--- doc {} ({}) ---", i + 1, doc.id);
        match serde_json::to_string_pretty(doc) {
            Ok(json) => println!("{}", json),
            Err(e) => println!("(failed to serialize doc: {})", e),
        }
    }
    if docs.len() > max_docs {
        println!("... {} more documents omitted", docs.len() - max_docs);
    }
}

fn run_script_with_output(
    executor: &mut QueryExecutor<RustStore>,
    script_path: &PathBuf,
    title: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n==================== {} ====================", title);
    println!("script: {}", script_path.display());
    let raw = std::fs::read_to_string(script_path)?;
    let file = parse_oql_file(&raw)?;

    let registry = SchemaRegistry::new();
    for statement in &file.statements {
        if let OQLFileStatement::Schema(schema_def) = statement {
            let empty_schema =
                owl_db::Schema::new(schema_def.name.clone(), std::collections::HashMap::new());
            let _ = registry.register(empty_schema);
        }
    }
    for statement in &file.statements {
        if let OQLFileStatement::Schema(schema_def) = statement {
            let schema = schema_definition_to_schema(schema_def, &registry)?;
            let _ = registry.register(schema.clone());
            executor.store().put_schema(&schema)?;
        }
    }

    let mut bindings: HashMap<String, OQLQuery> = HashMap::new();
    let mut value_bindings: HashMap<String, Expression> = HashMap::new();
    let mut query_no = 0usize;

    for statement in file.statements {
        match statement {
            OQLFileStatement::Schema(_) => {}
            OQLFileStatement::FunctionDef(func) => {
                executor.register_user_function(func)?;
            }
            OQLFileStatement::QueryBinding(binding) => {
                bindings.insert(binding.name.clone(), binding.query.clone());
                println!("binding: {}", binding.name);
            }
            OQLFileStatement::ValueBinding(binding) => {
                let value = resolve_value_with_bindings(&binding.value, &value_bindings);
                value_bindings.insert(binding.name.clone(), value);
                println!("binding: {}", binding.name);
            }
            OQLFileStatement::Query(query) => {
                query_no += 1;
                let resolved = resolve_query_with_value_bindings(
                    &resolve_query_with_bindings(&query, &bindings)?,
                    &value_bindings,
                );
                println!("\n[query {}] {}", query_no, describe_query(&resolved));
                if let Some(source) = value_bindings.get(&resolved.collection) {
                    let value =
                        executor.evaluate_script_expression(&Expression::CollectionExpression {
                            source: Box::new(source.clone()),
                            operations: resolved.operations.clone(),
                        })?;
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    let docs = executor.execute_query(&resolved)?;
                    print_documents(&docs, 3);
                }
            }
            OQLFileStatement::Transaction(statements) => {
                println!(
                    "transaction block encountered ({} statements): skipped by this demo runner",
                    statements.len()
                );
            }
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db_path = std::env::temp_dir();
    db_path.push(format!("support_search_demo_{}.owl", std::process::id()));
    let store = Arc::new(RustStore::new(
        db_path.to_str().ok_or("invalid temp path")?,
        64 * 1024 * 1024,
    )?);
    let mut executor = QueryExecutor::new_with_real_clock(store.clone());

    let setup = PathBuf::from("oql/use_cases/support_search/phaseA_support_search_setup.oql");
    let queries = PathBuf::from("oql/use_cases/support_search/phaseA_support_search_queries.oql");
    let eval = PathBuf::from("oql/use_cases/support_search/phaseA_support_search_eval.oql");

    // Use built-in script executor for setup (schemas + inserts).
    let setup_raw = std::fs::read_to_string(&setup)?;
    let setup_file = parse_oql_file(&setup_raw)?;
    let setup_result = execute_oql_file(&setup_file, &executor)?;
    println!(
        "setup complete: schemas={}, queries={}, errors={}",
        setup_result.schema_count,
        setup_result.query_count,
        setup_result.errors.len()
    );

    run_script_with_output(&mut executor, &queries, "PHASE A QUERIES")?;
    run_script_with_output(&mut executor, &eval, "PHASE A EVAL")?;

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{}.wal", db_path.display()));
    let _ = std::fs::remove_file(format!("{}.sb", db_path.display()));
    let _ = std::fs::remove_dir_all(format!("{}.vindex", db_path.display()));

    Ok(())
}
