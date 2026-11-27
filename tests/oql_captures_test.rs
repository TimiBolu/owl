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
    // A value written as a bare field name, with no dot.
    assert_eq!(
        db.table(
            "people { boss = manager  select { id, peers: people.where { .manager == boss && .id != people.id }.count() } }",
            &["id", "peers"],
        ),
        rows(&[&["ada", "1"], &["ben", "1"], &["cy", "0"], &["kim", "1"], &["lee", "1"]])
    );
    assert_eq!(
        db.table(
            "assignments { double = hours * 2  select { id, over: assignments.where { .hours > double }.count() } }",
            &["id", "over"],
        ),
        rows(&[&["a1", "0"], &["a2", "1"], &["a3", "0"], &["a4", "2"]])
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

#[test]
fn explain_shows_each_capture_with_its_rows() {
    let db = work();
    let explained = db
        .try_rows(
            "tasks {
               where { id != \"t4\" }
               b in .blocked_by.blocker
               c? in b.blocked_by.blocker
               far in c.walk(.blocked_by.blocker)
               where { far.id == tasks.id }
               select { a: id, b: b.id, c: c.id }
               explain()
             }",
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let row = &explained[0];
    assert_eq!(row.get_field("path"), Some(&Value::String("general executor".to_string())));
    // One step for the rows the block starts from, then one for every
    // entry from the first capture on, each with its rows in and out.
    let Some(Value::List(steps)) = row.get_field("steps") else {
        panic!("no steps in {:?}", row.fields);
    };
    let shown: Vec<(String, String, String)> = steps
        .iter()
        .map(|step| {
            let Value::Object(step) = step else { panic!("a step is an object") };
            let cell = |name: &str| match step.get(name) {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Integer(number)) => number.to_string(),
                _ => "-".to_string(),
            };
            assert!(matches!(step.get("ms"), Some(Value::Float(_))));
            (cell("step"), cell("rows_in"), cell("rows_out"))
        })
        .collect();
    let expected = [
        ("tasks, 1 entry", "-", "3"),
        ("b in .blocked_by.blocker", "3", "3"),
        ("c? in b.blocked_by.blocker", "3", "3"),
        ("far in c.walk(.blocked_by.blocker).where { .. }", "3", "3"),
        ("select { .. }", "3", "3"),
    ];
    assert_eq!(
        shown,
        expected.map(|(step, rows_in, rows_out)| (step.to_string(), rows_in.to_string(), rows_out.to_string()))
    );
    // The same, as lines of the plan.
    let Some(Value::String(plan)) = row.get_field("plan") else { panic!("no plan") };
    assert!(plan.contains("far in c.walk(.blocked_by.blocker).where { .. }: 3 rows in, 3 out"), "{plan}");
    assert!(plan.contains("tasks, 1 entry: 3 rows"), "{plan}");

    // A block that starts at its first capture, and one that only counts.
    let counted = db.try_rows("tasks { tag in .tags  count()  explain() }").unwrap();
    let Some(Value::String(plan)) = counted[0].get_field("plan") else { panic!("no plan") };
    assert!(plan.contains("tasks: 4 rows"), "{plan}");
    assert!(plan.contains("tag in .tags: 4 rows in, 3 out"), "{plan}");
    assert!(plan.contains("count(): 3 rows in, 1 out"), "{plan}");
    // With no other result entry, `explain()` is the one.
    let inside = db.try_rows("tasks { tag in .tags  explain() }").unwrap();
    assert!(matches!(inside[0].get_field("steps"), Some(Value::List(steps)) if steps.len() == 2));
    // Nothing follows it, and it does not explain itself.
    assert!(db.try_rows("tasks { tag in .tags  explain()  count() }").is_err());
    assert!(db.try_rows("tasks { tag in .tags  explain()  explain() }").is_err());
}

/// The steps `explain()` gives for a block, as text.
fn steps(db: &Db, block: &str) -> Vec<String> {
    let explained = db.try_rows(block).unwrap_or_else(|error| panic!("{error}\n{block}"));
    let Some(Value::List(steps)) = explained[0].get_field("steps") else {
        panic!("no steps for {block}");
    };
    steps
        .iter()
        .map(|step| {
            let Value::Object(step) = step else { panic!("a step is an object") };
            let cell = |name: &str| match step.get(name) {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Integer(number)) => number.to_string(),
                _ => "-".to_string(),
            };
            format!("{} | {} | {}", cell("step"), cell("rows_in"), cell("rows_out"))
        })
        .collect()
}

