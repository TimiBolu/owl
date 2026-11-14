//! Executable conformance checks for the parts of the OQL contract that are
//! about least surprise: the same text means the same thing wherever it
//! appears and however the engine chooses to run it.

use owl_db::storage::RustStore;
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
