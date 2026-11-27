//! Captures: `name in collection` in a scoped block. Where `name = value`
//! names a whole value, a capture names each member in turn, and the block
//! then has one row for every combination. Each case is small enough to
//! work out by hand, and the four at the top were also run as SQL, Cypher
//! and a MongoDB pipeline on the same rows, which gave these answers.

use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, QueryExecutor, Value};
use std::sync::Arc;
use tempfile::TempDir;

struct Db {
    _directory: TempDir,
    executor: QueryExecutor<RustStore>,
}

impl Db {
    fn try_rows(&self, query: &str) -> Result<Vec<owl_db::Document>, String> {
        let query = parse_oql(query).map_err(|error| error.to_string())?;
        self.executor.execute_query(&query).map_err(|error| error.to_string())
    }

    /// The named fields of every row, in result order, as text.
    fn table(&self, query: &str, fields: &[&str]) -> Vec<Vec<String>> {
        self.try_rows(query)
            .unwrap_or_else(|error| panic!("{error}\n{query}"))
            .iter()
            .map(|row| {
                fields
                    .iter()
                    .map(|field| match row.get_field(field) {
                        Some(Value::String(text)) => text.clone(),
                        Some(Value::Integer(number)) => number.to_string(),
                        Some(Value::Null) | None => "null".to_string(),
                        Some(other) => format!("{other:?}"),
                    })
                    .collect()
            })
            .collect()
    }

    fn error(&self, query: &str) -> String {
        match self.try_rows(query) {
            Ok(rows) => panic!("expected an error, got {} rows from\n{query}", rows.len()),
            Err(error) => error,
        }
    }
}

fn rows(expected: &[&[&str]]) -> Vec<Vec<String>> {
    expected
        .iter()
        .map(|row| row.iter().map(|cell| cell.to_string()).collect())
        .collect()
}

/// Five people under two managers, two projects, who works on which and
/// as what, four tasks, and which task blocks which: t1, t2 and t3 block
/// one another in a ring, and t3 also blocks t4.
fn work() -> Db {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
    let db = Db { _directory: directory, executor: QueryExecutor::new_with_real_clock(store) };
    let source = r#"
        schema people { id: String, name: String, manager: Optional(Ref(people)) as reports }
        schema projects { id: String, name: String, owner: Ref(people) as owned }
        schema assignments { id: String, person: Ref(people), project: Ref(projects), role: String, hours: Integer }
        schema tasks { id: String, project: Ref(projects), tags: List(String), notes: List({ kind: String, mins: Integer }) }
        schema links { id: String, blocker: Ref(tasks) as blocking, blocked: Ref(tasks) as blocked_by, kind: String }

        people.insert [
          { id: "kim", name: "Kim" }, { id: "lee", name: "Lee" },
          { id: "ada", name: "Ada", manager: "kim" }, { id: "ben", name: "Ben", manager: "kim" },
          { id: "cy", name: "Cy", manager: "lee" }
        ]
        projects.insert [
          { id: "apollo", name: "Apollo", owner: "kim" }, { id: "gemini", name: "Gemini", owner: "lee" }
        ]
        assignments.insert [
          { id: "a1", person: "ada", project: "apollo", role: "lead", hours: 20 },
          { id: "a2", person: "ada", project: "gemini", role: "dev", hours: 10 },
          { id: "a3", person: "ben", project: "apollo", role: "dev", hours: 30 },
          { id: "a4", person: "cy", project: "apollo", role: "dev", hours: 5 }
        ]
        tasks.insert [
          { id: "t1", project: "apollo", tags: ["api", "db"], notes: [{ kind: "call", mins: 5 }] },
          { id: "t2", project: "apollo", tags: [], notes: [] },
          { id: "t3", project: "gemini", tags: ["ui"], notes: [{ kind: "call", mins: 7 }, { kind: "mail", mins: 2 }] },
          { id: "t4", project: "gemini", tags: [], notes: [] }
        ]
        links.insert [
          { id: "l1", blocker: "t1", blocked: "t2", kind: "hard" },
          { id: "l2", blocker: "t2", blocked: "t3", kind: "hard" },
          { id: "l3", blocker: "t3", blocked: "t1", kind: "soft" },
          { id: "l4", blocker: "t3", blocked: "t4", kind: "hard" }
        ]
    "#;
    let file = parse_oql_file(source).unwrap();
    let report = execute_oql_file(&file, &db.executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    db
}

#[test]
fn three_tasks_that_block_one_another_in_a_ring() {
    let db = work();
    assert_eq!(
        db.table(
            "tasks {
               b in .blocked_by.blocker
               c in b.blocked_by.blocker
               where { c.blocked_by.blocker contains tasks.id }
               select { a: id, b: b.id, c: c.id }
             }",
            &["a", "b", "c"],
        ),
        rows(&[&["t1", "t3", "t2"], &["t2", "t1", "t3"], &["t3", "t2", "t1"]])
    );
}