#[test]
fn a_condition_runs_where_its_names_first_exist() {
    let db = work();
    // Written last, the three conditions are about three different things.
    // One reads only the task, so it joins the read of the tasks. One reads
    // `b` and one reads `c`: each filters the collection its row is
    // captured from. And since the one on `b` leaves few tasks to be `b`,
    // the block starts from those and comes back to the tasks they block.
    assert_eq!(
        steps(
            &db,
            "tasks {
               b in .blocked_by.blocker
               c in b.blocked_by.blocker
               where { c.id != \"t9\" && b.project.id == \"gemini\" && id != \"t2\" }
               select { a: id, b: b.id, c: c.id }
               explain()
             }",
        ),
        [
            "tasks, 1 entry, reached back from b | - | 2",
            "b in .blocked_by.blocker.where { .. } | 2 | 2",
            "c in b.blocked_by.blocker.where { .. } | 2 | 2",
            "select { .. } | 2 | 2",
        ]
    );
    // The tasks one named task blocks: that task is read, and its links
    // followed back, not every task.
    assert_eq!(
        steps(&db, "tasks { b in .blocked_by.blocker  where { b.id == \"t3\" }  select { id }  explain() }"),
        [
            "tasks, reached back from b | - | 2",
            "b in .blocked_by.blocker.where { .. } | 2 | 2",
            "select { .. } | 2 | 2",
        ]
    );
    // Back through a relationship reached by its collection's name.
    assert_eq!(
        steps(&db, "people { j in .assignments.project  where { j.name == \"Gemini\" }  select { name }  explain() }"),
        [
            "people, reached back from j | - | 1",
            "j in .assignments.project.where { .. } | 1 | 1",
            "select { .. } | 1 | 1",
        ]
    );
    // A condition that pins a later capture chooses the start too: the
    // block comes back to its rows through every capture on the way.
    assert_eq!(
        steps(
            &db,
            "people { a in .assignments  j in a.project  o in j.owner  where { o.name == \"Lee\" }  select { name, j: j.name }  explain() }",
        ),
        [
            "people, reached back from o | - | 1",
            "a in .assignments | 1 | 2",
            "j in a.project | 2 | 2",
            "o in j.owner.where { .. } | 2 | 1",
            "select { .. } | 1 | 1",
        ]
    );
    // Not through an optional capture, whose rows with no member stay.
    assert_eq!(
        steps(&db, "people { a? in .assignments  t in a.project.tasks  where { t.id == \"t3\" }  count()  explain() }"),
        ["people | - | 5", "a? in .assignments | 5 | 6", "t in a.project.tasks.where { .. } | 6 | 1", "count() | 1 | 1"]
    );
    // The plan then says what is read first, which is not the block's own
    // collection.
    let explained = db
        .try_rows("people { a in .assignments  t in a.project.tasks  where { t.id == \"t3\" }  count()  explain() }")
        .unwrap();
    let Some(Value::String(plan)) = explained[0].get_field("plan") else { panic!("no plan") };
    assert!(
        plan.starts_with("read the rows of tasks that t may be, and follow the block's path back from them to people"),
        "{plan}"
    );
    assert!(!plan.contains("scan people"), "{plan}");
    // The start is chosen by counting. Three of the four assignments pass
    // this condition, so starting from them would leave about three of the
    // five people: a scan is as good, and the block starts as written.
    assert_eq!(
        steps(&db, "people { a in .assignments  where { a.hours > 5 && a.id != id }  count()  explain() }"),
        ["people | - | 5", "a in .assignments.where { .. }.where { .. } | 5 | 3", "count() | 3 | 1"]
    );
    // One assignment passes this one, which leaves about one person.
    assert_eq!(
        steps(&db, "people { a in .assignments  where { a.hours > 25 && a.id != id }  count()  explain() }"),
        ["people, reached back from a | - | 1", "a in .assignments.where { .. }.where { .. } | 1 | 1", "count() | 1 | 1"]
    );
    // What was expected is said, beside what was found.
    let explained = db
        .try_rows("tasks { b in .blocked_by.blocker  where { b.id == \"t3\" }  select { id }  explain() }")
        .unwrap();
    let Some(Value::String(plan)) = explained[0].get_field("plan") else { panic!("no plan") };
    assert!(plan.contains("about 1 of its 4 rows were expected"), "{plan}");
    // Of two captures a condition pins, the one expected to leave fewer
    // starting rows is where the block starts: one assignment leads back
    // to one person, where one task leads back to the two on its project.
    assert_eq!(
        steps(
            &db,
            "people { a in .assignments  t in a.project.tasks  where { a.hours > 25 && t.id == \"t1\" }  count()  explain() }",
        )[0],
        "people, reached back from a | - | 1"
    );
    // A condition that reads the block's row as well cannot.
    assert_eq!(
        steps(&db, "people { a in .assignments  where { a.hours > 5 || a.id == id }  count()  explain() }"),
        ["people | - | 5", "a in .assignments.where { .. } | 5 | 3", "count() | 3 | 1"]
    );
    // The members of a list are not rows of a collection, so a condition
    // on one is asked after it is captured.
    assert_eq!(
        steps(&db, "tasks { n in .notes  where { n.mins > 2 }  count()  explain() }"),
        ["tasks | - | 4", "n in .notes | 4 | 3", "where { .. } | 3 | 2", "count() | 2 | 1"]
    );
    // A condition on a row of a whole collection filters the collection:
    // one person is read for each project, not every person.
    assert_eq!(
        steps(&db, "projects { p in people  where { p.id == owner }  select { name, owner: p.name }  explain() }"),
        ["projects | - | 2", "p in people.where { .. } | 2 | 2", "select { .. } | 2 | 2"]
    );
    // Not when the capture is optional: its rows with no match must stay.
    assert_eq!(
        steps(&db, "projects { p? in people  where { p.id == owner }  count()  explain() }"),
        ["projects | - | 2", "p? in people | 2 | 10", "where { .. } | 10 | 2", "count() | 2 | 1"]
    );
}

