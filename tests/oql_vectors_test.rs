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
        // And every `$k` is three.
        params.insert("k".to_string(), Value::Integer(3));
        params.insert("none".to_string(), Value::Integer(0));
        let bound = match query.contains('$') {
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
    use owl_db::executor::{last_vector_index_search, vector_column_searches};
    let db = library();
    let near = |k: usize, metric: &str| {
        format!(r#"nearest {{ field: .embedding, query: $q, metric: "{metric}", k: {k} }}.select {{ id, score: ._score }}"#)
    };
    // The same search two ways: of the whole collection, which is
    // answered from the vectors where they are kept packed, and after a
    // filter that every row passes, which reads each row. The field's
    // index is declared for the cosine, so a search by the cosine is
    // answered from the index and one by another metric from the column.
    let both = |db: &Db, k: usize, metric: &str| {
        let before = vector_column_searches();
        last_vector_index_search();
        let packed = db.rows(&format!("docs.{}", near(k, metric)));
        let from_index = usize::from(last_vector_index_search().is_some());
        let from_column = (vector_column_searches() - before) as usize + from_index;
        assert_eq!(from_index, usize::from(metric == "cosine" && from_column == 1), "{metric}");
        let by_row = db.rows(&format!(r#"docs.where {{ .id != "" }}.{}"#, near(k, metric)));
        assert_eq!(vector_column_searches() - before, (from_column - from_index) as u64, "a filtered search reads rows");
        assert!(last_vector_index_search().is_none(), "a filtered search reads rows");
        let shown = |rows: &[Document]| -> Vec<String> {
            rows.iter().map(|row| format!("{:?} {:?}", row.get_field("id"), row.get_field("score"))).collect()
        };
        assert_eq!(shown(&packed), shown(&by_row), "k {k}, {metric}");
        (packed.len(), from_column)
    };
    // The store makes a collection's columns once it has been asked twice.
    both(&db, 2, "l2");
    for metric in ["cosine", "l2", "dot"] {
        for k in [1, 2, 3, 4, 99] {
            let (found, from_column) = both(&db, k, metric);
            // d5 has no embedding, so there are four to find.
            assert_eq!(found, k.min(4));
            assert_eq!(from_column, 1, "k {k}, {metric}: answered without reading every row");
        }
    }

    // What is written since is seen: a new nearest row, a row that loses
    // its embedding, a row deleted, and a row whose embedding changes.
    db.rows(r#"docs.insert { id: "d0", owner: "a", kind: "x", embedding: [1.0, 0.0, 0.0] }"#);
    db.rows(r#"docs.where { .id == "d2" }.update { embedding: null }"#);
    db.rows(r#"docs.where { .id == "d4" }.delete()"#);
    db.rows(r#"docs.where { .id == "d3" }.update { embedding: [0.99, 0.01, 0.0] }"#);
    for metric in ["cosine", "l2", "l2", "dot", "dot"] {
        both(&db, 3, metric);
    }
    for metric in ["cosine", "l2", "dot"] {
        assert_eq!(both(&db, 3, metric).1, 1, "{metric}");
    }
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
/// to keep and to search. They are in 300 clumps, as the embeddings of
/// documents on a few hundred subjects are. Run with
/// `--release -- --ignored --nocapture`.
#[test]
#[ignore]
fn what_twenty_thousand_vectors_cost() {
    use owl_db::executor::last_vector_index_search;
    use std::time::Instant;
    const ROWS: usize = 20_000;
    const NUMBERS: usize = 384;
    const CLUMPS: u64 = 300;
    let directory = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 4 << 30).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    let schema = format!(
        "schema docs {{ id: String, kind: String, embedding: Vector({NUMBERS}), index(.embedding, metric: \"cosine\") }}"
    );
    let report = execute_oql_file(&parse_oql_file(&schema).unwrap(), &executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let vector_of = |row: u64| -> Vec<f32> {
        let clump = row % CLUMPS;
        (0..NUMBERS as u64)
            .map(|at| unit(clump * 100_000 + at) + 0.3 * unit(1_000_000_000 + row * 1000 + at))
            .collect()
    };
    let row_of = |row: usize| {
        let mut fields = owl_db::Fields::default();
        fields.insert("id".to_string(), Value::String(format!("d{row:06}")));
        let kind = if row % 10 == 0 { "rare" } else { "common" };
        fields.insert("kind".to_string(), Value::String(kind.to_string()));
        fields.insert(
            "embedding".to_string(),
            Value::List(vector_of(row as u64).iter().map(|number| Value::Float(f64::from(*number))).collect()),
        );
        fields
    };
    let started = Instant::now();
    for chunk in 0..ROWS / 1000 {
        executor.insert_documents("docs", (0..1000).map(|at| row_of(chunk * 1000 + at)).collect()).unwrap();
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

    let ids = |text: &str| -> Vec<String> {
        executor
            .execute_query(&parse_oql(text).unwrap())
            .unwrap()
            .iter()
            .map(|row| row.get_field("id").map(Value::to_string_repr).unwrap_or_default())
            .collect()
    };
    let time = |what: &str, text: &str| {
        let parsed = parse_oql(text).unwrap();
        let mut found = 0;
        let mut times: Vec<f64> = (0..5)
            .map(|_| {
                let started = Instant::now();
                found = executor.execute_query(&parsed).unwrap().len();
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        // The first run may make what later runs find kept.
        let first = times[0];
        times.sort_by(f64::total_cmp);
        println!("{what}: {:.2} ms, {found} rows (first run {first:.1} ms)", times[2]);
    };
    let near = |query: &[f32], metric: &str, options: &str| {
        format!(
            "nearest {{ field: .embedding, query: {}, metric: \"{metric}\", k: 10{options} }}.select {{ id }}",
            written(query)
        )
    };
    let query = vector_of(5_000_000);

    // The index is built by the first search that wants it.
    let started = Instant::now();
    ids(&format!("docs.{}", near(&query, "cosine", "")));
    let did = last_vector_index_search().expect("answered from the index");
    println!(
        "the first search, which builds the index: {:.0} ms, into {} lists",
        started.elapsed().as_secs_f64() * 1000.0,
        did.lists
    );
    time("nearest 10, from the index", &format!("docs.{}", near(&query, "cosine", "")));
    let did = last_vector_index_search().unwrap();
    println!("  it compared {} of {} vectors, in {} of {} lists", did.compared, did.held, did.probed, did.lists);
    time("nearest 10, from the index, exact", &format!("docs.{}", near(&query, "cosine", ", exact: true")));
    time(
        "nearest 10, every row read",
        &format!("docs.where {{ .id != \"\" }}.{}", near(&query, "cosine", "")),
    );
    time("nearest 10 by distance, no index, from the column", &format!("docs.{}", near(&query, "l2", "")));
    time(
        "nearest 10 of the rare tenth, which rows pass read off the column",
        &format!("docs.where {{ .kind == \"rare\" }}.{}", near(&query, "cosine", "")),
    );
    time(
        "nearest 10 of the rare tenth, every row read",
        &format!("docs.where {{ .kind == \"rare\" && .id != \"\" }}.{}", near(&query, "cosine", "")),
    );
    time(
        "sort by similarity, limit 10",
        &format!("docs.sort {{ similarity(.embedding, {}) desc }}.limit(10).select {{ id }}", written(&query)),
    );
    time(
        "sort by similarity, limit 10, of the rare tenth",
        &format!(
            "docs.where {{ .kind == \"rare\" }}.sort {{ similarity(.embedding, {}) desc }}.limit(10).select {{ id }}",
            written(&query)
        ),
    );

    // How much of the truth the index finds when left to itself.
    let (mut right, mut compared) = (0, 0);
    for probe in 0..40u64 {
        let query = vector_of(6_000_000 + probe * 13);
        let truth = ids(&format!("docs.{}", near(&query, "cosine", ", exact: true")));
        let found = ids(&format!("docs.{}", near(&query, "cosine", "")));
        compared += last_vector_index_search().unwrap().compared;
        right += truth.iter().filter(|id| found.contains(id)).count();
    }
    println!(
        "over 40 searches it found {:.3} of the true nearest ten, comparing {:.3} of the vectors",
        right as f64 / 400.0,
        compared as f64 / (40.0 * ROWS as f64)
    );

    // A row written between searches costs the index one vector's work.
    let mut after_write = Vec::new();
    for extra in 0..5 {
        executor.insert_documents("docs", vec![row_of(ROWS + extra)]).unwrap();
        let started = Instant::now();
        ids(&format!("docs.{}", near(&query, "cosine", "")));
        after_write.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    after_write.sort_by(f64::total_cmp);
    println!("nearest 10 straight after a row is written: {:.2} ms", after_write[2]);
    let mut after_write = Vec::new();
    for extra in 5..10 {
        executor.insert_documents("docs", vec![row_of(ROWS + extra)]).unwrap();
        let started = Instant::now();
        ids(&format!("docs.{}", near(&query, "l2", "")));
        after_write.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    after_write.sort_by(f64::total_cmp);
    println!("  the same by distance, with no index: {:.1} ms", after_write[2]);

    // Opened again: the lists are read back, and only the vectors are
    // read from the rows.
    let question = format!("docs.{}", near(&query, "cosine", ""));
    store.checkpoint().unwrap();
    drop(executor);
    drop(store);
    let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 4 << 30).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let divisions = owl_db::storage_engine::vector_index_divisions();
    let started = Instant::now();
    let found = executor.execute_query(&parse_oql(&question).unwrap()).unwrap().len();
    println!(
        "the first search after the database is opened again: {:.0} ms, {found} rows, the centres worked out {} times",
        started.elapsed().as_secs_f64() * 1000.0,
        owl_db::storage_engine::vector_index_divisions() - divisions
    );
}

/// Six thousand vectors of sixteen numbers in sixty clumps: enough for an
/// index to divide into lists, and small enough to compare all of them
/// for the truth.
fn clumps() -> Db {
    let db = Db::open(TempDir::new().unwrap());
    let schema = r#"
        schema owners { id: String, name: String }
        schema docs { id: String, owner: Ref(owners), kind: String, embedding: Optional(Vector(16)), index(.embedding, metric: "cosine") }
    "#;
    let report = execute_oql_file(&parse_oql_file(schema).unwrap(), &db.executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let owners: Vec<owl_db::Fields> = (0..OWNERS)
        .map(|owner| {
            let mut fields = owl_db::Fields::default();
            fields.insert("id".to_string(), Value::String(format!("o{owner:02}")));
            fields.insert("name".to_string(), Value::String(format!("Owner {owner}")));
            fields
        })
        .collect();
    db.executor.insert_documents("owners", owners).unwrap();
    for chunk in 0..6 {
        let rows: Vec<owl_db::Fields> = (0..1000).map(|at| clump_row(chunk * 1000 + at)).collect();
        db.executor.insert_documents("docs", rows).unwrap();
    }
    db
}

/// How many owners the documents of [`clumps`] are shared among.
const OWNERS: usize = 37;

/// SplitMix64 of `seed`, as a number between -1 and 1.
fn unit(seed: u64) -> f32 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
}

/// The vector of row `row`: its clump's centre, with a little of its own.
fn clump_vector(row: usize) -> Vec<f32> {
    let clump = (row % 60) as u64;
    (0..16u64).map(|at| unit(clump * 1000 + at) + 0.2 * unit(1_000_000 + row as u64 * 16 + at)).collect()
}

fn clump_row(row: usize) -> owl_db::Fields {
    let mut fields = owl_db::Fields::default();
    fields.insert("id".to_string(), Value::String(format!("d{row:05}")));
    fields.insert("kind".to_string(), Value::String(if row % 2 == 0 { "even" } else { "odd" }.to_string()));
    fields.insert("owner".to_string(), Value::String(format!("o{:02}", row % OWNERS)));
    fields.insert(
        "embedding".to_string(),
        Value::List(clump_vector(row).iter().map(|number| Value::Float(f64::from(*number))).collect()),
    );
    fields
}

/// A vector as it is written in a query. Plain decimals: OQL has no form
/// for a number with an exponent, which is how a very small one prints.
fn written(vector: &[f32]) -> String {
    format!("[{}]", vector.iter().map(|number| format!("{number:.9}")).collect::<Vec<_>>().join(", "))
}

#[test]
fn an_index_finds_the_nearest_among_a_fraction_and_exactly_when_asked() {
    use owl_db::executor::last_vector_index_search;
    let db = clumps();
    let ids = |query: &str| db.column(query, "id");
    let (mut right, mut wanted, mut compared) = (0usize, 0usize, 0usize);
    for probe in 0..25usize {
        // A vector near the middle of a clump, not one that is stored.
        let query = written(&clump_vector(100_000 + probe * 7));
        let near = |options: &str| {
            format!(r#"nearest {{ field: .embedding, query: {query}, metric: "cosine", k: 10{options} }}.select {{ id, score: ._score }}"#)
        };
        // The truth: every row read and compared.
        last_vector_index_search();
        let truth = ids(&format!(r#"docs.where {{ .id != "" }}.{}"#, near("")));
        assert!(last_vector_index_search().is_none());

        // Asked to be exact, the index gives the same rows in the same
        // order, having compared every vector and read ten rows.
        let exact = db.rows(&format!("docs.{}", near(", exact: true")));
        let did = last_vector_index_search().expect("answered from the index");
        assert_eq!(did.held, 6000);
        assert_eq!((did.compared, did.probed), (6000, did.lists));
        assert!(did.lists > 1, "six thousand vectors are divided into lists");
        let exact_ids: Vec<String> = exact.iter().map(|row| row.get_field("id").unwrap().to_string_repr()).collect();
        assert_eq!(exact_ids, truth);

        // Left to itself it compares an eighth, or a little over.
        let found = ids(&format!("docs.{}", near("")));
        let did = last_vector_index_search().expect("answered from the index");
        assert!(did.compared < 6000 / 4, "compared {} of 6000", did.compared);
        assert!(did.probed < did.lists);
        right += truth.iter().filter(|id| found.contains(id)).count();
        wanted += truth.len();
        compared += did.compared;

        // Told how many to compare, it compares about that many.
        ids(&format!("docs.{}", near(", num_candidates: 200")));
        let did = last_vector_index_search().unwrap();
        assert!((200..600).contains(&did.compared), "compared {}", did.compared);
    }
    let recall = right as f64 / wanted as f64;
    assert!(recall >= 0.97, "found {recall:.3} of the true nearest, comparing {} of 6000 a search", compared / 25);
}

#[test]
fn an_index_is_kept_in_step_with_what_is_written() {
    use owl_db::executor::last_vector_index_search;
    let db = clumps();
    let query = written(&clump_vector(100_003));
    let exact = format!(r#"nearest {{ field: .embedding, query: {query}, metric: "cosine", k: 12, exact: true }}.select {{ id }}"#);
    let same = |db: &Db, held: usize| {
        last_vector_index_search();
        let by_row = db.column(&format!(r#"docs.where {{ .id != "" }}.{exact}"#), "id");
        let by_index = db.column(&format!("docs.{exact}"), "id");
        let did = last_vector_index_search().expect("answered from the index");
        assert_eq!(did.held, held, "vectors the index holds");
        assert_eq!(by_index, by_row);
        by_index
    };
    let before = same(&db, 6000);

    // The nearest row is deleted, the next loses its embedding, the third
    // is moved far away, and a row is added at the very spot asked about.
    db.rows(&format!(r#"docs.where {{ .id == "{}" }}.delete()"#, before[0]));
    db.rows(&format!(r#"docs.where {{ .id == "{}" }}.update {{ embedding: null }}"#, before[1]));
    db.rows(&format!(r#"docs.where {{ .id == "{}" }}.update {{ embedding: .embedding * -1 }}"#, before[2]));
    db.rows(&format!(r#"docs.insert {{ id: "new", owner: "o00", kind: "even", embedding: {query} }}"#));
    let after = same(&db, 5999);
    assert_eq!(after[0], "new");
    for gone in &before[..3] {
        assert!(!after.contains(gone), "{gone} is no longer among the nearest");
    }

    // Many more rows than there were, which divides the index again.
    for chunk in 6..13 {
        let rows: Vec<owl_db::Fields> = (0..1000).map(|at| clump_row(chunk * 1000 + at)).collect();
        db.executor.insert_documents("docs", rows).unwrap();
    }
    same(&db, 12_999);

    // Inside a transaction that has written, every row is read: the index
    // knows nothing of what is not committed. Rolled back, nothing changed.
    let transaction = db.store.begin_transaction().unwrap();
    {
        let _bound = db.store.bind_transaction(transaction).unwrap();
        db.rows(r#"docs.where { .id == "new" }.delete()"#);
        last_vector_index_search();
        let inside = db.column(&format!("docs.{exact}"), "id");
        assert!(last_vector_index_search().is_none(), "read row by row");
        assert_ne!(inside[0], "new");
    }
    db.store.rollback_transaction(transaction).unwrap();
    assert_eq!(same(&db, 12_999)[0], "new");

    // Opened afresh, the index is built again from the rows.
    let db = db.reopened();
    assert_eq!(same(&db, 12_999)[0], "new");
    // The index is the field's, for the metric it was declared with: by
    // straight-line distance every vector is compared another way.
    last_vector_index_search();
    db.rows(&format!(r#"docs.nearest {{ field: .embedding, query: {query}, metric: "l2", k: 3 }}"#));
    assert!(last_vector_index_search().is_none());
}

/// The rows of a query as text: each row's fields in name order.
fn shown(rows: &[Document]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let mut fields: Vec<String> = row
                .fields
                .iter()
                .filter(|(name, _)| !name.starts_with('_'))
                .map(|(name, value)| format!("{name}={value:?}"))
                .collect();
            fields.sort();
            fields.join(" ")
        })
        .collect()
}

#[test]
fn a_search_after_a_filter_ranks_only_the_rows_that_pass_and_reads_only_the_rows_found() {
    use owl_db::executor::last_vector_index_search;
    let db = clumps();
    let query = written(&clump_vector(100_011));
    let near = |k: usize| {
        format!(r#"nearest {{ field: .embedding, query: {query}, metric: "cosine", k: {k} }}.select {{ id, kind, score: ._score }}"#)
    };
    // The same filter two ways. Written plainly, which rows pass is read
    // off the column and the index ranks exactly those. With a condition
    // on the id added, which every row passes, each row is read.
    let both = |filter: &str, k: usize| -> (Vec<String>, Option<usize>) {
        last_vector_index_search();
        let by_row = shown(&db.rows(&format!(r#"docs.where {{ {filter} && .id != "" }}.{}"#, near(k))));
        assert!(last_vector_index_search().is_none(), "read row by row");
        let found = shown(&db.rows(&format!("docs.where {{ {filter} }}.{}", near(k))));
        assert_eq!(found, by_row, "{filter}, k {k}");
        (found, last_vector_index_search().map(|did| did.compared))
    };
    // The store makes a collection's columns once it has been asked twice.
    both(r#".kind == "even""#, 10);
    let (found, compared) = both(r#".kind == "even""#, 10);
    assert_eq!(found.len(), 10);
    assert!(found.iter().all(|row| row.contains(r#"kind=String("even")"#)));
    assert_eq!(compared, Some(3000), "the three thousand even rows, and no other");

    let (_, compared) = both(r#".kind != "even""#, 7);
    assert_eq!(compared, Some(3000));
    // Two filters, one after the other.
    last_vector_index_search();
    let chained = shown(&db.rows(&format!(r#"docs.where {{ .kind == "odd" }}.where {{ .owner == "o05" }}.{}"#, near(4))));
    let did = last_vector_index_search();
    let by_row = shown(&db.rows(&format!(r#"docs.where {{ .kind == "odd" && .owner == "o05" && .id != "" }}.{}"#, near(4))));
    assert_eq!(chained, by_row);
    assert_eq!(chained.len(), 4);
    // A reference compared with an id is not read off a column; whichever
    // way it was answered, the rows are right.
    let _ = did;

    // Fewer rows pass than are asked for; and none.
    db.rows(r#"docs.where { .id in ["d00000", "d00002", "d00004"] }.update { kind: "rare" }"#);
    both(r#".kind == "rare""#, 10);
    let (found, compared) = both(r#".kind == "rare""#, 10);
    assert_eq!((found.len(), compared), (3, Some(3)));
    let (found, _) = both(r#".kind == "nothing""#, 10);
    assert!(found.is_empty());
    // A row that passes the filter and has no embedding is not found.
    db.rows(r#"docs.where { .id == "d00002" }.update { embedding: null }"#);
    both(r#".kind == "rare""#, 10);
    assert_eq!(both(r#".kind == "rare""#, 10).0.len(), 2);
}

#[test]
fn a_search_through_a_relationship_ranks_only_the_rows_that_refer_to_the_row() {
    use owl_db::executor::last_vector_index_search;
    let db = clumps();
    let query = written(&clump_vector(100_021));
    let near = format!(r#"nearest {{ field: .embedding, query: {query}, metric: "cosine", k: 3 }}"#);
    let per_owner = format!("owners.select {{ id, near: .docs.{near}.select {{ id, score: ._score }} }}");
    let nearest_one = format!("owners.select {{ id, best: .docs.{near}.first().id, far: .docs.{near}.where {{ ._score < 0.5 }}.count() }}");
    let captured = format!("owners {{ d in .docs.{near}  select {{ id, doc: d.id }} }}");

    last_vector_index_search();
    let found = shown(&db.rows(&per_owner));
    let did = last_vector_index_search().expect("each owner's documents ranked from the index");
    // Six thousand documents among thirty-seven owners.
    assert!((150..=170).contains(&did.compared), "compared {} for one owner", did.compared);
    assert_eq!(found.len(), OWNERS);
    let first = shown(&db.rows(&nearest_one));
    let pairs = shown(&db.rows(&captured));
    assert_eq!(pairs.len(), OWNERS * 3);

    // The truth: inside a transaction that has written, the index is not
    // asked and every related row is read.
    let transaction = db.store.begin_transaction().unwrap();
    {
        let _bound = db.store.bind_transaction(transaction).unwrap();
        db.rows(r#"owners.insert { id: "zz", name: "Nobody" }"#);
        last_vector_index_search();
        let by_row = shown(&db.rows(&per_owner));
        assert!(last_vector_index_search().is_none(), "read row by row");
        assert_eq!(by_row[..OWNERS], found[..]);
        // An owner with no documents has none near.
        assert!(by_row[OWNERS].contains("near=List([])"), "{}", by_row[OWNERS]);
        assert_eq!(shown(&db.rows(&nearest_one))[..OWNERS], first[..]);
        assert_eq!(shown(&db.rows(&captured)), pairs);
    }
    db.store.rollback_transaction(transaction).unwrap();

    // One owner's documents by the search of the whole collection,
    // filtered: the same three.
    let one = db.rows(&format!(r#"owners.where {{ .id == "o07" }}.select {{ near: .docs.{near}.select {{ id }} }}"#));
    let filtered = db.column(&format!(r#"docs.where {{ .owner == "o07" && .id != "" }}.{near}.select {{ id }}"#), "id");
    let Some(Value::List(near_o07)) = one[0].get_field("near") else {
        panic!("{:?}", one[0].fields);
    };
    let near_o07: Vec<String> = near_o07
        .iter()
        .map(|row| match row {
            Value::Object(fields) => fields["id"].to_string_repr(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(near_o07, filtered);
}

#[test]
fn how_many_rows_to_find_may_be_a_parameter_or_worked_out() {
    let db = library();
    let near = |k: &str| format!(r#"docs.nearest {{ field: .embedding, query: $q, metric: "cosine", k: {k} }}.select {{ id }}"#);
    let three = db.column(&near("3"), "id");
    assert_eq!(three, ["d1", "d2", "d4"]);
    assert_eq!(db.column(&near("$k"), "id"), three);
    assert_eq!(db.column(&near("$k - 1"), "id"), three[..2]);
    assert_eq!(db.column(&near("1 + 2"), "id"), three);
    // Through a relationship, and after a filter, the same.
    let per_owner = db.rows(r#"owners.select { name, n: .docs.nearest { field: .embedding, query: $q, k: $k }.count() }"#);
    assert_eq!(per_owner[0].get_field("n"), Some(&Value::Integer(2)));
    assert_eq!(per_owner[1].get_field("n"), Some(&Value::Integer(2)));
    assert_eq!(
        db.column(r#"docs.where { .kind == "y" }.nearest { field: .embedding, query: $q, k: $k - 1 }.select { id }"#, "id"),
        ["d2", "d4"]
    );
    // What is not a number of rows is refused, and says what it was given.
    let none = db.error(&near("$none"));
    assert!(none.contains("a whole number above zero, and it was given 0"), "{none}");
    let text = db.error(&near(r#"upper("x")"#));
    assert!(text.contains("it was given X"), "{text}");
    assert!(db.error(&near("0")).contains("nearest.k must be a positive integer"));
    let unbound = db.error(&near("$missing"));
    assert!(unbound.contains("$missing has no value"), "{unbound}");
}

#[test]
fn a_sort_by_how_near_cut_to_a_few_is_answered_as_a_search() {
    use owl_db::executor::{last_vector_index_search, vector_column_searches};
    let db = clumps();
    let query = written(&clump_vector(100_031));
    let nearest = |filter: &str, metric: &str, k: usize| {
        format!(r#"docs{filter}.nearest {{ field: .embedding, query: {query}, metric: "{metric}", k: {k}, exact: true }}.select {{ id }}"#)
    };
    let sorted = |filter: &str, key: &str, k: usize| format!("docs{filter}.sort {{ {key} }}.limit({k}).select {{ id }}");

    // By similarity: the field's index, asked to be exact, as a sort is.
    last_vector_index_search();
    let found = db.column(&sorted("", &format!("similarity(.embedding, {query}) desc"), 10), "id");
    let did = last_vector_index_search().expect("answered from the index");
    assert_eq!(did.compared, 6000, "every vector compared: a sort is exact");
    assert_eq!(found, db.column(&nearest("", "cosine", 10), "id"));
    // The row's id as the key after, the vector written first, and the
    // method form are the same question.
    for key in [
        format!("similarity(.embedding, {query}) desc, .id"),
        format!("similarity({query}, .embedding) desc"),
        format!(".embedding.similarity({query}) desc"),
    ] {
        last_vector_index_search();
        assert_eq!(db.column(&sorted("", &key, 10), "id"), found, "{key}");
        assert!(last_vector_index_search().is_some(), "{key}");
    }
    // What a sort gives are the rows as they are: no score with them.
    let rows = db.rows(&format!("docs.sort {{ similarity(.embedding, {query}) desc }}.limit(2)"));
    assert!(rows.iter().all(|row| row.get_field("_score").is_none() && row.get_field("embedding").is_some()));

    // After a filter the column says which rows pass. Its columns are
    // made once it has been asked twice.
    let even = r#".where { .kind == "even" }"#;
    db.rows(&sorted(even, &format!("similarity(.embedding, {query}) desc"), 10));
    last_vector_index_search();
    let filtered = db.column(&sorted(even, &format!("similarity(.embedding, {query}) desc"), 10), "id");
    assert_eq!(last_vector_index_search().map(|did| did.compared), Some(3000));
    assert_eq!(filtered, db.column(&nearest(even, "cosine", 10), "id"));

    // By distance, nearest first: no index is declared for it, so every
    // vector is compared where they are kept packed.
    let by_distance = sorted("", &format!("distance(.embedding, {query})"), 10);
    db.rows(&by_distance);
    let before = vector_column_searches();
    let near = db.column(&by_distance, "id");
    assert_eq!(vector_column_searches() - before, 1);
    assert_eq!(near, db.column(&nearest("", "l2", 10), "id"));

    // What is not that question is sorted as any sort is: farthest first,
    // a key that reads the row twice, an offset before the limit.
    last_vector_index_search();
    let farthest = db.column(&sorted("", &format!("similarity(.embedding, {query})"), 3), "id");
    assert!(last_vector_index_search().is_none());
    assert!(farthest.iter().all(|id| !found.contains(id)));
    db.rows("docs.sort { similarity(.embedding, .embedding) desc }.limit(3)");
    assert!(last_vector_index_search().is_none());
    let paged = db.column(&format!("docs.sort {{ similarity(.embedding, {query}) desc }}.offset(2).limit(3).select {{ id }}"), "id");
    assert_eq!(paged, found[2..5]);

    // Fewer rows have a vector than are asked for: the sort goes on to
    // the rows with none, which a search would not give.
    let small = library();
    assert_eq!(
        small.column("docs.sort { similarity(.embedding, $q) desc }.limit(5).select { id }", "id"),
        ["d1", "d2", "d4", "d3", "d5"]
    );
    assert_eq!(
        small.column("docs.sort { similarity(.embedding, $q) desc }.limit(3).select { id }", "id"),
        ["d1", "d2", "d4"]
    );
}

#[test]
fn a_vector_s_numbers_read_as_they_were_written() {
    let db = library();
    let d2 = |expression: &str| db.value(&format!(r#"docs.where {{ .id == "d2" }}.select {{ v: {expression} }}"#));
    // d2 is (0.9, 0.1, 0). Kept in 32 bits, 0.9 is 0.89999997615814208984375;
    // read back it is 0.9, not 0.8999999761581421.
    assert_eq!(d2(".embedding[0]"), Value::Float(0.9));
    assert_eq!(d2(".embedding[1]"), Value::Float(0.1));
    assert_eq!(
        d2(".embedding[0:2]"),
        Value::List(vec![Value::Float(0.9), Value::Float(0.1)])
    );
    assert_eq!(vector(&[0.9, 0.1, 0.0]).to_clean_json(), serde_json::json!([0.9, 0.1, 0.0]));
    assert_eq!(vector(&[0.6, 0.6]).to_string_repr(), "[0.6, 0.6]");
    // So a number of a vector equals the number written, and is found.
    assert_eq!(db.column("docs.where { .embedding[0] == 0.9 }.select { id }", "id"), ["d2"]);
    assert_eq!(db.column("docs.where { .embedding contains 0.9 }.select { id }", "id"), ["d2"]);
    assert_eq!(db.column("docs.where { .embedding contains 0.8 }.select { id }", "id"), ["d4"]);
    // What is worked out from vectors is given at the width it was worked
    // out at: a score, a length, a dot product.
    assert_eq!(d2("dot(.embedding, $q)"), Value::Float(0.9));
    assert_eq!(d2("(.embedding * 2)[0]"), Value::Float(1.8));
    let scores = db.rows(r#"docs.nearest { field: .embedding, query: $q, metric: "cosine", k: 2 }.select { id, score: ._score, same: similarity(.embedding, $q) }"#);
    assert_eq!(scores[0].get_field("score"), Some(&Value::Float(1.0)));
    assert_eq!(scores[1].get_field("score"), scores[1].get_field("same"));
    let Some(Value::Float(score)) = scores[1].get_field("score") else {
        panic!("{:?}", scores[1].fields);
    };
    assert!(score.to_string().len() <= 10, "{score} is written in the digits a 32-bit number has");
    // Narrowed again it is the number it was: a vector read and written
    // back is unchanged.
    db.rows("docs.update { embedding: vector(.embedding[0:3]) }");
    assert_eq!(db.value(r#"docs.where { .id == "d2" }.select { embedding }"#), vector(&[0.9, 0.1, 0.0]));
}

#[test]
fn a_number_may_be_written_with_an_exponent() {
    let db = library();
    let one = |expression: &str| db.value(&format!("docs.limit(1).select {{ v: {expression} }}"));
    assert_eq!(one("1.6e-5"), Value::Float(0.000016));
    assert_eq!(one("2E3"), Value::Float(2000.0));
    assert_eq!(one("1e+2"), Value::Float(100.0));
    assert_eq!(one("1.5e2 + 1"), Value::Float(151.0));
    assert_eq!(one("-2.5e-1"), Value::Float(-0.25));
    // In a vector, which is where a very small number is most often met.
    assert_eq!(
        db.column(r#"docs.nearest { field: .embedding, query: [1.0, 1e-9, 0.0e0], metric: "cosine", k: 2 }.select { id }"#, "id"),
        ["d1", "d2"]
    );
    assert_eq!(db.column("docs.where { .embedding[0] >= 9e-1 }.select { id }", "id"), ["d1", "d2"]);
    // An `e` with no digit after it is not an exponent: a name that
    // begins with one is still a name, and a number is still a number.
    assert_eq!(db.value("docs.limit(1).select { e5: 7 }"), Value::Integer(7));
    assert_eq!(one("2"), Value::Integer(2));
    assert_eq!(one("2.5"), Value::Float(2.5));
    assert_eq!(db.column("docs.limit(2).select { id }", "id").len(), 2);
    // A number too large to hold is refused, and the refusal says so.
    let large = db.error("docs.limit(1).select { v: 1e999 }");
    assert!(large.contains("1e999 is too large a number to hold"), "{large}");
    assert!(db.error("docs.limit(1)\n.select { v: 2.5E400 }").contains("line 2: 2.5E400 is too large"));
}

#[test]
fn an_index_s_lists_are_kept_across_a_reopen_and_only_as_a_hint() {
    use owl_db::executor::last_vector_index_search;
    use owl_db::storage_engine::vector_index_divisions;
    let db = clumps();
    let query = written(&clump_vector(100_041));
    let exact = format!(r#"nearest {{ field: .embedding, query: {query}, metric: "cosine", k: 10, exact: true }}.select {{ id }}"#);
    // The index against a search that reads every row, and what it did.
    let same = |db: &Db| {
        last_vector_index_search();
        let by_row = db.column(&format!(r#"docs.where {{ .id != "" }}.{exact}"#), "id");
        let by_index = db.column(&format!("docs.{exact}"), "id");
        assert_eq!(by_index, by_row);
        last_vector_index_search().expect("answered from the index")
    };
    let kept = db.directory.path().join("db.owl.vectors.docs.embedding");

    // The first search divides the index into lists and keeps them.
    let before = vector_index_divisions();
    let first = same(&db);
    assert!(first.lists > 1);
    assert_eq!(vector_index_divisions() - before, 1);
    assert!(kept.exists(), "the lists are kept beside the database");

    // Rows written since: some gone, some changed, some new.
    db.rows(r#"docs.where { .id in ["d00001", "d00002", "d00003"] }.delete()"#);
    db.rows(r#"docs.where { .id == "d00010" }.update { embedding: .embedding * -1 }"#);
    db.executor.insert_documents("docs", (6000..6200).map(clump_row).collect()).unwrap();
    same(&db);

    // Opened again, the lists are read back, not worked out again, and
    // the rows written since the lists were kept are all found.
    let db = db.reopened();
    let before = vector_index_divisions();
    let again = same(&db);
    assert_eq!(vector_index_divisions() - before, 0, "the centres were not worked out again");
    assert_eq!(again.lists, first.lists);
    assert_eq!(again.held, 6000 - 3 + 200);
    // Left to itself it still compares a fraction.
    db.rows(&format!(r#"docs.nearest {{ field: .embedding, query: {query}, metric: "cosine", k: 10 }}"#));
    let did = last_vector_index_search().unwrap();
    assert!(did.probed < did.lists && did.compared < did.held / 4);

    // A damaged file is as if there were none: the index is made afresh,
    // and answers the same.
    let mut bytes = std::fs::read(&kept).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    std::fs::write(&kept, &bytes).unwrap();
    let db = db.reopened();
    let before = vector_index_divisions();
    same(&db);
    assert_eq!(vector_index_divisions() - before, 1, "worked out afresh");

    // So is a missing one, and one kept for vectors that have all changed.
    std::fs::remove_file(&kept).unwrap();
    let db = db.reopened();
    let before = vector_index_divisions();
    same(&db);
    assert_eq!(vector_index_divisions() - before, 1);
    db.rows("docs.update { embedding: [.embedding[15], .embedding[14], .embedding[13], .embedding[12], .embedding[11], .embedding[10], .embedding[9], .embedding[8], .embedding[7], .embedding[6], .embedding[5], .embedding[4], .embedding[3], .embedding[2], .embedding[1], .embedding[0]] }");
    let db = db.reopened();
    let before = vector_index_divisions();
    same(&db);
    assert_eq!(vector_index_divisions() - before, 1, "the kept lists no longer suit the rows");
}
