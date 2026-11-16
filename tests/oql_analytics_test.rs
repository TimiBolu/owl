//! The parts of OQL that analytical queries lean on: distinct counts and
//! percentiles, set operations, walking a relationship to any depth,
//! functions of a row's position, and reaching back from a list element to
//! the row that holds it. Each is checked on rows small enough to work out
//! by hand.

use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, Document, QueryExecutor, Value};
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

    fn try_script(&self, source: &str) -> Result<(), String> {
        let file = parse_oql_file(source).map_err(|error| error.to_string())?;
        let report = execute_oql_file(&file, &self.executor).map_err(|error| error.to_string())?;
        match report.errors.is_empty() {
            true => Ok(()),
            false => Err(report.errors.join("; ")),
        }
    }

    fn script(&self, source: &str) {
        self.try_script(source)
            .unwrap_or_else(|error| panic!("{error}\n{source}"));
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

    /// The named fields of every row, in result order.
    fn table(&self, query: &str, fields: &[&str]) -> Vec<Vec<Value>> {
        self.rows(query)
            .iter()
            .map(|row| {
                fields
                    .iter()
                    .map(|field| row.get_field(field).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect()
    }

    fn ids(&self, query: &str) -> Vec<String> {
        self.table(query, &["id"])
            .into_iter()
            .map(|row| match &row[0] {
                Value::String(id) => id.clone(),
                other => panic!("row has no string id: {other:?}"),
            })
            .collect()
    }

    fn scalar(&self, query: &str, field: &str) -> Value {
        let rows = self.rows(query);
        assert_eq!(rows.len(), 1, "expected one row from {query}");
        rows[0].get_field(field).cloned().unwrap_or(Value::Null)
    }
}

fn text(value: &str) -> Value {
    Value::String(value.to_string())
}

fn int(value: i64) -> Value {
    Value::Integer(value)
}

/// A small company: a tree of teams four levels deep, six people with a
/// chain of bosses, a few timed pings, a cycle of nodes, and jobs that each
/// hold a list of notes. Every tree is inserted in one statement.
fn company() -> Db {
    let db = Db::new();
    db.script(
        r#"
        schema teams { id: String, parent: Optional(Ref(teams)) as children }
        schema staff {
          id: String, team: Ref(teams), boss: Optional(Ref(staff)) as reports,
          pay: Integer, grade: Optional(Integer), median: Optional(Integer)
        }
        schema pings { id: String, who: Ref(staff), at: DateTime, n: Integer }
        schema nodes { id: String }
        schema edges { id: String, src: Ref(nodes) as outgoing, dst: Ref(nodes) as incoming }
        schema jobs { id: String, state: String, notes: List({ kind: String, mins: Integer }) }

        teams.insert [
          { id: "t1" }, { id: "t2", parent: "t1" }, { id: "t3", parent: "t1" },
          { id: "t4", parent: "t2" }, { id: "t5", parent: "t4" }
        ]
        staff.insert [
          { id: "a", team: "t1", pay: 100, grade: 1, median: 7 },
          { id: "b", team: "t2", boss: "a", pay: 80, grade: 2 },
          { id: "c", team: "t2", boss: "a", pay: 80 },
          { id: "d", team: "t4", boss: "b", pay: 60, grade: 2 },
          { id: "e", team: "t5", boss: "d", pay: 40, grade: 1 },
          { id: "f", team: "t3", boss: "a", pay: 90, grade: 3 }
        ]
        pings.insert [
          { id: "p1", who: "a", at: "2026-01-01T10:00:00Z", n: 5 },
          { id: "p2", who: "a", at: "2026-01-01T10:10:00Z", n: -2 },
          { id: "p3", who: "a", at: "2026-01-01T18:00:00Z", n: 4 },
          { id: "p4", who: "b", at: "2026-01-01T09:00:00Z", n: 1 },
          { id: "p5", who: "b", at: "2026-01-01T09:30:00Z", n: 1 }
        ]
        nodes.insert [ { id: "n1" }, { id: "n2" }, { id: "n3" } ]
        edges.insert [
          { id: "e1", src: "n1", dst: "n2" },
          { id: "e2", src: "n2", dst: "n3" },
          { id: "e3", src: "n3", dst: "n1" }
        ]
        jobs.insert [
          { id: "j1", state: "open", notes: [ { kind: "x", mins: 5 }, { kind: "y", mins: 7 } ] },
          { id: "j2", state: "done", notes: [ { kind: "x", mins: 1 } ] },
          { id: "j3", state: "open", notes: [ { kind: "x", mins: 2 } ] }
        ]
        "#,
    );
    db
}

#[test]
fn distinct_counts_and_percentiles_work_in_every_form() {
    let db = company();
    // The terminal form, over a path and over an expression.
    assert_eq!(db.scalar("staff.count_distinct(.pay)", "count_distinct"), int(5));
    assert_eq!(db.scalar("staff.count_distinct(.pay / 50)", "count_distinct"), int(3));
    // An absent value is not a value: `c` has no grade.
    assert_eq!(db.scalar("staff.count_distinct(.grade)", "count_distinct"), int(3));
    // 40 60 80 80 90 100: the middle is between the two eighties.
    assert_eq!(db.scalar("staff.median(.pay)", "median"), Value::Float(80.0));
    // A quarter of the way along five steps is 1.25: a quarter past 60.
    assert_eq!(
        db.scalar("staff.percentile(.pay, 0.25)", "percentile"),
        Value::Float(65.0)
    );
    assert_eq!(db.scalar("staff.percentile(.pay, 0)", "percentile"), Value::Float(40.0));
    assert_eq!(db.scalar("staff.percentile(.pay, 1)", "percentile"), Value::Float(100.0));

    // Per group.
    assert_eq!(
        db.table(
            "staff.group_by { .team.id }
               .select { team_id, people: count(), pays: count_distinct(.pay), mid: median(.pay),
                         top: percentile(.pay, 1) }
               .sort { .team_id.asc }",
            &["team_id", "people", "pays", "mid", "top"],
        ),
        vec![
            vec![text("t1"), int(1), int(1), Value::Float(100.0), Value::Float(100.0)],
            vec![text("t2"), int(2), int(1), Value::Float(80.0), Value::Float(80.0)],
            vec![text("t3"), int(1), int(1), Value::Float(90.0), Value::Float(90.0)],
            vec![text("t4"), int(1), int(1), Value::Float(60.0), Value::Float(60.0)],
            vec![text("t5"), int(1), int(1), Value::Float(40.0), Value::Float(40.0)],
        ]
    );
    // Over every row, with no grouping.
    assert_eq!(
        db.table("staff.select { mid: median(.pay), bosses: count_distinct(.boss) }", &["mid", "bosses"]),
        vec![vec![Value::Float(80.0), int(3)]]
    );
    // Through a relationship, in a select and in a filter.
    assert_eq!(
        db.table(
            "teams.where { .staff.median(.pay) >= 80 }.sort { .id.asc }
               .select { id, mid: .staff.median(.pay), bosses: .staff.count_distinct(.boss) }",
            &["id", "mid", "bosses"],
        ),
        vec![
            vec![text("t1"), Value::Float(100.0), int(0)],
            vec![text("t2"), Value::Float(80.0), int(1)],
            vec![text("t3"), Value::Float(90.0), int(1)],
        ]
    );

    // No rows: nothing to count, and no middle.
    assert_eq!(
        db.scalar("staff.where { .pay > 999 }.count_distinct(.pay)", "count_distinct"),
        int(0)
    );
    assert_eq!(db.scalar("staff.where { .pay > 999 }.median(.pay)", "median"), Value::Null);

    // What cannot mean anything is an error.
    let error = db.try_rows("staff.percentile(.pay, 1.5)").unwrap_err();
    assert!(error.contains("between 0 and 1"), "{error}");
    let error = db.try_rows("staff.median(.id)").unwrap_err();
    assert!(error.contains("numeric"), "{error}");
    assert!(db.try_rows("staff.where { median(.pay) > 1 }").is_err());

    // The words are operations only where they are applied: data may still
    // have a field called `median`.
    assert_eq!(
        db.table("staff.where { .median == 7 }.select { id, median }", &["id", "median"]),
        vec![vec![text("a"), int(7)]]
    );
    assert_eq!(db.ids("staff.sort { .median.desc, .id.asc }.limit(1)"), ["a"]);
}

#[test]
fn rows_of_two_queries_combine_as_sets() {
    let db = company();
    // A union keeps the order of the rows it is called on, then adds what
    // the other query has that it lacks.
    assert_eq!(
        db.ids("staff.sort { .id.desc }.limit(2).union(staff.sort { .id.asc }.limit(2))"),
        ["f", "e", "a", "b"]
    );
    // A row both queries have appears once.
    assert_eq!(
        db.scalar("staff.where { .pay >= 80 }.union(staff.where { .pay <= 80 }).count()", "count"),
        int(6)
    );
    assert_eq!(
        db.ids("staff.where { .pay >= 80 }.intersect(staff.where { .team == \"t2\" }).sort { .id.asc }"),
        ["b", "c"]
    );
    assert_eq!(
        db.ids("staff.where { .pay >= 80 }.except(staff.where { .team == \"t2\" }).sort { .id.asc }"),
        ["a", "f"]
    );
    // They chain, applied in the order written.
    assert_eq!(
        db.ids(
            "staff.where { .pay == 100 }
               .union(staff.where { .pay == 40 })
               .union(staff.where { .team == \"t2\" })
               .except(staff.where { .id == \"c\" })
               .sort { .id.asc }"
        ),
        ["a", "b", "e"]
    );
    // Shaped rows are compared by their fields, and a result is a set.
    assert_eq!(
        db.table(
            "staff.select { pay }.intersect(staff.where { .team == \"t2\" }.select { pay })",
            &["pay"]
        ),
        vec![vec![int(80)]]
    );
    assert_eq!(db.scalar("staff.select { pay }.union(staff.select { pay }).count()", "count"), int(5));
    // The rows are still the collection's rows: a relationship can be
    // followed from them.
    assert_eq!(
        db.table(
            "staff.where { .id == \"a\" }.union(staff.where { .id == \"b\" })
               .sort { .id.asc }.select { id, reports: .reports.count() }",
            &["id", "reports"],
        ),
        vec![vec![text("a"), int(3)], vec![text("b"), int(1)]]
    );
    // Nothing in common, and nothing left.
    assert!(db.rows("staff.where { .pay > 90 }.intersect(staff.where { .pay < 50 })").is_empty());
    assert!(db.rows("staff.except(staff)").is_empty());
    // The words remain usable as names.
    assert!(db.try_rows("staff.select { union: .pay, except: .id }").is_ok());
}

#[test]
fn a_walk_follows_a_relationship_to_any_depth() {
    let db = company();
    // Down a tree, nearest first, each row with the steps it took.
    assert_eq!(
        db.table(
            "teams.where { .id == \"t1\" }.walk(.children).sort { .id.asc }.select { id, depth: ._depth }",
            &["id", "depth"],
        ),
        vec![
            vec![text("t2"), int(1)],
            vec![text("t3"), int(1)],
            vec![text("t4"), int(2)],
            vec![text("t5"), int(3)],
        ]
    );
    // No further than asked.
    assert_eq!(
        db.ids("teams.where { .id == \"t1\" }.walk(.children, 2).sort { .id.asc }"),
        ["t2", "t3", "t4"]
    );
    assert!(db.rows("teams.where { .id == \"t1\" }.walk(.children, 0)").is_empty());
    // Up a chain of single references, in the order reached.
    assert_eq!(db.ids("staff.where { .id == \"e\" }.walk(.boss)"), ["d", "b", "a"]);
    // A leaf has nothing below it, and a root nothing above.
    assert!(db.rows("teams.where { .id == \"t5\" }.walk(.children)").is_empty());
    assert!(db.rows("staff.where { .id == \"a\" }.walk(.boss)").is_empty());

    // From each row of a select, and onward through another relationship.
    assert_eq!(
        db.table(
            "teams.sort { .id.asc }.select {
               id,
               below: .walk(.children).count(),
               deepest: .walk(.children).max(._depth),
               people: .staff.count() + .walk(.children).staff.count()
             }",
            &["id", "below", "deepest", "people"],
        ),
        vec![
            vec![text("t1"), int(4), int(3), int(6)],
            vec![text("t2"), int(2), int(2), int(4)],
            vec![text("t3"), int(0), Value::Null, int(1)],
            vec![text("t4"), int(1), int(1), int(2)],
            vec![text("t5"), int(0), Value::Null, int(1)],
        ]
    );
    // In a filter: everyone with at least three people under them.
    assert_eq!(db.ids("staff.where { .walk(.reports).count() >= 3 }.sort { .id.asc }"), ["a"]);
    assert_eq!(db.scalar("staff.where { .id == \"a\" }.walk(.reports).count()", "count"), int(5));

    // Through a collection that links rows of one kind, round a cycle: every
    // node is reached once, the start included, and the walk ends.
    assert_eq!(
        db.table(
            "nodes.where { .id == \"n1\" }.walk(.outgoing.dst).sort { .id.asc }.select { id, depth: ._depth }",
            &["id", "depth"],
        ),
        vec![
            vec![text("n1"), int(3)],
            vec![text("n2"), int(1)],
            vec![text("n3"), int(2)],
        ]
    );
    // Starting from several rows, a row reached from two of them is one row.
    assert_eq!(
        db.ids("teams.where { .id == \"t1\" || .id == \"t2\" }.walk(.children).sort { .id.asc }"),
        ["t2", "t3", "t4", "t5"]
    );
}

#[test]
fn position_functions_see_the_rows_around_a_row() {
    let db = company();
    let n = |value: i64| int(value);
    // Per person, in time order.
    assert_eq!(
        db.table(
            "pings.sort { .who.id.asc, .at.asc }.partition_by { .who }.select {
               id,
               k: row_number(),
               run: running_sum(.n),
               before: prev(.id),
               after: next(.id),
               gap: seconds_until(prev(.at), .at),
               low: running_min(.n),
               high: running_max(.n),
               seen: running_count()
             }",
            &["id", "k", "run", "before", "after", "gap", "low", "high", "seen"],
        ),
        vec![
            vec![text("p1"), n(1), n(5), Value::Null, text("p2"), Value::Null, n(5), n(5), n(1)],
            vec![text("p2"), n(2), n(3), text("p1"), text("p3"), n(600), n(-2), n(5), n(2)],
            vec![text("p3"), n(3), n(7), text("p2"), Value::Null, n(28_200), n(-2), n(5), n(3)],
            vec![text("p4"), n(1), n(1), Value::Null, text("p5"), Value::Null, n(1), n(1), n(1)],
            vec![text("p5"), n(2), n(2), text("p4"), Value::Null, n(1_800), n(1), n(1), n(2)],
        ]
    );
    // With no partition the whole result is one sequence.
    assert_eq!(
        db.table(
            "pings.sort { .id.asc }.select { id, k: row_number(), run: running_sum(.n), mean: running_avg(.n) }",
            &["k", "run", "mean"],
        ),
        vec![
            vec![n(1), n(5), Value::Float(5.0)],
            vec![n(2), n(3), Value::Float(1.5)],
            vec![n(3), n(7), Value::Float(7.0 / 3.0)],
            vec![n(4), n(8), Value::Float(2.0)],
            vec![n(5), n(9), Value::Float(1.8)],
        ]
    );
    // A partition is its rows wherever they fall, not a run of neighbours:
    // in time order the two people's pings interleave.
    assert_eq!(
        db.table(
            "pings.sort { .at.asc }.partition_by { .who }.select { id, k: row_number(), before: prev(.id) }",
            &["id", "k", "before"],
        ),
        vec![
            vec![text("p4"), n(1), Value::Null],
            vec![text("p5"), n(2), text("p4")],
            vec![text("p1"), n(1), Value::Null],
            vec![text("p2"), n(2), text("p1")],
            vec![text("p3"), n(3), text("p2")],
        ]
    );
    // Further back than one row, and past the edge.
    assert_eq!(
        db.table(
            "pings.sort { .id.asc }.select { id, two: prev(.id, 2), far: next(.id, 9) }",
            &["two", "far"],
        ),
        vec![
            vec![Value::Null, Value::Null],
            vec![Value::Null, Value::Null],
            vec![text("p1"), Value::Null],
            vec![text("p2"), Value::Null],
            vec![text("p3"), Value::Null],
        ]
    );
    // Equal values that follow one another share a rank.
    assert_eq!(
        db.table(
            "staff.sort { .pay.desc, .id.asc }.select { id, rank: rank(.pay), dense: dense_rank(.pay) }",
            &["id", "rank", "dense"],
        ),
        vec![
            vec![text("a"), n(1), n(1)],
            vec![text("f"), n(2), n(2)],
            vec![text("b"), n(3), n(3)],
            vec![text("c"), n(3), n(3)],
            vec![text("d"), n(5), n(4)],
            vec![text("e"), n(6), n(5)],
        ]
    );
    // One inside another: a session starts when an hour passes in silence,
    // and the running count of starts numbers the sessions.
    assert_eq!(
        db.table(
            "pings.sort { .who.id.asc, .at.asc }.partition_by { .who }.select {
               id,
               session: running_sum(
                 if prev(.at) == null || seconds_until(prev(.at), .at) >= 3600 then 1 else 0)
             }",
            &["id", "session"],
        ),
        vec![
            vec![text("p1"), n(1)],
            vec![text("p2"), n(1)],
            vec![text("p3"), n(2)],
            vec![text("p4"), n(1)],
            vec![text("p5"), n(1)],
        ]
    );
    // What a select computed can be filtered, grouped and sorted on.
    assert_eq!(
        db.ids(
            "pings.sort { .who.id.asc, .at.asc }.partition_by { .who }
               .select { id, k: row_number() }.where { .k == 1 }"
        ),
        ["p1", "p4"]
    );
    // The rows of a relationship are a sequence of their own.
    let nested = db.rows(
        "staff.where { .id == \"a\" }.select {
           id, pings: .pings.sort { .at.asc }.select { id, run: running_sum(.n) }
         }",
    );
    let Some(Value::List(pings)) = nested[0].get_field("pings") else {
        panic!("no list of pings: {:?}", nested[0]);
    };
    let runs: Vec<_> = pings
        .iter()
        .map(|ping| match ping {
            Value::Object(fields) => fields.get("run").cloned().unwrap_or(Value::Null),
            other => panic!("not a row: {other:?}"),
        })
        .collect();
    assert_eq!(runs, [n(5), n(3), n(7)]);

    // Outside a select there are no rows to have a position among.
    let error = db.try_rows("staff.where { row_number() > 1 }").unwrap_err();
    assert!(error.contains("select"), "{error}");
    assert!(db.try_rows("staff.select { k: row_number(1) }").is_err());
    assert!(db.try_rows("staff.select { k: running_sum() }").is_err());
}

#[test]
fn an_element_can_answer_for_the_row_that_holds_it() {
    let db = company();
    // After the step into `notes`, `jobs` is the job each note belongs to.
    assert_eq!(
        db.table(
            "jobs.notes
               .group_by { state: jobs.state, .kind }
               .select { state, kind, notes: count(), mins: sum(.mins) }
               .sort { .state.asc, .kind.asc }",
            &["state", "kind", "notes", "mins"],
        ),
        vec![
            vec![text("done"), text("x"), int(1), int(1)],
            vec![text("open"), text("x"), int(2), int(7)],
            vec![text("open"), text("y"), int(1), int(7)],
        ]
    );
    // In a filter and a select, with a filter before the step.
    assert_eq!(
        db.table(
            "jobs.where { .state == \"open\" }.notes
               .where { jobs.id != \"j3\" }
               .select { job: jobs.id, kind, mins }
               .sort { .mins.asc }",
            &["job", "kind", "mins"],
        ),
        vec![
            vec![text("j1"), text("x"), int(5)],
            vec![text("j1"), text("y"), int(7)],
        ]
    );
    // Through a relationship as well as a list.
    assert_eq!(
        db.table(
            "teams.staff.select { id, team: teams.id }.sort { .id.asc }.limit(2)",
            &["id", "team"],
        ),
        vec![vec![text("a"), text("t1")], vec![text("b"), text("t2")]]
    );
}

#[test]
fn rows_written_together_can_refer_to_one_another() {
    let db = company();
    // `company` inserted each tree in one statement; the references hold.
    assert_eq!(db.ids("teams.where { .parent == \"t4\" }"), ["t5"]);
    assert_eq!(db.scalar("staff.where { .id == \"e\" }.select { up: .boss.boss.id }", "up"), text("b"));

    // A transaction that writes a row and then rows that refer to it.
    db.script(
        r#"
        transaction {
          teams.insert { id: "t6", parent: "t1" }
          teams.insert { id: "t7", parent: "t6" }
          staff.insert { id: "g", team: "t7", boss: "a", pay: 10 }
        }
        "#,
    );
    assert_eq!(
        db.scalar("staff.where { .id == \"g\" }.select { top: .team.parent.parent.id }", "top"),
        text("t1")
    );
    assert_eq!(db.scalar("teams.where { .id == \"t6\" }.select { n: .children.count() }", "n"), int(1));

    // A reference to a row that is not there is still refused, and nothing
    // of the statement is kept.
    let error = db
        .try_script(r#"teams.insert [ { id: "t8" }, { id: "t9", parent: "nowhere" } ]"#)
        .unwrap_err();
    assert!(error.contains("nowhere"), "{error}");
    assert!(db.rows("teams.where { .id == \"t8\" }").is_empty());
    // So is a repeated id, whichever side of the reference it falls.
    assert!(db
        .try_script(r#"teams.insert [ { id: "ta" }, { id: "tb", parent: "ta" }, { id: "ta" } ]"#)
        .is_err());
    assert!(db.rows("teams.where { .id == \"ta\" || .id == \"tb\" }").is_empty());
}

/// The executor reads less than a query literally asks for in several
/// ways: it stops a costly filter once a limit is met, applies a limit
/// before a select, and answers a question about a referenced row once for
/// every row that points at it. None of that may change an answer.
#[test]
fn reading_less_changes_no_answer() {
    let db = company();
    // The same question about the same team, asked by each of its people.
    assert_eq!(
        db.table("staff.sort { .id.asc }.select { id, mates: .team.staff.count() }", &["id", "mates"]),
        vec![
            vec![text("a"), int(1)],
            vec![text("b"), int(2)],
            vec![text("c"), int(2)],
            vec![text("d"), int(1)],
            vec![text("e"), int(1)],
            vec![text("f"), int(1)],
        ]
    );
    // A question that also depends on who asks is asked afresh each time:
    // `b` and `c` share a team and get different answers.
    assert_eq!(
        db.table(
            "staff.where { .team == \"t2\" }.sort { .id.asc }
               .select { id, after: .team.staff.where { .id > staff.id }.count() }",
            &["id", "after"],
        ),
        vec![vec![text("b"), int(1)], vec![text("c"), int(0)]]
    );
    assert_eq!(
        db.scalar("staff.where { .team.staff.where { .pay > 70 }.count() == 2 }.count()", "count"),
        int(2)
    );

    // A limit written after a select.
    assert_eq!(
        db.table(
            "staff.select { id, reports: .reports.count() }.sort { .id.asc }.limit(2)",
            &["id", "reports"],
        ),
        vec![vec![text("a"), int(3)], vec![text("b"), int(1)]]
    );
    assert_eq!(
        db.table(
            "staff.sort { .id.asc }.select { id, reports: .reports.count() }.offset(1).limit(2)",
            &["id", "reports"],
        ),
        vec![vec![text("b"), int(1)], vec![text("c"), int(0)]]
    );
    // The sort is on what the select made of `id`, not on the stored id, so
    // it has to stay after the select.
    assert_eq!(
        db.table("staff.select { id: .team.id, pay }.sort { .id.desc }.limit(1)", &["id", "pay"]),
        vec![vec![text("t5"), int(40)]]
    );
    assert_eq!(
        db.table(
            "staff.select { id, pay: .pay * -1 }.sort { .pay.asc, .id.asc }.limit(2)",
            &["id", "pay"],
        ),
        vec![vec![text("a"), int(-100)], vec![text("f"), int(-90)]]
    );

    // A filter that walks a relationship, ahead of a sort and a limit.
    assert_eq!(
        db.ids("staff.where { .reports.count() >= 1 }.sort { .pay.asc, .id.asc }.limit(2)"),
        ["d", "b"]
    );
    assert_eq!(
        db.ids("staff.where { .reports.exists() && .pay < 100 }.sort { .id.desc }.limit(5)"),
        ["d", "b"]
    );
    // A sort cut by an offset and a limit; ties keep their order.
    assert_eq!(db.ids("staff.sort { .pay.desc, .id.asc }.offset(2).limit(2)"), ["b", "c"]);
    assert_eq!(db.ids("staff.where { .pay <= 80 }.sort { .pay.desc }.limit(3).sort { .id.asc }"), ["b", "c", "d"]);
    assert!(db.rows("staff.sort { .pay.desc }.offset(9).limit(2)").is_empty());

    // A path through a reference and on through a relationship, and a
    // field read through two references.
    assert_eq!(
        db.table(
            "pings.sort { .id.asc }.limit(2).select { id, peers: .who.team.staff.count(), top: .who.boss.id }",
            &["id", "peers", "top"],
        ),
        vec![vec![text("p1"), int(1), Value::Null], vec![text("p2"), int(1), Value::Null]]
    );
    assert_eq!(db.scalar("pings.where { .who.boss.id == \"a\" }.count()", "count"), int(2));
    // Elements of a list, counted and filtered where they lie.
    assert_eq!(db.scalar("jobs.where { .notes.where { .mins > 4 }.count() >= 2 }.count()", "count"), int(1));
    assert_eq!(db.scalar("jobs.where { .state == \"open\" }.notes.sum(.mins)", "sum"), int(14));
}

/// Enough rows for the shortcuts that only start once many rows have asked
/// the same question: a hundred people, ten clubs, two clubs each.
#[test]
fn questions_asked_by_many_rows_keep_their_answers() {
    let db = Db::new();
    db.script(
        r#"
        schema clubs { id: String }
        schema folk { id: String, rank: Integer }
        schema joins { id: String, who: Ref(folk), club: Ref(clubs) }
        "#,
    );
    let first = |person: usize| person % 10;
    let second = |person: usize| (person % 10 + 1 + (person / 10) % 9) % 10;
    let clubs: Vec<String> = (0..10).map(|club| format!("{{ id: \"c{club}\" }}")).collect();
    let folk: Vec<String> = (0..100)
        .map(|person| format!("{{ id: \"f{person:03}\", rank: {person} }}"))
        .collect();
    let joins: Vec<String> = (0..100)
        .flat_map(|person| {
            [first(person), second(person)].into_iter().enumerate().map(move |(slot, club)| {
                format!("{{ id: \"j{person:03}{slot}\", who: \"f{person:03}\", club: \"c{club}\" }}")
            })
        })
        .collect();
    db.script(&format!(
        "clubs.insert [ {} ]\nfolk.insert [ {} ]\njoins.insert [ {} ]",
        clubs.join(", "),
        folk.join(", "),
        joins.join(", ")
    ));
    let members = |club: usize| (0..100usize).filter(move |person| first(*person) == club || second(*person) == club);
    let count = |query: &str| match db.scalar(query, "count") {
        Value::Integer(count) => count as usize,
        other => panic!("not a count: {other:?}\n{query}"),
    };

    // Which of a person's memberships are in one named club.
    assert_eq!(
        count("folk.where { .joins.where { .club == \"c3\" }.exists() }.count()"),
        members(3).count()
    );
    assert_eq!(
        count("joins.where { .who.joins.where { .club == \"c3\" }.exists() }.count()"),
        members(3).count() * 2
    );
    // Everyone who shares a club with one person, that person included.
    let shares = (0..100)
        .filter(|person| {
            [first(*person), second(*person)]
                .iter()
                .any(|club| *club == first(0) || *club == second(0))
        })
        .count();
    assert_eq!(
        count(
            "folk.where { .joins.where { .club.joins.where { .who == \"f000\" }.exists() }.exists() }.count()"
        ),
        shares
    );
    // The same, stopped after a few rows.
    assert_eq!(
        db.ids(
            "folk.where { .id != \"f000\" && .joins.where { .club.joins.where { .who == \"f000\" }.exists() }.exists() }
               .sort { .id.asc }.limit(3)"
        )
        .len(),
        3
    );
    // A filter only the evaluator can apply, asked by every row.
    let big: Vec<usize> = (0..10).filter(|club| members(*club).count() > 20).collect();
    let in_big = (0..100)
        .filter(|person| big.contains(&first(*person)) || big.contains(&second(*person)))
        .count();
    assert_eq!(
        count("folk.where { .joins.where { .club.joins.count() > 20 }.exists() }.count()"),
        in_big
    );
    let both_big = (0..100)
        .filter(|person| big.contains(&first(*person)) && big.contains(&second(*person)))
        .count();
    assert_eq!(
        count("folk.where { .joins.where { .club.joins.count() > 20 }.count() == 2 }.count()"),
        both_big
    );

    // A filter inside a group, counted where the group's rows are.
    let per_club = db.table(
        "joins.group_by { .club.id }
           .select { club_id, low: where { .who.rank < 50 }.count(), all: count(),
                     top: where { .who.rank >= 50 }.sort { .id.desc }.limit(1).select { id } }
           .sort { .club_id.asc }",
        &["club_id", "low", "all"],
    );
    for (club, row) in per_club.iter().enumerate() {
        assert_eq!(row[0], text(&format!("c{club}")));
        assert_eq!(row[1], int(members(club).filter(|person| *person < 50).count() as i64));
        assert_eq!(row[2], int(members(club).count() as i64));
    }

    // A limit after position functions that only look back runs first...
    assert_eq!(
        db.table(
            "folk.sort { .id.asc }.select { id, n: row_number(), run: running_sum(.rank) }.limit(3)",
            &["id", "n", "run"],
        ),
        vec![
            vec![text("f000"), int(1), int(0)],
            vec![text("f001"), int(2), int(1)],
            vec![text("f002"), int(3), int(3)],
        ]
    );
    assert_eq!(
        db.table(
            "folk.sort { .id.asc }.partition_by { odd: .rank % 2 }
               .select { id, n: row_number(), before: prev(.id) }.limit(4)",
            &["id", "n", "before"],
        ),
        vec![
            vec![text("f000"), int(1), Value::Null],
            vec![text("f001"), int(1), Value::Null],
            vec![text("f002"), int(2), text("f000")],
            vec![text("f003"), int(2), text("f001")],
        ]
    );
    // ...and one that looks ahead still sees the rows beyond the limit.
    assert_eq!(
        db.table("folk.sort { .id.asc }.select { id, after: next(.id) }.limit(2)", &["id", "after"]),
        vec![vec![text("f000"), text("f001")], vec![text("f001"), text("f002")]]
    );
    // Rows reached by a nested step, counted without being read whole.
    assert_eq!(
        db.table(
            "clubs.where { .id == \"c3\" }.select { id, ranks: .joins.who.count(), top: .joins.who.max(.rank) }",
            &["ranks", "top"],
        ),
        vec![vec![
            int(members(3).count() as i64),
            int(members(3).max().unwrap_or_default() as i64)
        ]]
    );
}

/// Elements of an embedded list are judged through one reused row, and
/// related rows where the store holds them. Neither may let one element's
/// values show through to the next.
#[test]
fn elements_and_related_rows_judged_in_place_keep_their_answers() {
    let db = Db::new();
    db.script(
        r#"
        schema boxes { id: String, parts: List({ kind: String, grams: Optional(Integer), spare: Optional(Boolean) }) }
        schema marks { id: String, box: Ref(boxes), score: Integer, note: Optional(String) }
        boxes.insert [
          { id: "b1", parts: [ { kind: "nut", grams: 5 }, { kind: "bolt" }, { kind: "nut", grams: 9, spare: true } ] },
          { id: "b2", parts: [ { kind: "bolt", grams: 7 } ] },
          { id: "b3", parts: [] },
          { id: "b4", parts: [ { kind: "nut" }, { kind: "nut", grams: 1 } ] }
        ]
        marks.insert [
          { id: "m1", box: "b1", score: 3, note: "ok" },
          { id: "m2", box: "b1", score: 8 },
          { id: "m3", box: "b2", score: 8, note: "ok" },
          { id: "m4", box: "b4", score: 1 }
        ]
        "#,
    );
    let ids = |query: &str| db.ids(query);
    // A field one element has and the next lacks is absent for the next,
    // not left over from the one before.
    assert_eq!(ids("boxes.where { .parts.where { .grams > 4 }.count() == 2 }.sort { .id.asc }"), ["b1"]);
    assert_eq!(ids("boxes.where { .parts.where { .grams == null }.exists() }.sort { .id.asc }"), ["b1", "b4"]);
    assert_eq!(ids("boxes.where { .parts.where { .spare == true }.exists() }"), ["b1"]);
    assert_eq!(ids("boxes.where { !.parts.where { .kind == \"nut\" }.exists() }.sort { .id.asc }"), ["b2", "b3"]);
    assert_eq!(ids("boxes.where { .parts.where { .kind == \"nut\" && .grams < 5 }.count() == 1 }"), ["b4"]);
    assert_eq!(ids("boxes.where { .parts.count() == 0 }"), ["b3"]);
    assert_eq!(
        db.table(
            "boxes.sort { .id.asc }.select { id, nuts: .parts.where { .kind == \"nut\" }.count(), heavy: .parts.where { .grams >= 7 }.exists() }",
            &["id", "nuts", "heavy"],
        ),
        vec![
            vec![text("b1"), int(2), Value::Boolean(true)],
            vec![text("b2"), int(0), Value::Boolean(true)],
            vec![text("b3"), int(0), Value::Boolean(false)],
            vec![text("b4"), int(2), Value::Boolean(false)],
        ]
    );
    assert_eq!(db.scalar("boxes.where { .parts.where { .kind != \"nut\" }.exists() }.count()", "count"), int(2));

    // Related rows behind a filter storage cannot apply.
    assert_eq!(
        db.table(
            "boxes.sort { .id.asc }.select {
               id,
               good: .marks.where { .note == \"ok\" || .score > 5 }.count(),
               any: .marks.where { .box.parts.count() > 1 && .score < 5 }.exists(),
               best: .marks.where { .note != \"ok\" }.max(.score)
             }",
            &["id", "good", "any", "best"],
        ),
        vec![
            vec![text("b1"), int(2), Value::Boolean(true), int(8)],
            vec![text("b2"), int(1), Value::Boolean(false), Value::Null],
            vec![text("b3"), int(0), Value::Boolean(false), Value::Null],
            vec![text("b4"), int(0), Value::Boolean(true), int(1)],
        ]
    );
    // A plain field against a constant, for every kind of value and for a
    // field that is absent.
    assert_eq!(ids("marks.where { .score >= 8 }.sort { .id.asc }"), ["m2", "m3"]);
    assert_eq!(ids("marks.where { .score == 8.0 && .note == \"ok\" }"), ["m3"]);
    assert_eq!(ids("marks.where { .note != \"ok\" }.sort { .id.asc }"), ["m2", "m4"]);
    assert_eq!(ids("marks.where { .note == null }.sort { .id.asc }"), ["m2", "m4"]);
    assert_eq!(ids("marks.where { .box == \"b1\" && .score < 5 }"), ["m1"]);
}
