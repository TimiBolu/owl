//! Can a program learn OQL from one page?
//!
//! Questions in plain English, each with the answer SQLite gives to a
//! reference query. A language model is handed the language card, the
//! catalogue of this database and the questions, and nothing else; what
//! it writes is checked, run on Owl, and compared with SQLite's answer.
//!
//! Two numbers come out: how many answers are right at the first try, and
//! how many after the model has been shown, once, what the database said
//! about the ones it refused. A wrong answer earns no hint, because in use
//! nobody is there to give one.
//!
//! The harness never calls a model. It writes a prompt and scores a file
//! of answers, so any model can sit between the two:
//!
//! ```text
//! gauntlet ask                               # are the questions sound?
//! gauntlet ask --prompt prompt.md            # what the model is given
//! gauntlet ask --answers first.json --feedback again.md
//! gauntlet ask --answers second.json
//! ```
//!
//! An answers file is a JSON object from a question's id to OQL.

use crate::Loaded;
use anyhow::{Context, Result};
use owl_db::assist;
use owl_db::storage::RustStore;
use owl_db::QueryExecutor;
use serde_json::Value as Json;
use std::path::Path;
use storefront::engines::first_difference;

struct Question {
    id: &'static str,
    /// What is asked, with the names the answer's fields must have.
    text: &'static str,
    /// Whether the order of the rows is part of the answer.
    ordered: bool,
    /// Where the answer key comes from.
    key: Key,
    /// One way to say it in OQL. Never shown; it proves the question can
    /// be answered and that the key is right.
    oql: &'static str,
}

/// Where a question's answer key comes from.
enum Key {
    /// A reference query, on SQLite.
    Sql(&'static str),
    /// Worked out here from the notes, in plain arithmetic: SQLite has no
    /// vectors, and the key must not come from the engine being asked.
    Notes(fn(&[Note], &[String]) -> Vec<Json>),
}

/// A note: a row with a vector, kept in Owl alone.
struct Note {
    id: String,
    person: String,
    topic: &'static str,
    about: [f32; 4],
}

const TOPICS: [&str; 3] = ["plan", "idea", "risk"];

/// Two notes for each person, the same every run.
fn notes(people: &[String]) -> Vec<Note> {
    let unit = |seed: u64| {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        // Three decimal places, between 0 and 1.
        (((z ^ (z >> 31)) >> 40) % 1001) as f32 / 1000.0
    };
    let mut made = Vec::with_capacity(people.len() * 2);
    for (at, person) in people.iter().enumerate() {
        for which in 0..2u64 {
            let seed = (at as u64 * 2 + which) * 4;
            made.push(Note {
                id: format!("n{:05}", at * 2 + which as usize),
                person: person.clone(),
                topic: TOPICS[(at + which as usize * 2) % TOPICS.len()],
                about: [unit(seed), unit(seed + 1), unit(seed + 2), unit(seed + 3)],
            });
        }
    }
    made
}

fn dot(a: &[f32; 4], b: &[f64; 4]) -> f64 {
    a.iter().zip(b).map(|(a, b)| f64::from(*a) * b).sum()
}

fn cosine(a: &[f32; 4], b: &[f64; 4]) -> f64 {
    let lengths = dot(a, &a.map(f64::from)).sqrt() * b.iter().map(|b| b * b).sum::<f64>().sqrt();
    if lengths == 0.0 {
        0.0
    } else {
        dot(a, b) / lengths
    }
}

fn apart(a: &[f32; 4], b: &[f64; 4]) -> f64 {
    a.iter().zip(b).map(|(a, b)| (f64::from(*a) - b).powi(2)).sum::<f64>().sqrt()
}

/// The ids of the `k` notes nearest `to` by direction, nearest first.
/// The key is only a key if no two of them are so close that the 32-bit
/// arithmetic of the engine could order them the other way.
fn nearest_notes<'a>(notes: impl Iterator<Item = &'a Note>, to: &[f64; 4], k: usize) -> Vec<&'a Note> {
    let mut scored: Vec<(f64, &Note)> = notes.map(|note| (cosine(&note.about, to), note)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    for pair in scored.windows(2).take(k) {
        assert!(pair[0].0 - pair[1].0 > 1e-5, "two notes are too nearly as near as each other for a key");
    }
    scored.into_iter().take(k).map(|(_, note)| note).collect()
}

