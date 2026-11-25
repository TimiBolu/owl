//! The completeness matrix: what SQL and MongoDB can say, and whether Owl can.
//!
//! Every row names a feature, how SQL and MongoDB spell it, and one of three
//! answers for Owl:
//!
//! - **yes**: the OQL on the row is run here against a small shop and has to
//!   give the answer written beside it;
//! - **different**: Owl reaches the same end another way on purpose, and that
//!   way is run here the same;
//! - **no**: Owl cannot do it. Where there is something a user would try, it
//!   is run here and has to be refused, so the row is corrected the day it
//!   starts working.
//!
//! A few rows are about behaviour no one query shows (durability, backup).
//! Those name the test that covers them, and this file checks it exists.
//!
//! `docs/COMPLETENESS_MATRIX.md` is written from these rows and checked
//! against them. After changing a row:
//!
//! ```bash
//! OWL_WRITE_MATRIX=1 cargo test --release --test completeness_matrix_test
//! ```

use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, QueryExecutor};
use std::fmt::Write as _;
use std::sync::Arc;
use tempfile::TempDir;

const DOCUMENT: &str = "docs/COMPLETENESS_MATRIX.md";

/// The shop every row is run against.
const SHOP: &str = r#"
schema customers {
  id: String,
  name: String,
  email: String?,
  city: String,
  joined: DateTime,
  tags: List(String),
  address: { street: String, zip: String },
  referrer: Optional(Ref(customers)) as referred
}
schema products {
  id: String,
  name: String,
  price: Float,
  stock: Integer,
  category: String
}
schema orders {
  id: String,
  customer: Ref(customers),
  status: String,
  placed: DateTime,
  total: Float,
  items: List({ product: Ref(products), qty: Integer, price: Float })
}
customers.insert [
  { id: "c1", name: "Ada", address: { street: "1 Marina", zip: "100001" }, email: "ada@x.io", city: "Lagos", joined: "2024-01-10", tags: ["vip", "early"] },
  { id: "c2", name: "Bob", address: { street: "2 Ring Rd", zip: "00233" }, city: "Accra", joined: "2024-03-02", tags: [], referrer: "c1" },
  { id: "c3", name: "Cy", address: { street: "3 Allen Ave", zip: "100271" }, email: "cy@y.org", city: "Lagos", joined: "2025-06-20", tags: ["vip"], referrer: "c1" },
  { id: "c4", name: "Di", address: { street: "4 Kenyatta Ave", zip: "00100" }, email: "di@x.io", city: "Nairobi", joined: "2025-07-01", tags: ["new"], referrer: "c3" },
  { id: "c5", name: "Eve", address: { street: "5 Oxford St", zip: "00233" }, email: "eve@y.org", city: "Accra", joined: "2026-02-14", tags: [] }
]
products.insert [
  { id: "p1", name: "Pen", price: 2.0, stock: 100, category: "stationery" },
  { id: "p2", name: "Book", price: 12.5, stock: 40, category: "stationery" },
  { id: "p3", name: "Lamp", price: 30.0, stock: 0, category: "home" },
  { id: "p4", name: "Desk", price: 150.0, stock: 5, category: "home" },
  { id: "p5", name: "Mug", price: 8.0, stock: 25, category: "kitchen" }
]
orders.insert [
  { id: "o1", customer: "c1", status: "paid", placed: "2026-01-05", total: 26.5,
    items: [ { product: "p1", qty: 7, price: 2.0 }, { product: "p2", qty: 1, price: 12.5 } ] },
  { id: "o2", customer: "c1", status: "paid", placed: "2026-02-10", total: 150.0,
    items: [ { product: "p4", qty: 1, price: 150.0 } ] },
  { id: "o3", customer: "c2", status: "open", placed: "2026-02-11", total: 60.0,
    items: [ { product: "p3", qty: 2, price: 30.0 } ] },
  { id: "o4", customer: "c3", status: "paid", placed: "2026-03-01", total: 42.0,
    items: [ { product: "p3", qty: 1, price: 30.0 }, { product: "p1", qty: 6, price: 2.0 } ] },
  { id: "o5", customer: "c3", status: "cancelled", placed: "2026-03-02", total: 12.5,
    items: [ { product: "p2", qty: 1, price: 12.5 } ] },
  { id: "o6", customer: "c4", status: "paid", placed: "2026-03-15", total: 25.0,
    items: [ { product: "p2", qty: 2, price: 12.5 } ] }
]
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    Different,
    No,
}

impl Answer {
    fn word(self) -> &'static str {
        match self {
            Answer::Yes => "yes",
            Answer::Different => "different",
            Answer::No => "no",
        }
    }
}

struct Row {
    feature: &'static str,
    sql: &'static str,
    mongo: &'static str,
    answer: Answer,
    /// What is run. A query on its own, or a script followed by `check`.
    oql: &'static str,
    /// The query whose rows are compared, when `oql` is a script.
    check: &'static str,
    /// The rows expected, one JSON object per row, separated by a space.
    expect: &'static str,
    /// The answer only has to hold `expect`, where the rest is not fixed.
    holding: bool,
    /// What a user might try that has to be refused.
    refuses: &'static str,
    /// A test elsewhere that covers the row: file, then function.
    test: Option<(&'static str, &'static str)>,
    note: &'static str,
}