#[test]
fn planning_a_block_never_changes_its_answer() {
    let db = work();
    let blocks = [
        "tasks { b in .blocked_by.blocker  c in b.blocked_by.blocker  where { c.blocked_by.blocker contains tasks.id }  select { a: id, b: b.id, c: c.id } }",
        "tasks { b in .blocked_by.blocker  c in b.blocked_by.blocker  where { c.id != \"t9\" && b.project.id == \"gemini\" && id != \"t2\" }  select { a: id, b: b.id, c: c.id } }",
        "people { a in .assignments  t in a.project.tasks  where { a.role == \"dev\" }  select { person: name, role: a.role, task: t.id } }",
        "people { j in .assignments.project  other in j.assignments.person  where { other.manager == manager && id < other.id }  select { a: name, b: other.name, project: j.name } }",
        "tasks { where { id == \"t4\" }  near in .blocked_by.blocker  far in near.walk(.blocked_by.blocker)  where { far._depth < 3 }  select { task: id, near: near.id, far: far.id, steps: far._depth } }",
        "projects { p in people  where { p.id == owner }  select { name, owner: p.name } }",
        "projects { p in people  where { p.manager == owner && p.name != name }  select { name, who: p.name } }",
        "projects { p in people  q in people  where { p.manager == q.id && q.id == owner }  select { name, p: p.name, q: q.name } }",
        "projects { p in people.where { .manager != null }  where { p.manager == projects.owner || p.id == \"cy\" }  select { name, who: p.name } }",
        "projects { p? in people  where { p.id == owner }  select { name, owner: p.name } }",
        "people { a? in .assignments  where { a == null }  select { name } }",
        "people { x in .assignments  y in .assignments  where { x != y && y.hours > 5 }  select { name, x: x.id, y: y.id } }",
        "people { a in .assignments  where { a.hours > 5 }  t in tasks  where { t.project == a.project && name != \"Cy\" }  select { name, a: a.id, t: t.id } }",
        "tasks { b in .blocked_by.blocker  where { links.where { .blocker == b.id && .blocked == tasks.id && .kind == \"soft\" }.exists() }  select { id, b: b.id } }",
        "people { a in .assignments  sort { a.hours.desc }  limit(2)  select { name, hours: a.hours } }",
        // A condition that pins the captured end: the block starts there.
        "tasks { b in .blocked_by.blocker  where { b.id == \"t3\" }  select { id, b: b.id } }",
        "tasks { where { id != \"t1\" }  b in .blocked_by.blocker  where { b.id == \"t3\" && tasks.id != \"t9\" }  select { id, b: b.id } }",
        "people { a in .assignments  where { a.role == \"lead\" }  select { name, a: a.id } }",
        "people { a in .assignments  where { a.role == \"dev\" }  t in a.project.tasks  select { name, t: t.id } }",
        "people { j in .assignments.project  where { j.name == \"Gemini\" }  select { name, j: j.id } }",
        "people { m in .manager  where { m.name == \"Kim\" }  select { name } }",
        "projects { o in .owner.reports  where { o.name == \"Ada\" }  select { name, o: o.id } }",
        "tasks { b in .blocked_by.blocker  where { b.id == \"nobody\" }  select { id } }",
        // A condition that pins a later capture: the block starts from that one.
        "tasks { b in .blocked_by.blocker  c in b.blocked_by.blocker  where { c.id == \"t1\" }  select { a: id, b: b.id, c: c.id } }",
        "people { a in .assignments  t in a.project.tasks  where { t.id == \"t3\" && a.hours > 1 }  select { name, a: a.id, t: t.id } }",
        "people { a in .assignments  j in a.project  o in j.owner  where { o.name == \"Lee\" }  select { name, j: j.name } }",
        "people { x in .assignments  a in .assignments  t in a.project.tasks  where { t.id == \"t3\" }  select { name, x: x.id, a: a.id } }",
        "people { a? in .assignments  t in a.project.tasks  where { t.id == \"t3\" }  select { name, t: t.id } }",
        "people { a in .assignments  where { a.hours > 5 }  t in a.project.tasks  where { t.id == \"t1\" }  select { name, a: a.id } }",
        // Conditions on members of a list and of a walk.
        "tasks { n in .notes  where { n.mins > 2 }  select { id, kind: n.kind } }",
        "tasks { where { id == \"t4\" }  far in .walk(.blocked_by.blocker)  where { far._depth < 3 && far.id != \"t9\" }  select { far: far.id, steps: far._depth } }",
        "people { boss = manager  a in .assignments  where { a.hours > 5 }  select { name, peers: people.where { .manager == boss && .id != people.id }.count() } }",
    ];
    for block in blocks {
        let shown = |rows: Vec<owl_db::Document>| rows.iter().map(|row| format!("{:?}", row.fields)).collect::<Vec<_>>();
        let planned = shown(db.try_rows(block).unwrap_or_else(|error| panic!("{error}\n{block}")));
        owl_db::executor::set_capture_planning(false);
        let written = db.try_rows(block);
        owl_db::executor::set_capture_planning(true);
        let written = shown(written.unwrap_or_else(|error| panic!("{error}\n{block}")));
        assert_eq!(planned, written, "{block}");
        assert!(
            !planned.is_empty() || block.contains("a == null") || block.contains("nobody"),
            "nothing to compare for {block}"
        );
    }
}

