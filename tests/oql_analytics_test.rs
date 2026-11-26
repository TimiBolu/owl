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
    assert_eq!(db.scalar("staff.pay.distinct().count()", "count"), int(5));
    assert_eq!(
        db.scalar("staff.select { band: .pay / 50 }.distinct().count()", "count"),
        int(3)
    );
    // An absent value is not a value: `c` has no grade.
    assert_eq!(db.scalar("staff.grade.distinct().count()", "count"), int(3));
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
               .select { team_id, people: count(), pays: .pay.distinct().count(), mid: median(.pay),
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
        db.table("staff.select { mid: median(.pay), top: percentile(.pay, 1) }", &["mid", "top"]),
        vec![vec![Value::Float(80.0), Value::Float(100.0)]]
    );
    // Through a relationship, in a select and in a filter.
    assert_eq!(
        db.table(
            "teams.where { .staff.median(.pay) >= 80 }.sort { .id.asc }
               .select { id, mid: .staff.median(.pay), bosses: .staff.boss.distinct().count() }",
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
        db.scalar("staff.where { .pay > 999 }.pay.distinct().count()", "count"),
        int(0)
    );
    assert_eq!(db.scalar("staff.where { .pay > 999 }.median(.pay)", "median"), Value::Null);

    // What cannot mean anything is an error.
    let error = db.try_rows("staff.percentile(.pay, 1.5)").unwrap_err();
    assert!(error.contains("between 0 and 1"), "{error}");
    let error = db.try_rows("staff.median(.id)").unwrap_err();
    assert!(error.contains("numeric"), "{error}");
    assert!(db.try_rows("staff.where { median(.pay) > 1 }").is_err());

    assert_eq!(db.scalar("staff.boss.distinct().count()", "count"), int(3));
    // A distinct count is the chain; there is no word for it.
    let error = db.try_rows("staff.select { n: count_distinct(.pay) }").unwrap_err();
    assert!(error.contains("Unknown function: count_distinct"), "{error}");

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
               run: so_far().n.sum(),
               before: prev(.id),
               after: next(.id),
               gap: seconds_until(prev(.at), .at),
               low: so_far().n.min(),
               high: so_far().max(.n),
               seen: so_far().count()
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
            "pings.sort { .id.asc }.select { id, k: row_number(), run: so_far().n.sum(), mean: so_far().n.avg() }",
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
    // and the count of starts so far numbers the sessions. In a filter on
    // the rows so far, `prev` is each row's own neighbour.
    assert_eq!(
        db.table(
            "pings.sort { .who.id.asc, .at.asc }.partition_by { .who }.select {
               id,
               session: so_far()
                 .where { prev(.at) == null || seconds_until(prev(.at), .at) >= 3600 }
                 .count()
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
           id, pings: .pings.sort { .at.asc }.select { id, run: so_far().n.sum() }
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
}

#[test]
fn a_walk_keeps_the_way_to_each_row() {
    let db = company();
    // The way to a row is a collection: the row the walk started at, then
    // each row in turn, ending with the row itself.
    assert_eq!(
        db.table(
            "staff.where { .id == \"e\" }.walk(.boss).select {
               id, steps: ._path.count(), paid: ._path.pay.sum(), from: ._path.first().id
             }",
            &["id", "steps", "paid", "from"],
        ),
        vec![
            vec![text("d"), int(2), int(100), text("e")],
            vec![text("b"), int(3), int(180), text("e")],
            vec![text("a"), int(4), int(280), text("e")],
        ]
    );
    // From each row of a select.
    assert_eq!(
        db.table(
            "teams.where { .id == \"t1\" }.select {
               id, longest: .walk(.children).max(._path.count())
             }",
            &["longest"],
        ),
        vec![vec![int(4)]]
    );

    // A bill of materials: a bolt is used in the frame and in the hub.
    let db = Db::new();
    db.script(
        r#"
        schema parts { id: String }
        schema uses { id: String, whole: Ref(parts) as uses, part: Ref(parts) as used_in, qty: Integer }
        parts.insert [
          { id: "bike" }, { id: "wheel" }, { id: "frame" }, { id: "spoke" }, { id: "hub" }, { id: "bolt" }
        ]
        uses.insert [
          { id: "u1", whole: "bike", part: "wheel", qty: 2 },
          { id: "u2", whole: "bike", part: "frame", qty: 1 },
          { id: "u3", whole: "wheel", part: "spoke", qty: 32 },
          { id: "u4", whole: "wheel", part: "hub", qty: 1 },
          { id: "u5", whole: "frame", part: "bolt", qty: 4 },
          { id: "u6", whole: "hub", part: "bolt", qty: 2 }
        ]
        "#,
    );
    // `walk` reaches each part once, by the shortest way.
    assert_eq!(
        db.ids("parts.where { .id == \"bike\" }.walk(.uses.part).sort { .id.asc }"),
        ["bolt", "frame", "hub", "spoke", "wheel"]
    );
    // `routes` gives a row for every way to it. The rows passed through
    // are on the way, so the quantities along it multiply.
    assert_eq!(
        db.table(
            "parts.where { .id == \"bike\" }.routes(.uses.part)
               .select { id, depth: ._depth, needed: ._path.qty.product() }
               .sort { .id.asc, .needed.asc }",
            &["id", "depth", "needed"],
        ),
        vec![
            vec![text("bolt"), int(2), int(4)],
            vec![text("bolt"), int(3), int(4)],
            vec![text("frame"), int(1), int(1)],
            vec![text("hub"), int(2), int(2)],
            vec![text("spoke"), int(2), int(64)],
            vec![text("wheel"), int(1), int(2)],
        ]
    );
    // Added up for each part: what the whole bike needs.
    assert_eq!(
        db.table(
            "parts.where { .id == \"bike\" }.routes(.uses.part)
               .select { id, needed: ._path.qty.product() }
               .group_by { .id }.select { id, needed: sum(.needed) }.sort { .id.asc }",
            &["id", "needed"],
        ),
        vec![
            vec![text("bolt"), int(8)],
            vec![text("frame"), int(1)],
            vec![text("hub"), int(2)],
            vec![text("spoke"), int(64)],
            vec![text("wheel"), int(2)],
        ]
    );
    // No further than asked, and a route does not pass through a row twice.
    assert_eq!(db.rows("parts.where { .id == \"bike\" }.routes(.uses.part, 1)").len(), 2);
    let ring = company();
    assert_eq!(
        ring.ids("nodes.where { .id == \"n1\" }.routes(.outgoing.dst).sort { .id.asc }"),
        ["n2", "n3"]
    );

    // `product` is an aggregate like `sum`; of nothing it is 1.
    assert_eq!(db.scalar("uses.select { all: product(.qty) }", "all"), int(2 * 32 * 4 * 2));
    assert_eq!(db.scalar("uses.where { .qty > 99 }.select { all: product(.qty) }", "all"), int(1));
    assert_eq!(db.scalar("parts.where { .id == \"wheel\" }.select { n: .uses.qty.product() }", "n"), int(32));
}