impl Row {
    fn note(mut self, note: &'static str) -> Self {
        self.note = note;
        self
    }
    /// The rows a script leaves behind are read with this query.
    fn check(mut self, check: &'static str) -> Self {
        self.check = check;
        self
    }
    /// Owl gets there another way, on purpose.
    fn differently(mut self) -> Self {
        self.answer = Answer::Different;
        self
    }
    /// What a user might try, which has to be refused.
    fn refusing(mut self, oql: &'static str) -> Self {
        self.refuses = oql;
        self
    }
    /// What happens instead, for a row marked no: the script, the query that
    /// shows it, and what the answer holds.
    fn shown(mut self, oql: &'static str, check: &'static str, expect: &'static str) -> Self {
        self.oql = oql;
        self.check = check;
        self.expect = expect;
        self.holding = true;
        self
    }
    /// The answer has more in it than is fixed; it has to hold `expect`.
    fn holding(mut self) -> Self {
        self.holding = true;
        self
    }
}

fn yes(
    feature: &'static str,
    sql: &'static str,
    mongo: &'static str,
    oql: &'static str,
    expect: &'static str,
) -> Row {
    Row {
        feature,
        sql,
        mongo,
        answer: Answer::Yes,
        oql,
        check: "",
        expect,
        holding: false,
        refuses: "",
        test: None,
        note: "",
    }
}

fn no(feature: &'static str, sql: &'static str, mongo: &'static str, note: &'static str) -> Row {
    Row {
        feature,
        sql,
        mongo,
        answer: Answer::No,
        oql: "",
        check: "",
        expect: "",
        holding: false,
        refuses: "",
        test: None,
        note,
    }
}

fn tested(
    feature: &'static str,
    sql: &'static str,
    mongo: &'static str,
    file: &'static str,
    function: &'static str,
) -> Row {
    Row {
        feature,
        sql,
        mongo,
        answer: Answer::Yes,
        oql: "",
        check: "",
        expect: "",
        holding: false,
        refuses: "",
        test: Some((file, function)),
        note: "",
    }
}

/// Something the engine gets wrong, found while writing a row. The matrix
/// uses a form that works; this pins the wrong answer so that fixing it
/// takes it off the list.
struct Defect {
    what: &'static str,
    oql: &'static str,
    gives: &'static str,
    should: &'static str,
}

include!("completeness_matrix/rows.rs");

struct Db {
    _directory: TempDir,
    executor: QueryExecutor<RustStore>,
}

impl Db {
    fn shop() -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
        let db = Self {
            _directory: directory,
            executor: QueryExecutor::new_with_real_clock(store),
        };
        db.script(SHOP).unwrap();
        db
    }

    fn script(&self, source: &str) -> Result<(), String> {
        let file = parse_oql_file(source).map_err(|error| error.to_string())?;
        let report = execute_oql_file(&file, &self.executor).map_err(|error| error.to_string())?;
        if report.errors.is_empty() {
            Ok(())
        } else {
            Err(report.errors.join("; "))
        }
    }

    fn rows(&self, query: &str) -> Result<String, String> {
        let query = parse_oql(query).map_err(|error| error.to_string())?;
        let rows = self
            .executor
            .execute_query(&query)
            .map_err(|error| error.to_string())?;
        Ok(rows
            .iter()
            .map(|row| row.fields_to_clean_json().to_string())
            .collect::<Vec<_>>()
            .join(" "))
    }
}

/// What running a row gives: its rows, or why it was refused.
fn run(oql: &str, check: &str) -> Result<String, String> {
    let db = Db::shop();
    if check.is_empty() {
        // A query if it is one, and otherwise a script.
        db.rows(oql)
            .or_else(|_| db.script(oql).map(|()| String::new()))
    } else {
        db.script(oql)?;
        db.rows(check)
    }
}