#[test]
fn a_name_means_the_same_row_however_deep_it_is_read() {
    let db = work();
    // Two queries down, the collection's name is still the row the outer
    // query is on: the projects are counted for an assignment that some
    // other assignment has more hours than. Read when the innermost query
    // ran, the name was every assignment's hours, and nothing matched.
    assert_eq!(
        db.table(
            "assignments.select { id, k: projects.where { assignments.where { .hours > assignments.hours }.count() > 0 }.count() }",
            &["id", "k"],
        ),
        rows(&[&["a1", "2"], &["a2", "2"], &["a3", "0"], &["a4", "2"]])
    );
    // The same through a name given with `=`: there are five people.
    assert_eq!(
        db.table(
            "assignments { h = .hours  select { id, k: projects.where { people.where { .id != \"x\" }.count() < h }.count() } }",
            &["id", "k"],
        ),
        rows(&[&["a1", "2"], &["a2", "2"], &["a3", "2"], &["a4", "0"]])
    );
    // And when the middle query is a path from the row: a person's
    // assignments, counted if their manager owns a project.
    assert_eq!(
        db.table(
            "people.select { id, k: .assignments.where { projects.where { .owner == people.manager }.count() > 0 }.count() }",
            &["id", "k"],
        ),
        rows(&[&["ada", "2"], &["ben", "1"], &["cy", "1"], &["kim", "0"], &["lee", "0"]])
    );
    // In a query on the block's own collection the name is the outer row,
    // and in a query inside that one it is the nearer row: the people
    // after this one who own a project.
    assert_eq!(
        db.table(
            "people.select { id, k: people.where { .id > people.id && projects.where { .owner == people.id }.count() > 0 }.count() }",
            &["id", "k"],
        ),
        rows(&[&["ada", "2"], &["ben", "2"], &["cy", "2"], &["kim", "1"], &["lee", "0"]])
    );
}