#[test]
fn a_collection_name_always_means_something() {
    let db = company();
    // In a query on staff with no row above it, `staff` is the row in hand.
    assert_eq!(
        db.table("staff.sort { .id.asc }.limit(2).select { id, pay: staff.pay, me: staff.id }", &["pay", "me"]),
        vec![vec![int(100), text("a")], vec![int(80), text("b")]]
    );
    // Under a step it is still the row the step was taken from.
    assert_eq!(
        db.scalar(
            "staff.where { .id == \"a\" }.select { n: .reports.where { .pay < staff.pay }.count() }",
            "n"
        ),
        int(3)
    );
    // Another collection's name, with no row of it in reach, is that
    // collection: every row's value.
    assert_eq!(
        db.table(
            "teams.where { .id == \"t1\" }.select { all: staff.pay.sum(), most: staff.pay.max(), n: staff.pay.count() }",
            &["all", "most", "n"]
        ),
        vec![vec![int(450), int(100), int(6)]]
    );
    // `staff.pay` is one value here, so reducing it is refused, and the
    // message says how to ask for every row.
    let error = db.try_rows("staff.select { id, all: staff.pay.sum() }").unwrap_err();
    assert!(error.contains("one value") && error.contains("staff.sum(.pay)"), "{error}");
    // Which is this, and it still works.
    assert_eq!(
        db.scalar("staff.where { .id == \"a\" }.select { all: staff.sum(.pay) }", "all"),
        int(450)
    );

    // A collection may be called by a word the language also uses.
    db.script(
        "schema first { id: String, n: Integer }
         first.insert [ { id: \"x\", n: 1 }, { id: \"y\", n: 2 } ]",
    );
    assert_eq!(db.ids("first.where { .n > 1 }"), ["y"]);
    assert_eq!(db.scalar("first.count()", "count"), int(2));
    assert_eq!(db.scalar("first.sort { .n.desc }.first().select { id }", "id"), text("y"));
    assert_eq!(
        db.scalar("staff.where { .id == \"a\" }.select { n: first.where { .n > 0 }.count() }", "n"),
        int(2)
    );
}

/// Shorter ways to the same answer: a table of rows keyed by what a
/// function matches on, calls that end once enough ordered rows are in
/// hand, a sort through a reference that reads only the first rows, set
/// operations run as one filter, and library statistics computed directly.
/// Each is checked against the long way.
#[test]
fn shorter_ways_give_the_same_answers() {
    let db = Db::new();
    db.script(
        r#"
        schema clubs { id: String, tier: Integer }
        schema folk { id: String, rank: Integer, club: Ref(clubs), level: Integer }
        fn same(p: folk) { folk.where { .club == p.club && .level == p.level && .id > p.id } }
        fn same_long(p: folk) {
          folk.where { .club == p.club && .level == p.level && .id > p.id }.sort { .id.asc }
        }
        fn my_variance(values) {
          n = values.count()
          m = values.avg()
          if n < 2 then null else to_float(values.select { d: (. - m) * (. - m) }.d.sum()) / (n - 1)
        }
        "#,
    );
    let clubs: Vec<String> = (0..10)
        .map(|club| format!("{{ id: \"c{club}\", tier: {} }}", club % 3))
        .collect();
    let folk: Vec<String> = (0..100)
        .map(|person| {
            format!(
                "{{ id: \"f{person:03}\", rank: {}, club: \"c{}\", level: {} }}",
                (person * 37) % 101,
                (person * 7) % 10,
                person % 4
            )
        })
        .collect();
    db.script(&format!("clubs.insert [ {} ]\nfolk.insert [ {} ]", clubs.join(", "), folk.join(", ")));

    // Pairs through a function: the keyed table against the long way.
    let pairs = |function: &str, tail: &str| {
        db.table(
            &format!("folk.{function}().select {{ a: folk.id, b: .id }}.sort {{ {tail} }}"),
            &["a", "b"],
        )
    };
    let all = pairs("same", ".a.asc, .b.asc");
    assert!(all.len() > 50, "{}", all.len());
    assert_eq!(all, pairs("same_long", ".a.asc, .b.asc"));
    // Cut short, the calls end early; the rows are the first of them all.
    for keep in [1, 5, 40] {
        assert_eq!(pairs("same", &format!(".a.asc, .b.asc }}.limit({keep}).select {{ a, b")), all[..keep]);
    }
    let backwards = pairs("same", ".a.desc, .b.asc");
    assert_eq!(pairs("same", ".a.desc, .b.asc }.limit(7).select { a, b"), backwards[..7]);

    // A sort through a reference, cut short.
    for (filter, sort) in [
        ("", ".club.id.asc, .rank.desc"),
        ("", ".club.id.desc, .id.asc"),
        ("", ".club.tier.desc, .rank.asc, .id.asc"),
        (".where { .level == 1 }", ".club.tier.asc, .id.desc"),
    ] {
        let whole = db.ids(&format!("folk{filter}.sort {{ {sort} }}"));
        for (skip, keep) in [(0, 1), (0, 6), (3, 4), (0, 12)] {
            assert_eq!(
                db.ids(&format!("folk{filter}.sort {{ {sort} }}.offset({skip}).limit({keep})")),
                whole[skip..skip + keep],
                "{filter} {sort} {skip} {keep}"
            );
        }
    }

    // Set operations on one collection against the filter they amount to.
    let count = |query: &str| db.scalar(query, "count");
    assert_eq!(
        count("folk.where { .level == 1 }.intersect(folk.where { .rank < 50 }).count()"),
        count("folk.where { .level == 1 && .rank < 50 }.count()")
    );
    assert_eq!(
        count("folk.where { .level == 1 }.except(folk.where { .rank < 50 }).count()"),
        count("folk.where { .level == 1 && .rank >= 50 }.count()")
    );
    assert_eq!(
        count("folk.where { .level == 1 }.union(folk.where { .rank < 50 }).count()"),
        count("folk.where { .level == 1 || .rank < 50 }.count()")
    );
    assert_eq!(
        db.ids(
            "folk.where { .level == 1 }.union(folk.where { .rank < 50 })
               .except(folk.where { .club.tier == 0 }).sort { .id.desc }.limit(9)"
        ),
        db.ids("folk.where { (.level == 1 || .rank < 50) && .club.tier != 0 }.sort { .id.desc }.limit(9)")
    );
    // Without a sort, a union still puts the other query's rows after.
    let joined = db.ids("folk.where { .rank < 3 }.union(folk.where { .rank > 80 })");
    assert!(joined.len() > 3);
    assert_eq!(joined[..3], db.ids("folk.where { .rank < 3 }")[..]);

    // The library's statistics against the same definition written here.
    let spread = db.table(
        "folk.group_by { .level }.select {
           level, a: variance(.rank), b: my_variance(.rank), c: .rank.stddev(), d: stddev(.rank)
         }.sort { .level.asc }",
        &["a", "b", "c", "d"],
    );
    assert_eq!(spread.len(), 4);
    for row in spread {
        let [Value::Float(a), Value::Float(b), Value::Float(c), Value::Float(d)] = row.as_slice() else {
            panic!("not numbers: {row:?}");
        };
        assert!((a - b).abs() < 1e-9 * b.abs(), "{a} {b}");
        assert!((c - b.sqrt()).abs() < 1e-9 * c.abs(), "{c} {b}");
        assert_eq!(c, d);
    }
    // One value has no spread to speak of.
    assert_eq!(
        db.scalar(
            "folk.where { .id == \"f000\" }.group_by { .level }.select { v: variance(.rank) }",
            "v"
        ),
        Value::Null
    );
}

/// A filter on plain fields is decided from the fields' columns before a
/// row is read. The rows chosen must be the rows the filter chooses, and a
/// write must be seen by the next query.
#[test]
fn whole_columns_choose_the_rows_a_filter_chooses() {
    let db = company();
    db.script(
        "schema posts { id: String, tags: List(String) }
         posts.insert [
           { id: \"p1\", tags: [\"a\", \"b\"] }, { id: \"p2\", tags: [\"b\"] },
           { id: \"p3\", tags: [] }, { id: \"p4\", tags: [\"a\", \"c\", \"a\"] }
         ]",
    );
    let queries = [
        "staff.where { .pay > 60 }.count()",
        "staff.where { .pay >= 80 && .grade != null }.count()",
        "staff.where { .grade == null }.count()",
        "staff.where { .grade == 2 }.sort { .id.asc }",
        "staff.where { .grade != 2 }.sort { .id.asc }",
        "staff.where { .grade < 3 }.sort { .id.asc }",
        "staff.where { .pay == 80.0 }.count()",
        "staff.where { .pay < 90.5 && .boss != null }.sort { .id.desc }.limit(2)",
        "staff.where { .boss == null }.count()",
        "staff.where { .boss != null }.count()",
        "staff.where { .boss == \"a\" }.count()",
        "staff.where { .boss == \"a\" && .pay > 80 }.count()",
        "staff.where { .id != \"a\" && .pay > 50 }.count()",
        "staff.where { .pay > 50 && .reports.count() > 0 }.sort { .id.asc }",
        "staff.where { .reports.count() > 0 && .pay > 50 }.sort { .id.asc }",
        "staff.where { .median == null && .pay > 0 }.sort { .id.asc }.select { id }",
        "staff.where { .median != 7 }.count()",
        "staff.where { .pay > 60 }.where { .grade == 1 }.count()",
        "staff.where { .pay > 1000 }.count()",
        "staff.where { .pay > 60 }.team.count()",
        "pings.where { .at >= \"2026-01-01T10:10:00Z\" }.count()",
        "pings.where { .at < \"2026-01-01T10:10:00Z\" && .n > 0 }.sort { .id.asc }",
        "pings.where { .n < 0 || .n > 4 }.count()",
        "pings.where { .n != 1 }.sort { .n.desc, .id.asc }.limit(3)",
        "jobs.where { .state == \"open\" }.count()",
        "jobs.where { .state != \"open\" }.count()",
        "jobs.where { .state starts_with \"o\" }.count()",
        "jobs.where { .state >= \"open\" }.sort { .id.asc }",
        // Sorts that are cut short take their keys from columns too.
        "staff.sort { .pay.desc, .id.asc }.limit(3)",
        "staff.sort { .pay.asc, .id.desc }.limit(4)",
        "staff.sort { .grade.asc, .id.asc }.limit(4)",
        "staff.sort { .grade.desc, .pay.asc, .id.asc }.limit(5)",
        "staff.sort { .grade.desc, .id.asc }.offset(2).limit(3)",
        "staff.where { .pay < 100 }.sort { .grade.asc, .id.desc }.limit(3)",
        "staff.where { .reports.count() == 0 }.sort { .pay.desc, .id.asc }.limit(2)",
        "staff.sort { .boss.asc, .id.asc }.limit(4)",
        "staff.sort { .id.desc }.limit(2)",
        "pings.sort { .at.desc, .id.asc }.limit(3)",
        "pings.sort { .n.asc, .at.desc }.limit(4)",
        "pings.where { .n > 0 }.sort { .at.asc }.limit(2).select { id, n }",
        "jobs.sort { .state.desc, .id.asc }.limit(2)",
        // Aggregates and groups read their fields from columns.
        "staff.sum(.pay)",
        "staff.avg(.pay)",
        "staff.min(.grade)",
        "staff.max(.pay)",
        "staff.where { .pay > 60 }.sum(.pay)",
        "staff.where { .grade != null }.avg(.grade)",
        "staff.where { .pay > 1000 }.sum(.pay)",
        "staff.where { .pay > 1000 }.max(.pay)",
        "staff.select { n: count(), total: sum(.pay), graded: count(.grade), low: min(.pay) }",
        "staff.group_by { .grade }.select { grade, n: count(), total: sum(.pay) }",
        "staff.group_by { .team }.select { team, n: count(), top: max(.pay) }",
        "staff.group_by { .boss }.select { boss, n: count() }",
        "staff.where { .pay < 100 }.group_by { .grade }.select { grade, n: count(), lo: min(.pay), mean: avg(.pay) }",
        "staff.group_by { .grade, .pay }.select { grade, pay, n: count() }",
        "staff.group_by { .grade }.having { count() > 1 }.select { grade, n: count() }",
        "pings.group_by { .who }.select { who, n: count(), last: max(.at), total: sum(.n) }",
        "pings.where { .n > 0 }.group_by { .who }.select { who, first: min(.at) }",
        "jobs.group_by { .state }.select { state, n: count() }",
        // One field of the elements of a list the row holds, reduced.
        "jobs.sort { .id.asc }.select { id, total: .notes.mins.sum(), top: .notes.max(.mins) }",
        "jobs.sort { .id.asc }.select { id, mean: .notes.mins.avg(), low: .notes.min(.mins), last: .notes.kind.max() }",
        "jobs.where { .notes.mins.sum() > 5 }.sort { .id.asc }",
        "jobs.where { .notes.sum(.mins) <= 5 }.count()",
        "staff.where { .id == \"e\" }.walk(.boss).select { id, paid: ._path.pay.sum(), top: ._path.pay.max() }",
        // The elements of a list each row holds, grouped: folded as they
        // are read, with no row made of an element.
        "jobs.notes.group_by { .kind }.select { kind, n: count(), total: sum(.mins) }",
        "jobs.notes.group_by { .kind }.select { kind, most: max(.mins), least: min(.mins), mean: avg(.mins) }",
        "jobs.notes.group_by { state: jobs.state, .kind }.select { state, kind, n: count(), total: sum(.mins) }",
        "jobs.notes.group_by { whose: jobs.id }.select { whose, n: count() }",
        "jobs.where { .state == \"open\" }.notes.group_by { .kind }.select { kind, n: count() }",
        "jobs.where { .state == \"gone\" }.notes.group_by { .kind }.select { kind, n: count() }",
        "jobs.notes.group_by { .kind }.having { count() > 1 }.select { kind, n: count() }",
        "jobs.notes.group_by { .kind }.select { kind, n: count() }.sort { .n.desc, .kind.asc }.limit(1)",
        "jobs.notes.group_by { long: .mins > 4 }.select { long, n: count() }",
        "posts.tags.group_by { tag: . }.select { tag, n: count() }",
        "posts.where { .id != \"p2\" }.tags.group_by { tag: . }.select { tag, n: count() }.sort { .n.desc, .tag.asc }",
        "posts.tags.group_by { tag: ., whose: posts.id }.select { tag, whose, n: count() }",
        // A sort of every row, by keys read where the rows hold them.
        "staff.sort { .pay.desc, .id.asc }",
        "staff.sort { .grade.asc, .id.desc }",
        "staff.where { .pay > 50 }.sort { .team.id.asc, .pay.asc, .id.asc }",
        "pings.sort { .who.id.desc, .at.asc }",
        // A costly filter ahead of a sort and a limit.
        "staff.where { .reports.max(.pay) > 50 }.sort { .id.asc }.limit(2)",
        "staff.where { .reports.count() >= 1 }.sort { .pay.desc, .id.asc }.limit(2)",
    ];
    let shape = |rows: Result<Vec<Document>, String>| {
        rows.map(|rows| {
            rows.iter()
                .map(|row| {
                    let mut fields: Vec<(String, Value)> =
                        row.fields.iter().map(|(name, value)| (name.clone(), value.clone())).collect();
                    fields.sort_by(|left, right| left.0.cmp(&right.0));
                    (row.id.clone(), fields)
                })
                .collect::<Vec<_>>()
        })
    };
    let compare_all = |when: &str| {
        let mut by_columns = 0;
        for query in queries {
            owl_db::executor::set_column_aggregation(false);
            let reference = shape(db.try_rows(query));
            owl_db::executor::set_column_aggregation(true);
            // Columns are built the second time a collection is read after
            // a write, so each question is asked twice.
            let first = shape(db.try_rows(query));
            let before = owl_db::executor::column_scans_run();
            let fast = shape(db.try_rows(query));
            by_columns += usize::from(owl_db::executor::column_scans_run() > before);
            assert!(reference.is_ok(), "{when}: {query}: {reference:?}");
            assert_eq!(first, reference, "{when}: {query}");
            assert_eq!(fast, reference, "{when}: {query}");
        }
        // The comparison is only worth something if columns did the choosing.
        assert!(by_columns >= 12, "{when}: only {by_columns} queries used columns");
    };
    compare_all("at first");
    let count = |query: &str| db.scalar(query, "count");
    assert_eq!(count("staff.where { .pay > 60 }.count()"), int(4));

    // Every kind of write is seen by the next query.
    db.script("staff.insert { id: \"z\", team: \"t1\", pay: 85 }");
    assert_eq!(count("staff.where { .pay > 60 }.count()"), int(5));
    db.rows("staff.where { .id == \"b\" }.update { pay: 10 }");
    assert_eq!(count("staff.where { .pay > 60 }.count()"), int(4));
    db.rows("staff.where { .id == \"z\" }.update { grade: 2 }");
    assert_eq!(count("staff.where { .grade == 2 }.count()"), int(3));
    db.rows("staff.where { .id == \"z\" }.delete()");
    assert_eq!(count("staff.where { .pay > 60 }.count()"), int(3));
    assert_eq!(count("staff.where { .grade == 2 }.count()"), int(2));
    compare_all("after writes");

    // Inside a transaction that has written, the rows are asked instead.
    db.script(
        "transaction {
           staff.insert { id: \"y\", team: \"t1\", pay: 95 }
           staff.where { .id == \"a\" }.update { pay: 1 }
         }",
    );
    assert_eq!(count("staff.where { .pay > 60 }.count()"), int(3));
    compare_all("after a transaction");

    // What is kept of one collection outlives a write to another, and not
    // a write to itself.
    assert_eq!(count("pings.where { .n > 0 }.count()"), int(4));
    db.rows("staff.where { .id == \"f\" }.update { pay: 91 }");
    assert_eq!(count("pings.where { .n > 0 }.count()"), int(4));
    assert_eq!(count("staff.where { .pay > 90 }.count()"), int(2));
    db.rows("pings.where { .id == \"p1\" }.update { n: -5 }");
    assert_eq!(count("pings.where { .n > 0 }.count()"), int(3));
    db.script("pings.insert { id: \"p9\", who: \"a\", at: \"2026-02-01T00:00:00Z\", n: 9 }");
    assert_eq!(count("pings.where { .n > 0 }.count()"), int(4));
    assert_eq!(count("staff.where { .pay > 90 }.count()"), int(2));
    // A new collection, which changes the schemas, drops everything kept.
    db.script("schema extras { id: String, n: Integer }\nextras.insert [ { id: \"x1\", n: 1 }, { id: \"x2\", n: 5 } ]");
    assert_eq!(count("extras.where { .n > 2 }.count()"), int(1));
    assert_eq!(count("pings.where { .n > 0 }.count()"), int(4));
    compare_all("after writes to other collections");
}

#[test]
fn the_rows_so_far_are_a_collection() {
    let db = company();
    let n = |value: i64| int(value);
    let column = |expression: &str| -> Vec<Value> {
        db.table(
            &format!("pings.sort {{ .id.asc }}.select {{ id, v: {expression} }}"),
            &["v"],
        )
        .into_iter()
        .map(|mut row| row.remove(0))
        .collect()
    };
    let whole = |values: [i64; 5]| values.map(int).to_vec();
    // In id order the values are 5, -2, 4, 1, 1.
    assert_eq!(column("so_far().n.sum()"), whole([5, 3, 7, 8, 9]));
    assert_eq!(column("so_far().sum(.n * 2)"), whole([10, 6, 14, 16, 18]));
    assert_eq!(column("so_far().count()"), whole([1, 2, 3, 4, 5]));
    // A moving window is the last few of them.
    assert_eq!(column("so_far().last(2).n.sum()"), whole([5, 3, 2, 5, 2]));
    assert_eq!(column("so_far().last(3).n.max()"), whole([5, 5, 5, 4, 4]));
    assert_eq!(
        column("so_far().last(2).n.avg()"),
        [5.0, 1.5, 1.0, 2.5, 1.0].map(Value::Float)
    );
    // Being a collection, it takes a filter, on either side of the window.
    assert_eq!(column("so_far().where { .n > 0 }.n.sum()"), whole([5, 5, 9, 10, 11]));
    assert_eq!(column("so_far().last(2).where { .n > 0 }.count()"), whole([1, 1, 1, 2, 2]));
    assert_eq!(column("so_far().where { .n > 0 }.last(2).n.sum()"), whole([5, 5, 9, 5, 2]));
    // Anything else a collection takes works too, the long way.
    assert_eq!(column("so_far().limit(100).n.sum()"), whole([5, 3, 7, 8, 9]));
    assert_eq!(column("so_far().last(2).limit(100).n.avg()"), column("so_far().last(2).n.avg()"));
    assert_eq!(column("so_far().distinct().count()"), whole([1, 2, 3, 4, 5]));
    assert_eq!(
        column("so_far().sort { .n.asc }.first().id"),
        ["p1", "p2", "p2", "p2", "p2"].map(text)
    );
    assert_eq!(column("so_far().n.median()"), column("so_far().limit(100).n.median()"));
    // So does a function written in OQL.
    let spread = column("so_far().n.stddev()");
    assert_eq!(spread[0], Value::Null);
    assert!(
        matches!(spread[1], Value::Float(value) if (value - 24.5_f64.sqrt()).abs() < 1e-9),
        "{spread:?}"
    );
    // It reaches back to the start of the row's partition and no further.
    assert_eq!(
        db.table(
            "pings.sort { .who.id.asc, .at.asc }.partition_by { .who }.select {
               id, run: so_far().n.sum(), two: so_far().last(2).count()
             }",
            &["id", "run", "two"],
        ),
        vec![
            vec![text("p1"), n(5), n(1)],
            vec![text("p2"), n(3), n(2)],
            vec![text("p3"), n(7), n(2)],
            vec![text("p4"), n(1), n(1)],
            vec![text("p5"), n(2), n(2)],
        ]
    );
    // `last(n)` is the counterpart of `limit(n)` on any rows.
    assert_eq!(db.ids("pings.sort { .id.asc }.last(2)"), ["p4", "p5"]);
    assert_eq!(db.ids("pings.sort { .id.asc }.last(9)").len(), 5);
    assert_eq!(db.ids("pings.sort { .id.asc }.last(0)").len(), 0);

    // Outside a select there is no row for the rows to lead up to.
    let error = db.try_rows("pings.where { so_far().count() > 1 }").unwrap_err();
    assert!(error.contains("select"), "{error}");
    assert!(db.try_rows("pings.select { v: so_far(3).count() }").is_err());
    // `last()` with no count is the last row, as `first()` is the first.
    assert_eq!(db.ids("pings.sort { .id.asc }.last()"), ["p5"]);
    // The words this replaced are gone.
    assert!(db.try_rows("pings.select { v: running_sum(.n) }").is_err());
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
            "folk.sort { .id.asc }.select { id, n: row_number(), run: so_far().rank.sum() }.limit(3)",
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

/// A path through a collection has a value for every row it reaches. It
/// used to read as null, so a comparison with it was false for every row
/// and said nothing.
#[test]
fn a_path_through_a_collection_has_many_values() {
    let db = company();
    let sorted = |value: Value| match value {
        Value::List(mut items) => {
            items.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
            items
        }
        other => panic!("not a list: {other:?}"),
    };
    // As a value it is the list of what each row gives.
    assert_eq!(
        sorted(db.scalar("staff.where { .id == \"a\" }.select { ids: .reports.id }", "ids")),
        [text("b"), text("c"), text("f")]
    );
    assert_eq!(
        sorted(db.scalar("staff.where { .id == \"a\" }.select { pays: .reports.pay }", "pays")),
        [int(80), int(80), int(90)]
    );
    // Through a list of objects, through a reference first, and through
    // two collections, which flattens.
    assert_eq!(
        sorted(db.scalar("jobs.where { .id == \"j1\" }.select { mins: .notes.mins }", "mins")),
        [int(5), int(7)]
    );
    assert_eq!(
        sorted(db.scalar("pings.where { .id == \"p1\" }.select { under: .who.reports.id }", "under")),
        [text("b"), text("c"), text("f")]
    );
    assert_eq!(
        sorted(db.scalar("teams.where { .id == \"t1\" }.select { deep: .children.children.id }", "deep")),
        [text("t4")]
    );
    // Absent values are left out, and no rows is an empty list.
    assert_eq!(
        sorted(db.scalar("staff.where { .id == \"a\" }.select { grades: .reports.grade }", "grades")),
        [int(2), int(3)]
    );
    assert_eq!(
        db.scalar("staff.where { .id == \"e\" }.select { ids: .reports.id }", "ids"),
        Value::List(Vec::new())
    );

    // "Any of them" is said with `contains` or `in`, or by filtering.
    assert_eq!(db.ids("staff.where { .reports.id contains \"d\" }"), ["b"]);
    assert_eq!(db.ids("staff.where { \"d\" in .reports.id }"), ["b"]);
    assert_eq!(db.ids("staff.where { .reports.where { .id == \"d\" }.exists() }"), ["b"]);
    assert_eq!(db.ids("jobs.where { .notes.kind contains \"y\" }"), ["j1"]);
    assert_eq!(db.ids("jobs.where { .notes.mins contains 1 }"), ["j2"]);
    // References reached this way match the id they point at.
    assert_eq!(db.ids("teams.where { .staff.boss contains \"d\" }"), ["t5"]);

    // Compared with one value it is an error that says what to write.
    for (query, advice) in [
        ("staff.where { .reports.id == \"d\" }", ".reports.where { .id == \"d\" }.exists()"),
        ("staff.where { .reports.pay > 50 }", ".reports.where { .pay > 50 }.exists()"),
        ("staff.where { 50 < .reports.pay }", "many values"),
        ("staff.where { .reports.id != \"d\" }", "many values"),
        ("staff.where { .reports == null }", ".reports.exists()"),
        ("jobs.where { .notes.mins >= 5 }", ".notes.where { .mins >= 5 }.exists()"),
        ("pings.where { .who.reports.pay <= 80 }", "many values"),
        // Inside a nested filter and inside a select as well.
        ("teams.where { .staff.where { .reports.pay > 1 }.exists() }", "many values"),
        ("staff.select { id, odd: if .reports.pay > 1 then 1 else 0 }", "many values"),
    ] {
        let error = db.try_rows(query).unwrap_err();
        assert!(error.contains("many values") && error.contains(advice), "{query}\n{error}");
    }
    // A list is still compared with a list.
    assert!(db.try_rows("staff.where { .reports.pay == [80, 80, 90] }").is_ok());

    // What has one value is unchanged: a plain field, a field through
    // references, an absent field, and a field that is absent on some rows.
    assert_eq!(db.ids("staff.where { .boss.boss.id == \"a\" }"), ["d"]);
    assert_eq!(db.ids("staff.where { .grade == null }"), ["c"]);
    assert_eq!(db.ids("staff.where { .boss == null }"), ["a"]);
    assert_eq!(db.ids("staff.where { .boss.grade == null && .boss != null }.sort { .id.asc }"), Vec::<String>::new());
    assert_eq!(db.scalar("staff.where { .team.parent.id == \"t1\" }.count()", "count"), int(3));
}

/// Text that reads as a query but cannot mean what it says used to give an
/// answer anyway: null, zero, no rows, or an unsorted result.
#[test]
fn what_cannot_mean_anything_says_so() {
    let mut db = company();
    let fails = |db: &Db, query: &str, parts: &[&str]| {
        let error = match db.try_rows(query) {
            Ok(rows) => panic!("answered with {} row(s): {query}", rows.len()),
            Err(error) => error,
        };
        for part in parts {
            assert!(error.contains(part), "{query}\n{error}\nlacks: {part}");
        }
    };

    // A misspelt name anywhere along a path, wherever the path is written.
    fails(&db, "staff.where { .team.parnet == \"t1\" }", &["'teams'", "'parnet'", "Did you mean 'parent'"]);
    fails(&db, "staff.select { id, x: .boss.boss.pya }", &["'staff'", "'pya'", "Did you mean 'pay'"]);
    fails(&db, "teams.where { .staff.where { .pya > 3 }.exists() }", &["'staff'", "'pya'"]);
    fails(&db, "teams.select { id, n: .staff.sum(.pya) }", &["'staff'", "'pya'"]);
    fails(&db, "teams.select { id, n: .staff.select { id, b: .boss.pya } }", &["'pya'"]);
    fails(&db, "staff.where { .pay > staff.avg(.pya) }", &["'pya'"]);
    fails(&db, "staff.sort { .team.idd.asc }", &["'teams'", "'idd'"]);
    fails(&db, "pings.group_by { .who.pya }.select { n: count() }", &["'pya'"]);
    // Inside an embedded object whose fields the schema declares.
    fails(&db, "jobs.where { .notes.where { .minz > 3 }.exists() }", &["'jobs.notes'", "'minz'", "Did you mean 'mins'"]);
    fails(&db, "jobs.select { m: .notes.minz }", &["'minz'"]);
    fails(&db, "jobs.notes.group_by { .knid }.select { n: count() }", &["'knid'"]);
    // A name that is declared and merely has no value is still null, and
    // the tests for presence may ask about anything.
    assert_eq!(db.ids("staff.where { .boss.grade == null && .boss != null }.sort { .id.asc }"), Vec::<String>::new());
    assert_eq!(db.ids("staff.where { .boss.boss.grade == 1 }"), ["d"]);
    assert!(db.try_rows("teams.walk(.children).select { id, d: ._depth }").is_ok());
    assert!(db.try_rows("staff.where { exists(.boss.grade) }").is_ok());

    // Arithmetic on a path through a collection. `.children*.id` is `*`
    // written to mean "any number of steps".
    fails(&db, "teams.select { all: .children*.id }", &["many values", ".walk(.children)"]);
    fails(&db, "staff.select { x: .reports.pay * 2 }", &["many values", ".reports.sum(.pay)"]);
    fails(&db, "staff.select { x: 1 + .reports.pay }", &["many values"]);
    assert_eq!(db.scalar("staff.where { .id == \"b\" }.select { x: .pay * 2 }", "x"), int(160));
    assert!(db.try_rows("jobs.select { kinds: .notes.kind + [\"z\"] }").is_ok());

    // In a grouped select, a path that is not a key is read from the
    // group's rows.
    assert_eq!(
        db.table(
            "staff.group_by { .team.id }
               .select { team_id, bosses: .boss.distinct().count(), reports: .reports.count(), pays: .pay }
               .sort { .team_id.asc }",
            &["team_id", "bosses", "reports", "pays"],
        ),
        vec![
            vec![text("t1"), int(0), int(3), Value::List(vec![int(100)])],
            vec![text("t2"), int(1), int(1), Value::List(vec![int(80), int(80)])],
            vec![text("t3"), int(1), int(0), Value::List(vec![int(90)])],
            vec![text("t4"), int(1), int(1), Value::List(vec![int(60)])],
            vec![text("t5"), int(1), int(0), Value::List(vec![int(40)])],
        ]
    );
    fails(&db, "staff.group_by { .team.id }.having { .pay > 50 }.select { team_id }", &["many values"]);

    // A function called as a method on the current row is given the row.
    for definition in [
        "fn is_boss(s: staff) -> Boolean { s.reports.count() > 0 }",
        "fn band(pay: Integer) -> String { if pay >= 80 then \"high\" else \"low\" }",
    ] {
        let owl_db::parser::OQLStatement::FunctionDef(definition) =
            owl_db::parse_oql_statement(definition).unwrap()
        else {
            panic!("not a function");
        };
        db.executor.register_user_function(definition).unwrap();
    }
    assert_eq!(db.ids("staff.where { .is_boss() }.sort { .id.asc }"), ["a", "b", "d"]);
    assert_eq!(db.ids("staff.where { is_boss(*) }.sort { .id.asc }"), ["a", "b", "d"]);
    assert_eq!(db.ids("staff.where { .boss.is_boss() && !.is_boss() }.sort { .id.asc }"), ["c", "e", "f"]);
    assert_eq!(db.ids("staff.where { .pay.band() == \"high\" }.sort { .id.asc }"), ["a", "b", "c", "f"]);

    // A sort by a function that does not exist, at any depth.
    fails(&db, "staff.sort { nonesuch(.pay).desc }", &["Unknown function: nonesuch"]);
    fails(&db, "staff.sort { .id.asc, abs(nonesuch(.pay)).desc }.limit(2)", &["nonesuch"]);
    fails(&db, "teams.select { id, s: .staff.sort { nonesuch(.pay).asc }.select { id } }", &["nonesuch"]);
    // One that exists still sorts, and a key that cannot be worked out for
    // a row is still null for that row, not a failure.
    assert_eq!(db.ids("staff.sort { abs(.pay - 70).asc, .id.asc }.limit(1)"), ["b"]);
    assert_eq!(db.ids("staff.sort { (.grade * 2).desc, .id.asc }.limit(2)"), ["f", "b"]);
}

/// Composition instead of new words: an aggregate follows a path of
/// values, and a function written in OQL takes a collection, so what would
/// otherwise be a built-in is a chain or a definition.
#[test]
fn values_and_functions_compose() {
    let mut db = company();
    // An aggregate with nothing named aggregates the values it follows.
    assert_eq!(db.scalar("staff.pay.max()", "max"), int(100));
    assert_eq!(db.scalar("staff.pay.min()", "min"), int(40));
    assert_eq!(db.scalar("staff.pay.sum()", "sum"), int(450));
    assert_eq!(db.scalar("staff.pay.avg()", "avg"), Value::Float(75.0));
    assert_eq!(db.scalar("staff.pay.median()", "median"), Value::Float(80.0));
    assert_eq!(db.scalar("staff.pay.percentile(0.25)", "percentile"), Value::Float(65.0));
    assert_eq!(db.scalar("staff.pay.distinct().count()", "count"), int(5));
    // It is the same value as naming the field.
    assert_eq!(db.scalar("staff.pay.sum()", "sum"), db.scalar("staff.sum(.pay)", "sum"));
    // Through a relationship, in a filter, and in a group.
    assert_eq!(
        db.table(
            "staff.where { .id == \"a\" }.select { top: .reports.pay.max(), all: .reports.pay.sum(), kinds: .reports.pay.distinct().count() }",
            &["top", "all", "kinds"],
        ),
        vec![vec![int(90), int(250), int(2)]]
    );
    assert_eq!(db.ids("staff.where { .reports.pay.max() > 85 }"), ["a"]);
    assert_eq!(
        db.table(
            "staff.group_by { .team.id }.select { team_id, top: .pay.max(), kinds: .pay.distinct().count() }
               .where { .team_id == \"t2\" }",
            &["top", "kinds"],
        ),
        vec![vec![int(80), int(1)]]
    );
    assert_eq!(db.scalar("jobs.notes.mins.sum()", "sum"), int(15));

    for definition in [
        "fn spread(rows) { rows.max(.pay) - rows.min(.pay) }",
        "fn bosses_among(rows) { rows.where { .reports.exists() }.count() }",
        "fn distinct_count(values) { values.distinct().count() }",
        "fn total(values) { values.sum() }",
        "fn raise(pay: Integer, by: Float = 0.5) -> Float { pay * (1 + by) }",
        "fn seniors(least: Integer = 70) { staff.where { .pay > least } }",
        "fn headline(s: staff) { s.id + \":\" + to_string(s.pay) }",
    ] {
        let owl_db::parser::OQLStatement::FunctionDef(definition) =
            owl_db::parse_oql_statement(definition).unwrap_or_else(|error| panic!("{error}\n{definition}"))
        else {
            panic!("not a function");
        };
        db.executor.register_user_function(definition).unwrap();
    }

    // A function is given a collection: a relationship keeps its rows as
    // rows, so the function can filter and follow relationships from them.
    assert_eq!(db.scalar("staff.where { .id == \"a\" }.select { s: spread(.reports) }", "s"), int(10));
    assert_eq!(db.scalar("staff.where { .id == \"a\" }.select { s: .reports.spread() }", "s"), int(10));
    assert_eq!(
        db.table("staff.sort { .id.asc }.select { id, n: bosses_among(.reports) }", &["id", "n"]),
        vec![
            vec![text("a"), int(1)],
            vec![text("b"), int(1)],
            vec![text("c"), int(0)],
            vec![text("d"), int(0)],
            vec![text("e"), int(0)],
            vec![text("f"), int(0)],
        ]
    );
    // What a built-in word would do, written in the language.
    assert_eq!(
        db.table(
            "staff.where { .id == \"a\" }.select { kinds: distinct_count(.reports.pay), all: total(.reports.pay) }",
            &["kinds", "all"],
        ),
        vec![vec![int(2), int(250)]]
    );
    assert_eq!(db.ids("staff.where { total(.reports.pay) > 100 }"), ["a"]);

    // In a grouped select the function is given the group's values.
    assert_eq!(
        db.table(
            "staff.group_by { .team.id }.select { team_id, all: total(.pay), kinds: distinct_count(.pay) }
               .where { .team_id == \"t2\" }",
            &["all", "kinds"],
        ),
        vec![vec![int(160), int(1)]]
    );

    // A parameter left out takes its default.
    assert_eq!(db.scalar("staff.where { .id == \"e\" }.select { r: raise(.pay) }", "r"), Value::Float(60.0));
    assert_eq!(db.scalar("staff.where { .id == \"e\" }.select { r: raise(.pay, 1.0) }", "r"), Value::Float(80.0));
    assert_eq!(db.scalar("staff.where { .id == \"e\" }.select { r: .pay.raise() }", "r"), Value::Float(60.0));
    assert!(db.try_rows("staff.select { r: raise() }").unwrap_err().contains("1 to 2"));
    assert!(db.try_rows("staff.select { r: raise(.pay, 1.0, 2.0) }").is_err());

    // A function starts a query, and what follows applies to its rows,
    // which are still rows of their collection.
    assert_eq!(db.scalar("seniors().count()", "count"), int(4));
    assert_eq!(db.ids("seniors(85).sort { .id.asc }"), ["a", "f"]);
    assert_eq!(db.scalar("seniors(85).where { .reports.exists() }.count()", "count"), int(1));
    assert_eq!(
        db.table(
            "seniors().sort { .pay.desc, .id.asc }.limit(2).select { id, n: .reports.count(), h: .headline() }",
            &["id", "n", "h"],
        ),
        vec![
            vec![text("a"), int(3), text("a:100")],
            vec![text("f"), int(0), text("f:90")],
        ]
    );
    assert_eq!(db.scalar("staff.where { .id == \"a\" }.select { n: seniors(85).count() }", "n"), int(2));
    assert!(db.try_rows("nonesuch().count()").unwrap_err().contains("Unknown function: nonesuch"));
}

/// Inside a nested select too, a collection the path passed through is
/// the row each result came from.
#[test]
fn a_nested_path_can_name_the_rows_it_passed_through() {
    let db = company();
    let nested = |query: &str, field: &str| match db.scalar(query, field) {
        Value::List(items) => {
            let mut rows: Vec<Vec<(String, Value)>> = items
                .into_iter()
                .map(|item| match item {
                    Value::Object(fields) => {
                        let mut fields: Vec<_> = fields
                            .into_iter()
                            .filter(|(name, _)| !name.starts_with('$'))
                            .collect();
                        fields.sort_by(|left, right| left.0.cmp(&right.0));
                        fields
                    }
                    other => panic!("not a row: {other:?}"),
                })
                .collect();
            rows.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
            rows
        }
        other => panic!("not a list: {other:?}\n{query}"),
    };
    let row = |id: &str, who: &str, pay: i64| {
        vec![
            ("id".to_string(), text(id)),
            ("pay".to_string(), int(pay)),
            ("who".to_string(), text(who)),
        ]
    };
    // Team t2 holds b and c; only b has pings.
    assert_eq!(
        nested(
            "teams.where { .id == \"t2\" }.select { p: .staff.pings.select { id, who: staff.id, pay: staff.pay } }",
            "p",
        ),
        vec![row("p4", "b", 80), row("p5", "b", 80)]
    );
    // In a filter and an aggregate as well as a projection.
    assert_eq!(
        db.table(
            "teams.sort { .id.asc }.limit(2).select { id, n: .staff.pings.where { staff.pay >= 90 }.count() }",
            &["id", "n"],
        ),
        vec![vec![text("t1"), int(3)], vec![text("t2"), int(0)]]
    );
    // The enclosing row's own collection still means the enclosing row:
    // every ping reached from `a` reports `a`, not the report it belongs to.
    assert_eq!(
        nested(
            "staff.where { .id == \"a\" }.select { p: .reports.pings.select { id, who: staff.id, pay: staff.pay } }",
            "p",
        ),
        vec![row("p4", "a", 100), row("p5", "a", 100)]
    );
    // A path that names nothing on the way is unchanged.
    assert_eq!(db.scalar("teams.where { .id == \"t1\" }.select { n: .staff.pings.count() }", "n"), int(3));
}

/// The rest of composition: names after a select are checked, a function
/// follows paths from the rows it is given, any collection a query steps
/// through can be named, and after `partition_by` the rows in hand are the
/// row's partition.
#[test]
fn rows_in_hand_are_named_and_reduced_wherever_they_are() {
    let mut db = company();

    // After a select, a name is one the select produced, a relationship of
    // the row, or a mistake.
    let error = db.try_rows("staff.select { id }.where { .nmae == 1 }").unwrap_err();
    assert!(error.contains("'nmae'") && error.contains("select"), "{error}");
    let error = db.try_rows("staff.select { id, wage: .pay }.sort { .wagee.asc }").unwrap_err();
    assert!(error.contains("Did you mean 'wage'"), "{error}");
    assert_eq!(db.ids("staff.select { id, wage: .pay }.where { .wage > 85 }.sort { .id.asc }"), ["a", "f"]);
    assert_eq!(db.ids("staff.select { id }.where { .reports.count() > 0 }.sort { .id.asc }"), ["a", "b", "d"]);
    assert_eq!(db.ids("staff.select { * }.where { .pay > 90 }"), ["a"]);
    assert_eq!(db.scalar("staff.select { n: count() }.where { .n > 1 }", "n"), int(6));
    assert!(db.try_rows("staff.group_by { .team.id }.select { team_id, n: count() }.where { .n > 1 }").is_ok());

    for definition in [
        "fn grand(rows) { rows.reports.count() }",
        "fn top_pay(rows) { rows.pay.max() }",
        "fn pays(rows) { rows.pay }",
        "fn under(s: staff) { s.reports.reports.count() }",
    ] {
        let owl_db::parser::OQLStatement::FunctionDef(definition) =
            owl_db::parse_oql_statement(definition).unwrap()
        else {
            panic!("not a function");
        };
        db.executor.register_user_function(definition).unwrap();
    }
    // A function follows a path from the rows it was given.
    assert_eq!(
        db.table(
            "staff.where { .id == \"a\" }.select { g: grand(.reports), t: top_pay(.reports), u: under(*), v: .under() }",
            &["g", "t", "u", "v"],
        ),
        vec![vec![int(1), int(90), int(1), int(1)]]
    );
    let Value::List(mut pays) = db.scalar("staff.where { .id == \"a\" }.select { p: pays(.reports) }", "p") else {
        panic!("not a list");
    };
    pays.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
    assert_eq!(pays, [int(80), int(80), int(90)]);

    // At the top level, a collection reached on the way can be named.
    assert_eq!(
        db.table("teams.staff.pings.select { id, who: staff.id }.sort { .id.asc }", &["id", "who"]),
        vec![
            vec![text("p1"), text("a")],
            vec![text("p2"), text("a")],
            vec![text("p3"), text("a")],
            vec![text("p4"), text("b")],
            vec![text("p5"), text("b")],
        ]
    );
    assert_eq!(
        db.table(
            "teams.where { .id == \"t2\" }.staff.pings.group_by { pay: staff.pay }.select { pay, n: count() }",
            &["pay", "n"],
        ),
        vec![vec![int(80), int(2)]]
    );
    assert_eq!(
        db.table(
            "teams.staff.where { .pay < 100 }.pings.select { id, who: staff.id }.sort { .id.asc }",
            &["id", "who"],
        ),
        vec![vec![text("p4"), text("b")], vec![text("p5"), text("b")]]
    );
    assert_eq!(
        db.table("jobs.notes.where { .mins > 5 }.select { kind, job: jobs.id }", &["kind", "job"]),
        vec![vec![text("y"), text("j1")]]
    );

    // After `partition_by`, an aggregate or an operation with no receiver
    // is over the row's partition, and every row is kept.
    assert_eq!(
        db.table(
            "staff.sort { .id.asc }.partition_by { .team.id }.select {
               id,
               total: sum(.pay),
               n: count(),
               rich: where { .pay >= 80 }.count(),
               top: sort { .pay.desc, .id.asc }.first().id,
               k: row_number()
             }",
            &["id", "total", "n", "rich", "top", "k"],
        ),
        vec![
            vec![text("a"), int(100), int(1), int(1), text("a"), int(1)],
            vec![text("b"), int(160), int(2), int(2), text("b"), int(1)],
            vec![text("c"), int(160), int(2), int(2), text("b"), int(2)],
            vec![text("d"), int(60), int(1), int(0), text("d"), int(1)],
            vec![text("e"), int(40), int(1), int(0), text("e"), int(1)],
            vec![text("f"), int(90), int(1), int(1), text("f"), int(1)],
        ]
    );
    // One partition for everything gives a share of the whole.
    assert_eq!(
        db.table(
            "staff.sort { .id.asc }.partition_by { all: 1 }.select { id, total: sum(.pay), most: max(.pay) }.limit(2)",
            &["id", "total", "most"],
        ),
        vec![vec![text("a"), int(450), int(100)], vec![text("b"), int(450), int(100)]]
    );
    // A limit after such a select does not shrink the partitions.
    assert_eq!(
        db.table(
            "staff.sort { .id.asc }.partition_by { .team.id }.select { id, total: sum(.pay), k: row_number() }.limit(2)",
            &["id", "total"],
        ),
        vec![vec![text("a"), int(100)], vec![text("b"), int(160)]]
    );
    // Without `partition_by` an aggregate select is still one row.
    assert_eq!(db.rows("staff.select { total: sum(.pay) }").len(), 1);
    // Through a relationship as well.
    let Value::List(rows) = db.scalar(
        "staff.where { .id == \"a\" }.select { r: .reports.sort { .id.asc }.partition_by { .pay }.select { id, same: count() } }",
        "r",
    ) else {
        panic!("not a list");
    };
    let same: Vec<Value> = rows
        .iter()
        .map(|row| match row {
            Value::Object(fields) => fields.get("same").cloned().unwrap_or(Value::Null),
            other => panic!("not a row: {other:?}"),
        })
        .collect();
    assert_eq!(same, [int(2), int(2), int(1)]);
}

/// Two collections joined with `+` keep every row of both, and such a sum
/// can start a query.
#[test]
fn collections_join_with_plus() {
    let db = company();
    let count = |query: &str| db.scalar(query, "count");
    assert_eq!(count("(staff.where { .pay >= 90 } + staff.where { .pay >= 80 }).count()"), int(6));
    // Every row is kept, where `union` keeps one of each.
    assert_eq!(
        db.ids("(staff.where { .pay >= 90 } + staff.where { .pay >= 80 }).sort { .id.asc }"),
        ["a", "a", "b", "c", "f", "f"]
    );
    assert_eq!(count("staff.where { .pay >= 90 }.union(staff.where { .pay >= 80 }).count()"), int(4));
    assert_eq!(count("(staff.where { .pay >= 90 } + staff.where { .pay >= 80 }).distinct().count()"), int(4));
    // The order is the first collection's rows, then the second's.
    assert_eq!(
        db.ids("(staff.where { .id == \"f\" } + staff.where { .id == \"a\" } + staff.where { .id == \"c\" })"),
        ["f", "a", "c"]
    );
    // Whole collections by name, and collections of different kinds.
    assert_eq!(count("(staff + staff).count()"), int(12));
    assert_eq!(count("(teams + staff).count()"), int(11));
    // The rows are still rows of their collections.
    assert_eq!(
        db.table(
            "(staff.where { .id == \"b\" } + staff.where { .id == \"a\" }).select { id, n: .reports.count() }",
            &["id", "n"],
        ),
        vec![vec![text("b"), int(1)], vec![text("a"), int(3)]]
    );
    assert_eq!(
        db.table(
            "(staff.where { .team == \"t2\" } + staff.where { .pay == 100 })
               .group_by { .pay }.select { pay, n: count() }.sort { .pay.asc }",
            &["pay", "n"],
        ),
        vec![vec![int(80), int(2)], vec![int(100), int(1)]]
    );
    // Inside an expression it is the same thing.
    assert_eq!(
        db.scalar("teams.where { .id == \"t1\" }.select { n: (.staff + .children).count() }", "n"),
        int(3)
    );
    // Parentheses round one collection are just that collection.
    assert_eq!(db.ids("(staff.where { .pay == 40 }).select { id }"), ["e"]);
    // In a script, among other statements.
    db.script("both = 1\n(staff + teams).count()\nstaff.count()");
}

/// A field the schema declares as a list, compared with one value, is a
/// mistake that used to be false for every row.
#[test]
fn a_declared_list_is_not_compared_with_one_value() {
    let db = Db::new();
    db.script(
        r#"
        schema kits {
          id: String,
          tags: List(String),
          scores: Optional(List(Integer)),
          parts: List({ kind: String, sizes: List(Integer) })
        }
        kits.insert [
          { id: "k1", tags: ["red", "big"], scores: [1, 9], parts: [ { kind: "a", sizes: [1, 2] } ] },
          { id: "k2", tags: ["red"], parts: [] },
          { id: "k3", tags: [], scores: [4], parts: [ { kind: "b", sizes: [] } ] }
        ]
        loose.insert [ { id: "l1", tags: ["red"] }, { id: "l2", tags: "red" } ]
        "#,
    );
    for (query, advice) in [
        ("kits.where { .tags == \"red\" }", ".tags contains \"red\""),
        ("kits.where { \"red\" == .tags }", ".tags contains \"red\""),
        ("kits.where { .tags != \"red\" }", "!(.tags contains \"red\")"),
        ("kits.where { .scores > 3 }", ".scores.where { . > 3 }.exists()"),
        ("kits.select { id, red: if .tags == \"red\" then 1 else 0 }", "is a list"),
        ("kits.where { .parts.where { .sizes == 2 }.exists() }", ".sizes contains 2"),
        ("kits.sort { .id.asc }.limit(1).where { .tags == \"red\" }", "is a list"),
    ] {
        let error = match db.try_rows(query) {
            Ok(rows) => panic!("answered with {} row(s): {query}", rows.len()),
            Err(error) => error,
        };
        assert!(error.contains("is a list") && error.contains(advice), "{query}\n{error}");
    }
    // What the messages point at, and what was always fine.
    assert_eq!(db.ids("kits.where { .tags contains \"red\" }.sort { .id.asc }"), ["k1", "k2"]);
    assert_eq!(db.ids("kits.where { !(.tags contains \"red\") }"), ["k3"]);
    assert_eq!(db.ids("kits.where { .tags == [\"red\"] }"), ["k2"]);
    assert_eq!(db.ids("kits.where { .scores.where { . > 3 }.exists() }.sort { .id.asc }"), ["k1", "k3"]);
    assert_eq!(db.ids("kits.where { .scores == null }"), ["k2"]);
    assert_eq!(db.ids("kits.where { .tags != null }.sort { .id.asc }"), ["k1", "k2", "k3"]);
    assert_eq!(db.ids("kits.where { .parts.where { .sizes contains 2 }.exists() }"), ["k1"]);
    // With no schema nothing says the field is a list, and here it is not
    // always one: the comparison stays a quiet mismatch where it is.
    assert_eq!(db.ids("loose.where { .tags == \"red\" }"), ["l2"]);
    assert_eq!(db.ids("loose.where { .tags contains \"red\" }.sort { .id.asc }"), ["l1", "l2"]);
}

/// The standard library is written in OQL and is there from the start.
#[test]
fn the_standard_library_is_ordinary_functions() {
    let mut db = company();
    let near = |value: Value, expected: f64| match value {
        Value::Float(found) => assert!((found - expected).abs() < 1e-9, "{found} is not {expected}"),
        other => panic!("not a number: {other:?}, wanted {expected}"),
    };
    // Pay is 100 80 80 60 40 90: mean 75, squared distances sum to 2350.
    near(db.scalar("staff.pay.variance()", "variance"), 470.0);
    near(db.scalar("staff.pay.stddev()", "stddev"), 470.0_f64.sqrt());
    near(db.scalar("staff.pay.variance_pop()", "variance_pop"), 2350.0 / 6.0);
    near(db.scalar("staff.pay.stddev_pop()", "stddev_pop"), (2350.0_f64 / 6.0).sqrt());
    assert_eq!(db.scalar("staff.pay.mode()", "mode"), int(80));
    // After a filter, through a relationship, as a call, and in a group.
    near(db.scalar("staff.where { .team == \"t2\" }.pay.variance()", "variance"), 0.0);
    near(
        db.scalar("staff.where { .id == \"a\" }.select { s: .reports.pay.variance() }", "s"),
        100.0 / 3.0,
    );
    near(
        db.scalar("staff.where { .id == \"a\" }.select { s: stddev(.reports.pay) }", "s"),
        (100.0_f64 / 3.0).sqrt(),
    );
    assert_eq!(
        db.table(
            "staff.group_by { .team.id }.select { team_id, v: variance(.pay), m: .pay.mode() }
               .sort { .team_id.asc }.limit(2)",
            &["team_id", "v", "m"],
        ),
        vec![
            // One value has no spread to estimate.
            vec![text("t1"), Value::Null, int(100)],
            vec![text("t2"), Value::Float(0.0), int(80)],
        ]
    );
    // Too few values, and none.
    assert_eq!(db.scalar("staff.where { .id == \"a\" }.pay.stddev()", "stddev"), Value::Null);
    assert_eq!(db.scalar("staff.where { .pay > 999 }.pay.variance_pop()", "variance_pop"), Value::Null);
    assert_eq!(db.scalar("staff.where { .pay > 999 }.pay.mode()", "mode"), Value::Null);
    // Of values that tie, the least.
    assert_eq!(db.scalar("staff.where { .pay < 80 }.pay.mode()", "mode"), int(40));

    // Any function written in OQL ends a chain the same way: it is given
    // the rows in hand, and may take more arguments.
    db.script(
        "fn headcount(rows) { rows.count() }\nfn paid_over(rows, least: Integer = 0) { rows.where { .pay > least } }\nfn twice(x: Integer) { x * 2 }",
    );
    assert_eq!(db.scalar("staff.where { .pay > 70 }.headcount()", "headcount"), int(4));
    assert_eq!(db.scalar("staff.where { .reports.exists() }.headcount()", "headcount"), int(3));
    assert_eq!(db.ids("staff.paid_over(85).sort { .id.asc }"), ["a", "f"]);
    assert_eq!(db.scalar("staff.paid_over().headcount()", "headcount"), int(6));
    assert!(db.try_rows("staff.pay.nonesuch()").unwrap_err().contains("Unknown function: nonesuch"));
    assert!(db.try_rows("staff.paid_over(1, 2)").is_err());
    // A function a script defined is there for the queries after it.
    assert_eq!(db.scalar("staff.where { .id == \"e\" }.select { t: twice(.pay) }", "t"), int(80));

    // A library function is replaced like any other.
    let owl_db::parser::OQLStatement::FunctionDef(definition) =
        owl_db::parse_oql_statement("fn mode(values) { values.max() }").unwrap()
    else {
        panic!("not a function");
    };
    db.executor.register_user_function(definition).unwrap();
    assert_eq!(db.scalar("staff.pay.mode()", "mode"), int(100));
}

/// A function that takes a row is a step in a chain: it is called for each
/// row, and the rows it gives for all of them carry on. That is how rows
/// are paired with other rows without a join.
#[test]
fn a_function_of_a_row_is_a_step() {
    let db = company();
    db.script(
        r#"
        fn peers(s: staff) { staff.where { .boss == s.boss && .id > s.id } }
        fn paid_within(s: staff, gap: Integer = 10) { staff.where { .id != s.id && abs(.pay - s.pay) <= gap } }
        fn under(s: staff) { s.reports }
        fn team_pings(t: teams) { pings.where { .who.team == t.id } }
        fn headcount(rows) { rows.count() }
        "#,
    );
    // Each pair of people with the same boss, once. The collection's name
    // is the row the function was called for; `.` is the row it gave.
    assert_eq!(
        db.table(
            "staff.peers().select { a: staff.id, b: .id }.sort { .a.asc, .b.asc }",
            &["a", "b"],
        ),
        vec![
            vec![text("b"), text("c")],
            vec![text("b"), text("f")],
            vec![text("c"), text("f")],
        ]
    );
    assert_eq!(db.scalar("staff.peers().count()", "count"), int(3));
    // Further arguments, and a default.
    assert_eq!(db.ids("staff.where { .id == \"b\" }.paid_within().sort { .id.asc }"), ["c", "f"]);
    assert_eq!(db.scalar("staff.where { .id == \"b\" }.paid_within(20).count()", "count"), int(4));
    // The rows given are still rows of their collection, and whatever
    // follows applies to them: a relationship, a filter, a grouping.
    assert_eq!(
        db.table(
            "staff.peers().select { b: .id, pay: .pay, has: .reports.count() }.sort { .b.asc }.limit(1)",
            &["b", "pay", "has"],
        ),
        vec![vec![text("c"), int(80), int(0)]]
    );
    assert_eq!(
        db.table(
            "staff.peers().where { .pay > staff.pay }.select { a: staff.id, b: .id }.sort { .a.asc }",
            &["a", "b"],
        ),
        vec![vec![text("b"), text("f")], vec![text("c"), text("f")]]
    );
    assert_eq!(
        db.table(
            "staff.peers().group_by { left: staff.id }.select { left, n: count() }.sort { .left.asc }",
            &["left", "n"],
        ),
        vec![vec![text("b"), int(2)], vec![text("c"), int(1)]]
    );
    // From one collection to another.
    assert_eq!(
        db.table(
            "teams.team_pings().select { team: teams.id, id }.sort { .id.asc }",
            &["team", "id"],
        ),
        vec![
            vec![text("t1"), text("p1")],
            vec![text("t1"), text("p2")],
            vec![text("t1"), text("p3")],
            vec![text("t2"), text("p4")],
            vec![text("t2"), text("p5")],
        ]
    );
    // A function whose body is not a query, and steps one after another.
    assert_eq!(db.scalar("staff.where { .id == \"a\" }.under().count()", "count"), int(3));
    assert_eq!(db.scalar("staff.where { .id == \"a\" }.under().under().count()", "count"), int(1));
    // A function of the rows together is unchanged, and follows a step.
    assert_eq!(db.scalar("staff.peers().headcount()", "headcount"), int(3));
    // The tree form is still there for when pairs are not wanted flat.
    assert_eq!(db.scalar("staff.where { .id == \"b\" }.select { n: peers(*).count() }", "n"), int(2));
}

/// A select after `partition_by` shapes each partition with its own rows
/// in reach. When each partition's rows come together it does so a
/// partition at a time; when they are interleaved it holds them all. The
/// two must give every row the same answer, whatever the row asks of its
/// position: its neighbours, its number, the rows so far, the whole
/// partition.
#[test]
fn a_partition_is_shaped_the_same_together_or_apart() {
    let directory = TempDir::new().unwrap();
    let executor = QueryExecutor::new_with_real_clock(Arc::new(
        RustStore::new(directory.path().join("owl"), 16 << 20).unwrap(),
    ));
    let mut script = String::from(
        "schema readings { id: String, station: String, at: Integer, level: Integer }\nreadings.insert [\n",
    );
    for index in 0..120 {
        script.push_str(&format!(
            "  {{ id: \"r{index:03}\", station: \"s{}\", at: {}, level: {} }},\n",
            index % 7,
            (index * 37) % 101,
            (index * 13) % 29
        ));
    }
    script.truncate(script.len() - 2);
    script.push_str("\n]");
    let report = execute_oql_file(&parse_oql_file(&script).unwrap(), &executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let shaped = "select {
        id,
        number: row_number(),
        before: prev(.level),
        after: next(.level),
        running: so_far().level.sum(),
        recent: so_far().last(3).level.avg(),
        high: so_far().where { .level > 14 }.count(),
        rank: rank(.at),
        whole: count(),
        top: max(.level),
        share: .level * 100 / sum(.level)
    }";
    let ask = |sort: &str| -> Vec<String> {
        let query = format!("readings.sort {{ {sort} }}.partition_by {{ .station }}.{shaped}.sort {{ .id.asc }}");
        executor
            .execute_query(&parse_oql(&query).unwrap_or_else(|error| panic!("{error}\n{query}")))
            .unwrap_or_else(|error| panic!("{error}\n{query}"))
            .iter()
            .map(|row| {
                let mut fields: Vec<String> =
                    row.fields.iter().map(|(name, value)| format!("{name}={value:?}")).collect();
                fields.sort();
                fields.join(" ")
            })
            .collect()
    };
    // Each station's readings together, in order of time within it.
    let together = ask(".station.asc, .at.asc, .id.asc");
    // In order of time alone: the stations are interleaved, and each
    // station's readings are still in order of time among themselves.
    let apart = ask(".at.asc, .id.asc");
    assert_eq!(together.len(), 120);
    assert_eq!(together, apart);
    // And the answers are of the partition, not of the whole.
    assert!(together.iter().any(|row| row.contains("whole=Integer(18)")), "{:?}", &together[..2]);
    assert!(together.iter().all(|row| !row.contains("whole=Integer(120)")));
    assert!(together.iter().any(|row| row.contains("number=Integer(1) ")));
}
