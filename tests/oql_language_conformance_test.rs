//! Executable conformance checks for the parts of the OQL contract that are
//! about least surprise: the same text means the same thing wherever it
//! appears and however the engine chooses to run it.

use owl_db::storage::{DocumentStore, RustStore};
use owl_db::{
    bind_parameters, execute_oql_file, parse_oql, parse_oql_file, parse_oql_statement, Document,
    OQLFileStatement, QueryExecutor, Value,
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

struct Db {
    _directory: TempDir,
    executor: QueryExecutor<RustStore>,
}

impl Db {
    fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
        Self {
            _directory: directory,
            executor: QueryExecutor::new_with_real_clock(store),
        }
    }

    /// Run a script and require every statement to succeed.
    fn script(&self, source: &str) {
        let file = parse_oql_file(source).unwrap_or_else(|error| panic!("{error}\n{source}"));
        let report = execute_oql_file(&file, &self.executor).unwrap();
        assert!(report.errors.is_empty(), "{:?}\n{source}", report.errors);
    }

    fn define(&mut self, source: &str) {
        let owl_db::parser::OQLStatement::FunctionDef(definition) =
            parse_oql_statement(source).unwrap_or_else(|error| panic!("{error}\n{source}"))
        else {
            panic!("not a function definition: {source}");
        };
        self.executor.register_user_function(definition).unwrap();
    }

    fn try_rows(&self, query: &str) -> Result<Vec<Document>, String> {
        let query = parse_oql(query).map_err(|error| error.to_string())?;
        self.executor
            .execute_query(&query)
            .map_err(|error| error.to_string())
    }

    fn rows(&self, query: &str) -> Vec<Document> {
        self.try_rows(query)
            .unwrap_or_else(|error| panic!("{error}\n{query}"))
    }

    /// The `id` field of each result row, in result order.
    fn ids(&self, query: &str) -> Vec<String> {
        self.rows(query)
            .iter()
            .map(|row| match row.get_field("id") {
                Some(Value::String(id)) => id.clone(),
                other => panic!("row has no string id: {other:?}"),
            })
            .collect()
    }

    /// The single value produced by an aggregate or one-row projection.
    fn scalar(&self, query: &str, field: &str) -> Value {
        let rows = self.rows(query);
        assert_eq!(rows.len(), 1, "expected one row from {query}");
        rows[0].get_field(field).cloned().unwrap_or(Value::Null)
    }
}

fn text(value: &str) -> Value {
    Value::String(value.to_string())
}

// ---------------------------------------------------------------------------
// Lexical surface
// ---------------------------------------------------------------------------

#[test]
fn negative_numbers_are_ordinary_literals() {
    let db = Db::new();
    db.script(
        r#"
        readings.insert [
          { id: "cold", celsius: -12, drift: -0.5, samples: [-1, 0, 1] },
          { id: "warm", celsius: 21, drift: 0.25, samples: [2, 3] }
        ]
        "#,
    );

    assert_eq!(
        db.ids("readings.where { .celsius > -20 && .celsius < -5 }"),
        ["cold"]
    );
    assert_eq!(db.ids("readings.where { .celsius == -12 }"), ["cold"]);
    assert_eq!(db.ids("readings.where { .drift <= -0.5 }"), ["cold"]);
    assert_eq!(db.ids("readings.where { -1 in .samples }"), ["cold"]);
    assert_eq!(db.ids("readings.where { .celsius - -1 == 22 }"), ["warm"]);
    assert_eq!(
        db.scalar(
            r#"readings.where { .id == "warm" }.select { below: -.celsius }"#,
            "below"
        ),
        Value::Integer(-21)
    );
}

