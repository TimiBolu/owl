//! What a program that writes queries is given: a check that runs nothing,
//! problems as data, a catalogue of what is in the database, and a budget
//! that stops a query that costs too much.

use owl_db::assist::{self, Problem};
use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, with_budget, Budget, OQLError, QueryExecutor};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn database() -> (TempDir, QueryExecutor<RustStore>) {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let source = r#"
        schema people { id: String, name: String, level: Integer, manager: Optional(Ref(people)) as reports, index(.level), unique(.name) }
        schema projects { id: String, name: String, owner: Ref(people) as owned }
        schema assignments { id: String, person: Ref(people), project: Ref(projects), role: String, hours: Integer }
        schema tasks { id: String, project: Ref(projects), tags: List(String), notes: List({ kind: String, mins: Integer }) }
        fn senior(level: Integer) { level >= 3 }
        people.insert [
          { id: "kim", name: "Kim", level: 3 }, { id: "ada", name: "Ada", level: 1, manager: "kim" },
          { id: "ben", name: "Ben", level: 2, manager: "kim" }
        ]
        projects.insert [ { id: "apollo", name: "Apollo", owner: "kim" } ]
        assignments.insert [
          { id: "a1", person: "ada", project: "apollo", role: "lead", hours: 20 },
          { id: "a2", person: "ben", project: "apollo", role: "dev", hours: 30 }
        ]
    "#;
    let report = execute_oql_file(&parse_oql_file(source).unwrap(), &executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    (directory, executor)
}

fn problem(executor: &QueryExecutor<RustStore>, query: &str) -> Problem {
    match assist::check(executor, query) {
        Ok(checked) => panic!("expected a problem, and it passed: {checked:?}\n{query}"),
        Err(problem) => problem,
    }
}

fn count(executor: &QueryExecutor<RustStore>, collection: &str) -> usize {
    executor.execute_query(&parse_oql(collection).unwrap()).unwrap().len()
}

#[test]
fn a_statement_that_passes_says_what_it_would_do() {
    let (_directory, executor) = database();
    let read = assist::check(
        &executor,
        "people.where { .name == $who && senior(.level) }.select { name, n: .reports.count() }",
    )
    .unwrap();
    assert_eq!(read.collection, "people");
    assert!(!read.writes);
    assert_eq!(read.parameters, ["who"]);

    for query in [
        "projects { p in people  where { p.id == .owner.id }  select { name, boss: p.name } }",
        "assignments.group_by { .role }.select { role, hours: sum(.hours) }.sort { .hours desc }.explain()",
        "people.select { name, up: upper(.name), spread: .reports.level.stddev() }",
        "people.where { .reports.where { .level > 0 }.count() > 0 }",
        "people.where { .assignments.hours.sum() > 10 }",
        "tasks.where { .notes.mins.sum() > 3 && .tags contains \"api\" }",
        "people.where { .assignments.hours contains 20 }",
    ] {
        assert!(!assist::check(&executor, query).unwrap_or_else(|problem| panic!("{problem:?}\n{query}")).writes);
    }
}

#[test]
fn a_check_writes_nothing_and_says_which_statements_would() {
    let (_directory, executor) = database();
    for query in [
        "people.insert { id: \"zed\", name: \"Zed\", level: 2 }",
        "people.where { .level < 3 }.update { level += 1 }",
        "people.where { .level < 2 }.delete()",
    ] {
        assert!(assist::check(&executor, query).unwrap().writes, "{query}");
    }
    assert_eq!(count(&executor, "people"), 3);
    assert_eq!(count(&executor, "people.where { .level == 1 }"), 1);
}

#[test]
fn text_that_is_not_oql_says_where() {
    let (_directory, executor) = database();
    let found = problem(&executor, "people\n  .where { .level > }");
    assert_eq!(found.code, "parse_error");
    assert_eq!((found.line, found.column), (Some(2), Some(21)));
    assert!(found.message.contains("Expected literal value"), "{}", found.message);

    // An operation on a group's rows written with a dot before it, as
    // the first model to sit the questions wrote it.
    let found = problem(
        &executor,
        "assignments.group_by { .role }.select { role, big: .where { .hours > 25 }.count() }",
    );
    assert_eq!((found.code, found.column), ("parse_error", Some(52)));
    assert!(found.message.contains("written with no dot, as in `where { .. }.count()`"), "{}", found.message);

    // `=` where `==` was meant: the commonest slip of someone who knows SQL.
    let found = problem(&executor, "people.where { .name = \"Kim\" }");
    assert_eq!(found.code, "parse_error");
    assert_eq!((found.line, found.column), (Some(1), Some(22)));
}

#[test]
fn a_name_that_is_not_there_comes_with_the_names_that_are() {
    let (_directory, executor) = database();
    let people = ["id", "level", "manager", "name", "assignments", "owned", "reports"];

    let found = problem(&executor, "peple.where { .level > 1 }");
    assert_eq!(found.code, "unknown_collection");
    assert_eq!(found.suggestion.as_deref(), Some("people"));
    assert_eq!(found.names["collections"], ["assignments", "people", "projects", "tasks"]);

    for query in [
        "people.where { .levle > 1 }",
        "people.sort { .levle }",
        "people.group_by { .levle }.select { n: count() }",
        // Through a relationship, and inside the query nested in it.
        "people.where { .reports.where { .levle > 0 }.count() > 0 }",
    ] {
        let found = problem(&executor, query);
        assert_eq!(found.code, "unknown_name", "{query}");
        assert_eq!(found.suggestion.as_deref(), Some("level"), "{query}");
        assert_eq!(found.names["people"], people, "{query}");
    }

    // Through a reference: the name is one of the collection referred to.
    let found = problem(&executor, "projects.select { name, boss: .owner.nmae }");
    assert_eq!((found.code, found.suggestion.as_deref()), ("unknown_name", Some("name")));
    assert_eq!(found.names["people"], people);

    // Of a captured row.
    let found = problem(
        &executor,
        "projects { p in people  where { p.id == .owner.id && p.levle > 1 }  select { name } }",
    );
    assert_eq!(found.code, "unknown_name");
    assert_eq!(found.names["people"], people);

    // Inside an embedded object of a declared shape.
    let found = problem(&executor, "tasks.where { .notes.where { .knd == \"call\" }.exists() }");
    assert_eq!((found.code, found.suggestion.as_deref()), ("unknown_name", Some("kind")));

    let found = problem(&executor, "people.where { uper(.name) == \"KIM\" }");
    assert_eq!((found.code, found.suggestion.as_deref()), ("unknown_function", Some("upper")));
}

#[test]
fn a_comparison_that_could_never_hold_is_refused_before_any_row_is_read() {
    let (_directory, executor) = database();
    assert_eq!(problem(&executor, "people.where { .level > \"x\" }").code, "type_mismatch");

    // A path through a collection, compared with one value. Run, it fails
    // on the first row; checked, it fails with no row read, and both say
    // what to write.
    for query in [
        "people.where { .assignments.hours > 10 }",
        "people.where { .manager.reports.level == 1 }",
        "tasks.where { .notes.mins > 3 }",
    ] {
        let found = problem(&executor, query);
        assert!(found.message.contains("has many values"), "{query}: {}", found.message);
        assert!(found.message.contains(".exists()"), "{query}: {}", found.message);
    }
    let ran = executor
        .execute_query(&parse_oql("people.where { .assignments.hours > 10 }").unwrap())
        .unwrap_err()
        .to_string();
    assert!(ran.contains(&problem(&executor, "people.where { .assignments.hours > 10 }").message), "{ran}");

    let found = problem(&executor, "people.where { count() > 1 }");
    assert!(found.message.contains("count() has no collection"), "{}", found.message);
}

#[test]
fn the_catalogue_is_a_schema_with_what_a_schema_does_not_say() {
    let (_directory, executor) = database();
    let catalogue = assist::catalogue(&executor).unwrap();
    assert_eq!(
        catalogue.as_text(),
        "\
schema assignments {  // 2 rows
  id: String
  hours: Integer
  person: Ref(people)
  project: Ref(projects)
  role: String
}
schema people {  // 3 rows
  id: String
  level: Integer
  manager: Optional(Ref(people))
  name: String
  // .assignments: the assignments whose .person is this row
  // .owned: the projects whose .owner is this row
  // .reports: the people whose .manager is this row
  // index, ordered: level
  // index, unique: name
}
schema projects {  // 1 row
  id: String
  name: String
  owner: Ref(people)
  // .assignments: the assignments whose .project is this row
  // .tasks: the tasks whose .project is this row
}
schema tasks {  // 0 rows
  id: String
  notes: List({ kind: String, mins: Integer })
  project: Ref(projects)
  tags: List(String)
}
// functions defined here: senior(level: Integer)
"
    );

    let people = catalogue.collection("people").unwrap();
    assert_eq!(people.rows, Some(3));
    let manager = people.fields.iter().find(|field| field.name == "manager").unwrap();
    assert_eq!(manager.refers_to.as_deref(), Some("people"));
    assert!(catalogue.builtin.iter().any(|name| name == "upper"));
    assert!(catalogue.builtin.iter().any(|name| name == "stddev"));
    assert!(catalogue.functions.iter().all(|function| function.name != "stddev"));

    // Every relationship the catalogue names is one a query can follow.
    for collection in &catalogue.collections {
        for relationship in &collection.relationships {
            let query = format!("{}.select {{ n: .{}.count() }}", collection.name, relationship.name);
            assist::check(&executor, &query).unwrap_or_else(|problem| panic!("{problem:?}\n{query}"));
            executor.execute_query(&parse_oql(&query).unwrap()).unwrap();
        }
    }
}