#[test]
fn every_answer_in_the_matrix_is_what_the_engine_does() {
    let probing = std::env::var("OWL_MATRIX_PROBE").is_ok();
    let mut wrong = Vec::new();
    for (area, rows) in matrix() {
        for row in rows {
            let name = format!("{area} / {}", row.feature);
            if let Some((file, function)) = row.test {
                let source = std::fs::read_to_string(file).unwrap_or_default();
                if !source.contains(&format!("fn {function}(")) {
                    wrong.push(format!("{name}: no test {function} in {file}"));
                }
            }
            if !row.refuses.is_empty() {
                let found = run(row.refuses, "");
                if probing {
                    println!("[refuses] {name}\n    {found:?}");
                } else if let Ok(rows) = found {
                    wrong.push(format!(
                        "{name}: {} should be refused, but gave {rows}",
                        row.refuses
                    ));
                }
            }
            if row.oql.is_empty() {
                assert!(
                    row.answer == Answer::No || row.test.is_some(),
                    "{name}: marked {} with nothing to show for it",
                    row.answer.word()
                );
                continue;
            }
            let found = run(row.oql, row.check);
            if probing {
                println!("[{}] {name}\n    {found:?}", row.answer.word());
                continue;
            }
            match found {
                Err(error) => wrong.push(format!("{name}: {error}")),
                Ok(rows) if row.holding && rows.contains(row.expect) => {}
                Ok(rows) if rows == row.expect => {}
                Ok(rows) => wrong.push(format!(
                    "{name}:\n  expected {}\n  found    {rows}",
                    row.expect
                )),
            }
        }
    }
    for defect in defects() {
        let found = run(defect.oql, "");
        if probing {
            println!("[defect] {}\n    {found:?}", defect.what);
        } else if found.as_deref() != Ok(defect.gives) {
            wrong.push(format!(
                "defect \"{}\" no longer gives {}: it gives {found:?}. If it is fixed, take it off the list.",
                defect.what, defect.gives
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} rows are wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn the_written_matrix_is_these_rows() {
    let written = render();
    if std::env::var("OWL_WRITE_MATRIX").is_ok() {
        std::fs::write(DOCUMENT, &written).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(DOCUMENT).unwrap_or_default();
    assert!(
        on_disk == written,
        "{DOCUMENT} is out of date. Run:\n  OWL_WRITE_MATRIX=1 cargo test --release --test completeness_matrix_test"
    );
}

/// Code in a table cell: one code span a line, with the bar escaped.
fn code(source: &str) -> String {
    // Words in brackets are a remark, not code.
    if let Some(remark) = source
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let mut remark = remark.to_string();
        if let Some(first) = remark.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        return remark;
    }
    if source.is_empty() {
        return "No rows".to_string();
    }
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| format!("`{}`", line.replace('|', "\\|")))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn render() -> String {
    let areas = matrix();
    let mut out = String::new();
    out.push_str(INTRODUCTION);

    out.push_str("\n## Summary\n\n| Area | Yes | Different | No |\n|---|---:|---:|---:|\n");
    let mut totals = [0usize; 3];
    for (area, rows) in &areas {
        let mut counts = [0usize; 3];
        for row in rows {
            counts[row.answer as usize] += 1;
        }
        for (total, count) in totals.iter_mut().zip(counts) {
            *total += count;
        }
        writeln!(
            out,
            "| {area} | {} | {} | {} |",
            counts[0], counts[1], counts[2]
        )
        .unwrap();
    }
    writeln!(
        out,
        "| **All** | **{}** | **{}** | **{}** |",
        totals[0], totals[1], totals[2]
    )
    .unwrap();

    for (area, rows) in &areas {
        write!(
            out,
            "\n## {area}\n\n| Feature | SQL | MongoDB | Owl | In Owl |\n|---|---|---|---|---|\n"
        )
        .unwrap();
        for row in rows {
            let mut how = Vec::new();
            if !row.oql.is_empty() {
                how.push(code(row.oql));
            }
            if !row.check.is_empty() && row.check.contains(".explain()") {
                how.push(code(row.check));
            }
            if let Some((file, function)) = row.test {
                how.push(format!("Covered by `{function}` in `{file}`."));
            }
            if !row.note.is_empty() {
                how.push(row.note.to_string());
            }
            if !row.refuses.is_empty() {
                how.push(format!("Refused: {}", code(row.refuses)));
            }
            writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                row.feature,
                code(row.sql),
                code(row.mongo),
                row.answer.word(),
                how.join("<br>")
            )
            .unwrap();
        }
    }

    out.push_str(DEFECTS);
    if defects().is_empty() {
        out.push_str("\nNone at present.\n");
        return out;
    }
    out.push_str("\n| What | Query | Gives | Should give |\n|---|---|---|---|\n");
    for defect in defects() {
        writeln!(
            out,
            "| {} | {} | {} | {} |",
            defect.what,
            code(defect.oql),
            code(defect.gives),
            code(defect.should)
        )
        .unwrap();
    }
    out
}

/// Tries the queries in a file, one a line, against the shop and prints what
/// each gives. For finding out what a new row should say:
///
/// ```bash
/// OWL_MATRIX_TRY=queries.txt cargo test --release --test completeness_matrix_test tries -- --nocapture
/// ```
#[test]
fn tries_the_queries_in_a_file() {
    let Ok(path) = std::env::var("OWL_MATRIX_TRY") else {
        return;
    };
    for query in std::fs::read_to_string(path).unwrap().lines() {
        if query.trim().is_empty() {
            continue;
        }
        let db = Db::shop();
        let found = match query.split_once(" ;; ") {
            Some((script, check)) => db
                .script(&script.replace(" ; ", "\n"))
                .and_then(|()| db.rows(check)),
            None => db.rows(query),
        };
        println!("TRY {query}\n    {found:?}");
    }
}
