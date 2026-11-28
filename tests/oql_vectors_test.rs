//! Vectors: what a `Vector(n)` field holds. A vector is kept packed, as
//! 32-bit numbers, has arithmetic and functions of its own, and is what
//! `nearest` compares. Every case is small enough to work out by hand.

use owl_db::storage::{DocumentStore, RustStore};
use owl_db::{bind_parameters, execute_oql_file, parse_oql, parse_oql_file, Document, QueryExecutor, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

struct Db {
    directory: TempDir,
    store: Arc<RustStore>,
    executor: QueryExecutor<RustStore>,
}

impl Db {
    fn open(directory: TempDir) -> Self {
        let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
        let executor = QueryExecutor::new_with_real_clock(store.clone());
        Self { directory, store, executor }
    }

    /// Close the database and open it again from what is on disk.
    fn reopened(self) -> Self {
        self.store.checkpoint().unwrap();
        let Self { directory, store, executor } = self;
        drop(executor);
        drop(store);
        Self::open(directory)
    }

    fn try_rows(&self, query: &str) -> Result<Vec<Document>, String> {
        let parsed = parse_oql(query).map_err(|error| error.to_string())?;
        // Every `$q` here is the same direction: along the first axis.
        let mut params = HashMap::new();
        params.insert("q".to_string(), Value::List(vec![Value::Float(1.0), Value::Float(0.0), Value::Float(0.0)]));
        let bound = match query.contains("$q") {
            true => bind_parameters(&parsed, &params)?,
            false => parsed,
        };
        self.executor.execute_query(&bound).map_err(|error| error.to_string())
    }

    fn rows(&self, query: &str) -> Vec<Document> {
        self.try_rows(query).unwrap_or_else(|error| panic!("{error}\n{query}"))
    }

    /// One field of every row, in order, as text.
    fn column(&self, query: &str, field: &str) -> Vec<String> {
        self.rows(query)
            .iter()
            .map(|row| match row.get_field(field) {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Null) | None => "null".to_string(),
                Some(other) => other.to_string_repr(),
            })
            .collect()
    }

    /// The one value a query gives.
    fn value(&self, query: &str) -> Value {
        let rows = self.rows(query);
        assert_eq!(rows.len(), 1, "{query}");
        let fields: Vec<_> = rows[0].fields.iter().filter(|(name, _)| !name.starts_with('_')).collect();
        assert_eq!(fields.len(), 1, "{query}: {:?}", rows[0].fields);
        fields[0].1.clone()
    }

    fn number(&self, query: &str) -> f64 {
        match self.value(query) {
            Value::Float(number) => number,
            Value::Integer(number) => number as f64,
            other => panic!("not a number: {other:?}\n{query}"),
        }
    }

    fn error(&self, query: &str) -> String {
        match self.try_rows(query) {
            Ok(rows) => panic!("expected an error, got {} rows from\n{query}", rows.len()),
            Err(error) => error,
        }
    }
}

fn vector(numbers: &[f32]) -> Value {
    Value::Vector(numbers.to_vec())
}

fn close(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-5
}