#[test]
fn a_row_between_two_others_is_named_and_read() {
    let db = work();
    // The assignment between a person and a project holds the role.
    assert_eq!(
        db.table(
            "people {
               a in .assignments
               t in a.project.tasks
               where { a.role == \"dev\" }
               select { person: name, role: a.role, task: t.id }
             }",
            &["person", "role", "task"],
        ),
        rows(&[
            &["Ada", "dev", "t3"], &["Ada", "dev", "t4"],
            &["Ben", "dev", "t1"], &["Ben", "dev", "t2"],
            &["Cy", "dev", "t1"], &["Cy", "dev", "t2"],
        ])
    );
}

#[test]
fn two_rows_of_one_collection_are_compared() {
    let db = work();
    // Two people with the same manager on the same project, each pair once.
    assert_eq!(
        db.table(
            "people {
               j in .assignments.project
               other in j.assignments.person
               where { other.manager == manager && id < other.id }
               select { a: name, b: other.name, project: j.name }
             }",
            &["a", "b", "project"],
        ),
        rows(&[&["Ada", "Ben", "Apollo"]])
    );
}

#[test]
fn a_capture_takes_the_rows_of_a_walk() {
    let db = work();
    // A task, what blocks it directly, and everything behind that.
    assert_eq!(
        db.table(
            "tasks {
               where { id == \"t4\" }
               near in .blocked_by.blocker
               far in near.walk(.blocked_by.blocker)
               select { task: id, near: near.id, far: far.id, steps: far._depth }
             }",
            &["task", "near", "far", "steps"],
        ),
        rows(&[&["t4", "t3", "t2", "1"], &["t4", "t3", "t1", "2"], &["t4", "t3", "t3", "3"]])
    );
}