#[test]
fn a_query_is_stopped_at_its_budget() {
    let (_directory, executor) = database();
    // Three people, each paired with each, and again: 27 rows.
    let pairs = parse_oql("people { a in people  b in people  select { id, a: a.id, b: b.id } }").unwrap();
    assert_eq!(executor.execute_query(&pairs).unwrap().len(), 27);

    let rows = Budget { rows: Some(5), time: None };
    match with_budget(rows, || executor.execute_query(&pairs)) {
        Err(OQLError::BudgetExceeded { message }) => {
            assert!(message.contains("over its budget of 5"), "{message}")
        }
        other => panic!("expected the budget to stop it: {other:?}"),
    }
    let problem = Problem::of_error(
        &with_budget(rows, || executor.execute_query(&pairs)).unwrap_err(),
        &assist::catalogue(&executor).unwrap(),
    );
    assert_eq!(problem.code, "over_budget");

    let time = Budget { rows: None, time: Some(Duration::ZERO) };
    assert!(matches!(
        with_budget(time, || executor.execute_query(&pairs)),
        Err(OQLError::BudgetExceeded { .. })
    ));

    // Within the budget, and after it: as if there were none.
    let roomy = Budget { rows: Some(100), time: Some(Duration::from_secs(60)) };
    assert_eq!(with_budget(roomy, || executor.execute_query(&pairs)).unwrap().len(), 27);
    assert_eq!(executor.execute_query(&pairs).unwrap().len(), 27);

    // A statement stopped part way wrote nothing.
    let write = parse_oql("people { a in people  b in people  update { level: 9 } }");
    if let Ok(write) = write {
        let _ = with_budget(rows, || executor.execute_query(&write));
        assert_eq!(count(&executor, "people.where { .level == 9 }"), 0);
    }
}