/// A threshold is only a key if no note sits on it.
fn clear_of(value: f64, threshold: f64) -> bool {
    assert!((value - threshold).abs() > 1e-5, "a note is too near a threshold for a key");
    value > threshold
}

fn ids(notes: Vec<&Note>) -> Vec<Json> {
    notes.into_iter().map(|note| serde_json::json!({ "id": note.id })).collect()
}

const QUESTIONS: &[Question] = &[
    Question {
        id: "q01",
        text: "The id and name of every person at level 5 or above. Fields: id, name.",
        ordered: false,
        key: Key::Sql("SELECT id, name FROM people WHERE level >= 5"),
        oql: "people.where { .level >= 5 }.select { id, name }",
    },
    Question {
        id: "q02",
        text: "How many tasks are in state \"open\"? One row with the field n.",
        ordered: false,
        key: Key::Sql("SELECT COUNT(*) AS n FROM tasks WHERE state = 'open'"),
        oql: "tasks.where { .state == \"open\" }.select { n: count() }",
    },
    Question {
        id: "q03",
        text: "For every project in state \"paused\": its id, and the name of its owner. Fields: id, owner.",
        ordered: false,
        key: Key::Sql("SELECT p.id, o.name AS owner FROM projects p JOIN people o ON o.id = p.owner WHERE p.state = 'paused'"),
        oql: "projects.where { .state == \"paused\" }.select { id, owner: .owner.name }",
    },
    Question {
        id: "q04",
        text: "The id of every task in state \"review\" whose project's owner belongs to an org on the \"enterprise\" plan. Field: id.",
        ordered: false,
        key: Key::Sql("SELECT t.id FROM tasks t JOIN projects p ON p.id = t.project JOIN people o ON o.id = p.owner \
              JOIN orgs g ON g.id = o.org WHERE g.plan = 'enterprise' AND t.state = 'review'"),
        oql: "tasks.where { .state == \"review\" && .project.owner.org.plan == \"enterprise\" }.select { id }",
    },
    Question {
        id: "q05",
        text: "For every person at level 5 or above: their id, and how many people report to them directly. Fields: id, reports.",
        ordered: false,
        key: Key::Sql("SELECT p.id, (SELECT COUNT(*) FROM people r WHERE r.manager = p.id) AS reports FROM people p WHERE p.level >= 5"),
        oql: "people.where { .level >= 5 }.select { id, reports: .reports.count() }",
    },
    Question {
        id: "q06",
        text: "For each assignment role: the role, the number of assignments, and their total hours. Fields: role, n, hours.",
        ordered: false,
        key: Key::Sql("SELECT role, COUNT(*) AS n, SUM(hours) AS hours FROM assignments GROUP BY role"),
        oql: "assignments.group_by { .role }.select { role, n: count(), hours: sum(.hours) }",
    },
    Question {
        id: "q07",
        text: "For each unit that has at least 10 people: the unit's id and its headcount. Fields: unit, n.",
        ordered: false,
        key: Key::Sql("SELECT unit, COUNT(*) AS n FROM people GROUP BY unit HAVING COUNT(*) >= 10"),
        oql: "people.group_by { unit: .unit.id }.having { count() >= 10 }.select { unit, n: count() }",
    },
    Question {
        id: "q08",
        text: "For each project state: the state, and how many different people own a project in that state. Fields: state, owners.",
        ordered: false,
        key: Key::Sql("SELECT state, COUNT(DISTINCT owner) AS owners FROM projects GROUP BY state"),
        oql: "projects.group_by { .state }.select { state, owners: .owner.distinct().count() }",
    },
    Question {
        id: "q09",
        text: "The id of every person who has no assignment at all. Field: id.",
        ordered: false,
        key: Key::Sql("SELECT id FROM people p WHERE NOT EXISTS (SELECT 1 FROM assignments a WHERE a.person = p.id)"),
        oql: "people.where { !.assignments.exists() }.select { id }",
    },
    Question {
        id: "q10",
        text: "The id of every person who has the role \"lead\" on at least one project in state \"paused\". Field: id.",
        ordered: false,
        key: Key::Sql("SELECT p.id FROM people p WHERE EXISTS (SELECT 1 FROM assignments a JOIN projects j ON j.id = a.project \
              WHERE a.person = p.id AND a.role = 'lead' AND j.state = 'paused')"),
        oql: "people.where { .assignments.where { .role == \"lead\" && .project.state == \"paused\" }.exists() }.select { id }",
    },
    Question {
        id: "q11",
        text: "The id of every person at level 4 or above whose skills include \"rust\". Field: id.",
        ordered: false,
        key: Key::Sql("SELECT p.id FROM people p WHERE p.level >= 4 AND EXISTS \
              (SELECT 1 FROM people_skills s WHERE s.parent = p.id AND s.value = 'rust')"),
        oql: "people.where { .level >= 4 && .skills contains \"rust\" }.select { id }",
    },
    Question {
        id: "q12",
        text: "The five highest-paid people, highest first, ties by id ascending. Fields: id, salary.",
        ordered: true,
        key: Key::Sql("SELECT id, salary FROM people ORDER BY salary DESC, id LIMIT 5"),
        oql: "people.sort { .salary desc, .id }.limit(5).select { id, salary }",
    },
    Question {
        id: "q13",
        text: "The id of every person at level 2 who earns more than the average salary of all people. Field: id.",
        ordered: false,
        key: Key::Sql("SELECT id FROM people WHERE level = 2 AND salary > (SELECT AVG(salary) FROM people)"),
        oql: "people.where { .level == 2 && .salary > people.avg(.salary) }.select { id }",
    },
    Question {
        id: "q14",
        text: "The id of every person who earns more than their own manager. Field: id.",
        ordered: false,
        key: Key::Sql("SELECT p.id FROM people p JOIN people m ON m.id = p.manager WHERE p.salary > m.salary"),
        oql: "people.where { .salary > .manager.salary }.select { id }",
    },
    Question {
        id: "q15",
        text: "Every assignment of 39 hours or more: the person's name, the project's name, and the hours. Fields: person, project, hours.",
        ordered: false,
        key: Key::Sql("SELECT pe.name AS person, pr.name AS project, a.hours FROM assignments a \
              JOIN people pe ON pe.id = a.person JOIN projects pr ON pr.id = a.project WHERE a.hours >= 39"),
        oql: "assignments.where { .hours >= 39 }.select { person: .person.name, project: .project.name, hours }",
    },
    Question {
        id: "q16",
        text: "For every person at level 6 or above, one row for each project they own, with the person's id and the project's id. \
               A person who owns no project still gets one row, with project null. Fields: person, project.",
        ordered: false,
        key: Key::Sql("SELECT p.id AS person, j.id AS project FROM people p LEFT JOIN projects j ON j.owner = p.id WHERE p.level >= 6"),
        oql: "people { where { .level >= 6 }  j? in .owned  select { person: id, project: j.id } }",
    },
    Question {
        id: "q17",
        text: "Every pair of different people who are in the same unit and at the same level, where that level is 5 or above. \
               Each pair once, with the lesser id first. Fields: a, b (the two ids).",
        ordered: false,
        key: Key::Sql("SELECT x.id AS a, y.id AS b FROM people x JOIN people y ON y.unit = x.unit AND y.level = x.level AND x.id < y.id \
              WHERE x.level >= 5"),
        oql: "people { where { .level >= 5 }  y in people  where { y.unit == .unit && y.level == .level && y.id > .id }  \
              select { a: id, b: y.id } }",
    },
    Question {
        id: "q18",
        text: "Everyone below the person with id \"p000001\" in the management chain, at any depth: their id, and how many \
               steps down they are (1 for a direct report). Fields: id, depth.",
        ordered: false,
        key: Key::Sql("WITH RECURSIVE below(id, depth) AS (SELECT id, 1 FROM people WHERE manager = 'p000001' \
              UNION ALL SELECT p.id, b.depth + 1 FROM people p JOIN below b ON p.manager = b.id) SELECT id, depth FROM below"),
        oql: "people.where { .id == \"p000001\" }.walk(.reports).select { id, depth: ._depth }",
    },
    Question {
        id: "q19",
        text: "For each task state: the state, and the total minutes in the worklogs of the tasks in that state. \
               A state with no worklog entries is left out. Fields: state, minutes.",
        ordered: false,
        key: Key::Sql("SELECT t.state, SUM(w.minutes) AS minutes FROM tasks t JOIN tasks_worklog w ON w.parent = t.id GROUP BY t.state"),
        oql: "tasks.worklog.group_by { state: tasks.state }.select { state, minutes: sum(.minutes) }",
    },
    Question {
        id: "q20",
        text: "Every task whose worklog entries add up to more than 1500 minutes: its id and that total. Fields: id, minutes.",
        ordered: false,
        key: Key::Sql("SELECT parent AS id, SUM(minutes) AS minutes FROM tasks_worklog GROUP BY parent HAVING SUM(minutes) > 1500"),
        oql: "tasks.where { .worklog.minutes.sum() > 1500 }.select { id, minutes: .worklog.minutes.sum() }",
    },
    Question {
        id: "q21",
        text: "For each org: its id, and the id of its highest-paid person (ties by id ascending). Fields: org, top.",
        ordered: false,
        key: Key::Sql("SELECT g.id AS org, (SELECT p.id FROM people p WHERE p.org = g.id ORDER BY p.salary DESC, p.id LIMIT 1) AS top FROM orgs g"),
        oql: "orgs.select { org: id, top: .people.sort { .salary desc, .id }.first().id }",
    },
    Question {
        id: "q22",
        text: "For each project state: the state, the number of projects, and how many of them have a budget over 250000. Fields: state, n, big.",
        ordered: false,
        key: Key::Sql("SELECT state, COUNT(*) AS n, SUM(CASE WHEN budget > 250000 THEN 1 ELSE 0 END) AS big FROM projects GROUP BY state"),
        oql: "projects.group_by { .state }.select { state, n: count(), big: where { .budget > 250000 }.count() }",
    },
    Question {
        id: "q23",
        text: "For every person at level 5 or above: their id, and a band that is \"senior\" at level 6 or above and \"mid\" otherwise. Fields: id, band.",
        ordered: false,
        key: Key::Sql("SELECT id, CASE WHEN level >= 6 THEN 'senior' ELSE 'mid' END AS band FROM people WHERE level >= 5"),
        oql: "people.where { .level >= 5 }.select { id, band: if .level >= 6 then \"senior\" else \"mid\" }",
    },
    Question {
        id: "q24",
        text: "The id of every task in state \"review\" that is assigned to someone who owns at least one project. Field: id.",
        ordered: false,
        key: Key::Sql("SELECT t.id FROM tasks t WHERE t.state = 'review' AND t.assignee IN (SELECT owner FROM projects)"),
        oql: "tasks.where { .state == \"review\" && .assignee in projects.owner }.select { id }",
    },
    Question {
        id: "q25",
        text: "The id of every person, other than \"p000001\", who is assigned to at least one project that \"p000001\" is also assigned to. Field: id.",
        ordered: false,
        key: Key::Sql("SELECT DISTINCT p.id FROM people p JOIN assignments a ON a.person = p.id JOIN assignments b ON b.project = a.project \
              WHERE b.person = 'p000001' AND p.id <> 'p000001'"),
        oql: "people.where { .id != \"p000001\" && .assignments.where { .project.assignments.where { .person == \"p000001\" }.exists() }.exists() }.select { id }",
    },
    Question {
        id: "q26",
        text: "How many people were hired in each calendar year: the year as a whole number, and the count. Fields: year, n.",
        ordered: false,
        key: Key::Sql("SELECT CAST(strftime('%Y', hired) AS INTEGER) AS year, COUNT(*) AS n FROM people GROUP BY 1"),
        oql: "people.group_by { year: year(.hired) }.select { year, n: count() }",
    },
    Question {
        id: "q27",
        text: "The id of every task that has no assignee, is in state \"open\", and has 13 points. Field: id.",
        ordered: false,
        key: Key::Sql("SELECT id FROM tasks WHERE assignee IS NULL AND state = 'open' AND points = 13"),
        oql: "tasks.where { .assignee is null && .state == \"open\" && .points == 13 }.select { id }",
    },
    Question {
        id: "q28",
        text: "For each org: its id, and the total hours of all assignments held by its people (0 when there are none). Fields: id, hours.",
        ordered: false,
        key: Key::Sql("SELECT g.id, COALESCE((SELECT SUM(a.hours) FROM assignments a JOIN people p ON p.id = a.person WHERE p.org = g.id), 0) AS hours FROM orgs g"),
        oql: "orgs.select { id, hours: .people.assignments.hours.sum() }",
    },
    Question {
        id: "q29",
        text: "The three projects with the most assigned hours in total, most first, ties by project id ascending. \
               Fields: project (the project's id), hours.",
        ordered: true,
        key: Key::Sql("SELECT project, SUM(hours) AS hours FROM assignments GROUP BY project ORDER BY hours DESC, project LIMIT 3"),
        oql: "assignments.group_by { project: .project.id }.select { project, hours: sum(.hours) }.sort { .hours desc, .project }.limit(3)",
    },
    Question {
        id: "q30",
        text: "The ids of all tasks that block, directly or through a chain of blocking links of any kind, the task that is \
               blocked in the link with id \"l0000001\". Field: id.",
        ordered: false,
        key: Key::Sql("WITH RECURSIVE behind(id) AS (SELECT blocker FROM links WHERE blocked = (SELECT blocked FROM links WHERE id = 'l0000001') \
              UNION SELECT l.blocker FROM links l JOIN behind b ON l.blocked = b.id) SELECT id FROM behind"),
        oql: "links.where { .id == \"l0000001\" }.blocked.walk(.blocked_by.blocker).select { id }",
    },
    Question {
        id: "q31",
        text: "The five notes whose `about` vector is nearest, by direction, the vector [1.0, 0.0, 0.0, 0.0], nearest first. Field: id.",
        ordered: true,
        key: Key::Notes(|notes, _| ids(nearest_notes(notes.iter(), &[1.0, 0.0, 0.0, 0.0], 5))),
        oql: "notes.nearest { field: .about, query: [1.0, 0.0, 0.0, 0.0], metric: \"cosine\", k: 5 }.select { id }",
    },
    Question {
        id: "q32",
        text: "Among the notes with topic \"plan\" only, the three whose `about` vector is nearest, by direction, the vector [0.0, 1.0, 0.0, 0.0], nearest first. Field: id.",
        ordered: true,
        key: Key::Notes(|notes, _| {
            ids(nearest_notes(notes.iter().filter(|note| note.topic == "plan"), &[0.0, 1.0, 0.0, 0.0], 3))
        }),
        oql: "notes.where { .topic == \"plan\" }.nearest { field: .about, query: [0.0, 1.0, 0.0, 0.0], k: 3 }.select { id }",
    },
    Question {
        id: "q33",
        text: "For every person at level 6 or above: their id, and the id of the one note of theirs whose `about` vector is nearest, by direction, the vector [0.0, 0.0, 1.0, 0.0]. Fields: id, note.",
        ordered: false,
        key: Key::Notes(|notes, seniors| {
            seniors
                .iter()
                .map(|person| {
                    let theirs = notes.iter().filter(|note| &note.person == person);
                    let nearest = nearest_notes(theirs, &[0.0, 0.0, 1.0, 0.0], 1);
                    serde_json::json!({ "id": person, "note": nearest.first().map(|note| note.id.clone()) })
                })
                .collect()
        }),
        oql: "people.where { .level >= 6 }.select { id, note: .notes.nearest { field: .about, query: [0.0, 0.0, 1.0, 0.0], k: 1 }.first().id }",
    },
    Question {
        id: "q34",
        text: "For each note topic: the topic, and how many of its notes have an `about` vector whose cosine similarity to [1.0, 0.0, 0.0, 0.0] is above 0.8 (0 when there are none). Fields: topic, n.",
        ordered: false,
        key: Key::Notes(|notes, _| {
            TOPICS
                .iter()
                .map(|topic| {
                    let n = notes
                        .iter()
                        .filter(|note| note.topic == *topic)
                        .filter(|note| clear_of(cosine(&note.about, &[1.0, 0.0, 0.0, 0.0]), 0.8))
                        .count();
                    serde_json::json!({ "topic": topic, "n": n })
                })
                .collect()
        }),
        oql: "notes.group_by { .topic }.select { topic, n: where { similarity(.about, [1.0, 0.0, 0.0, 0.0]) > 0.8 }.count() }",
    },
    Question {
        id: "q35",
        text: "The id of every note whose `about` vector is at a straight-line distance of less than 0.4 from the vector [0.5, 0.5, 0.0, 0.0]. Field: id.",
        ordered: false,
        key: Key::Notes(|notes, _| {
            ids(notes.iter().filter(|note| !clear_of(apart(&note.about, &[0.5, 0.5, 0.0, 0.0]), 0.4)).collect())
        }),
        oql: "notes.where { distance(.about, [0.5, 0.5, 0.0, 0.0]) < 0.4 }.select { id }",
    },
];