/// Two owners and five documents in three dimensions. `d1` points along
/// the first axis, `d2` and `d4` nearly so, `d3` along the second, and
/// `d5` has no embedding.
fn library() -> Db {
    let db = Db::open(TempDir::new().unwrap());
    let source = r#"
        schema owners { id: String, name: String }
        schema docs { id: String, owner: Ref(owners), kind: String, embedding: Optional(Vector(3)), index(.embedding, metric: "cosine") }
        owners.insert [ { id: "a", name: "A" }, { id: "b", name: "B" } ]
        docs.insert [
          { id: "d1", owner: "a", kind: "x", embedding: [1.0, 0.0, 0.0] },
          { id: "d2", owner: "a", kind: "y", embedding: [0.9, 0.1, 0.0] },
          { id: "d3", owner: "b", kind: "x", embedding: [0.0, 1.0, 0.0] },
          { id: "d4", owner: "b", kind: "y", embedding: [0.8, 0.2, 0.1] },
          { id: "d5", owner: "b", kind: "y" }
        ]
    "#;
    let report = execute_oql_file(&parse_oql_file(source).unwrap(), &db.executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    db
}

#[test]
fn a_vector_is_kept_packed_however_it_is_written_and_survives_a_reopen() {
    let db = library();
    // Written in OQL.
    assert_eq!(db.value(r#"docs.where { .id == "d2" }.select { embedding }"#), vector(&[0.9, 0.1, 0.0]));
    // Written as documents, as a server does from JSON: whole numbers too.
    let mut fields = owl_db::Fields::default();
    fields.insert("id".to_string(), Value::String("d6".to_string()));
    fields.insert("owner".to_string(), Value::String("a".to_string()));
    fields.insert("kind".to_string(), Value::String("x".to_string()));
    fields.insert("embedding".to_string(), Value::List(vec![Value::Integer(0), Value::Float(0.5), Value::Integer(1)]));
    db.executor.insert_documents("docs", vec![fields]).unwrap();
    assert_eq!(db.value(r#"docs.where { .id == "d6" }.select { embedding }"#), vector(&[0.0, 0.5, 1.0]));
    // Written by an update, from its own value.
    db.rows(r#"docs.where { .id == "d3" }.update { embedding: .embedding * 0.5 + [0.5, 0.0, 0.0] }"#);

    let db = db.reopened();
    assert_eq!(db.value(r#"docs.where { .id == "d2" }.select { embedding }"#), vector(&[0.9, 0.1, 0.0]));
    assert_eq!(db.value(r#"docs.where { .id == "d3" }.select { embedding }"#), vector(&[0.5, 0.5, 0.0]));
    assert_eq!(db.value(r#"docs.where { .id == "d6" }.select { embedding }"#), vector(&[0.0, 0.5, 1.0]));
    assert_eq!(db.value(r#"docs.where { .id == "d5" }.select { embedding }"#), Value::Null);
    // To a client it is what it always was: a list of numbers.
    assert_eq!(vector(&[0.5, 0.25]).to_clean_json(), serde_json::json!([0.5, 0.25]));
}

#[test]
fn what_is_not_a_vector_of_the_declared_length_is_refused() {
    let db = library();
    let short = db.error(r#"docs.insert { id: "bad", owner: "a", kind: "x", embedding: [1.0, 2.0] }"#);
    assert!(short.contains("expects 3 dimensions, got 2"), "{short}");
    let text = db.error(r#"docs.insert { id: "bad", owner: "a", kind: "x", embedding: [1.0, "two", 3.0] }"#);
    assert!(text.contains("must be numeric"), "{text}");
    let grown = db.error(r#"docs.where { .id == "d1" }.update { embedding: [1.0, 0.0, 0.0, 0.0] }"#);
    assert!(grown.contains("expects 3 dimensions, got 4"), "{grown}");
    // A number a vector cannot hold: not finite, or too large for 32 bits.
    for number in [f64::NAN, f64::INFINITY, 1e40] {
        let mut fields = owl_db::Fields::default();
        fields.insert("id".to_string(), Value::String("bad".to_string()));
        fields.insert("owner".to_string(), Value::String("a".to_string()));
        fields.insert("kind".to_string(), Value::String("x".to_string()));
        fields.insert(
            "embedding".to_string(),
            Value::List(vec![Value::Float(1.0), Value::Float(number), Value::Float(0.0)]),
        );
        let refused = db.executor.insert_documents("docs", vec![fields]).unwrap_err().to_string();
        assert!(refused.contains("'embedding[1]' holds a number that is not finite"), "{refused}");
    }
    assert_eq!(db.rows("docs").len(), 5);
    // A function of values cannot end a chain, and says how it is written.
    let ended = db.error("docs.embedding.centroid()");
    assert!(ended.contains("cannot end a chain: write centroid(..)"), "{ended}");
}

#[test]
fn a_vector_has_arithmetic_of_its_own_and_lists_keep_theirs() {
    let db = library();
    let of = |expression: &str| db.value(&format!(r#"docs.where {{ .id == "d2" }}.select {{ v: {expression} }}"#));
    assert_eq!(of(".embedding + [1.0, 1.0, 1.0]"), vector(&[1.9, 1.1, 1.0]));
    assert_eq!(of("[1.0, 1.0, 1.0] + .embedding"), vector(&[1.9, 1.1, 1.0]));
    assert_eq!(of(".embedding - .embedding"), vector(&[0.0, 0.0, 0.0]));
    assert_eq!(of(".embedding * 2"), vector(&[1.8, 0.2, 0.0]));
    assert_eq!(of("2 * .embedding"), vector(&[1.8, 0.2, 0.0]));
    assert_eq!(of(".embedding / 2"), vector(&[0.45, 0.05, 0.0]));
    // Two lists are still joined, which is how an update appends.
    assert_eq!(of("[1, 2] + [3]"), Value::List(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]));

    let d1 = |expression: &str| db.error(&format!(r#"docs.where {{ .id == "d1" }}.select {{ v: {expression} }}"#));
    assert!(d1(".embedding + [1.0, 2.0]").contains("these have 3 and 2 numbers"));
    assert!(d1(r#".embedding + "x""#).contains("between a vector and a string"));
    assert!(d1(".embedding * .embedding").contains("multiplied or divided by a number"));
    assert!(d1(".embedding / 0").contains("divided by zero"));
}

#[test]
fn the_vector_functions_give_what_the_sums_give() {
    let db = library();
    let d2 = |expression: &str| db.number(&format!(r#"docs.where {{ .id == "d2" }}.select {{ v: {expression} }}"#));
    // d2 is (0.9, 0.1, 0) and $q is (1, 0, 0).
    assert!(close(d2("dot(.embedding, $q)"), 0.9));
    assert!(close(d2("norm(.embedding)"), 0.82f64.sqrt()));
    assert!(close(d2("similarity(.embedding, $q)"), 0.9 / 0.82f64.sqrt()));
    assert!(close(d2("distance(.embedding, $q)"), 0.02f64.sqrt()));
    // As a method, and on a list written in the query.
    assert!(close(d2(".embedding.similarity($q)"), 0.9 / 0.82f64.sqrt()));
    assert!(close(d2("dot([1, 2, 3], [4, 5, 6])"), 32.0));
    assert!(close(d2("norm(normalize(.embedding))"), 1.0));
    assert_eq!(
        db.value(r#"docs.where { .id == "d2" }.select { v: vector([1, 2]) }"#),
        vector(&[1.0, 2.0])
    );

    // A row with no embedding has no similarity; it is not an error.
    assert_eq!(
        db.column("docs.select { id, s: similarity(.embedding, $q) }", "s")[4],
        "null"
    );
    let unequal = db.error("docs.select { s: similarity(.embedding, [1.0, 0.0]) }");
    assert!(unequal.contains("needs two vectors of one length, and these have 3 and 2 numbers"), "{unequal}");
    let text = db.error(r#"docs.select { s: similarity(.embedding, "x") }"#);
    assert!(text.contains("takes a vector, and was given a string"), "{text}");
}

#[test]
fn a_vector_still_reads_as_the_list_of_its_numbers() {
    let db = library();
    let d2 = |expression: &str| db.value(&format!(r#"docs.where {{ .id == "d2" }}.select {{ v: {expression} }}"#));
    assert_eq!(d2("length(.embedding)"), Value::Integer(3));
    assert_eq!(d2("type_of(.embedding)"), Value::String("Vector".to_string()));
    assert!(matches!(d2(".embedding[0]"), Value::Float(first) if close(first, 0.9)));
    assert!(matches!(d2(".embedding[-1]"), Value::Float(last) if last == 0.0));
    assert!(matches!(d2(".embedding[0:2]"), Value::List(cut) if cut.len() == 2));
    // Equal to the list written in a query, at the width a vector holds.
    assert_eq!(db.column("docs.where { .embedding == [0.9, 0.1, 0.0] }.select { id }", "id"), ["d2"]);
    assert_eq!(db.column("docs.where { .embedding != [0.9, 0.1, 0.0] }.select { id }", "id").len(), 4);
    assert_eq!(db.column("docs.where { .embedding contains 1.0 }.select { id }", "id"), ["d1", "d3"]);
    assert_eq!(db.column("docs.where { .embedding is null }.select { id }", "id"), ["d5"]);
    // Rows that hold vectors are told apart like any others.
    assert_eq!(db.value("docs.select { embedding }.distinct().count()"), Value::Integer(5));
}

#[test]
fn nearest_is_the_rows_a_sort_by_similarity_puts_first() {
    let db = library();
    let near = r#"nearest { field: .embedding, query: $q, metric: "cosine", k: 3 }"#;
    let found = db.column(&format!("docs.{near}.select {{ id }}"), "id");
    assert_eq!(found, ["d1", "d2", "d4"]);
    assert_eq!(
        db.column(
            "docs.where { .embedding is not null }.sort { similarity(.embedding, $q) desc, .id }.limit(3).select { id }",
            "id"
        ),
        found
    );
    // The score is the similarity.
    let scores = db.rows(&format!("docs.{near}.select {{ id, score: ._score, same: similarity(.embedding, $q) }}"));
    for row in &scores {
        assert_eq!(row.get_field("score"), row.get_field("same"), "{:?}", row.fields);
    }
    // By straight-line distance, nearest first; by the dot product.
    assert_eq!(
        db.column(r#"docs.nearest { field: .embedding, query: [0.0, 1.0, 0.0], metric: "l2", k: 2 }.select { id }"#, "id"),
        ["d3", "d4"]
    );
    assert_eq!(
        db.column(r#"docs.nearest { field: .embedding, query: $q, metric: "dot", k: 1 }.select { id }"#, "id"),
        ["d1"]
    );
    // More asked for than there are: every row that has an embedding.
    assert_eq!(db.rows(r#"docs.nearest { field: .embedding, query: $q, k: 99 }"#).len(), 4);
    let wrong = db.error("docs.nearest { field: .embedding, query: [1.0, 0.0], k: 2 }");
    assert!(wrong.contains("Expected Vector(3), got Vector(2)"), "{wrong}");
}

#[test]
fn nearest_ranks_only_the_rows_it_is_given() {
    let db = library();
    let near = r#"nearest { field: .embedding, query: $q, metric: "cosine", k: 2 }"#;
    // After a filter: the nearest of the rows that passed it.
    assert_eq!(db.column(&format!(r#"docs.where {{ .kind == "y" }}.{near}.select {{ id }}"#), "id"), ["d2", "d4"]);
    // Through a relationship: each owner's own nearest.
    let per_owner = db.rows(&format!("owners.select {{ name, near: .docs.{near}.select {{ id }} }}"));
    let ids = |row: &Document| match row.get_field("near") {
        Some(Value::List(found)) => found
            .iter()
            .map(|one| match one {
                Value::Object(fields) => fields["id"].to_string_repr(),
                other => panic!("{other:?}"),
            })
            .collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    assert_eq!(ids(&per_owner[0]), ["d1", "d2"]);
    assert_eq!(ids(&per_owner[1]), ["d4", "d3"]);
    // From one owner, by a path step; and as a capture in a block.
    assert_eq!(db.column(&format!(r#"owners.where {{ .id == "b" }}.docs.{near}.select {{ id }}"#), "id"), ["d4", "d3"]);
    assert_eq!(
        db.column(&format!("owners {{ d in .docs.{near}  select {{ name, doc: d.id }} }}"), "doc"),
        ["d1", "d2", "d4", "d3"]
    );
    // The rows most like each row: itself, then its nearest other.
    let like = db.rows(
        r#"docs.where { .id == "d2" }.select { id, like: docs.nearest { field: .embedding, query: docs.embedding, metric: "cosine", k: 2 }.select { id } }"#,
    );
    assert!(matches!(like[0].get_field("like"), Some(Value::List(found)) if found.len() == 2));
}

#[test]
fn a_centroid_is_the_mean_of_the_vectors_it_is_given() {
    let db = library();
    // Of the rows a relationship reaches. B's d5 has no embedding and is
    // left out of the mean.
    let owners = db.rows("owners.select { name, c: centroid(.docs.embedding) }");
    assert_eq!(owners[0].get_field("c"), Some(&vector(&[0.95, 0.05, 0.0])));
    assert_eq!(owners[1].get_field("c"), Some(&vector(&[0.4, 0.6, 0.05])));
    // Of each group's rows.
    let kinds = db.rows("docs.group_by { .kind }.select { kind, c: centroid(.embedding), n: count() }");
    assert_eq!(kinds[0].get_field("c"), Some(&vector(&[0.5, 0.5, 0.0])));
    assert_eq!(kinds[1].get_field("c"), Some(&vector(&[0.85, 0.15, 0.05])));
    // Of a whole collection, named from another query.
    assert_eq!(
        db.value("owners.limit(1).select { c: centroid(docs.embedding) }"),
        vector(&[0.675, 0.325, 0.025])
    );
    // And used as the vector a search starts from: the documents nearest
    // the middle of what owner A holds.
    let near_a = db.rows(
        r#"owners.where { .id == "a" }.select { like: docs.nearest { field: .embedding, query: centroid(owners.docs.embedding), metric: "cosine", k: 2 }.select { id } }"#,
    );
    assert!(matches!(near_a[0].get_field("like"), Some(Value::List(found)) if found.len() == 2));
    // Of nothing it is null.
    assert_eq!(db.value(r#"docs.where { .id == "d5" }.select { c: centroid(.embedding) }"#), Value::Null);
}

#[test]
fn a_vector_stored_as_a_list_before_vectors_were_packed_is_still_found() {
    let db = library();
    // A row as an earlier build stored it: the embedding a list of numbers.
    let owner = db.rows(r#"docs.where { .id == "d1" }"#)[0].fields["owner"].clone();
    let mut fields = owl_db::Fields::default();
    fields.insert("id".to_string(), Value::String("old".to_string()));
    fields.insert("owner".to_string(), owner);
    fields.insert("kind".to_string(), Value::String("x".to_string()));
    fields.insert(
        "embedding".to_string(),
        Value::List(vec![Value::Float(0.95), Value::Float(0.05), Value::Float(0.0)]),
    );
    let mut old = Document::new("docs".to_string(), fields);
    old.id = "old".to_string();
    db.store.put_document(&old).unwrap();
    let db = db.reopened();
    assert!(matches!(db.value(r#"docs.where { .id == "old" }.select { embedding }"#), Value::List(_)));

    // Search and every vector function take it as the vector it is.
    assert_eq!(
        db.column(r#"docs.nearest { field: .embedding, query: $q, metric: "cosine", k: 2 }.select { id }"#, "id"),
        ["d1", "old"]
    );
    assert!(close(
        db.number(r#"docs.where { .id == "old" }.select { s: similarity(.embedding, $q) }"#),
        0.95 / (0.95f64 * 0.95 + 0.05 * 0.05).sqrt()
    ));
    assert_eq!(db.column("docs.where { .embedding == [0.95, 0.05, 0.0] }.select { id }", "id"), ["old"]);

    // Written again, it is packed, and from then on a vector in every way.
    db.rows("docs.update { embedding: vector(.embedding) }");
    let db = db.reopened();
    assert_eq!(db.value(r#"docs.where { .id == "old" }.select { embedding }"#), vector(&[0.95, 0.05, 0.0]));
    assert_eq!(
        db.value(r#"docs.where { .id == "old" }.select { v: .embedding + [1.0, 1.0, 1.0] }"#),
        vector(&[1.95, 1.05, 1.0])
    );
    assert_eq!(db.value(r#"docs.where { .id == "d5" }.select { embedding }"#), Value::Null);
}

#[test]
fn a_search_of_a_whole_collection_compares_packed_vectors_and_reads_only_the_rows_found() {
    use owl_db::executor::vector_column_searches;
    let db = library();
    let near = |k: usize, metric: &str| {
        format!(r#"nearest {{ field: .embedding, query: $q, metric: "{metric}", k: {k} }}.select {{ id, score: ._score }}"#)
    };
    // The same search two ways: of the whole collection, which is
    // answered from the vectors where they are kept packed, and after a
    // filter that every row passes, which reads each row.
    let both = |db: &Db, k: usize, metric: &str| {
        let before = vector_column_searches();
        let packed = db.rows(&format!("docs.{}", near(k, metric)));
        let from_column = vector_column_searches() - before;
        let by_row = db.rows(&format!(r#"docs.where {{ .id != "" }}.{}"#, near(k, metric)));
        assert_eq!(vector_column_searches() - before, from_column, "a filtered search reads rows");
        let shown = |rows: &[Document]| -> Vec<String> {
            rows.iter().map(|row| format!("{:?} {:?}", row.get_field("id"), row.get_field("score"))).collect()
        };
        assert_eq!(shown(&packed), shown(&by_row), "k {k}, {metric}");
        (packed.len(), from_column)
    };
    // The store makes a collection's columns once it has been asked twice.
    both(&db, 2, "cosine");
    for metric in ["cosine", "l2", "dot"] {
        for k in [1, 2, 3, 4, 99] {
            let (found, from_column) = both(&db, k, metric);
            // d5 has no embedding, so there are four to find.
            assert_eq!(found, k.min(4));
            assert_eq!(from_column, 1, "k {k}, {metric}: answered from the column");
        }
    }

    // What is written since is seen: a new nearest row, a row that loses
    // its embedding, a row deleted, and a row whose embedding changes.
    db.rows(r#"docs.insert { id: "d0", owner: "a", kind: "x", embedding: [1.0, 0.0, 0.0] }"#);
    db.rows(r#"docs.where { .id == "d2" }.update { embedding: null }"#);
    db.rows(r#"docs.where { .id == "d4" }.delete()"#);
    db.rows(r#"docs.where { .id == "d3" }.update { embedding: [0.99, 0.01, 0.0] }"#);
    both(&db, 3, "cosine");
    let (_, from_column) = both(&db, 3, "cosine");
    assert_eq!(from_column, 1);
    // Equal scores keep the order of a scan: d0 before d1.
    assert_eq!(db.column(&format!("docs.{}", near(3, "cosine")), "id"), ["d0", "d1", "d3"]);

    // After a reopen, and with what follows the search applied to its rows.
    let db = db.reopened();
    both(&db, 3, "l2");
    both(&db, 3, "l2");
    assert_eq!(
        db.column(
            r#"docs.nearest { field: .embedding, query: $q, metric: "cosine", k: 3 }.where { .kind == "x" }.sort { .id desc }.select { id, owner: .owner.name }"#,
            "id"
        ),
        ["d3", "d1", "d0"]
    );
    // A query vector of the wrong length is still refused.
    let wrong = db.error("docs.nearest { field: .embedding, query: [1.0, 0.0], k: 2 }");
    assert!(wrong.contains("Expected Vector(3), got Vector(2)"), "{wrong}");
}

/// Not a test of right and wrong: what 20,000 vectors of 384 numbers cost
/// to keep and to search. Run with `--release -- --ignored --nocapture`.
#[test]
#[ignore]
fn what_twenty_thousand_vectors_cost() {
    use std::time::Instant;
    const ROWS: usize = 20_000;
    const NUMBERS: usize = 384;
    let directory = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 4 << 30).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    let schema = format!(
        "schema docs {{ id: String, kind: String, embedding: Vector({NUMBERS}), index(.embedding, metric: \"cosine\") }}"
    );
    let report = execute_oql_file(&parse_oql_file(&schema).unwrap(), &executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    // SplitMix64, so the same rows every run.
    let mut state = 7u64;
    let mut unit = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    };
    let mut flat: Vec<f32> = Vec::with_capacity(ROWS * NUMBERS);
    let started = Instant::now();
    for chunk in 0..ROWS / 1000 {
        let rows: Vec<owl_db::Fields> = (0..1000)
            .map(|at| {
                let row = chunk * 1000 + at;
                let numbers: Vec<f32> = (0..NUMBERS).map(|_| unit()).collect();
                flat.extend(&numbers);
                let mut fields = owl_db::Fields::default();
                fields.insert("id".to_string(), Value::String(format!("d{row:06}")));
                let kind = if row % 10 == 0 { "rare" } else { "common" };
                fields.insert("kind".to_string(), Value::String(kind.to_string()));
                fields.insert(
                    "embedding".to_string(),
                    Value::List(numbers.iter().map(|number| Value::Float(f64::from(*number))).collect()),
                );
                fields
            })
            .collect();
        executor.insert_documents("docs", rows).unwrap();
    }
    println!("insert {ROWS} x {NUMBERS}: {:.1} s", started.elapsed().as_secs_f64());
    store.checkpoint().unwrap();
    fn bytes(path: &std::path::Path) -> u64 {
        std::fs::read_dir(path)
            .unwrap()
            .flatten()
            .map(|entry| match entry.metadata().unwrap() {
                found if found.is_dir() => bytes(&entry.path()),
                found => found.len(),
            })
            .sum()
    }
    println!(
        "on disk {:.1} MB; the numbers alone are {:.1} MB",
        bytes(directory.path()) as f64 / 1e6,
        (ROWS * NUMBERS * 4) as f64 / 1e6
    );

    let query: Vec<f32> = (0..NUMBERS).map(|_| unit()).collect();
    let written = format!("[{}]", query.iter().map(|number| format!("{number:.6}")).collect::<Vec<_>>().join(", "));
    let time = |what: &str, text: String| {
        let parsed = parse_oql(&text).unwrap();
        let mut found = Vec::new();
        let mut times: Vec<f64> = (0..5)
            .map(|_| {
                let started = Instant::now();
                found = executor.execute_query(&parsed).unwrap();
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        // The first run may make what later runs find kept.
        let first = times[0];
        times.sort_by(f64::total_cmp);
        println!("{what}: {:.1} ms, {} rows (first run {first:.1} ms)", times[2], found.len());
        found.iter().map(|row| row.get_field("id").map(Value::to_string_repr).unwrap_or_default()).collect::<Vec<_>>()
    };
    let near = format!("nearest {{ field: .embedding, query: {written}, metric: \"cosine\", k: 10 }}.select {{ id }}");
    let nearest = time("nearest 10 of all", format!("docs.{near}"));
    time("nearest 10 of the rare tenth", format!("docs.where {{ .kind == \"rare\" }}.{near}"));
    let sorted = time(
        "sort by similarity, limit 10",
        format!("docs.sort {{ similarity(.embedding, {written}) desc }}.limit(10).select {{ id }}"),
    );
    assert_eq!(nearest, sorted, "a search and a sort by similarity agree");
    time("for scale, every id", "docs.select { id }".to_string());

    let mut times: Vec<f64> = (0..5)
        .map(|_| {
            let started = Instant::now();
            assert_eq!(owl_db::vector::nearest(&flat, &query, owl_db::vector::Metric::Cosine, 10).len(), 10);
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    println!("the comparison alone, over packed numbers: {:.2} ms", times[2]);
}
