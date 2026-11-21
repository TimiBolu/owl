//! Owl in a browser.
//!
//! The engine is the same one the server runs, compiled to WebAssembly. The
//! database lives in memory: its files are held by the in-memory file system
//! Owl's own crash tests use, so nothing outlasts the page. There are no
//! threads here, so nothing checkpoints of its own accord.

use owl_db::simulation::filesystem::{FileSystem, SimulatedFileSystem};
use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql_file, Document, OQLFile, OQLFileStatement, QueryExecutor, Value};
use serde_json::{json, Value as Json};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

/// One database, open for as long as this is held.
#[wasm_bindgen]
pub struct Owl {
    executor: QueryExecutor<RustStore>,
}

#[wasm_bindgen]
impl Owl {
    /// Open an empty database in memory.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<Owl, JsError> {
        let files: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(0));
        let store = RustStore::new_with_fs("owl", 1 << 30, files).map_err(|error| JsError::new(&error.to_string()))?;
        Ok(Owl { executor: QueryExecutor::new_with_real_clock(Arc::new(store)) })
    }

    /// Run OQL: schemas, inserts, functions and queries, one after another.
    ///
    /// The answer is JSON text: `rows`, the rows of the last statement when
    /// it is a query; `columns`, their fields in the order the query gave
    /// them, which a JSON object does not keep; `ran`, how many statements
    /// ran; and `errors`, what went wrong, in the words Owl says it in.
    pub fn run(&self, source: &str) -> String {
        self.run_to_json(source).to_string()
    }
}

impl Owl {
    fn run_to_json(&self, source: &str) -> Json {
        let mut file = match parse_oql_file(source) {
            Ok(file) => file,
            Err(error) => {
                return json!({ "rows": [], "columns": [], "ran": 0, "errors": [error.to_string()] })
            }
        };
        // The last statement, when it is a query, is run for its rows.
        let last = match file.statements.last() {
            Some(OQLFileStatement::Query(_)) => file.statements.pop(),
            _ => None,
        };
        let mut errors = Vec::new();
        let mut ran = 0;
        if !file.statements.is_empty() {
            match execute_oql_file(&OQLFile { statements: file.statements }, &self.executor) {
                Ok(report) => {
                    ran += report.success_count;
                    errors.extend(report.errors);
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        let mut rows = Vec::new();
        let mut columns: Vec<String> = Vec::new();
        if let (Some(OQLFileStatement::Query(query)), true) = (last, errors.is_empty()) {
            match self.executor.execute_query(&query) {
                Ok(documents) => {
                    ran += 1;
                    for document in &documents {
                        for (name, _) in document.fields.iter().filter(|(name, _)| Self::shown(name)) {
                            if !columns.contains(name) {
                                columns.push(name.clone());
                            }
                        }
                    }
                    rows = documents.iter().map(|document| self.row(document)).collect();
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        json!({ "rows": rows, "columns": columns, "ran": ran, "errors": errors })
    }

    /// Whether a field is the query's own, not one the engine keeps beside.
    fn shown(name: &str) -> bool {
        !name.starts_with('_') && !name.starts_with('$')
    }

    /// A row as JSON, with a reference shown as the id of what it refers to.
    fn row(&self, document: &Document) -> Json {
        Json::Object(
            document
                .fields
                .iter()
                .filter(|(name, _)| Self::shown(name))
                .map(|(name, value)| {
                    let shown = self.executor.with_reference_ids(value).unwrap_or_else(|_| value.clone());
                    (name.clone(), to_json(&shown))
                })
                .collect(),
        )
    }
}

fn to_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Boolean(value) => json!(value),
        Value::Integer(value) => json!(value),
        Value::Float(value) => json!(value),
        Value::String(value) => json!(value),
        Value::DateTime(instant) => json!(instant.to_rfc3339()),
        Value::List(items) => Json::Array(items.iter().map(to_json).collect()),
        Value::Object(fields) => {
            Json::Object(fields.iter().map(|(name, value)| (name.clone(), to_json(value))).collect())
        }
        Value::Reference(_) | Value::GlobalReference(_) => json!("<reference>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same wrapper, run natively: what the browser is given is what
    /// this gives.
    #[test]
    fn a_script_runs_and_its_last_query_gives_rows() {
        let owl = Owl::new().ok().expect("an empty database opens");
        let answer = owl.run_to_json(
            r#"
            schema authors { id: String, name: String }
            schema books { id: String, title: String, year: Integer, author: Ref(authors) }
            authors.insert [ { id: "a1", name: "Ursula" }, { id: "a2", name: "Italo" } ]
            books.insert [
              { id: "b1", title: "The Dispossessed", year: 1974, author: "a1" },
              { id: "b2", title: "Invisible Cities", year: 1972, author: "a2" },
              { id: "b3", title: "The Lathe of Heaven", year: 1971, author: "a1" }
            ]
            books.where { .author.name == "Ursula" }.sort { .year.asc }.select { title, year, by: .author.name }
            "#,
        );
        assert_eq!(answer["errors"], json!([]), "{answer}");
        // The columns are in the order the query selected them.
        assert_eq!(answer["columns"], json!(["title", "year", "by"]));
        assert_eq!(
            answer["rows"],
            json!([
                { "title": "The Lathe of Heaven", "year": 1971, "by": "Ursula" },
                { "title": "The Dispossessed", "year": 1974, "by": "Ursula" }
            ])
        );
        // The database is still there for the next thing asked.
        let again = owl.run_to_json("authors.sort { .id.asc }.select { id, books: .books.count() }");
        assert_eq!(again["errors"], json!([]), "{again}");
        assert_eq!(again["rows"], json!([{ "id": "a1", "books": 2 }, { "id": "a2", "books": 1 }]));
        // What is wrong is said, not thrown.
        let wrong = owl.run_to_json("books.where { .nothing == 1 }");
        assert!(!wrong["errors"].as_array().unwrap().is_empty(), "{wrong}");
    }
}