#[test]
fn strings_support_escapes_and_reject_unknown_ones() {
    let db = Db::new();
    db.script(
        r#"
        notes.insert { id: "n1", body: "she said \"hi\"\n\ttab \\ slash \u{1F989}" }
        "#,
    );

    assert_eq!(
        db.scalar("notes.select { body }", "body"),
        text("she said \"hi\"\n\ttab \\ slash \u{1F989}")
    );
    assert_eq!(db.ids(r#"notes.where { .body contains "\"hi\"" }"#), ["n1"]);

    let error = parse_oql(r#"notes.where { .body == "bad \q escape" }"#).unwrap_err();
    assert!(error.to_string().contains("escape"), "{error}");
    let error = parse_oql(r#"notes.where { .body == "never closed }"#).unwrap_err();
    assert!(error.to_string().contains("unterminated"), "{error}");
}

#[test]
fn names_that_start_with_a_literal_word_are_still_names() {
    let db = Db::new();
    db.script(
        r#"
        falsey.insert { id: "f1", true_count: 3, nullable: "x", infinite: 1 }
        "#,
    );

    assert_eq!(db.ids("falsey.where { .true_count == 3 }"), ["f1"]);
    assert_eq!(
        db.scalar("falsey.select { true_count }", "true_count"),
        Value::Integer(3)
    );
    assert_eq!(db.ids(r#"falsey.where { .nullable == "x" }"#), ["f1"]);
}

#[test]
fn bang_negates_like_not() {
    let db = Db::new();
    db.script(
        r#"
        users.insert [
          { id: "a", active: true },
          { id: "b", active: false }
        ]
        "#,
    );

    assert_eq!(db.ids("users.where { !(.active == true) }"), ["b"]);
    assert_eq!(db.ids("users.where { not (.active == true) }"), ["b"]);
    assert_eq!(db.ids("users.where { !.active }"), ["b"]);
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

#[test]
fn statement_boundaries_ignore_layout_strings_and_comments() {
    let db = Db::new();
    db.script(
        r#"
        schema docs {
          id: String,

          body: String   // a brace in a comment: }
        }

        docs.insert {
          id: "d1",

          body: "braces { } and // slashes stay inside the string"
        }
        docs.insert { id: "d2", body: "second" }   // trailing comment {

        transaction {
          docs.where { .id == "d2" }.update { body: "changed" }

          docs.insert { id: "d3", body: "third" }
        }

        longest =
          docs.where { .body contains "braces" }

        docs
          .where { .id == "d1" }

          // a comment between chained steps
          .select { id }
        "#,
    );

    let mut ids = db.ids("docs.select { id }");
    ids.sort();
    assert_eq!(ids, ["d1", "d2", "d3"]);
    assert_eq!(
        db.scalar(r#"docs.where { .id == "d1" }.select { body }"#, "body"),
        text("braces { } and // slashes stay inside the string")
    );
    assert_eq!(
        db.scalar(r#"docs.where { .id == "d2" }.select { body }"#, "body"),
        text("changed")
    );
}

#[test]
fn a_script_splits_into_the_statements_a_reader_sees() {
    let file = parse_oql_file(
        r#"
        fn is_big(n: Integer) -> Bool
        {
          n > 100
        }
        total = 1 +
          2
        orders.where { is_big(.amount) }
        orders.select { id }
        "#,
    )
    .unwrap();

    let kinds: Vec<&str> = file
        .statements
        .iter()
        .map(|statement| match statement {
            OQLFileStatement::FunctionDef(_) => "fn",
            OQLFileStatement::ValueBinding(_) => "value",
            OQLFileStatement::QueryBinding(_) => "binding",
            OQLFileStatement::Query(_) => "query",
            OQLFileStatement::Schema(_) => "schema",
            OQLFileStatement::Transaction(_) => "transaction",
            OQLFileStatement::Drop(_) => "drop",
        })
        .collect();
    assert_eq!(kinds, ["fn", "value", "query", "query"]);
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[test]
fn comparisons_are_expressions_anywhere_a_value_is_expected() {
    let db = Db::new();
    db.script(
        r#"
        people.insert [
          { id: "kid", age: 9, city: "Lagos" },
          { id: "adult", age: 40, city: "Accra" }
        ]
        "#,
    );

    let rows = db.rows(
        r#"people.sort { .id.asc }.select {
             id,
             adult: .age >= 18,
             local: .city == "Lagos",
             either: .age >= 18 || .city == "Lagos",
             label: if .age >= 18 then "adult" else "minor"
           }"#,
    );
    assert_eq!(rows[0].get_field("adult"), Some(&Value::Boolean(true)));
    assert_eq!(rows[0].get_field("local"), Some(&Value::Boolean(false)));
    assert_eq!(rows[1].get_field("adult"), Some(&Value::Boolean(false)));
    assert_eq!(rows[1].get_field("local"), Some(&Value::Boolean(true)));
    assert_eq!(rows[1].get_field("either"), Some(&Value::Boolean(true)));
    assert_eq!(rows[1].get_field("label"), Some(&text("minor")));
}

#[test]
fn parentheses_group_operands_as_well_as_conditions() {
    let db = Db::new();
    db.script(
        r#"
        boxes.insert [
          { id: "small", w: 1, h: 2, sealed: true },
          { id: "large", w: 4, h: 5, sealed: false }
        ]
        "#,
    );

    assert_eq!(db.ids("boxes.where { (.w + .h) > 5 }"), ["large"]);
    assert_eq!(db.ids("boxes.where { (.w + .h) * 2 == 6 }"), ["small"]);
    assert_eq!(
        db.ids("boxes.where { (.w > 3 || .h < 3) && .sealed == true }"),
        ["small"]
    );
    assert_eq!(db.ids("boxes.where { ((.w)) == 4 }"), ["large"]);
}

#[test]
fn or_yields_the_first_present_value() {
    let db = Db::new();
    db.script(
        r#"
        users.insert [
          { id: "a", nickname: "Ace", name: "Alice" },
          { id: "b", name: "Bob" },
          { id: "c", nickname: "", name: "Cy" }
        ]
        "#,
    );

    let rows = db.rows("users.sort { .id.asc }.select { id, shown: .nickname || .name }");
    assert_eq!(rows[0].get_field("shown"), Some(&text("Ace")));
    assert_eq!(rows[1].get_field("shown"), Some(&text("Bob")));
    assert_eq!(rows[2].get_field("shown"), Some(&text("Cy")));
}

#[test]
fn integer_arithmetic_reports_overflow_instead_of_wrapping() {
    let db = Db::new();
    db.script(r#"counters.insert { id: "c", big: 9223372036854775807, zero: 0 }"#);

    for query in [
        "counters.select { v: .big + 1 }",
        "counters.select { v: .big * 2 }",
        "counters.select { v: (0 - .big - 1) / -1 }",
    ] {
        let error = db.try_rows(query).unwrap_err();
        assert!(error.contains("overflow"), "{query}: {error}");
    }
    assert!(db
        .try_rows("counters.select { v: .big / .zero }")
        .unwrap_err()
        .contains("zero"));
}

#[test]
fn string_functions_count_characters_and_bound_their_output() {
    let db = Db::new();
    db.script(r#"words.insert { id: "w", word: "héllo wörld" }"#);

    assert_eq!(
        db.scalar("words.select { v: substring(.word, 1, 4) }", "v"),
        text("éllo")
    );
    assert_eq!(
        db.scalar("words.select { v: substring(.word, 6) }", "v"),
        text("wörld")
    );
    assert_eq!(
        db.scalar("words.select { v: substring(.word, 2, -1) }", "v"),
        text("")
    );
    assert_eq!(
        db.scalar("words.select { v: substring(.word, 99) }", "v"),
        text("")
    );
    assert_eq!(
        db.scalar(r#"words.select { v: pad_left("é", 3, "*") }"#, "v"),
        text("**é")
    );
    assert!(db
        .try_rows(r#"words.select { v: pad_left("a", 99999999999) }"#)
        .unwrap_err()
        .contains("limit"));
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

#[test]
fn function_parameters_are_bound_by_name_everywhere_in_the_body() {
    let mut db = Db::new();
    db.script(
        r#"
        orders.insert [
          { id: "o1", amount: 50, n: 100000, status: "paid" },
          { id: "o2", amount: 500, n: 0, status: "open" }
        ]
        "#,
    );
    // None of these parameter names match a field holding the same value,
    // so a body that read the row instead of its arguments would be wrong.
    db.define("fn is_big(n: Integer) -> Bool { n > 100 }");
    db.define("fn double(v: Integer) -> Integer { v * 2 }");
    db.define(
        "fn is_huge(amount_in: Integer) -> Bool { is_big(double(amount_in)) && amount_in > 400 }",
    );
    db.define(r#"fn tier(n: Integer) -> String { if n >= 500 then "gold" else "standard" }"#);
    db.define(r#"fn label(s: String) -> String { match s { "paid" -> "done", _ -> "pending" } }"#);

    assert_eq!(db.ids("orders.where { is_big(.amount) }"), ["o2"]);
    assert_eq!(db.ids("orders.where { is_huge(.amount) }"), ["o2"]);
    assert_eq!(db.ids("orders.where { !is_big(.amount) }"), ["o1"]);

    let rows = db.rows(
        "orders.sort { .id.asc }.select { id, tier: tier(.amount), twice: double(.amount), label: label(.status) }",
    );
    assert_eq!(rows[0].get_field("tier"), Some(&text("standard")));
    assert_eq!(rows[0].get_field("twice"), Some(&Value::Integer(100)));
    assert_eq!(rows[0].get_field("label"), Some(&text("done")));
    assert_eq!(rows[1].get_field("tier"), Some(&text("gold")));
    assert_eq!(rows[1].get_field("label"), Some(&text("pending")));
}

#[test]
fn row_parameters_keep_full_path_semantics() {
    let mut db = Db::new();
    db.script(
        r#"
        schema teams { id: String, name: String }
        schema members {
          id: String,
          team: Ref(teams),
          tags: List(String),
          status: String
        }
        teams.insert { id: "t1", name: "Core" }
        members.insert [
          { id: "m1", team: "t1", tags: ["beta", "staff"], status: "active" },
          { id: "m2", team: "t1", tags: [], status: "inactive" }
        ]
        "#,
    );
    db.define(r#"fn is_active(m: members) -> Bool { m.status == "active" }"#);
    db.define("fn has_tag(m: members, tag: String) -> Bool { m.tags contains tag }");
    db.define("fn team_name(m: members) -> String { m.team.name }");
    db.define(r#"fn is_core_staff(m: members) -> Bool { is_active(m) && has_tag(m, "staff") && team_name(m) == "Core" }"#);

    assert_eq!(db.ids("members.where { is_active(*) }"), ["m1"]);
    assert_eq!(db.ids(r#"members.where { has_tag(*, "beta") }"#), ["m1"]);
    assert_eq!(db.ids("members.where { is_core_staff(*) }"), ["m1"]);
    assert_eq!(
        db.scalar(
            r#"members.where { .id == "m2" }.select { team: team_name(*) }"#,
            "team"
        ),
        text("Core")
    );
}

#[test]
fn function_arguments_are_checked_and_recursion_is_bounded() {
    let mut db = Db::new();
    db.script(r#"items.insert { id: "i1", price: 3, name: "bolt" }"#);
    db.define("fn half(x: Float) -> Float { x / 2 }");
    db.define("fn forever(x: Integer) -> Integer { forever(x + 1) }");

    // An integer widens to a declared Float.
    assert_eq!(
        db.scalar("items.select { v: half(.price) }", "v"),
        Value::Float(1.5)
    );
    let error = db.try_rows("items.select { v: half(.name) }").unwrap_err();
    assert!(error.contains("parameter 'x'"), "{error}");
    let error = db.try_rows("items.select { v: half(1, 2) }").unwrap_err();
    assert!(error.contains("expects 1 arguments"), "{error}");
    let error = db.try_rows("items.select { v: forever(0) }").unwrap_err();
    assert!(error.contains("call depth"), "{error}");
    // The failed call must not leave its scope behind.
    assert_eq!(
        db.scalar("items.select { v: half(.price) }", "v"),
        Value::Float(1.5)
    );
}

// ---------------------------------------------------------------------------
// One meaning, whichever way the engine runs the query
// ---------------------------------------------------------------------------

/// Two collections with identical rows. `typed` declares a schema, so simple
/// reads go through the physical pipeline; `loose` has none, so the same
/// reads go through the general executor.
fn twin_collections() -> Db {
    let db = Db::new();
    db.script(
        r#"
        schema typed {
          id: String,
          price: Optional(Float),
          qty: Integer,
          label: Optional(String)
        }
        "#,
    );
    for collection in ["typed", "loose"] {
        db.script(&format!(
            r#"
            {collection}.insert [
              {{ id: "a", price: 10.0, qty: 5, label: "x" }},
              {{ id: "b", price: 9.5, qty: -3, label: "y" }},
              {{ id: "c", qty: 0 }},
              {{ id: "d", price: 10.5, qty: 2, label: "x" }},
              {{ id: "e", price: -1.0, qty: 7 }}
            ]
            "#
        ));
    }
    db
}

#[test]
fn the_physical_pipeline_and_the_general_executor_agree() {
    let db = twin_collections();

    // Guard the premise: the schema-backed read really takes the planned
    // path and the schemaless one does not.
    let planned = db
        .executor
        .execute_query_with_plan(&parse_oql("typed.where { .qty > 1 }").unwrap())
        .unwrap();
    assert!(planned.physical_plan.is_some());
    let general = db
        .executor
        .execute_query_with_plan(&parse_oql("loose.where { .qty > 1 }").unwrap())
        .unwrap();
    assert!(general.physical_plan.is_none());

    let predicates = [
        // integer literal against a float field
        ".price >= 10",
        ".price == 10",
        ".price <= 10",
        ".price > 10",
        ".price < 10",
        // float literal against an integer field
        ".qty == 5.0",
        ".qty > 2.5",
        ".qty <= 2.5",
        ".qty == 2.5",
        // negatives and combinations
        ".qty > -4",
        ".price < 0",
        ".price > 9 && .qty > 0",
        ".price == 10 || .qty == -3",
        // rows with no price must not match an ordered comparison
        ".price < 1000",
        r#".label == "x""#,
        r#".label >= "x""#,
    ];
    for predicate in predicates {
        let mut typed = db.ids(&format!("typed.where {{ {predicate} }}"));
        let mut loose = db.ids(&format!("loose.where {{ {predicate} }}"));
        typed.sort();
        loose.sort();
        assert_eq!(typed, loose, "rows differ for {predicate}");

        // A trailing aggregate moves the typed query onto the general
        // executor; the count must match the rows the pipeline returned.
        for collection in ["typed", "loose"] {
            assert_eq!(
                db.scalar(
                    &format!("{collection}.where {{ {predicate} }}.count()"),
                    "count"
                ),
                Value::Integer(typed.len() as i64),
                "count differs for {collection} {predicate}"
            );
        }
    }

    // Spot-check the answers themselves, not only their agreement.
    let mut at_least_ten = db.ids("typed.where { .price >= 10 }");
    at_least_ten.sort();
    assert_eq!(at_least_ten, ["a", "d"]);
    assert_eq!(db.ids("typed.where { .price == 10 }"), ["a"]);
    assert_eq!(db.ids("typed.where { .qty == 5.0 }"), ["a"]);
    let mut priced = db.ids("typed.where { .price < 1000 }");
    priced.sort();
    assert_eq!(priced, ["a", "b", "d", "e"]);
}

#[test]
fn operations_apply_in_the_order_they_are_written() {
    let db = twin_collections();

    for collection in ["typed", "loose"] {
        // Filter the first two rows; do not return two matching rows.
        assert_eq!(
            db.ids(&format!(
                "{collection}.sort {{ .id.asc }}.limit(2).where {{ .qty > 0 }}"
            )),
            ["a"],
            "{collection}"
        );
        assert_eq!(
            db.ids(&format!(
                "{collection}.sort {{ .id.asc }}.offset(3).where {{ .qty > 0 }}"
            )),
            ["d", "e"],
            "{collection}"
        );
        assert_eq!(
            db.ids(&format!(
                "{collection}.where {{ .qty > 0 }}.sort {{ .id.asc }}.limit(2)"
            )),
            ["a", "d"],
            "{collection}"
        );
    }
}

#[test]
fn absent_values_never_fail_a_filter_or_a_sort() {
    let db = twin_collections();

    for collection in ["typed", "loose"] {
        // Row "c" has no price. The guard is unnecessary but must be legal,
        // and the unguarded form must behave the same.
        let mut guarded = db.ids(&format!(
            "{collection}.where {{ .price != null && .price > 9.9 }}"
        ));
        let mut unguarded = db.ids(&format!("{collection}.where {{ .price > 9.9 }}"));
        guarded.sort();
        unguarded.sort();
        assert_eq!(guarded, ["a", "d"], "{collection}");
        assert_eq!(unguarded, guarded, "{collection}");

        assert_eq!(
            db.ids(&format!("{collection}.where {{ .price == null }}")),
            ["c"],
            "{collection}"
        );
        // Comparing unrelated types is a non-match, not an error.
        assert_eq!(
            db.scalar(
                &format!("{collection}.where {{ .id > 5 }}.count()"),
                "count"
            ),
            Value::Integer(0),
            "{collection}"
        );

        // Nulls sort first, deterministically.
        assert_eq!(
            db.ids(&format!("{collection}.sort {{ .price.asc }}")),
            ["c", "e", "b", "a", "d"],
            "{collection}"
        );
    }
}

// ---------------------------------------------------------------------------
// Aggregates
// ---------------------------------------------------------------------------

#[test]
fn an_aggregate_gives_the_same_answer_in_every_form() {
    let db = twin_collections();
    db.script(
        r#"
        names.insert [
          { id: "n1", name: "pear", seen: "2026-03-01T00:00:00Z" },
          { id: "n2", name: "apple", seen: "2026-01-01T00:00:00Z" }
        ]
        "#,
    );

    // Terminal form and the function inside a select are one computation.
    assert_eq!(db.scalar("loose.sum(.qty)", "sum"), Value::Integer(11));
    assert_eq!(
        db.scalar("loose.select { v: sum(.qty) }", "v"),
        Value::Integer(11)
    );
    assert_eq!(db.scalar("loose.sum(.price)", "sum"), Value::Float(29.0));
    assert_eq!(db.scalar("loose.avg(.qty)", "avg"), Value::Float(2.2));
    assert_eq!(db.scalar("loose.min(.qty)", "min"), Value::Integer(-3));
    assert_eq!(db.scalar("loose.max(.price)", "max"), Value::Float(10.5));

    // min and max follow the value order, not only numbers.
    assert_eq!(db.scalar("names.min(.name)", "min"), text("apple"));
    assert_eq!(db.scalar("names.max(.name)", "max"), text("pear"));
    assert_eq!(
        db.scalar("names.max(.seen)", "max"),
        text("2026-03-01T00:00:00Z")
    );

    // Empty input: nothing to average or order, and a zero sum.
    for (query, field, expected) in [
        ("loose.where { .qty > 999 }.avg(.qty)", "avg", Value::Null),
        ("loose.where { .qty > 999 }.min(.qty)", "min", Value::Null),
        ("loose.where { .qty > 999 }.max(.qty)", "max", Value::Null),
        (
            "loose.where { .qty > 999 }.sum(.qty)",
            "sum",
            Value::Integer(0),
        ),
        (
            "loose.where { .qty > 999 }.count()",
            "count",
            Value::Integer(0),
        ),
        (
            "loose.where { .qty > 999 }.select { v: avg(.qty) }",
            "v",
            Value::Null,
        ),
    ] {
        assert_eq!(db.scalar(query, field), expected, "{query}");
    }
}

#[test]
fn grouping_follows_value_equality_and_empty_input_has_no_groups() {
    let db = Db::new();
    db.script(
        r#"
        sales.insert [
          { id: "s1", rate: 1, amount: 10 },
          { id: "s2", rate: 1.0, amount: 20 },
          { id: "s3", rate: 2.5, amount: 5 }
        ]
        "#,
    );

    let rows = db.rows("sales.group_by { .rate }.select { rate, total: sum(.amount) }");
    assert_eq!(rows.len(), 2, "1 and 1.0 are the same group key");
    let totals: Vec<&Value> = rows
        .iter()
        .filter_map(|row| row.get_field("total"))
        .collect();
    assert!(totals.contains(&&Value::Integer(30)));
    assert!(totals.contains(&&Value::Integer(5)));

    let none = db.rows(
        "sales.where { .amount > 999 }.group_by { .rate }.select { rate, total: sum(.amount) }",
    );
    assert!(none.is_empty(), "no rows means no groups: {none:?}");
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

#[test]
fn parameters_carry_values_into_a_fixed_query() {
    let db = twin_collections();
    let query =
        parse_oql("typed.where { .label == $label && .qty > $min }.sort { .id.asc }").unwrap();
    assert_eq!(
        owl_db::query_parameters(&query)
            .into_iter()
            .collect::<Vec<_>>(),
        ["label", "min"]
    );

    let bound = bind_parameters(
        &query,
        &HashMap::from([
            ("label".to_string(), text("x")),
            ("min".to_string(), Value::Integer(-1)),
            ("unused".to_string(), Value::Null),
        ]),
    )
    .unwrap();
    let rows = db.executor.execute_query(&bound).unwrap();
    assert_eq!(rows.len(), 2);

    // A value is data: text that looks like OQL stays an ordinary string.
    let hostile = bind_parameters(
        &query,
        &HashMap::from([
            (
                "label".to_string(),
                text(r#"x" || .qty > -100 || .label == ""#),
            ),
            ("min".to_string(), Value::Integer(-100)),
        ]),
    )
    .unwrap();
    assert!(db.executor.execute_query(&hostile).unwrap().is_empty());

    // Lists and nested positions work too.
    let listed = bind_parameters(
        &parse_oql("loose.where { .id in $ids }.select { id, over: .qty > $min }").unwrap(),
        &HashMap::from([
            ("ids".to_string(), Value::List(vec![text("a"), text("b")])),
            ("min".to_string(), Value::Integer(0)),
        ]),
    )
    .unwrap();
    assert_eq!(db.executor.execute_query(&listed).unwrap().len(), 2);

    let missing = bind_parameters(&query, &HashMap::new()).unwrap_err();
    assert!(missing.contains("$label"), "{missing}");
    // Executing without binding is an error, never a silent null.
    let error = db.executor.execute_query(&query).unwrap_err().to_string();
    assert!(error.contains("$"), "{error}");
}

#[test]
fn a_script_value_satisfies_a_parameter_of_the_same_name() {
    let db = Db::new();
    let file = parse_oql_file(
        r#"
        events.insert [
          { id: "e1", level: "error", at: "2026-02-01T00:00:00Z" },
          { id: "e2", level: "error", at: "2025-12-01T00:00:00Z" },
          { id: "e3", level: "info", at: "2026-02-02T00:00:00Z" }
        ]
        from = "2026-01-01T00:00:00Z"
        events.where { .level == "error" && .at >= $from }
        "#,
    )
    .unwrap();
    let report = execute_oql_file(&file, &db.executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(matches!(
        report.results.last(),
        Some(owl_db::StatementResult::Query(1))
    ));
}

// ---------------------------------------------------------------------------
// No silent wrong answers (specification phase 1)
// ---------------------------------------------------------------------------

/// Organisations own teams, teams have users, users place orders.
fn graph() -> Db {
    let db = Db::new();
    db.script(
        r#"
        schema orgs { id: String, name: String }
        schema teams { id: String, name: String, org: Ref(orgs) }
        schema users {
          id: String,
          name: String,
          team: Ref(teams),
          tags: List(String),
          profile: Object,
          nickname: Optional(String)
        }
        schema orders { id: String, user: Ref(users), amount: Float, status: String }

        orgs.insert [ { id: "o1", name: "Acme" }, { id: "o2", name: "Tiny" }, { id: "o3", name: "Empty" } ]
        teams.insert [
          { id: "t1", name: "Core", org: "o1" },
          { id: "t2", name: "Ops", org: "o1" },
          { id: "t3", name: "Solo", org: "o2" }
        ]
        users.insert [
          { id: "u1", name: "Ada", team: "t1", tags: ["admin", "beta"], profile: { city: "Lagos", langs: ["en", "yo"] } },
          { id: "u2", name: "Bob", team: "t2", tags: [], profile: { city: "Accra" }, nickname: "bobby" },
          { id: "u3", name: "Cy", team: "t1", tags: ["beta"], profile: {} },
          { id: "u4", name: "Di", team: "t3", tags: [], profile: {} }
        ]
        orders.insert [
          { id: "r1", user: "u1", amount: 120.0, status: "paid" },
          { id: "r2", user: "u1", amount: 40.0, status: "open" },
          { id: "r3", user: "u3", amount: 300.0, status: "paid" },
          { id: "r4", user: "u2", amount: 15.0, status: "cancelled" }
        ]
        "#,
    );
    db
}

#[test]
fn an_id_identifies_exactly_one_document() {
    let db = graph();

    let error = db
        .try_rows(r#"orgs.insert { id: "o1", name: "Impostor" }"#)
        .unwrap_err();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(
        db.scalar(r#"orgs.where { .id == "o1" }.count()"#, "count"),
        Value::Integer(1)
    );
    assert_eq!(
        db.scalar(r#"orgs.where { .id == "o1" }.select { name }"#, "name"),
        text("Acme")
    );

    // A batch is rejected as a whole, including for a clash within itself.
    for batch in [
        r#"orgs.insert [ { id: "o8", name: "New" }, { id: "o2", name: "Clash" } ]"#,
        r#"orgs.insert [ { id: "o9", name: "A" }, { id: "o9", name: "B" } ]"#,
    ] {
        assert!(db.try_rows(batch).unwrap_err().contains("already exists"));
    }
    assert_eq!(db.scalar("orgs.count()", "count"), Value::Integer(3));

    // Upsert is how an existing document is replaced.
    db.rows(r#"orgs.upsert on id { id: "o1", name: "Acme Ltd" }"#);
    assert_eq!(
        db.scalar(r#"orgs.where { .id == "o1" }.select { name }"#, "name"),
        text("Acme Ltd")
    );
    assert_eq!(db.scalar("orgs.count()", "count"), Value::Integer(3));

    // An id cannot be changed, and deleting frees it.
    let error = db
        .try_rows(r#"orgs.where { .id == "o3" }.update { id: "o30" }"#)
        .unwrap_err();
    assert!(error.contains("identity"), "{error}");
    db.rows(r#"orgs.where { .id == "o3" }.delete()"#);
    db.rows(r#"orgs.insert { id: "o3", name: "Reborn" }"#);
    assert_eq!(
        db.scalar(r#"orgs.where { .id == "o3" }.select { name }"#, "name"),
        text("Reborn")
    );

    // References still resolve to the document with that id.
    assert_eq!(
        db.scalar(r#"teams.where { .id == "t3" }.select { org: .org.name }"#, "org"),
        text("Tiny")
    );
}

#[test]
fn star_selects_every_field() {
    let db = graph();

    let rows = db.rows(r#"orgs.where { .id == "o1" }.select { * }"#);
    assert_eq!(rows[0].get_field("id"), Some(&text("o1")));
    assert_eq!(rows[0].get_field("name"), Some(&text("Acme")));
    assert_eq!(rows[0].get_field("*"), None);

    let rows = db.rows(
        r#"orgs.where { .id == "o1" }.select { *, shout: upper(.name), name: "replaced" }"#,
    );
    assert_eq!(rows[0].get_field("id"), Some(&text("o1")));
    assert_eq!(rows[0].get_field("shout"), Some(&text("ACME")));
    assert_eq!(rows[0].get_field("name"), Some(&text("replaced")));

    // The same on a nested collection.
    let nested = db.scalar(
        r#"users.where { .id == "u1" }.select { orders: .orders.where { .status == "paid" }.select { * } }"#,
        "orders",
    );
    let Value::List(orders) = nested else {
        panic!("expected a list: {nested:?}");
    };
    let Value::Object(order) = &orders[0] else {
        panic!("expected an object");
    };
    assert_eq!(order.get("id"), Some(&text("r1")));
    assert_eq!(order.get("amount"), Some(&Value::Float(120.0)));
}

#[test]
fn a_single_related_row_projects_as_an_object() {
    let db = graph();

    // A reference is one row, so shaping it yields one object.
    assert_eq!(
        db.scalar(r#"users.where { .id == "u1" }.select { team: .team.select { name } }"#, "team"),
        Value::Object(HashMap::from([("name".to_string(), text("Core"))]))
    );
    // Shaping can traverse onward from the referenced row.
    assert_eq!(
        db.scalar(
            r#"users.where { .id == "u1" }.select { team: .team.select { name, org: .org.name } }"#,
            "team"
        ),
        Value::Object(HashMap::from([
            ("name".to_string(), text("Core")),
            ("org".to_string(), text("Acme")),
        ]))
    );
    // `first()` narrows to one row, and shaping it afterwards keeps it one.
    assert_eq!(
        db.scalar(
            r#"users.where { .id == "u1" }.select { top: .orders.sort { .amount.desc }.first().select { id } }"#,
            "top"
        ),
        Value::Object(HashMap::from([("id".to_string(), text("r1"))]))
    );
    // No related row is null, not an empty object or a list.
    assert_eq!(
        db.scalar(
            r#"users.where { .id == "u4" }.select { top: .orders.first().select { id } }"#,
            "top"
        ),
        Value::Null
    );
    // The scoped form with a local name means the same.
    assert_eq!(
        db.scalar(
            "users {\n  team = .team\n  where { .id == \"u2\" }\n  select { t: team.select { name } }\n}",
            "t"
        ),
        Value::Object(HashMap::from([("name".to_string(), text("Ops"))]))
    );
    // An embedded object behaves the same way.
    assert_eq!(
        db.scalar(r#"users.where { .id == "u1" }.select { p: .profile.select { city } }"#, "p"),
        Value::Object(HashMap::from([("city".to_string(), text("Lagos"))]))
    );
}

#[test]
fn a_path_through_a_collection_reaches_every_row_beyond_it() {
    let db = graph();

    let people = |org: &str| {
        db.scalar(
            &format!(r#"orgs.where {{ .id == "{org}" }}.select {{ n: .teams.users.count() }}"#),
            "n",
        )
    };
    assert_eq!(people("o1"), Value::Integer(3));
    assert_eq!(people("o2"), Value::Integer(1));
    assert_eq!(people("o3"), Value::Integer(0));

    // Three hops, with operations on the flattened result.
    assert_eq!(
        db.scalar(
            r#"orgs.where { .id == "o1" }.select { revenue: .teams.users.orders.where { .status == "paid" }.sum(.amount) }"#,
            "revenue"
        ),
        Value::Float(420.0)
    );
    let Value::List(names) = db.scalar(
        r#"orgs.where { .id == "o1" }.select { names: .teams.users.sort { .name.asc }.select { name } }"#,
        "names",
    ) else {
        panic!("expected a list");
    };
    assert_eq!(names.len(), 3);

    // A forward step after a collection maps over it.
    assert_eq!(
        db.scalar(r#"teams.where { .id == "t1" }.select { n: .users.orders.count() }"#, "n"),
        Value::Integer(3)
    );
    // It works as a filter too.
    let mut busy = db.ids("orgs.where { .teams.users.orders.count() > 0 }");
    busy.sort();
    assert_eq!(busy, ["o1"]);
}

#[test]
fn a_reference_equals_the_id_of_its_target() {
    let db = graph();

    let mut by_reference = db.ids(r#"orders.where { .user == "u1" }"#);
    let mut by_id = db.ids(r#"orders.where { .user.id == "u1" }"#);
    by_reference.sort();
    by_id.sort();
    assert_eq!(by_reference, ["r1", "r2"]);
    assert_eq!(by_reference, by_id);

    assert_eq!(db.ids(r#"orders.where { "u3" == .user }"#), ["r3"]);
    let mut others = db.ids(r#"orders.where { .user != "u1" }"#);
    others.sort();
    assert_eq!(others, ["r3", "r4"]);
    let mut listed = db.ids(r#"orders.where { .user in ["u2", "u3"] }"#);
    listed.sort();
    assert_eq!(listed, ["r3", "r4"]);
    // Correlation can compare a reference with the outer row's id directly.
    assert_eq!(
        db.scalar(
            r#"users.where { .id == "u1" }.select { n: orders.where { .user == users.id }.count() }"#,
            "n"
        ),
        Value::Integer(2)
    );
}

#[test]
fn indexing_is_null_safe() {
    let db = graph();

    let rows = db.rows("users.sort { .id.asc }.select { id, second: .profile.langs[1], tag: .tags[0] }");
    assert_eq!(rows[0].get_field("second"), Some(&text("yo")));
    assert_eq!(rows[0].get_field("tag"), Some(&text("admin")));
    // No `langs` at all, and an empty `tags` list.
    assert_eq!(rows[1].get_field("second"), Some(&Value::Null));
    assert_eq!(rows[1].get_field("tag"), Some(&Value::Null));
    assert_eq!(db.ids(r#"users.where { .profile.langs[0] == "en" }"#), ["u1"]);
}

#[test]
fn a_binding_means_the_expression_it_names() {
    let db = graph();
    let run = |script: &str| {
        let file = parse_oql_file(script).unwrap();
        let report = execute_oql_file(&file, &db.executor).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        report.results.last().cloned()
    };

    // 118.75 is the mean; two orders exceed it.
    let inline = run("orders.where { .amount > orders.avg(.amount) }");
    let bound = run("mean = orders.avg(.amount)\norders.where { .amount > mean }");
    assert!(matches!(inline, Some(owl_db::StatementResult::Query(2))));
    assert!(matches!(bound, Some(owl_db::StatementResult::Query(2))));

    // Bindings compose inside expressions as well as at the root.
    let composed = run(
        "paid = orders.where { .status == \"paid\" }\nbig = paid.max(.amount)\norders.where { .amount == big }",
    );
    assert!(matches!(composed, Some(owl_db::StatementResult::Query(1))));
    let counted = run(
        "paid = orders.where { .status == \"paid\" }\nusers.where { paid.count() == 2 }",
    );
    assert!(matches!(counted, Some(owl_db::StatementResult::Query(4))));
}

#[test]
fn text_that_cannot_mean_anything_is_an_error() {
    let db = graph();

    // An aggregate with nothing to aggregate.
    let error = db.try_rows("users.where { count() > 0 }").unwrap_err();
    assert!(error.contains("no collection"), "{error}");

    // A misspelt relationship on a collection whose shape is declared.
    for query in [
        "users.select { n: .orderz.count() }",
        "users.where { .orderz.exists() }",
        "orgs.select { n: .teams.userz.count() }",
    ] {
        let error = db.try_rows(query).unwrap_err();
        assert!(error.contains("z'"), "{query}: {error}");
    }

    // A declared field that a row lacks is still just absent.
    assert_eq!(
        db.scalar(r#"users.where { .id == "u1" }.select { n: .nickname.count() }"#, "n"),
        Value::Integer(0)
    );
    // Without a schema nothing can be ruled out, so the read stays graceful.
    db.script(r#"loose.insert { id: "l1" }"#);
    assert_eq!(
        db.scalar("loose.select { n: .anything.count() }", "n"),
        Value::Integer(0)
    );

    // `exists` on a relationship asks whether there are related rows.
    let mut with_orders = db.ids("users.where { exists(.orders) }");
    with_orders.sort();
    assert_eq!(with_orders, ["u1", "u2", "u3"]);
    assert_eq!(db.ids("users.where { !exists(.orders) }"), ["u4"]);
    // ...and on a path it still asks whether the path is present.
    assert_eq!(db.ids("users.where { exists(.nickname) }"), ["u2"]);
    assert_eq!(db.ids("users.where { exists(.profile.langs) }"), ["u1"]);
}

// ---------------------------------------------------------------------------
// Ordinary data (specification phase 2)
// ---------------------------------------------------------------------------

#[test]
fn keywords_are_ordinary_field_names() {
    let db = Db::new();
    db.script(
        r#"
        metrics.insert [
          { id: "m1", count: 3, first: "a", limit: 9, sort: 1, select: 2, min: 0, max: 7,
            in: 5, on: 6, where: "here", exists: true, sum: 10, type: "t" },
          { id: "m2", count: 8, first: "b", limit: 1, sort: 2, select: 4, min: 1, max: 9,
            in: 0, on: 0, where: "there", exists: false, sum: 20, type: "u" }
        ]
        "#,
    );

    // As paths, in filters, sorts and projections.
    assert_eq!(db.ids("metrics.where { .count == 3 }"), ["m1"]);
    assert_eq!(db.ids("metrics.where { .limit > .count }"), ["m1"]);
    assert_eq!(db.ids(r#"metrics.where { .where == "there" && .exists == false }"#), ["m2"]);
    assert_eq!(db.ids("metrics.sort { .count.desc }"), ["m2", "m1"]);
    assert_eq!(db.ids("metrics.sort { .first.asc }.limit(1)"), ["m1"]);

    let rows = db.rows(
        r#"metrics.where { .id == "m1" }.select {
             count, first, limit,
             min: .min, max: .max, avg: (.min + .max) / 2,
             sum: .sum, where: .where, total: .count + .in + .on
           }"#,
    );
    let row = &rows[0];
    assert_eq!(row.get_field("count"), Some(&Value::Integer(3)));
    assert_eq!(row.get_field("first"), Some(&text("a")));
    assert_eq!(row.get_field("limit"), Some(&Value::Integer(9)));
    assert_eq!(row.get_field("max"), Some(&Value::Integer(7)));
    assert_eq!(row.get_field("avg"), Some(&Value::Integer(3)));
    assert_eq!(row.get_field("sum"), Some(&Value::Integer(10)));
    assert_eq!(row.get_field("where"), Some(&text("here")));
    assert_eq!(row.get_field("total"), Some(&Value::Integer(14)));

    // The operations of the same names still work, side by side with them.
    assert_eq!(db.scalar("metrics.count()", "count"), Value::Integer(2));
    assert_eq!(db.scalar("metrics.sum(.count)", "sum"), Value::Integer(11));
    assert_eq!(db.scalar("metrics.max(.max)", "max"), Value::Integer(9));
    assert_eq!(
        db.scalar("metrics.select { count: count(), sum: sum(.sum), min: min(.min) }", "sum"),
        Value::Integer(30)
    );
    let grouped = db.rows(
        "metrics.group_by { .type }.select { type, count: count(), avg: avg(.count) }.sort { .count.desc }",
    );
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped[0].get_field("count"), Some(&Value::Integer(1)));

    // In updates, upserts and nested objects.
    db.rows(r#"metrics.where { .id == "m1" }.update { count += 1, first: "z", limit: .limit - 1 }"#);
    db.rows(r#"metrics.upsert on id { id: "m3", count: 0, select: { where: 1, first: [1, 2] } }"#);
    assert_eq!(
        db.scalar(r#"metrics.where { .id == "m1" }.select { count }"#, "count"),
        Value::Integer(4)
    );
    assert_eq!(
        db.scalar(r#"metrics.where { .id == "m3" }.select { v: .select.where }"#, "v"),
        Value::Integer(1)
    );
    assert_eq!(
        db.scalar(r#"metrics.where { .id == "m3" }.select { v: .select.first[1] }"#, "v"),
        Value::Integer(2)
    );
}

#[test]
fn a_method_call_is_a_function_call_on_what_precedes_it() {
    let mut db = Db::new();
    db.script(
        r#"
        people.insert [
          { id: "p1", name: "Ada Lovelace", tags: ["math", "beta"], joined: "2024-03-15T10:00:00Z", score: 9.4 },
          { id: "p2", name: "bob", tags: [], joined: "2026-01-02T00:00:00Z", score: 3.6 }
        ]
        "#,
    );
    db.define("fn initial(s: String) -> String { substring(s, 0, 1) }");

    assert_eq!(db.ids(r#"people.where { .name.starts_with("Ada") }"#), ["p1"]);
    assert_eq!(db.ids(r#"people.where { .name.contains("o") && .name.ends_with("b") }"#), ["p2"]);
    assert_eq!(db.ids(r#"people.where { .tags.contains("beta") }"#), ["p1"]);
    assert_eq!(db.ids("people.where { .joined.year() == 2026 }"), ["p2"]);
    assert_eq!(db.ids("people.where { .name.lower().length() == 3 }"), ["p2"]);

    let rows = db.rows(
        r#"people.sort { .id.asc }.select {
             id,
             shout: .name.upper(),
             chained: .name.trim().upper().substring(0, 3),
             mine: .name.initial(),
             rounded: .score.round(),
             year: .joined.year(),
             same: upper(.name) == .name.upper()
           }"#,
    );
    assert_eq!(rows[0].get_field("shout"), Some(&text("ADA LOVELACE")));
    assert_eq!(rows[0].get_field("chained"), Some(&text("ADA")));
    assert_eq!(rows[0].get_field("mine"), Some(&text("A")));
    assert_eq!(rows[0].get_field("rounded"), Some(&Value::Integer(9)));
    assert_eq!(rows[0].get_field("year"), Some(&Value::Integer(2024)));
    assert_eq!(rows[0].get_field("same"), Some(&Value::Boolean(true)));
    assert_eq!(rows[1].get_field("rounded"), Some(&Value::Integer(4)));
}

#[test]
fn dot_is_the_current_value() {
    let db = Db::new();
    db.script(
        r#"
        posts.insert [
          { id: "a", tags: ["rust", "db", "beta"], scores: [3, 9, 4] },
          { id: "b", tags: ["beta"], scores: [] }
        ]
        "#,
    );

    // A list of scalars filters, projects, sorts and aggregates like any
    // other collection.
    let row = &db.rows(
        r#"posts.where { .id == "a" }.select {
             long: .tags.where { . != "db" },
             shouted: .tags.select { v: upper(.) },
             betas: .tags.where { . == "beta" }.count(),
             high: .scores.where { . > 3 }.count(),
             sorted: .scores.sort { . desc }.limit(2)
           }"#,
    )[0];
    assert_eq!(
        row.get_field("long"),
        Some(&Value::List(vec![text("rust"), text("beta")]))
    );
    assert_eq!(row.get_field("betas"), Some(&Value::Integer(1)));
    assert_eq!(row.get_field("high"), Some(&Value::Integer(2)));
    assert_eq!(
        row.get_field("sorted"),
        Some(&Value::List(vec![Value::Integer(9), Value::Integer(4)]))
    );
    let Some(Value::List(shouted)) = row.get_field("shouted") else {
        panic!("expected a list");
    };
    assert_eq!(
        shouted[0],
        Value::Object(HashMap::from([("v".to_string(), text("RUST"))]))
    );

    assert_eq!(db.ids(r#"posts.where { .tags.where { . == "rust" }.exists() }"#), ["a"]);

    // Removing one element of a list in an update.
    db.rows(r#"posts.where { .id == "a" }.update { tags: .tags.where { . != "beta" } }"#);
    assert_eq!(
        db.scalar(r#"posts.where { .id == "a" }.select { tags }"#, "tags"),
        Value::List(vec![text("rust"), text("db")])
    );
}

#[test]
fn timestamps_are_written_and_read_as_text() {
    let db = Db::new();
    db.script(
        r#"
        schema events { id: String, at: DateTime, seen: Optional(DateTime), days: List(DateTime) }
        events.insert [
          { id: "e1", at: "2026-01-05T10:00:00Z", days: ["2026-01-01", "2026-01-02"] },
          { id: "e2", at: "2026-02-05T12:30:00+01:00", seen: "2026-02-06", days: [] },
          { id: "e3", at: "2025-12-31", days: [] }
        ]
        "#,
    );

    assert_eq!(db.ids(r#"events.where { .at >= "2026-02-01" }"#), ["e2"]);
    assert_eq!(db.ids(r#"events.where { .at == "2026-02-05T11:30:00Z" }"#), ["e2"]);
    assert_eq!(
        db.ids("events.sort { .at.asc }"),
        ["e3", "e1", "e2"]
    );
    let row = &db.rows(
        r#"events.where { .id == "e2" }.select { y: year(.at), h: .at.hour(), d: .seen.day(), ym: format_date(.at, "%Y-%m") }"#,
    )[0];
    assert_eq!(row.get_field("y"), Some(&Value::Integer(2026)));
    assert_eq!(row.get_field("h"), Some(&Value::Integer(11)));
    assert_eq!(row.get_field("d"), Some(&Value::Integer(6)));
    assert_eq!(row.get_field("ym"), Some(&text("2026-02")));

    // The stored value is a timestamp, not the text it was written as.
    assert!(matches!(
        db.scalar(r#"events.where { .id == "e3" }.select { at }"#, "at"),
        Value::DateTime(_)
    ));
    db.rows(r#"events.where { .id == "e3" }.update { at: "2026-06-01T00:00:00Z", seen: "2026-06-02" }"#);
    assert_eq!(db.ids("events.where { .at.month() == 6 && .seen.day() == 2 }"), ["e3"]);

    let error = db
        .try_rows(r#"events.insert { id: "bad", at: "next tuesday", days: [] }"#)
        .unwrap_err();
    assert!(error.contains("timestamp"), "{error}");

    // Time functions also read timestamps that a collection keeps as text.
    db.script(r#"logs.insert { id: "l1", at: "2024-07-09T08:00:00Z" }"#);
    assert_eq!(
        db.scalar("logs.select { y: year(.at) }", "y"),
        Value::Integer(2024)
    );
    assert_eq!(db.ids("logs.where { .at.month() == 7 }"), ["l1"]);
}

#[test]
fn limit_and_offset_take_expressions_and_parameters() {
    let db = twin_collections();

    assert_eq!(db.ids("loose.sort { .id.asc }.limit(1 + 1)"), ["a", "b"]);
    assert_eq!(db.ids("loose.sort { .id.asc }.offset(2 * 2)"), ["e"]);
    assert_eq!(db.ids("loose.sort { .id.asc }.limit 2"), ["a", "b"]);

    for collection in ["typed", "loose"] {
        let page = parse_oql(&format!(
            "{collection}.where {{ .qty > $min }}.sort {{ .id.asc }}.offset($skip).limit($size)"
        ))
        .unwrap();
        let bound = bind_parameters(
            &page,
            &HashMap::from([
                ("min".to_string(), Value::Integer(-10)),
                ("skip".to_string(), Value::Integer(1)),
                ("size".to_string(), Value::Integer(2)),
            ]),
        )
        .unwrap();
        let ids: Vec<_> = db
            .executor
            .execute_query(&bound)
            .unwrap()
            .iter()
            .map(|row| row.get_field("id").cloned().unwrap())
            .collect();
        assert_eq!(ids, [text("b"), text("c")], "{collection}");
    }

    // A count computed from data.
    assert_eq!(
        db.ids("loose.sort { .id.asc }.limit(loose.where { .qty > 4 }.count())"),
        ["a", "b"]
    );

    assert!(parse_oql("loose.limit(-1)").is_err());
    let negative = bind_parameters(
        &parse_oql("loose.limit($n)").unwrap(),
        &HashMap::from([("n".to_string(), Value::Integer(-3))]),
    )
    .unwrap();
    assert!(db.executor.execute_query(&negative).is_err());
    let not_a_number = bind_parameters(
        &parse_oql("loose.limit($n)").unwrap(),
        &HashMap::from([("n".to_string(), text("ten"))]),
    )
    .unwrap();
    assert!(db.executor.execute_query(&not_a_number).is_err());
}

#[test]
fn lists_concatenate_measure_and_index_from_either_end() {
    let db = Db::new();
    db.script(
        r#"
        carts.insert [
          { id: "c1", items: ["pen", "ink", "pad"] },
          { id: "c2", items: [] },
          { id: "c3" }
        ]
        "#,
    );

    let rows = db.rows(
        "carts.sort { .id.asc }.select { id, n: length(.items), last: .items[-1], second_last: .items[-2], far: .items[-9], both: .items + [\"x\"] }",
    );
    assert_eq!(rows[0].get_field("n"), Some(&Value::Integer(3)));
    assert_eq!(rows[0].get_field("last"), Some(&text("pad")));
    assert_eq!(rows[0].get_field("second_last"), Some(&text("ink")));
    assert_eq!(rows[0].get_field("far"), Some(&Value::Null));
    assert_eq!(rows[1].get_field("n"), Some(&Value::Integer(0)));
    assert_eq!(rows[1].get_field("last"), Some(&Value::Null));
    assert_eq!(rows[2].get_field("n"), Some(&Value::Null));
    assert_eq!(rows[2].get_field("both"), Some(&Value::List(vec![text("x")])));
    assert_eq!(db.ids("carts.where { .items.length() == 3 }"), ["c1"]);

    // Appending is an ordinary update.
    db.rows(r#"carts.update { items: .items + ["gift"] }"#);
    assert_eq!(
        db.scalar(r#"carts.where { .id == "c1" }.select { items }"#, "items"),
        Value::List(vec![text("pen"), text("ink"), text("pad"), text("gift")])
    );
    assert_eq!(
        db.scalar(r#"carts.where { .id == "c3" }.select { items }"#, "items"),
        Value::List(vec![text("gift")])
    );
    assert_eq!(db.scalar(r#"carts.select { s: length("héllo") }.limit(1)"#, "s"), Value::Integer(5));
}

// ---------------------------------------------------------------------------
// One collection model (specification phase 3)
// ---------------------------------------------------------------------------

/// One user whose four orders are reachable five different ways: as a
/// top-level collection, a reverse relationship, a correlated nested query,
/// a path through another collection, and an embedded array of objects.
fn one_user_many_ways() -> Db {
    let db = Db::new();
    db.script(
        r#"
        schema teams { id: String }
        schema users { id: String, team: Ref(teams), order_list: List(Object) }
        schema orders { id: String, user: Ref(users), amount: Integer, status: String }

        teams.insert { id: "t1" }
        users.insert {
          id: "u1",
          team: "t1",
          order_list: [
            { id: "r1", amount: 120, status: "paid" },
            { id: "r2", amount: 40, status: "open" },
            { id: "r3", amount: 300, status: "paid" },
            { id: "r4", amount: 15, status: "open" }
          ]
        }
        orders.insert [
          { id: "r1", user: "u1", amount: 120, status: "paid" },
          { id: "r2", user: "u1", amount: 40, status: "open" },
          { id: "r3", user: "u1", amount: 300, status: "paid" },
          { id: "r4", user: "u1", amount: 15, status: "open" }
        ]
        "#,
    );
    db
}

#[test]
fn every_operation_means_the_same_on_every_kind_of_collection() {
    let db = one_user_many_ways();
    let sources = [
        ("top-level collection", "orders"),
        ("reverse relationship", ".orders"),
        ("correlated nested query", "orders.where { .user == users.id }"),
        ("path through a collection", ".team.users.orders"),
        ("embedded array", ".order_list"),
    ];
    let int = Value::Integer;
    let id = |value: &str| Value::Object(HashMap::from([("id".to_string(), text(value))]));
    let ids = |values: &[&str]| Value::List(values.iter().map(|value| id(value)).collect());
    let operations: Vec<(&str, Value)> = vec![
        ("count()", int(4)),
        ("exists()", Value::Boolean(true)),
        ("sum(.amount)", int(475)),
        ("avg(.amount)", Value::Float(118.75)),
        ("min(.amount)", int(15)),
        ("max(.amount)", int(300)),
        ("sum(.amount * 2)", int(950)),
        ("max(.amount - 100)", int(200)),
        ("where { .amount > 50 }.count()", int(2)),
        ("where { .amount > 9000 }.exists()", Value::Boolean(false)),
        ("where { .amount > 9000 }.sum(.amount)", int(0)),
        ("where { .amount > 9000 }.max(.amount)", Value::Null),
        (
            r#"where { .status == "paid" }.sort { .id.asc }.select { id }"#,
            ids(&["r1", "r3"]),
        ),
        ("sort { .amount.desc }.select { id }", ids(&["r3", "r1", "r2", "r4"])),
        ("sort { .amount.asc }.limit(2).select { id }", ids(&["r4", "r2"])),
        ("sort { .amount.asc }.offset(3).select { id }", ids(&["r3"])),
        ("sort { .amount.asc }.limit(1 + 1).offset(1).select { id }", ids(&["r2"])),
        ("sort { .amount.desc }.first().select { id }", id("r3")),
        ("where { .amount > 9000 }.first().select { id }", Value::Null),
        (
            "sort { .id.asc }.select { status }.distinct()",
            Value::List(vec![
                Value::Object(HashMap::from([("status".to_string(), text("paid"))])),
                Value::Object(HashMap::from([("status".to_string(), text("open"))])),
            ]),
        ),
        ("select { status }.distinct().count()", int(2)),
        (
            "group_by { .status }.select { status, n: count(), total: sum(.amount) }.sort { .status.asc }",
            Value::List(vec![
                Value::Object(HashMap::from([
                    ("status".to_string(), text("open")),
                    ("n".to_string(), int(2)),
                    ("total".to_string(), int(55)),
                ])),
                Value::Object(HashMap::from([
                    ("status".to_string(), text("paid")),
                    ("n".to_string(), int(2)),
                    ("total".to_string(), int(420)),
                ])),
            ]),
        ),
        (
            "group_by { big: .amount > 100 }.select { big, n: count() }.sort { .n.desc }.first().select { n }",
            Value::Object(HashMap::from([("n".to_string(), int(2))])),
        ),
    ];

    for (operation, expected) in &operations {
        for (kind, source) in sources {
            let query = format!(
                r#"users.where {{ .id == "u1" }}.select {{ v: {source}.{operation} }}"#
            );
            assert_eq!(
                &db.scalar(&query, "v"),
                expected,
                "{operation} on a {kind}\n{query}"
            );
        }
    }

    // A list of scalars is the same kind of thing, with `.` for the element.
    let db = Db::new();
    db.script(r#"bags.insert { id: "b", sizes: [3, 9, 4, 9] }"#);
    for (operation, expected) in [
        ("count()", int(4)),
        ("where { . > 3 }.count()", int(3)),
        ("sort { . desc }.limit(2)", Value::List(vec![int(9), int(9)])),
        ("distinct().count()", int(3)),
        ("sort { . asc }.first()", int(3)),
        ("where { . > 100 }.exists()", Value::Boolean(false)),
    ] {
        assert_eq!(
            db.scalar(&format!("bags.select {{ v: .sizes.{operation} }}"), "v"),
            expected,
            "{operation} on a list of scalars"
        );
    }
}

#[test]
fn aggregates_range_over_expressions() {
    let db = Db::new();
    db.script(
        r#"
        lines.insert [
          { id: "l1", order: "A", price: 20.0, qty: 2, discount: 5.0 },
          { id: "l2", order: "A", price: 2.5, qty: 10 },
          { id: "l3", order: "B", price: 40.0, qty: 1, discount: 10.0 }
        ]
        "#,
    );

    // Terminal form, function form, and inside larger expressions.
    assert_eq!(db.scalar("lines.sum(.price * .qty)", "sum"), Value::Float(105.0));
    assert_eq!(db.scalar("lines.max(.price * .qty)", "max"), Value::Float(40.0));
    assert_eq!(
        db.scalar("lines.select { v: sum(.price * .qty) }", "v"),
        Value::Float(105.0)
    );
    assert_eq!(
        db.scalar("lines.select { v: sum(.price * .qty) - sum(.discount) }", "v"),
        Value::Float(90.0)
    );
    // Rows where the expression has no value do not count as inputs.
    assert_eq!(db.scalar("lines.select { v: count(.discount) }", "v"), Value::Integer(2));
    assert_eq!(
        db.scalar("lines.avg(coalesce(.discount, 0.0))", "avg"),
        Value::Float(5.0)
    );
    assert_eq!(
        db.scalar(r#"lines.sum(if .order == "A" then .qty else 0)"#, "sum"),
        Value::Integer(12)
    );

    // Per group, and in `having`.
    let rows = db.rows(
        "lines.group_by { .order }.select { order, value: sum(.price * .qty), top: max(.price * .qty) }.sort { .order.asc }",
    );
    assert_eq!(rows[0].get_field("value"), Some(&Value::Float(65.0)));
    assert_eq!(rows[0].get_field("top"), Some(&Value::Float(40.0)));
    assert_eq!(rows[1].get_field("value"), Some(&Value::Float(40.0)));
    let big = db.rows(
        "lines.group_by { .order }.having { sum(.price * .qty) > 50 }.select { order }",
    );
    assert_eq!(big.len(), 1);
    assert_eq!(big[0].get_field("order"), Some(&text("A")));
}

#[test]
fn group_keys_may_be_computed() {
    let db = Db::new();
    db.script(
        r#"
        schema sales { id: String, placed: DateTime, amount: Integer, region: String }
        sales.insert [
          { id: "s1", placed: "2026-01-05", amount: 10, region: "north" },
          { id: "s2", placed: "2026-01-20", amount: 20, region: "south" },
          { id: "s3", placed: "2026-02-03", amount: 30, region: "north" },
          { id: "s4", placed: "2025-12-31", amount: 40, region: "north" }
        ]
        "#,
    );

    // Time-bucketed reporting needs nothing more than a named key.
    let rows = db.rows(
        r#"sales.group_by { month: format_date(.placed, "%Y-%m") }.select { month, total: sum(.amount) }.sort { .month.asc }"#,
    );
    let months: Vec<_> = rows
        .iter()
        .map(|row| (row.get_field("month").cloned(), row.get_field("total").cloned()))
        .collect();
    assert_eq!(
        months,
        [
            (Some(text("2025-12")), Some(Value::Integer(40))),
            (Some(text("2026-01")), Some(Value::Integer(30))),
            (Some(text("2026-02")), Some(Value::Integer(30))),
        ]
    );

    // A bare call is named after its function; keys of all kinds combine.
    let rows = db.rows(
        "sales.group_by { year(.placed), .region, big: .amount >= 30 }.select { year, region, big, n: count() }.sort { .year.asc, .region.asc, .n.desc }",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].get_field("year"), Some(&Value::Integer(2025)));
    assert_eq!(rows[0].get_field("big"), Some(&Value::Boolean(true)));
    assert_eq!(rows[1].get_field("region"), Some(&text("north")));

    // A keyword works as a key name, and method form as its value.
    let rows = db.rows("sales.group_by { count: .placed.month() }.select { count, n: count() }");
    assert_eq!(rows.len(), 3);

    assert!(parse_oql("sales.group_by { .amount + 1 }.select { n: count() }")
        .unwrap_err()
        .to_string()
        .contains("needs a name"));
    assert!(parse_oql("sales.group_by { .region, region: .amount }.select { n: count() }")
        .unwrap_err()
        .to_string()
        .contains("two keys"));
}

#[test]
fn distinct_removes_equal_rows() {
    let db = Db::new();
    db.script(
        r#"
        visits.insert [
          { id: "v1", page: "home", score: 1, who: { name: "ada", tier: 1 } },
          { id: "v2", page: "docs", score: 1.0, who: { tier: 1, name: "ada" } },
          { id: "v3", page: "home", score: 2, who: { name: "bob", tier: 2 } },
          { id: "v4", page: "home", score: 1, who: { name: "ada", tier: 1 } }
        ]
        "#,
    );

    let pages = db.rows("visits.sort { .id.asc }.select { page }.distinct()");
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].get_field("page"), Some(&text("home")));
    assert_eq!(pages[1].get_field("page"), Some(&text("docs")));

    // Equality is value equality: 1 and 1.0 are one value, and an object's
    // field order is irrelevant.
    assert_eq!(db.rows("visits.select { score }.distinct()").len(), 2);
    assert_eq!(db.rows("visits.select { who }.distinct()").len(), 2);
    assert_eq!(db.rows("visits.select { page, score }.distinct()").len(), 3);
    // Whole rows differ by id, so nothing is removed.
    assert_eq!(db.rows("visits.distinct()").len(), 4);

    assert_eq!(
        db.scalar("visits.select { page }.distinct().count()", "count"),
        Value::Integer(2)
    );
    // A field called `distinct` is still a field.
    db.script(r#"flags.insert { id: "f", distinct: true }"#);
    assert_eq!(db.ids("flags.where { .distinct == true }"), ["f"]);
}

#[test]
fn an_operation_with_no_receiver_applies_to_the_rows_in_hand() {
    let db = one_user_many_ways();

    // In a grouped select the rows in hand are the group's.
    let rows = db.rows(
        r#"orders.group_by { .status }.select {
             status,
             n: count(),
             big: where { .amount > 100 }.count(),
             top: sort { .amount.desc }.first().select { id },
             ids: sort { .id.asc }.select { id },
             top_two_total: sort { .amount.desc }.limit(2).sum(.amount)
           }.sort { .status.asc }"#,
    );
    let (open, paid) = (&rows[0], &rows[1]);
    assert_eq!(open.get_field("n"), Some(&Value::Integer(2)));
    assert_eq!(open.get_field("big"), Some(&Value::Integer(0)));
    assert_eq!(paid.get_field("big"), Some(&Value::Integer(2)));
    assert_eq!(
        paid.get_field("top"),
        Some(&Value::Object(HashMap::from([("id".to_string(), text("r3"))])))
    );
    assert_eq!(open.get_field("top_two_total"), Some(&Value::Integer(55)));
    let Some(Value::List(ids)) = open.get_field("ids") else {
        panic!("expected a list");
    };
    assert_eq!(ids.len(), 2);

    // Without grouping they are all the rows the select was given.
    let row = &db.rows(
        r#"orders.where { .amount > 20 }.select {
             all: count(),
             paid: where { .status == "paid" }.count(),
             share: where { .status == "paid" }.sum(.amount) / sum(.amount),
             largest: sort { .amount.desc }.first().select { id }
           }"#,
    )[0];
    assert_eq!(row.get_field("all"), Some(&Value::Integer(3)));
    assert_eq!(row.get_field("paid"), Some(&Value::Integer(2)));
    assert_eq!(row.get_field("share"), Some(&Value::Integer(0)));
    assert_eq!(
        row.get_field("largest"),
        Some(&Value::Object(HashMap::from([("id".to_string(), text("r3"))])))
    );
}

#[test]
fn a_scoped_block_ends_the_way_a_chain_can() {
    let db = one_user_many_ways();
    let scoped = |body: &str| format!("orders {{\n  where {{ .status == \"paid\" }}\n  {body}\n}}");

    for (terminal, field, expected) in [
        ("count()", "count", Value::Integer(2)),
        ("exists()", "exists", Value::Boolean(true)),
        ("sum(.amount)", "sum", Value::Integer(420)),
        ("max(.amount * 2)", "max", Value::Integer(600)),
    ] {
        assert_eq!(db.scalar(&scoped(terminal), field), expected, "{terminal}");
        // The chained form gives the same answer.
        assert_eq!(
            db.scalar(
                &format!(r#"orders.where {{ .status == "paid" }}.{terminal}"#),
                field
            ),
            expected
        );
    }
    assert_eq!(
        db.ids("orders {\n  sort { .amount.desc }\n  first()\n}"),
        ["r3"]
    );

    // Mutating terminals, with a block local in play.
    db.rows(
        "orders {\n  bonus = 5\n  where { .status == \"open\" }\n  update { amount: .amount + bonus }\n}",
    );
    assert_eq!(
        db.scalar(r#"orders.where { .status == "open" }.sum(.amount)"#, "sum"),
        Value::Integer(65)
    );
    db.rows("orders {\n  where { .amount < 30 }\n  delete()\n}");
    assert_eq!(db.scalar("orders.count()", "count"), Value::Integer(3));

    // A block still has to produce something, exactly once.
    for (block, message) in [
        ("orders {\n  where { .amount > 1 }\n}", "must end in"),
        ("orders {\n  count()\n  limit(1)\n}", "count() must be the final"),
        ("orders {\n  select { id }\n  count()\n}", "select must be the final"),
    ] {
        let error = parse_oql(block).unwrap_err().to_string();
        assert!(error.contains(message), "{block}: {error}");
    }
}

// ---------------------------------------------------------------------------
// The schema as the single source of truth (specification phase 4)
// ---------------------------------------------------------------------------

impl Db {
    /// Run a script and return the errors of the statements that failed.
    fn script_errors(&self, source: &str) -> Vec<String> {
        let file = parse_oql_file(source).unwrap_or_else(|error| panic!("{error}\n{source}"));
        execute_oql_file(&file, &self.executor).unwrap().errors
    }
}

#[test]
fn constraints_defaults_and_indexes_are_declared_with_the_schema() {
    let db = Db::new();
    db.script(
        r#"
        schema accounts {
          id: String,
          email: String unique,
          tenant: String,
          handle: String,
          age: Integer,
          plan: String = "free",
          joined: DateTime = now(),
          label: String = upper(.handle),
          unique(tenant, handle),
          check(.age >= 0 && .age < 150),
          index(.plan, .age)
        }
        accounts.insert { id: "a1", email: "ada@x.io", tenant: "t1", handle: "ada", age: 36 }
        "#,
    );

    // Defaults fill what the insert left out, and may be computed.
    let row = &db.rows(r#"accounts.where { .id == "a1" }"#)[0];
    assert_eq!(row.get_field("plan"), Some(&text("free")));
    assert_eq!(row.get_field("label"), Some(&text("ADA")));
    assert!(matches!(row.get_field("joined"), Some(Value::DateTime(_))));
    // A written value wins over the default.
    db.rows(r#"accounts.insert { id: "a2", email: "bob@x.io", tenant: "t1", handle: "bob", age: 20, plan: "pro" }"#);
    assert_eq!(
        db.scalar(r#"accounts.where { .id == "a2" }.select { plan }"#, "plan"),
        text("pro")
    );

    // Uniqueness, single and composite.
    let error = db
        .try_rows(r#"accounts.insert { id: "a3", email: "ada@x.io", tenant: "t2", handle: "x", age: 1 }"#)
        .unwrap_err();
    assert!(error.to_lowercase().contains("unique"), "{error}");
    let error = db
        .try_rows(r#"accounts.insert { id: "a4", email: "d@x.io", tenant: "t1", handle: "ada", age: 1 }"#)
        .unwrap_err();
    assert!(error.to_lowercase().contains("unique"), "{error}");
    // The same handle under another tenant is a different key.
    db.rows(r#"accounts.insert { id: "a5", email: "e@x.io", tenant: "t2", handle: "ada", age: 1 }"#);
    let error = db
        .try_rows(r#"accounts.where { .id == "a5" }.update { email: "bob@x.io" }"#)
        .unwrap_err();
    assert!(error.to_lowercase().contains("unique"), "{error}");

    // Checks hold on insert and on update.
    for bad in [
        r#"accounts.insert { id: "a6", email: "f@x.io", tenant: "t3", handle: "f", age: -1 }"#,
        r#"accounts.where { .id == "a1" }.update { age: 200 }"#,
    ] {
        let error = db.try_rows(bad).unwrap_err();
        assert!(error.contains("check("), "{error}");
    }
    assert_eq!(db.scalar("accounts.count()", "count"), Value::Integer(3));

    // The declared indexes exist, and a matching read uses one.
    let mut indexes: Vec<_> = db
        .executor
        .store()
        .list_scalar_indexes(Some("accounts"))
        .unwrap()
        .into_iter()
        .map(|index| (index.name, index.unique))
        .collect();
    indexes.sort();
    assert_eq!(
        indexes,
        [
            ("accounts_by_plan_age".to_string(), false),
            ("accounts_unique_email".to_string(), true),
            ("accounts_unique_tenant_handle".to_string(), true),
        ]
    );
    let plan = db
        .executor
        .explain_indexed_read(&parse_oql(r#"accounts.where { .plan == "free" }"#).unwrap())
        .unwrap();
    assert!(plan.is_some(), "the declared index should serve this read");

    // A vector index is declared the same way.
    db.script(
        r#"
        schema notes { id: String, embedding: Vector(3), index(.embedding, metric: "cosine") }
        notes.insert [ { id: "n1", embedding: [1.0, 0.0, 0.0] }, { id: "n2", embedding: [0.0, 1.0, 0.0] } ]
        "#,
    );
    assert_eq!(
        db.ids(r#"notes.nearest { field: .embedding, query: [0.9, 0.1, 0.0], metric: "cosine", k: 1 }"#),
        ["n1"]
    );

    // Declarations that cannot be honoured are reported.
    for bad in [
        "schema b1 { id: String, unique(missing) }",
        "schema b2 { id: String, title: String on delete cascade }",
        "schema b3 { id: String, title: String as things }",
        r#"schema b4 { id: String, title: String, index(.title, metric: "cosine") }"#,
    ] {
        assert_eq!(db.script_errors(bad).len(), 1, "{bad}");
    }
    assert!(parse_oql_file("schema b5 { id: String, n: Integer sideways }").is_err());
}

#[test]
fn embedded_objects_have_declared_shapes() {
    let db = Db::new();
    db.script(
        r#"
        schema skus { id: String }
        schema shops {
          id: String,
          address: { city: String, zip: String?, geo: { lat: Float, lng: Float } },
          items: List({ sku: Ref(skus), qty: Integer, added: DateTime }),
          notes: Object
        }
        skus.insert { id: "pen" }
        shops.insert {
          id: "s1",
          address: { city: "Lagos", geo: { lat: 6.5, lng: 3.4 } },
          items: [ { sku: "pen", qty: 2, added: "2026-01-05" } ],
          notes: { anything: [1, "two"] }
        }
        "#,
    );

    // Values inside a declared shape are typed like top-level fields: the
    // timestamp is a timestamp and the id is a reference.
    let row = &db.rows(
        "shops.select { year: .items[0].added.year(), city: .address.city, lat: .address.geo.lat }",
    )[0];
    assert_eq!(row.get_field("year"), Some(&Value::Integer(2026)));
    assert_eq!(row.get_field("city"), Some(&text("Lagos")));
    assert_eq!(row.get_field("lat"), Some(&Value::Float(6.5)));

    let shop = |address: &str, items: &str| {
        format!(r#"shops.insert {{ id: "bad", address: {address}, items: {items}, notes: {{}} }}"#)
    };
    let good_address = r#"{ city: "Accra", geo: { lat: 1.0, lng: 2.0 } }"#;
    for (query, expected) in [
        (shop(r#"{ geo: { lat: 1.0, lng: 2.0 } }"#, "[]"), "city"),
        (shop(r#"{ city: "Accra", geo: { lat: 1.0 } }"#, "[]"), "lng"),
        (shop(r#"{ city: "Accra", extra: 1, geo: { lat: 1.0, lng: 2.0 } }"#, "[]"), "extra"),
        (shop(r#"{ city: 7, geo: { lat: 1.0, lng: 2.0 } }"#, "[]"), "String"),
        (shop(good_address, r#"[ { sku: "pen", qty: "many", added: "2026-01-01" } ]"#), "Integer"),
        (shop(good_address, r#"[ { sku: "pen", qty: 1, added: "someday" } ]"#), "timestamp"),
        (shop(good_address, r#"[ { sku: "ghost", qty: 1, added: "2026-01-01" } ]"#), "ghost"),
    ] {
        let error = db.try_rows(&query).unwrap_err();
        assert!(error.contains(expected), "{query}\n{error}");
    }
    // Updates are held to the shape too.
    let error = db
        .try_rows(r#"shops.where { .id == "s1" }.update { address.city: 5 }"#)
        .unwrap_err();
    assert!(error.contains("String"), "{error}");
    db.rows(r#"shops.where { .id == "s1" }.update { address.zip: "100001" }"#);
    assert_eq!(db.scalar("shops.select { zip: .address.zip }", "zip"), text("100001"));
    assert_eq!(db.scalar("shops.count()", "count"), Value::Integer(1));
}

#[test]
fn a_reverse_relationship_can_be_named_where_it_is_declared() {
    let db = Db::new();
    db.script(
        r#"
        schema accounts { id: String }
        schema transfers {
          id: String,
          from: Ref(accounts) as outgoing,
          to: Ref(accounts) as incoming,
          amount: Integer
        }
        accounts.insert [ { id: "a" }, { id: "b" }, { id: "c" } ]
        transfers.insert [
          { id: "t1", from: "a", to: "b", amount: 10 },
          { id: "t2", from: "a", to: "c", amount: 20 },
          { id: "t3", from: "b", to: "a", amount: 5 }
        ]
        "#,
    );

    let rows = db.rows(
        "accounts.sort { .id.asc }.select { id, sent: .outgoing.sum(.amount), received: .incoming.sum(.amount), touched: .transfers.count() }",
    );
    let figures: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.get_field("sent").cloned(),
                row.get_field("received").cloned(),
                row.get_field("touched").cloned(),
            )
        })
        .collect();
    let int = |value| Some(Value::Integer(value));
    assert_eq!(
        figures,
        [
            (int(30), int(5), int(3)),
            (int(5), int(10), int(2)),
            (int(0), int(20), int(1)),
        ]
    );
    // A named relationship is a collection like any other.
    assert_eq!(db.ids("accounts.where { .outgoing.where { .amount > 15 }.exists() }"), ["a"]);
    assert_eq!(
        db.scalar(r#"accounts.where { .id == "a" }.select { n: .outgoing.to.incoming.count() }"#, "n"),
        Value::Integer(2)
    );
}

#[test]
fn deleting_a_referenced_document_follows_the_declared_rule() {
    let db = Db::new();
    db.script(
        r#"
        schema users { id: String }
        schema posts { id: String, author: Ref(users) on delete cascade }
        schema comments { id: String, post: Ref(posts) on delete cascade, reviewer: Optional(Ref(users)) on delete nullify }
        schema invoices { id: String, payer: Ref(users) }

        users.insert [ { id: "ada" }, { id: "bob" }, { id: "cy" } ]
        posts.insert [ { id: "p1", author: "ada" }, { id: "p2", author: "bob" } ]
        comments.insert [
          { id: "c1", post: "p1", reviewer: "bob" },
          { id: "c2", post: "p2", reviewer: "ada" },
          { id: "c3", post: "p2" }
        ]
        invoices.insert { id: "i1", payer: "cy" }
        "#,
    );

    // The default is to refuse, and to leave everything as it was.
    let error = db.try_rows(r#"users.where { .id == "cy" }.delete()"#).unwrap_err();
    assert!(error.contains("invoices") && error.contains("payer"), "{error}");
    // A batch containing one such document deletes nothing.
    assert!(db.try_rows("users.delete()").is_err());
    assert_eq!(db.scalar("users.count()", "count"), Value::Integer(3));
    assert_eq!(db.scalar("posts.count()", "count"), Value::Integer(2));

    // Cascade follows the chain; nullify clears the reference instead.
    db.rows(r#"users.where { .id == "ada" }.delete()"#);
    assert_eq!(db.ids("posts.select { id }"), ["p2"]);
    let mut comments = db.ids("comments.select { id }");
    comments.sort();
    assert_eq!(comments, ["c2", "c3"]);
    assert_eq!(
        db.scalar(r#"comments.where { .id == "c2" }.select { reviewer }"#, "reviewer"),
        Value::Null
    );

    // Once nothing refers to it, the restricted document can go.
    db.rows(r#"invoices.where { .id == "i1" }.delete()"#);
    db.rows(r#"users.where { .id == "cy" }.delete()"#);
    assert_eq!(db.ids("users.select { id }"), ["bob"]);
}

#[test]
fn redeclaring_a_schema_changes_it_when_existing_documents_allow() {
    let db = Db::new();
    db.script(
        r#"
        schema people { id: String, name: String, age: Integer, legacy: String }
        people.insert [
          { id: "p1", name: "Ada", age: 36, legacy: "x" },
          { id: "p2", name: "Bob", age: 20, legacy: "y" }
        ]
        "#,
    );
    let original = "id: String, name: String, age: Integer, legacy: String";

    // Changes that could invalidate or silently lose data are refused, and
    // say what to do instead.
    for (change, expected) in [
        ("id: String, name: String, age: Integer", "legacy: removed"),
        (&format!("{original}, email: String"), "default"),
        ("id: String, name: Integer, age: Integer, legacy: String", "cannot change from String to Integer"),
        ("id: String, name: String, age: String, legacy: String", "cannot change"),
    ] {
        let errors = db.script_errors(&format!("schema people {{ {change} }}"));
        assert_eq!(errors.len(), 1, "{change}");
        assert!(errors[0].contains(expected), "{change}\n{}", errors[0]);
    }
    // A refused change leaves the collection exactly as it was.
    assert_eq!(db.scalar(r#"people.where { .id == "p1" }.select { legacy }"#, "legacy"), text("x"));
    assert!(db.try_rows(r#"people.insert { id: "p3", name: "Cy", age: 1 }"#).is_err());

    // Add an optional field, a defaulted field, widen a number, rename, and
    // drop, in one reviewable declaration.
    db.script(
        r#"
        schema people {
          id: String,
          full_name: String from name,
          age: Float,
          nickname: String?,
          plan: String = "free",
          tier: String = if .age >= 30 then "senior" else "junior",
          legacy: removed,
          index(.plan)
        }
        "#,
    );
    let rows = db.rows("people.sort { .id.asc }");
    assert_eq!(rows[0].get_field("full_name"), Some(&text("Ada")));
    assert_eq!(rows[0].get_field("name"), None);
    assert_eq!(rows[0].get_field("legacy"), None);
    assert_eq!(rows[0].get_field("age"), Some(&Value::Float(36.0)));
    assert_eq!(rows[0].get_field("plan"), Some(&text("free")));
    assert_eq!(rows[0].get_field("tier"), Some(&text("senior")));
    assert_eq!(rows[1].get_field("tier"), Some(&text("junior")));
    assert_eq!(rows[0].get_field("nickname"), None);
    // The new shape governs new writes.
    db.rows(r#"people.insert { id: "p3", full_name: "Cy", age: 1.5 }"#);
    assert!(db.try_rows(r#"people.insert { id: "p4", name: "Old", age: 1.0 }"#).is_err());
    assert_eq!(db.ids(r#"people.where { .plan == "free" }.sort { .id.asc }"#), ["p1", "p2", "p3"]);

    // A field may become optional, and an index that is no longer declared
    // goes away. Running the same declaration twice changes nothing.
    let relaxed = r#"schema people { id: String, full_name: String, age: Float?, nickname: String?, plan: String = "free", tier: String }"#;
    db.script(relaxed);
    db.script(relaxed);
    assert!(db
        .executor
        .store()
        .list_scalar_indexes(Some("people"))
        .unwrap()
        .is_empty());
    assert_eq!(db.scalar("people.count()", "count"), Value::Integer(3));
    // Optional cannot quietly become required again while a value is absent.
    db.rows(r#"people.insert { id: "p5", full_name: "Di", tier: "junior" }"#);
    let errors = db.script_errors(
        r#"schema people { id: String, full_name: String, age: Float, nickname: String?, plan: String = "free", tier: String }"#,
    );
    assert_eq!(errors.len(), 1);
    // An empty collection may be reshaped freely.
    db.script("schema drafts { id: String, title: String }\nschema drafts { id: String, words: Integer }");
    db.rows(r#"drafts.insert { id: "d1", words: 3 }"#);
}

#[test]
fn a_collection_can_be_dropped_when_nothing_refers_to_it() {
    let db = Db::new();
    db.script(
        r#"
        schema authors { id: String, email: String unique }
        schema books { id: String, author: Ref(authors) }
        authors.insert { id: "a1", email: "a@x.io" }
        books.insert { id: "b1", author: "a1" }
        "#,
    );

    let errors = db.script_errors("drop authors");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("books.author"), "{}", errors[0]);
    assert_eq!(db.scalar("authors.count()", "count"), Value::Integer(1));

    db.script("drop books\ndrop authors");
    assert_eq!(db.scalar("authors.count()", "count"), Value::Integer(0));
    assert!(db.executor.store().get_schema("authors").unwrap().is_none());
    assert!(db
        .executor
        .store()
        .list_scalar_indexes(Some("authors"))
        .unwrap()
        .is_empty());
    assert_eq!(db.script_errors("drop authors").len(), 1);

    // The name is free for a different shape.
    db.script("schema authors { id: String, pen_name: String }\nauthors.insert { id: \"a1\", pen_name: \"Q\" }");
    assert_eq!(db.scalar("authors.select { pen_name }", "pen_name"), text("Q"));
}

// ---------------------------------------------------------------------------
// Scripts, transactions and tooling (specification phase 5)
// ---------------------------------------------------------------------------

#[test]
fn a_transaction_block_is_all_or_nothing() {
    let db = Db::new();
    db.script(
        r#"
        schema wallets { id: String, balance: Integer, check(.balance >= 0) }
        wallets.insert [ { id: "w1", balance: 100 }, { id: "w2", balance: 50 } ]
        "#,
    );

    // The second update would overdraw, so neither is kept, nor is the
    // function the block defined.
    let errors = db.script_errors(
        r#"
        transaction {
          fn doubled(n: Integer) -> Integer { n * 2 }
          wallets.where { .id == "w2" }.update { balance += 500 }
          wallets.insert { id: "w3", balance: 1 }
          wallets.where { .id == "w1" }.update { balance -= 500 }
        }
        "#,
    );
    assert!(errors.iter().any(|error| error.contains("rolled back")), "{errors:?}");
    assert_eq!(db.scalar("wallets.sum(.balance)", "sum"), Value::Integer(150));
    assert_eq!(db.scalar("wallets.count()", "count"), Value::Integer(2));
    assert!(db
        .script_errors("wallets.select { v: doubled(.balance) }")
        .iter()
        .any(|error| error.contains("doubled")));

    // A block that succeeds is kept whole, and later statements in it see
    // the earlier ones.
    db.script(
        r#"
        transaction {
          wallets.where { .id == "w1" }.update { balance -= 40 }
          wallets.where { .id == "w2" }.update { balance += 40 }
          wallets.insert { id: "w3", balance: 0 }
          wallets.where { .id == "w3" }.update { balance: wallets.where { .id == "w2" }.first().balance }
        }
        "#,
    );
    assert_eq!(db.ids("wallets.sort { .id.asc }.select { id }"), ["w1", "w2", "w3"]);
    assert_eq!(
        db.scalar(r#"wallets.where { .id == "w3" }.select { balance }"#, "balance"),
        Value::Integer(90)
    );
    assert_eq!(db.scalar("wallets.sum(.balance)", "sum"), Value::Integer(240));
}

#[test]
fn a_script_stops_at_the_first_statement_that_fails() {
    let db = Db::new();
    let errors = db.script_errors(
        r#"
        steps.insert { id: "one" }
        steps.insert { id: "one" }
        steps.insert { id: "three" }
        steps.where { .id == "one" }.delete()
        "#,
    );
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].contains("already exists"));
    assert!(errors[1].contains("2 later statement(s) were not run"));
    assert_eq!(db.ids("steps.select { id }"), ["one"]);
}

#[test]
fn functions_have_local_names_and_can_be_redefined() {
    let mut db = Db::new();
    db.script(
        r#"
        lines.insert [
          { id: "l1", price: 20.0, qty: 2, discount: 5.0 },
          { id: "l2", price: 10.0, qty: 1 }
        ]
        "#,
    );
    db.define(
        "fn net(l: lines) -> Float {\n  gross = l.price * l.qty\n  off = coalesce(l.discount, 0.0)\n  gross - off\n}",
    );
    db.define(
        "fn is_large(l: lines) -> Bool {\n  gross = l.price * l.qty\n  threshold = 30\n  gross > threshold\n}",
    );
    let rows = db.rows("lines.sort { .id.asc }.select { id, net: net(*) }");
    assert_eq!(rows[0].get_field("net"), Some(&Value::Float(35.0)));
    assert_eq!(rows[1].get_field("net"), Some(&Value::Float(10.0)));
    assert_eq!(db.ids("lines.where { is_large(*) }"), ["l1"]);

    // A later definition replaces an earlier one.
    db.define("fn net(l: lines) -> Float { 0.0 }");
    assert_eq!(db.scalar("lines.select { v: net(*) }.limit(1)", "v"), Value::Float(0.0));

    for (source, expected) in [
        ("fn f(n: Integer) -> Integer { n = 1  n }", "hide the parameter"),
        ("fn f(n: Integer) -> Integer { a = 1  a = 2  a }", "already defined"),
    ] {
        let error = parse_oql_statement(source).unwrap_err().to_string();
        assert!(error.contains(expected), "{source}: {error}");
    }
    let builtin = match parse_oql_statement("fn upper(s: String) -> String { s }").unwrap() {
        owl_db::parser::OQLStatement::FunctionDef(definition) => definition,
        _ => unreachable!(),
    };
    assert!(db
        .executor
        .register_user_function(builtin)
        .unwrap_err()
        .to_string()
        .contains("built-in"));
}

#[test]
fn parse_errors_point_at_the_users_text() {
    let error = parse_oql("users.where { .age between 1 }").unwrap_err().to_string();
    assert!(error.contains("'between'"), "{error}");
    assert!(error.contains("line 1, column 20"), "{error}");
    assert!(error.contains("users.where { .age between 1 }\n"), "{error}");
    assert!(error.ends_with("                   ^"), "{error:?}");

    let error = parse_oql("users\n  .where { .age > 1 }\n  .select { id, name: }")
        .unwrap_err()
        .to_string();
    assert!(error.contains("line 3"), "{error}");
    assert!(error.contains(".select { id, name: }"), "{error}");
    assert!(error.contains("'}'"), "{error}");

    // No parser-internal names reach the user.
    for query in [
        "users.where { .age between 1 }",
        "users.select { id, name: }",
        "users.where { .a == }",
        "users.limit(",
        "users.insert { id \"x\" }",
        "users.sort { .a.asc",
    ] {
        let error = parse_oql(query).unwrap_err().to_string();
        for internal in [
            "Identifier(", "FieldAccess(", "RightBrace", "LeftBrace", "RightParen", "Some(",
            "EOF", "StringLiteral(", "Colon",
        ] {
            assert!(!error.contains(internal), "{query}: {error}");
        }
    }

    // In a script, the failing statement is named by its line.
    let error = parse_oql_file("users.insert { id: \"a\" }\n\nusers.where { .x == }\n")
        .unwrap_err()
        .to_string();
    assert!(error.contains("starts on line 3"), "{error}");

    // Nesting is bounded with an error, not a crash.
    let deep = format!("users.where {{ {}.a == 1{} }}", "(".repeat(300), ")".repeat(300));
    let error = parse_oql(&deep).unwrap_err().to_string();
    assert!(error.contains("nested more than"), "{error}");
    let shallow = format!("users.where {{ {}.a == 1{} }}", "(".repeat(20), ")".repeat(20));
    assert!(parse_oql(&shallow).is_ok());
}

#[test]
fn explain_reports_how_a_query_would_run() {
    let db = Db::new();
    db.script(
        r#"
        schema tasks { id: String, state: String, points: Integer, index(.state) }
        tasks.insert [ { id: "t1", state: "open", points: 3 }, { id: "t2", state: "done", points: 5 } ]
        "#,
    );

    let indexed = &db.rows(r#"tasks.where { .state == "open" }.explain()"#)[0];
    assert_eq!(indexed.get_field("path"), Some(&text("physical pipeline")));
    assert_eq!(indexed.get_field("uses_index"), Some(&Value::Boolean(true)));
    assert!(matches!(indexed.get_field("plan"), Some(Value::String(plan)) if !plan.is_empty()));

    let scanned = &db.rows("tasks.where { .points > 3 }.explain()")[0];
    assert_eq!(scanned.get_field("path"), Some(&text("physical pipeline")));
    assert_eq!(scanned.get_field("uses_index"), Some(&Value::Boolean(false)));

    let general = &db.rows("tasks.group_by { .state }.select { state, n: count() }.explain()")[0];
    assert_eq!(general.get_field("path"), Some(&text("general executor")));

    // Explaining does not run the query.
    db.rows(r#"tasks.where { .state == "open" }.delete().explain()"#);
    assert_eq!(db.scalar("tasks.count()", "count"), Value::Integer(2));
    assert!(db
        .try_rows("tasks.explain().limit(1)")
        .unwrap_err()
        .contains("last operation"));
}

#[test]
fn select_can_leave_fields_out() {
    let db = Db::new();
    db.script(r#"profiles.insert { id: "p1", name: "Ada", secret: "s3", count: 2, tags: ["x"] }"#);

    let row = &db.rows("profiles.select { *, -secret, -count }")[0];
    assert_eq!(row.get_field("name"), Some(&text("Ada")));
    assert_eq!(row.get_field("secret"), None);
    assert_eq!(row.get_field("count"), None);
    assert!(row.get_field("tags").is_some());

    // Order matters, as everywhere else: what comes later decides.
    let row = &db.rows(r#"profiles.select { *, -name, name: "shown", extra: -1 }"#)[0];
    assert_eq!(row.get_field("name"), Some(&text("shown")));
    assert_eq!(row.get_field("extra"), Some(&Value::Integer(-1)));
}

#[test]
fn an_unknown_name_on_a_declared_shape_is_an_error() {
    let db = graph();

    for (query, expected) in [
        ("users.where { .nmae == \"Ada\" }", "Did you mean 'name'?"),
        ("users.select { id, .nickame }", "Did you mean 'nickname'?"),
        ("users.sort { .zzz.asc }", "no field or relationship 'zzz'"),
        ("users.where { .tean.name == \"Core\" }", "Did you mean 'team'?"),
        ("users.sum(.agee)", "no field or relationship 'agee'"),
        ("users.where { .name == \"Ada\" }.select { x: .profil.city }", "Did you mean 'profile'?"),
    ] {
        let error = db.try_rows(query).unwrap_err();
        assert!(error.contains(expected), "{query}: {error}");
    }

    // Everything that is declared, derived or system-provided is fine.
    db.rows("users.where { .nickname == null }.select { id, created_at, n: .orders.count(), t: .team.name }");
    db.rows("users.select { id, shout: upper(.name) }.where { .shout == \"ADA\" }.sort { .shout.asc }");
    db.rows("orders.group_by { .status }.select { status, n: count() }.sort { .n.desc }");
    // Paths inside a nested collection are judged against its own rows.
    db.rows("users.select { id, big: .orders.where { .amount > 100 }.select { amount } }");
    // A collection with no declared shape stays graceful.
    db.script(r#"loose.insert { id: "l" }"#);
    assert_eq!(db.scalar("loose.select { v: .anything }", "v"), Value::Null);
}

#[test]
fn remaining_expression_forms_behave() {
    let db = graph();

    // Match arms may carry a guard.
    let rows = db.rows(
        r#"orders.sort { .id.asc }.select { id, size: match .status { "paid" if .amount > 200 -> "big", "paid" -> "paid", _ -> "other" } }"#,
    );
    let sizes: Vec<_> = rows.iter().map(|row| row.get_field("size").cloned()).collect();
    assert_eq!(
        sizes,
        [Some(text("paid")), Some(text("other")), Some(text("big")), Some(text("other"))]
    );

    assert_eq!(
        db.scalar(r#"orgs.select { a: to_bool("yes"), b: to_bool(0), c: to_bool("maybe") }.limit(1)"#, "a"),
        Value::Boolean(true)
    );
    assert_eq!(
        db.scalar(r#"orgs.select { y: to_datetime("2026-03-04").year(), bad: to_datetime("soon") }.limit(1)"#, "y"),
        Value::Integer(2026)
    );

    // Inside a nested query over the same collection, the collection name
    // is the enclosing row and `.` is the row being considered.
    let rows = db.rows(
        "users.sort { .id.asc }.select { id, teammates: users.where { .team == users.team && .id != users.id }.count() }",
    );
    let counts: Vec<_> = rows.iter().map(|row| row.get_field("teammates").cloned()).collect();
    let int = |value| Some(Value::Integer(value));
    assert_eq!(counts, [int(1), int(0), int(1), int(0)]);
}
