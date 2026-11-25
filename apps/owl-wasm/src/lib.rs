//! Owl in a browser.
//!
//! The engine is the same one the server runs, compiled to WebAssembly.
//!
//! A database opened with `new Owl()` lives in memory: its files are held
//! by the in-memory file system Owl's own crash tests use, and nothing
//! outlasts the page. One opened with `Owl.openKept(handles)` keeps its
//! files in the browser's own file system, through a [`pool`] of files
//! opened beforehand, and is there when the page is loaded again.
//!
//! There are no threads here, so nothing checkpoints of its own accord: a
//! page that keeps its database asks for a checkpoint now and then.

#[cfg(target_arch = "wasm32")]
mod opfs;
pub mod pool;

use owl_db::simulation::filesystem::{FileSystem, SimulatedFileSystem};
use pool::PoolFileSystem;
use owl_db::storage::RustStore;
use owl_db::schema::{FieldType, Schema, SchemaConstraint};
use owl_db::storage::DocumentStore;
use owl_db::{execute_oql_file, parse_oql_file, Document, OQLFile, OQLFileStatement, QueryExecutor, Value};
use serde_json::{json, Value as Json};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

/// One database, open for as long as this is held.
#[wasm_bindgen]
pub struct Owl {
    executor: QueryExecutor<RustStore>,
    /// The pool the database's files are kept in, when they are kept.
    kept: Option<PoolFileSystem>,
}