#[test]
fn a_block_after_a_capture_works_on_combinations() {
    let db = work();
    // A row with no member is not in the answer: t2 and t4 have no tags.
    assert_eq!(
        db.table("tasks { tag in .tags  select { id, tag } }", &["id", "tag"]),
        rows(&[&["t1", "api"], &["t1", "db"], &["t3", "ui"]])
    );
    // The objects of a list are captured as they are.
    assert_eq!(
        db.table(
            "tasks { n in .notes  where { n.mins > 2 }  select { id, kind: n.kind, mins: n.mins } }",
            &["id", "kind", "mins"],
        ),
        rows(&[&["t1", "call", "5"], &["t3", "call", "7"]])
    );
    // Counting, sorting, limiting and grouping are over the combinations.
    assert_eq!(db.table("tasks { n in .notes  count() }", &["count"]), rows(&[&["3"]]));
    assert_eq!(
        db.table(
            "people { a in .assignments  sort { a.hours.desc }  limit(2)  select { name, hours: a.hours } }",
            &["name", "hours"],
        ),
        rows(&[&["Ben", "30"], &["Ada", "20"]])
    );
    let mut groups = db.table(
        "people { a in .assignments  group_by { a.role }  select { role, people: count(), hours: sum(a.hours) }  }",
        &["role", "people", "hours"],
    );
    groups.sort();
    assert_eq!(groups, rows(&[&["dev", "3", "45"], &["lead", "1", "20"]]));
    // A filter may sit between two captures, and a name given with `=`
    // still names a whole value beside them.
    assert_eq!(
        db.table(
            "people {
               mine = .assignments.count()
               a in .assignments
               where { a.role == \"lead\" }
               t in a.project.tasks
               select { name, mine, task: t.id }
             }",
            &["name", "mine", "task"],
        ),
        rows(&[&["Ada", "2", "t1"], &["Ada", "2", "t2"]])
    );
    // Two captures may land on the same row; nothing is left out unasked.
    assert_eq!(
        db.table(
            "people { where { id == \"ada\" }  x in .assignments  y in .assignments  count() }",
            &["count"],
        ),
        rows(&[&["4"]])
    );
    // A whole collection can be captured: every pair, here narrowed.
    assert_eq!(
        db.table(
            "projects { p in people  where { p.id == owner }  select { name, owner: p.name } }",
            &["name", "owner"],
        ),
        rows(&[&["Apollo", "Kim"], &["Gemini", "Lee"]])
    );
    // A single reference is one member, or none.
    assert_eq!(
        db.table("people { m in .manager  select { name, boss: m.name } }", &["name", "boss"]),
        rows(&[&["Ada", "Kim"], &["Ben", "Kim"], &["Cy", "Lee"]])
    );
    // Inside a nested query the name is still the row captured for the
    // block's row: the tasks with a blocker that blocks them softly.
    assert_eq!(
        db.table(
            "tasks {
               b in .blocked_by.blocker
               where { links.where { .blocker == b.id && .blocked == tasks.id && .kind == \"soft\" }.exists() }
               select { id, b: b.id }
             }",
            &["id", "b"],
        ),
        rows(&[&["t1", "t3"]])
    );
    assert_eq!(
        db.table(
            "tasks {
               b in .blocked_by.blocker
               select { id, b: b.id, same: tasks.where { .project == b.project }.count() }
             }",
            &["id", "b", "same"],
        ),
        rows(&[&["t1", "t3", "2"], &["t2", "t1", "2"], &["t3", "t2", "2"], &["t4", "t3", "2"]])
    );
    // Two captured rows are compared as rows: by which row each is.
    assert_eq!(
        db.table(
            "people { where { id == \"ada\" }  x in .assignments  y in .assignments  where { x != y }  count() }",
            &["count"],
        ),
        rows(&[&["2"]])
    );
    // A path with operations, and a query on a collection, are collections.
    assert_eq!(
        db.table(
            "people { a in .assignments.where { .hours > 10 }  select { name, hours: a.hours } }",
            &["name", "hours"],
        ),
        rows(&[&["Ada", "20"], &["Ben", "30"]])
    );
    assert_eq!(
        db.table(
            "projects { p in people.where { .manager == projects.owner }.sort { .name.asc }  select { name, who: p.name } }",
            &["name", "who"],
        ),
        rows(&[&["Apollo", "Ada"], &["Apollo", "Ben"], &["Gemini", "Cy"]])
    );
    // The way a walk took to a row is read of the captured row.
    assert_eq!(
        db.table(
            "tasks { where { id == \"t4\" }  far in .walk(.blocked_by.blocker)  select { far: far.id, rows: far._path.count() } }",
            &["far", "rows"],
        ),
        rows(&[&["t3", "3"], &["t2", "5"], &["t1", "7"]])
    );
    // Rows that are not reshaped do not show what was captured for them.
    let first = db.try_rows("tasks { tag in .tags  first() }").unwrap();
    assert!(first[0].fields.keys().all(|name| !name.starts_with('$')), "{:?}", first[0].fields);
}

#[test]
fn what_a_capture_cannot_mean_is_refused() {
    let db = work();
    let error = db.error("tasks { b in .blocked_by.blocker  update { tags: [] } }");
    assert!(error.contains("cannot follow a capture"), "{error}");
    let error = db.error("tasks { b in .blocked_by.blocker  delete() }");
    assert!(error.contains("cannot follow a capture"), "{error}");
    let error = db.error("tasks { tasks in .blocked_by.blocker  count() }");
    assert!(error.contains("the row the block starts from"), "{error}");
    let error = db.error("tasks { b in .blocked_by.blocker  b in .blocking.blocked  count() }");
    assert!(error.contains("already defined"), "{error}");
    let error = db.error("tasks { where { b.id == \"t1\" }  b in .blocked_by.blocker  count() }");
    assert!(error.contains("before it is defined"), "{error}");
    // A name the captured row's collection does not have is a slip, not
    // an absent value, here and inside a nested query.
    let error = db.error("tasks { b in .blocked_by.blocker  where { b.nope == 1 }  count() }");
    assert!(error.contains("'b' is a row of 'tasks', which has no field 'nope'"), "{error}");
    let error = db.error(
        "tasks { b in .blocked_by.blocker  select { n: links.where { .kind == b.nope }.count() } }",
    );
    assert!(error.contains("has no field 'nope'"), "{error}");
    let error = db.error("tasks { b in .nope  count() }");
    assert!(error.contains("has no field 'nope'"), "{error}");
    // A capture is an entry of a block and nothing else.
    assert!(db.try_rows("tasks { b in .blocked_by.blocker }").is_err());
}