/// The notes, made in Owl if this database has none yet, and the ids of
/// the people at level 6 or above, whom one question is about.
fn with_notes(executor: &QueryExecutor<RustStore>) -> Result<(Vec<Note>, Vec<String>)> {
    let column = |query: &str| -> Result<Vec<String>> {
        Ok(rows_as_served(executor, query)
            .map_err(|error| anyhow::anyhow!("{error}\n{query}"))?
            .iter()
            .filter_map(|row| row["id"].as_str().map(str::to_string))
            .collect())
    };
    let people = column("people.sort { .id.asc }.select { id }")?;
    let made = notes(&people);
    if rows_as_served(executor, "notes.limit(1)").is_err() {
        let schema = "schema notes { id: String, person: Ref(people), topic: String, about: Vector(4), index(.about, metric: \"cosine\") }";
        let file = owl_db::parse_oql_file(schema).map_err(|error| anyhow::anyhow!("{error}"))?;
        let report = owl_db::execute_oql_file(&file, executor).map_err(|error| anyhow::anyhow!("{error}"))?;
        anyhow::ensure!(report.errors.is_empty(), "{}", report.errors.join("; "));
        let rows: Vec<owl_db::Fields> = made
            .iter()
            .map(|note| {
                use owl_db::Value;
                let mut fields = owl_db::Fields::default();
                fields.insert("id".to_string(), Value::String(note.id.clone()));
                fields.insert("person".to_string(), Value::String(note.person.clone()));
                fields.insert("topic".to_string(), Value::String(note.topic.to_string()));
                fields.insert(
                    "about".to_string(),
                    Value::List(note.about.iter().map(|number| Value::Float(owl_db::vector::widened(*number))).collect()),
                );
                fields
            })
            .collect();
        executor.insert_documents("notes", rows).map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    Ok((made, column("people.where { .level >= 6 }.sort { .id.asc }.select { id }")?))
}

/// The language card, as the test suite runs it.
const CARD: &str = include_str!("../../../docs/OQL_CARD.md");

fn sorted(mut rows: Vec<Json>) -> Vec<Json> {
    rows.sort_by_cached_key(|row| row.to_string());
    rows
}

/// A query's rows as the server gives them: a reference is the id of the
/// row it points at, so a reference selected or grouped by is an id.
fn rows_as_served(executor: &QueryExecutor<RustStore>, oql: &str) -> Result<Vec<Json>, String> {
    fn json(value: &owl_db::Value) -> Json {
        use owl_db::Value;
        match value {
            Value::Null | Value::Reference(_) | Value::GlobalReference(_) => Json::Null,
            Value::Vector(numbers) => Json::Array(numbers.iter().map(|number| serde_json::Number::from_f64(owl_db::vector::widened(*number)).map_or(Json::Null, Json::Number)).collect()),
            Value::Boolean(value) => Json::from(*value),
            Value::Integer(value) => Json::from(*value),
            Value::Float(value) => Json::from(*value),
            Value::String(value) => Json::from(value.clone()),
            Value::DateTime(instant) => Json::from(instant.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            Value::List(items) => Json::Array(items.iter().map(json).collect()),
            Value::Object(fields) => {
                Json::Object(fields.iter().map(|(name, value)| (name.clone(), json(value))).collect())
            }
        }
    }
    let query = owl_db::parse_oql(oql).map_err(|error| error.to_string())?;
    let rows = executor.execute_query(&query).map_err(|error| error.to_string())?;
    rows.iter()
        .map(|row| {
            let mut object = serde_json::Map::new();
            for (name, value) in row.fields.iter().filter(|(name, _)| !name.starts_with('_')) {
                let value = executor.with_reference_ids(value).map_err(|error| error.to_string())?;
                object.insert(name.clone(), json(&value));
            }
            Ok(Json::Object(object))
        })
        .collect()
}

/// What became of one answer.
enum Verdict {
    Right(usize),
    /// Accepted and run, and not what was asked. Nothing tells the writer.
    Wrong(String),
    /// Refused by the check, before anything ran: the problem, as data.
    Refused(Json),
    /// Passed the check and failed when run.
    Failed(String),
    Unanswered,
}

fn judge(
    loaded: &Loaded,
    executor: &QueryExecutor<RustStore>,
    (notes, seniors): &(Vec<Note>, Vec<String>),
    question: &Question,
    oql: Option<&str>,
) -> Result<Verdict> {
    let Some(oql) = oql else {
        return Ok(Verdict::Unanswered);
    };
    if let Err(problem) = assist::check(executor, oql) {
        return Ok(Verdict::Refused(serde_json::to_value(problem)?));
    }
    let owl = match rows_as_served(executor, oql) {
        Ok(rows) => rows,
        Err(error) => return Ok(Verdict::Failed(error)),
    };
    let key = match &question.key {
        Key::Sql(sql) => loaded.sqlite.query(sql).with_context(|| format!("{}: the key", question.id))?,
        Key::Notes(worked_out) => worked_out(notes, seniors),
    };
    let difference = match question.ordered {
        true => first_difference(&owl, &key),
        false => first_difference(&sorted(owl), &sorted(key.clone())),
    };
    Ok(match difference {
        None => Verdict::Right(key.len()),
        Some(difference) => Verdict::Wrong(difference),
    })
}

/// What the model is given: the card, this database's catalogue, the
/// questions, and the form its answers take.
fn prompt(executor: &QueryExecutor<RustStore>) -> Result<String> {
    let catalogue = assist::catalogue(executor).map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut text = String::from(
        "You are given a database, a one-page description of its query language, OQL, and a list of questions.\n\
         OQL is not SQL. Write one OQL query for each question, using only what is on this page.\n\n\
         Answer with a single JSON object from each question's id to its query as a string, and nothing else:\n\
         {\"q01\": \"people.where { .. }.select { .. }\", \"q02\": \"..\"}\n\n\
         The fields of each answer must have exactly the names the question gives.\n\n\
         ---\n\n",
    );
    text.push_str(CARD);
    text.push_str("\n---\n\n## The database you are asked about\n\nThis is not the example database above.\n\n```oql\n");
    text.push_str(&catalogue.as_text());
    text.push_str("```\n\n## The questions\n\n");
    for question in QUESTIONS {
        text.push_str(&format!("- **{}**: {}\n", question.id, question.text));
    }
    Ok(text)
}

pub fn run(loaded: &Loaded, prompt_to: Option<&Path>, answers: Option<&Path>, feedback: Option<&Path>) -> Result<()> {
    let executor = QueryExecutor::new_with_real_clock(loaded.owl.store().clone());
    let noted = with_notes(&executor)?;
    if let Some(path) = prompt_to {
        let text = prompt(&executor)?;
        std::fs::write(path, &text).with_context(|| format!("writing {}", path.display()))?;
        println!("the prompt, {} characters, is in {}", text.len(), path.display());
    }
    let given: Option<serde_json::Map<String, Json>> = match answers {
        Some(path) => {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            Some(serde_json::from_str(&text).with_context(|| format!("{} is not a JSON object", path.display()))?)
        }
        None if prompt_to.is_some() => return Ok(()),
        None => None,
    };

    let (mut right, mut wrong, mut refused, mut failed, mut unanswered) = (0, 0, 0, 0, 0);
    let mut again = String::new();
    for question in QUESTIONS {
        let answer = match &given {
            Some(given) => given.get(question.id).and_then(Json::as_str),
            // With no answers to score, the questions are held to their
            // own reference queries.
            None => Some(question.oql),
        };
        let verdict = judge(loaded, &executor, &noted, question, answer)?;
        let said = match &verdict {
            Verdict::Right(rows) => {
                right += 1;
                format!("right, {rows} rows")
            }
            Verdict::Wrong(difference) => {
                wrong += 1;
                format!("WRONG: {difference}")
            }
            Verdict::Refused(problem) => {
                refused += 1;
                again.push_str(&format!(
                    "- **{}**: {}\n  You wrote: `{}`\n  The check refused it:\n  ```json\n  {problem}\n  ```\n",
                    question.id,
                    question.text,
                    answer.unwrap_or_default()
                ));
                format!("refused by the check ({}): {}", problem["code"].as_str().unwrap_or("?"), problem["message"].as_str().unwrap_or("?"))
            }
            Verdict::Failed(error) => {
                failed += 1;
                again.push_str(&format!(
                    "- **{}**: {}\n  You wrote: `{}`\n  It failed when run: {error}\n",
                    question.id,
                    question.text,
                    answer.unwrap_or_default()
                ));
                format!("failed when run: {error}")
            }
            Verdict::Unanswered => {
                unanswered += 1;
                "no answer".to_string()
            }
        };
        println!("{}  {said}", question.id);
        if !matches!(verdict, Verdict::Right(_)) {
            if let Some(answer) = answer {
                println!("     {answer}");
            }
        }
    }
    println!(
        "\n{right} of {} right; {refused} refused by the check, {failed} failed when run, {wrong} wrong, {unanswered} unanswered.",
        QUESTIONS.len()
    );
    if let Some(path) = feedback {
        let text = match again.is_empty() {
            true => String::new(),
            false => format!(
                "These queries were not accepted. Here is what the database said about each. Write each again.\n\
                 Answer as before: a single JSON object from id to query, with only these ids.\n\n{again}"
            ),
        };
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        println!("what the database said about the {} it did not accept is in {}", refused + failed, path.display());
    }
    if given.is_none() && right != QUESTIONS.len() {
        anyhow::bail!("a reference query does not give the key's answer");
    }
    Ok(())
}