#[wasm_bindgen]
impl Owl {
    /// Open an empty database in memory.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<Owl, JsError> {
        Self::over(Arc::new(SimulatedFileSystem::new(0)), None).map_err(|error| JsError::new(&error))
    }

    /// Open the database kept in the browser's own file system, or begin
    /// one there. `handles` are files of that file system, each opened for
    /// reading and writing without waiting (`createSyncAccessHandle`), which
    /// a browser allows only in a worker. They are the pool the database's
    /// files are kept in; a pool that has kept a database before brings it.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = openKept)]
    pub fn open_kept(handles: js_sys::Array) -> Result<Owl, JsError> {
        let files = PoolFileSystem::open(opfs::slots(handles)).map_err(|error| JsError::new(&error.to_string()))?;
        Self::over(Arc::new(files.clone()), Some(files)).map_err(|error| JsError::new(&error))
    }

    /// Add files to the pool of a database that is kept.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = addSlots)]
    pub fn add_slots(&self, handles: js_sys::Array) -> Result<(), JsError> {
        match &self.kept {
            Some(files) => files.add(opfs::slots(handles)).map_err(|error| JsError::new(&error.to_string())),
            None => Err(JsError::new("this database is held in memory and has no pool")),
        }
    }

    /// How many files of the pool hold nothing. The engine takes one for
    /// each file it makes, and making one fails when none is left, so a
    /// page adds more while there are still some.
    #[wasm_bindgen(js_name = freeSlots)]
    pub fn free_slots(&self) -> usize {
        self.kept.as_ref().map_or(0, PoolFileSystem::free)
    }

    /// The files a kept database is made of, as JSON: name and size.
    pub fn files(&self) -> String {
        let files = self.kept.as_ref().map(PoolFileSystem::files).unwrap_or_default();
        json!(files.iter().map(|(name, size)| json!({ "name": name, "size": size })).collect::<Vec<_>>()).to_string()
    }

    /// What is in the database, as JSON: each collection with how many
    /// rows it has, its fields (the id, then the rest by name) with their
    /// types as OQL writes them, the collection each reference points at,
    /// the names by which other collections' references reach it, and the
    /// rules its schema states.
    pub fn describe(&self) -> String {
        self.describe_to_json().to_string()
    }

    /// Fold the log into the database's checkpoint, so that the log does
    /// not grow for as long as the database is used and opening it does
    /// not replay all of it. Nothing does this unasked in a browser.
    pub fn checkpoint(&self) -> Result<(), JsError> {
        self.checkpoint_kept().map_err(|error| JsError::new(&error))
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
    /// A database whose files are kept by `files`.
    fn over(files: Arc<dyn FileSystem>, kept: Option<PoolFileSystem>) -> Result<Owl, String> {
        let store = RustStore::new_with_fs("owl", 1 << 30, files).map_err(|error| error.to_string())?;
        Ok(Owl { executor: QueryExecutor::new_with_real_clock(Arc::new(store)), kept })
    }

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

    fn describe_to_json(&self) -> Json {
        let store = self.executor.store();
        let mut names = store.list_schemas().unwrap_or_default();
        // The engine keeps its own things in collections named with a `$`.
        names.retain(|name| !name.starts_with('$'));
        let schemas: Vec<Schema> = names.iter().filter_map(|name| store.get_schema(name).ok().flatten()).collect();
        let collections: Vec<Json> = schemas
            .iter()
            .map(|schema| {
                // The engine keeps a schema's fields by name, not in the
                // order they were written: the id first, then by name.
                let mut order: Vec<&String> = schema.fields.keys().collect();
                order.sort_by_key(|name| (name.as_str() != "id", name.as_str()));
                let fields: Vec<Json> = order
                    .iter()
                    .filter_map(|name| Some((*name, schema.fields.get(*name)?)))
                    .map(|(name, field_type)| {
                        json!({
                            "name": name,
                            "type": type_text(schema, name, field_type),
                            "ref": target_of(field_type),
                            "optional": field_type.is_optional(),
                            "list": matches!(plain(field_type), FieldType::List(_)),
                            "kind": plain(field_type).type_name(),
                        })
                    })
                    .collect();
                // The references of other collections that point here, and
                // the name each is reached by from this side.
                let mut reached_by = Vec::new();
                for other in &schemas {
                    let mut fields: Vec<(&String, &FieldType)> = other.fields.iter().collect();
                    fields.sort_by(|left, right| left.0.cmp(right.0));
                    for (field, field_type) in fields {
                        if target_of(field_type).as_deref() != Some(schema.name.as_str()) {
                            continue;
                        }
                        let name = other
                            .constraints
                            .iter()
                            .find_map(|constraint| match constraint {
                                SchemaConstraint::ReverseName { field: named, name } if named == field => Some(name.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| other.name.clone());
                        reached_by.push(json!({ "name": name, "from": other.name, "field": field }));
                    }
                }
                let rules: Vec<String> = schema
                    .constraints
                    .iter()
                    .filter_map(|constraint| match constraint {
                        SchemaConstraint::Unique { fields, .. } => Some(format!("unique({})", fields.join(", "))),
                        SchemaConstraint::CheckExpression { expression } => Some(format!("check({expression})")),
                        SchemaConstraint::Default { field, expression } => Some(format!("{field} defaults to {expression}")),
                        _ => None,
                    })
                    .collect();
                json!({
                    "name": schema.name,
                    "rows": store.collection_len(&schema.name).ok().flatten(),
                    "fields": fields,
                    "reachedBy": reached_by,
                    "rules": rules,
                })
            })
            .collect();
        json!(collections)
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

/// A field's type with `Optional` taken off.
fn plain(field_type: &FieldType) -> &FieldType {
    match field_type {
        FieldType::Optional(inner) => plain(inner),
        other => other,
    }
}

/// The collection a field refers to, when it is a reference or a list of them.
fn target_of(field_type: &FieldType) -> Option<String> {
    match field_type {
        FieldType::Reference(reference) => Some(reference.target_schema.clone()),
        FieldType::Optional(inner) | FieldType::List(inner) => target_of(inner),
        _ => None,
    }
}

/// A field's type as OQL writes it. The shape of an embedded object is
/// kept beside the schema's fields, under the path that reaches it.
fn type_text(schema: &Schema, path: &str, field_type: &FieldType) -> String {
    let shape = |path: &str| {
        schema.constraints.iter().find_map(|constraint| match constraint {
            SchemaConstraint::Shape { path: named, fields } if named == path => Some(format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|(name, inner)| format!("{name}: {}", type_text(schema, &format!("{path}.{name}"), inner)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            _ => None,
        })
    };
    match field_type {
        FieldType::Object => shape(path).unwrap_or_else(|| "Object".to_string()),
        FieldType::Vector { dim } => format!("Vector({dim})"),
        FieldType::Reference(reference) if reference.required => format!("Ref({})", reference.target_schema),
        FieldType::Reference(reference) => format!("Optional(Ref({}))", reference.target_schema),
        FieldType::List(inner) => match inner.as_ref() {
            FieldType::Object => format!("List({})", shape(&format!("{path}[]")).unwrap_or_else(|| "Object".to_string())),
            other => format!("List({})", type_text(schema, &format!("{path}[]"), other)),
        },
        FieldType::Optional(inner) => format!("Optional({})", type_text(schema, path, inner)),
        other => other.type_name().to_string(),
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

impl Owl {
    /// What [`Owl::checkpoint`] does, with what went wrong as text.
    fn checkpoint_kept(&self) -> Result<(), String> {
        self.executor.store().checkpoint().map(|_| ()).map_err(|error| error.to_string())
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
        // The database says what is in it.
        let described = owl.describe_to_json();
        let books = described.as_array().unwrap().iter().find(|c| c["name"] == "books").unwrap();
        assert_eq!(books["rows"], json!(3));
        assert_eq!(
            books["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect::<Vec<_>>(),
            ["id", "author", "title", "year"],
            "the id, then the rest by name"
        );
        assert_eq!(books["fields"][1]["type"], json!("Ref(authors)"));
        assert_eq!(books["fields"][1]["ref"], json!("authors"));
        let authors = described.as_array().unwrap().iter().find(|c| c["name"] == "authors").unwrap();
        assert_eq!(authors["reachedBy"], json!([{ "name": "books", "from": "books", "field": "author" }]));
        // Nothing of the engine's own is shown.
        assert!(described.as_array().unwrap().iter().all(|c| !c["name"].as_str().unwrap().starts_with('$')));

        // The database is still there for the next thing asked.
        let again = owl.run_to_json("authors.sort { .id.asc }.select { id, books: .books.count() }");
        assert_eq!(again["errors"], json!([]), "{again}");
        assert_eq!(again["rows"], json!([{ "id": "a1", "books": 2 }, { "id": "a2", "books": 1 }]));
        // What is wrong is said, not thrown.
        let wrong = owl.run_to_json("books.where { .nothing == 1 }");
        assert!(!wrong["errors"].as_array().unwrap().is_empty(), "{wrong}");
    }

    use pool::{MemorySlot, SlotStorage};

    /// A database over a pool of files, as it is opened in a browser, with
    /// bytes in memory standing for the browser's files.
    fn kept(slots: &[MemorySlot]) -> Owl {
        let files = PoolFileSystem::open(
            slots.iter().map(|slot| Box::new(slot.clone()) as Box<dyn SlotStorage>).collect(),
        )
        .unwrap();
        Owl::over(Arc::new(files.clone()), Some(files)).unwrap_or_else(|error| panic!("{error}"))
    }

    /// What a page that keeps its database does and then finds on loading
    /// again: after writes left in the log, after a checkpoint, after more
    /// writes and a compaction, and after a function was defined.
    #[test]
    fn a_kept_database_is_there_when_it_is_opened_again() {
        let slots: Vec<MemorySlot> = (0..48).map(|_| MemorySlot::default()).collect();
        let books = "books.sort { .id.asc }.select { id, title, by: .author.name }";
        let count = |owl: &Owl| owl.run_to_json("books.count()")["rows"][0]["count"].clone();
        {
            let owl = kept(&slots);
            let answer = owl.run_to_json(
                r#"
                schema authors { id: String, name: String }
                schema books { id: String, title: String, year: Integer, author: Ref(authors), tags: List(String) }
                authors.insert [ { id: "a1", name: "Ursula" }, { id: "a2", name: "Italo" } ]
                books.insert [
                  { id: "b1", title: "The Dispossessed", year: 1974, author: "a1", tags: ["utopia"] },
                  { id: "b2", title: "Invisible Cities", year: 1972, author: "a2", tags: ["cities"] }
                ]
                fn decade(year) { year - year % 10 }
                "#,
            );
            assert_eq!(answer["errors"], json!([]), "{answer}");
            assert_eq!(count(&owl), json!(2));
            // Shapes inside lists, optional references and reverse names
            // are described as they were written.
            let more = owl.run_to_json(
                r#"schema reviews {
                     id: String,
                     book: Ref(books) as notes,
                     by: Optional(Ref(authors)),
                     stars: Integer,
                     lines: List({ page: Integer, text: String }),
                     check(.stars >= 1)
                   }"#,
            );
            assert_eq!(more["errors"], json!([]), "{more}");
            let described = owl.describe_to_json();
            let find = |name: &str| described.as_array().unwrap().iter().find(|c| c["name"] == name).unwrap().clone();
            let reviews = find("reviews");
            let types: Vec<String> =
                reviews["fields"].as_array().unwrap().iter().map(|f| f["type"].as_str().unwrap().to_string()).collect();
            assert_eq!(
                types,
                ["String", "Ref(books)", "Optional(Ref(authors))", "List({ page: Integer, text: String })", "Integer"]
            );
            assert_eq!(reviews["rows"], json!(0));
            assert!(reviews["rules"].as_array().unwrap().iter().any(|rule| rule.as_str().unwrap().starts_with("check(")));
            assert!(find("books")["reachedBy"]
                .as_array()
                .unwrap()
                .contains(&json!({ "name": "notes", "from": "reviews", "field": "book" })));
        }
        // From the log alone.
        let expected = {
            let owl = kept(&slots);
            assert_eq!(count(&owl), json!(2));
            let rows = owl.run_to_json(books);
            assert_eq!(rows["errors"], json!([]), "{rows}");
            assert_eq!(rows["rows"][0], json!({ "id": "b1", "title": "The Dispossessed", "by": "Ursula" }));
            // The function is the database's, so it is still there.
            let decades = owl.run_to_json("books.group_by { d: decade(.year) }.select { d, n: count() }");
            assert_eq!(decades["rows"], json!([{ "d": 1970, "n": 2 }]), "{decades}");
            owl.executor.store().checkpoint().unwrap();
            let more = owl.run_to_json(
                r#"books.insert { id: "b3", title: "The Lathe of Heaven", year: 1971, author: "a1", tags: [] }"#,
            );
            assert_eq!(more["errors"], json!([]), "{more}");
            let changed = owl.run_to_json(r#"books.where { .id == "b2" }.update { title: "Le città invisibili" }"#);
            assert_eq!(changed["errors"], json!([]), "{changed}");
            owl.run_to_json(books)["rows"].clone()
        };
        assert_eq!(expected.as_array().unwrap().len(), 3);
        // From a checkpoint with a log after it.
        {
            let owl = kept(&slots);
            assert_eq!(owl.run_to_json(books)["rows"], expected);
            owl.executor.store().checkpoint().unwrap();
            owl.executor.store().compact().unwrap();
            assert_eq!(owl.run_to_json(books)["rows"], expected);
            let removed = owl.run_to_json(r#"books.where { .id == "b1" }.delete"#);
            assert_eq!(removed["errors"], json!([]), "{removed}");
        }
        // From a compacted database with a log after it, many times over:
        // each opening and each checkpoint makes and removes files, and
        // the pool must not run dry.
        for round in 0..12 {
            let owl = kept(&slots);
            assert_eq!(count(&owl), json!(2 + round), "round {round}");
            let id = format!("n{round}");
            let added = owl.run_to_json(&format!(
                r#"books.insert {{ id: "{id}", title: "Round {round}", year: 2000, author: "a2", tags: ["new"] }}"#
            ));
            assert_eq!(added["errors"], json!([]), "round {round}: {added}");
            if round % 2 == 0 {
                owl.executor.store().checkpoint().unwrap();
            }
            if round % 5 == 4 {
                owl.executor.store().compact_and_reclaim().unwrap();
            }
            assert!(owl.free_slots() > 8, "round {round}: {} files left in the pool", owl.free_slots());
        }
        let owl = kept(&slots);
        assert_eq!(count(&owl), json!(14));
        owl.executor.store().verify_integrity().unwrap();
    }
    /// A page keeps some files of the pool spare and gives it more between
    /// one run and the next, never during one, since the engine cannot
    /// wait for a file. So no run and no checkpoint may make more files
    /// at once than are spare. This does what `worker.js` does, with its
    /// numbers, over a database that grows through many checkpoints.
    #[test]
    fn no_run_needs_more_files_than_a_page_keeps_spare() {
        // As in `worker.js`.
        const FIRST: usize = 32;
        const SPARE: usize = 12;
        const MORE: usize = 16;
        const CHECKPOINT_EVERY: usize = 20;

        let mut slots: Vec<MemorySlot> = (0..FIRST).map(|_| MemorySlot::default()).collect();
        let files = PoolFileSystem::open(
            slots.iter().map(|slot| Box::new(slot.clone()) as Box<dyn SlotStorage>).collect(),
        )
        .unwrap();
        let owl = Owl::over(Arc::new(files.clone()), Some(files.clone())).unwrap_or_else(|error| panic!("{error}"));
        let mut most_needed = 0;
        let mut most_files = 0;
        // After a call into the engine: it needed no more files than were
        // spare, and the page then tops the pool up.
        let mut after = |what: &str, held_before: usize, spare_before: usize, slots: &mut Vec<MemorySlot>| {
            let needed = files.most_held().saturating_sub(held_before);
            most_needed = most_needed.max(needed);
            most_files = most_files.max(files.files().len());
            assert!(
                needed <= SPARE,
                "{what} made {needed} files at once, and a page keeps {SPARE} spare ({spare_before} were)"
            );
            if files.free() < SPARE {
                let more: Vec<MemorySlot> = (0..MORE).map(|_| MemorySlot::default()).collect();
                files
                    .add(more.iter().map(|slot| Box::new(slot.clone()) as Box<dyn SlotStorage>).collect())
                    .unwrap();
                slots.extend(more);
            }
        };

        let (held, spare) = (files.files().len(), files.free());
        files.most_held();
        let made = owl.run_to_json(
            r#"
            schema authors { id: String, name: String }
            schema books { id: String, title: String, year: Integer, author: Ref(authors), tags: List(String) }
            schema notes { id: String, about: Ref(books), text: String }
            authors.insert [ { id: "a1", name: "Ursula" }, { id: "a2", name: "Italo" } ]
            "#,
        );
        assert_eq!(made["errors"], json!([]), "{made}");
        // Indexes of the database's own, besides the ones every database
        // has, in case each is more files while a checkpoint is made.
        owl.executor
            .store()
            .write_batch(
                [("by_year", "books", "year"), ("by_title", "books", "title"), ("by_text", "notes", "text"), ("by_name", "authors", "name")]
                    .into_iter()
                    .map(|(name, schema, field)| {
                        owl_db::storage_engine::WriteOperation::PutScalarIndex(owl_db::storage::ScalarIndexDefinition {
                            name: name.to_string(),
                            schema: schema.to_string(),
                            fields: vec![field.to_string()],
                            unique: false,
                        })
                    })
                    .collect(),
            )
            .unwrap();
        after("the first run", held, spare, &mut slots);

        for run in 0..400usize {
            let (held, spare) = (files.files().len(), files.free());
            files.most_held();
            let rows = (0..25)
                .map(|row| {
                    let id = run * 25 + row;
                    format!(r#"{{ id: "b{id}", title: "Book {id}", year: {}, author: "a{}", tags: ["t{}"] }}"#, 1900 + id % 120, 1 + id % 2, id % 7)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let added = owl.run_to_json(&format!(
                r#"books.insert [ {rows} ]
                notes.insert {{ id: "n{run}", about: "b{}", text: "note {run}" }}"#,
                run * 25
            ));
            assert_eq!(added["errors"], json!([]), "run {run}: {added}");
            after(&format!("run {run}"), held, spare, &mut slots);
            if (run + 1) % CHECKPOINT_EVERY == 0 {
                let (held, spare) = (files.files().len(), files.free());
                files.most_held();
                owl.checkpoint_kept().unwrap_or_else(|error| panic!("checkpoint after run {run}: {error}"));
                after(&format!("the checkpoint after run {run}"), held, spare, &mut slots);
            }
        }
        println!(
            "a call needed at most {most_needed} files at once; the database is {} files, {most_files} at most, in a pool of {}",
            files.files().len(),
            slots.len()
        );
        assert_eq!(owl.run_to_json("books.count()")["rows"][0]["count"], json!(10_000));
        owl.executor.store().verify_integrity().unwrap();
    }
}