#[test]
fn a_name_given_with_equals_means_the_blocks_row_inside_a_nested_query() {
    let db = work();
    // A list of values, asked about in a query on another collection.
    assert_eq!(
        db.table(
            "people { mine = .assignments.project.id  select { id, n: projects.where { .id in mine }.count() } }",
            &["id", "n"],
        ),
        rows(&[&["ada", "2"], &["ben", "1"], &["cy", "1"], &["kim", "0"], &["lee", "0"]])
    );
    // One value, in a query on the block's own collection.
    assert_eq!(
        db.table(
            "people { boss = .manager  select { id, peers: people.where { .manager == boss && .id != people.id }.count() } }",
            &["id", "peers"],
        ),
        rows(&[&["ada", "1"], &["ben", "1"], &["cy", "0"], &["kim", "1"], &["lee", "1"]])
    );
    // In the operations of a path from the row.
    assert_eq!(
        db.table(
            "people { mine = .assignments.count()  select { id, big: .assignments.where { .hours * mine > 25 }.count() } }",
            &["id", "big"],
        ),
        rows(&[&["ada", "1"], &["ben", "1"], &["cy", "0"], &["kim", "0"], &["lee", "0"]])
    );
    // A name for a collection with operations of its own.
    assert_eq!(
        db.table(
            "people {
               devs = .assignments.where { .role == \"dev\" }
               select { id, n: projects.where { .id in devs.project.id }.count() }
             }",
            &["id", "n"],
        ),
        rows(&[&["ada", "1"], &["ben", "1"], &["cy", "1"], &["kim", "0"], &["lee", "0"]])
    );
    // And in a filter.
    assert_eq!(
        db.table(
            "people { mine = .assignments.project.id  where { projects.where { .id in mine }.count() == 2 }  select { id } }",
            &["id"],
        ),
        rows(&[&["ada"]])
    );
}

#[test]
fn an_optional_capture_keeps_a_row_with_no_member() {
    let db = work();
    // `name? in`: a row with no member stays, once, with the name empty.
    assert_eq!(
        db.table("tasks { tag? in .tags  select { id, tag } }", &["id", "tag"]),
        rows(&[&["t1", "api"], &["t1", "db"], &["t2", "null"], &["t3", "ui"], &["t4", "null"]])
    );
    // Every path from an empty name is absent.
    assert_eq!(
        db.table("people { m? in .manager  select { name, boss: m.name } }", &["name", "boss"]),
        rows(&[&["Ada", "Kim"], &["Ben", "Kim"], &["Cy", "Lee"], &["Kim", "null"], &["Lee", "null"]])
    );
    // The rows that have none are the ones whose name is null.
    assert_eq!(
        db.table("people { a? in .assignments  where { a == null }  select { name } }", &["name"]),
        rows(&[&["Kim"], &["Lee"]])
    );
    assert_eq!(
        db.table("people { a? in .assignments  where { a != null && a.hours > 10 }  select { name } }", &["name"]),
        rows(&[&["Ada"], &["Ben"]])
    );
    // A capture from an empty name has no members, and may be optional too.
    assert_eq!(
        db.table("people { a? in .assignments  t? in a.project.tasks  count() }", &["count"]),
        rows(&[&["10"]])
    );
    assert_eq!(
        db.table("people { a? in .assignments  t in a.project.tasks  count() }", &["count"]),
        rows(&[&["8"]])
    );
    // Counting what was captured, with the rows that have none at nought.
    let mut hours = db.table(
        "people { a? in .assignments  group_by { name }  select { name, hours: sum(a.hours) } }",
        &["name", "hours"],
    );
    hours.sort();
    assert_eq!(
        hours,
        rows(&[&["Ada", "30"], &["Ben", "30"], &["Cy", "5"], &["Kim", "0"], &["Lee", "0"]])
    );
    // A misspelt field is still refused, though the first row has no member.
    let error = db.error("people { a? in .assignments  where { a.nope == 1 }  count() }");
    assert!(error.contains("has no field 'nope'"), "{error}");
    // The mark belongs to a capture.
    assert!(db.try_rows("people { a? = .assignments  count() }").is_err());
}