#[test]
fn a_blocks_answer_is_carried_on_by_a_chain() {
    let db = work();
    // A block gives rows, and a chain after its closing brace works on
    // them, as a chain after `select` does. This is what lets a block
    // group, shape, and then sort by what it worked out.
    assert_eq!(
        db.table(
            "people { a in .assignments  group_by { a.role }  select { role, people: count() } }.sort { .people.desc }.limit(1)",
            &["role", "people"],
        ),
        rows(&[&["dev", "3"]])
    );
    assert_eq!(
        db.table(
            "people { select { id, n: .assignments.count() } }.sort { .n.desc, .id.asc }.limit(2)",
            &["id", "n"],
        ),
        rows(&[&["ada", "2"], &["ben", "1"]])
    );
    assert_eq!(
        db.table("people { select { id, n: .assignments.count() } }.where { .n > 1 }", &["id", "n"]),
        rows(&[&["ada", "2"]])
    );
    assert_eq!(
        db.table("people { a in .assignments  select { name, hours: a.hours } }.where { .hours > 10 }.count()", &["count"]),
        rows(&[&["2"]])
    );
    // A block that ends in one value has that to give.
    assert_eq!(db.table("tasks { tag in .tags  count() }.explain()", &["path"]), rows(&[&["general executor"]]));

    // The block's names ended with the block.
    let error = db.error("people { mine = .assignments  select { id } }.where { mine.count() > 0 }");
    assert!(error.contains("'mine' is a name of the block, and the block has closed"), "{error}");
    let error = db.error("people { a in .assignments  select { id } }.where { a.hours > 1 }");
    assert!(error.contains("'a' is a name of the block"), "{error}");
    // And a write still cannot follow a capture.
    let error = db.error("tasks { b in .blocked_by.blocker  select { id } }.delete()");
    assert!(error.contains("a write cannot follow a block with a capture"), "{error}");
}

#[test]
fn explain_shows_what_a_walk_reached_at_each_depth() {
    let db = work();
    // t4 is blocked by t3, t3 by t2, t2 by t1, and t1 by t3 again.
    let query = "tasks.where { .id == \"t4\" }.walk(.blocked_by.blocker).select { id }.explain()";
    assert_eq!(
        steps(&db, query),
        ["tasks, 1 entry | - | 1", "walk(.blocked_by.blocker) | 1 | 3", "select { .. } | 3 | 3"]
    );
    let explained = db.try_rows(query).unwrap();
    let Some(Value::String(plan)) = explained[0].get_field("plan") else { panic!("no plan") };
    assert!(
        plan.contains("walk(.blocked_by.blocker): 1 rows in, 3 out (depth 1: 1, depth 2: 1, depth 3: 1)"),
        "{plan}"
    );
    // From every task, with the row each was reached from named: one walk
    // for each starting row.
    assert_eq!(
        steps(&db, "tasks.walk(.blocked_by.blocker).select { from: tasks.id, id }.explain()"),
        ["tasks | - | 4", "walk(.blocked_by.blocker) | 4 | 12", "select { .. } | 12 | 12"]
    );
    // A walk that writes afterwards is described, not run.
    let before = db.table("tasks.count()", &["count"]);
    let described = db.try_rows("tasks.where { .id == \"t4\" }.walk(.blocked_by.blocker).delete().explain()").unwrap();
    assert!(described[0].get_field("steps").is_none());
    assert_eq!(db.table("tasks.count()", &["count"]), before);
}
