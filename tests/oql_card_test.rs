//! The language card, `docs/OQL_CARD.md`, is what a program is given to
//! learn OQL from, so nothing on it may be wrong. Every example on it is
//! checked and run here, and every example of a mistake must be refused.

use owl_db::storage::RustStore;
use owl_db::{assist, execute_oql_file, parse_oql, parse_oql_file, QueryExecutor};
use std::sync::Arc;
use tempfile::TempDir;

const CARD: &str = include_str!("../docs/OQL_CARD.md");

/// The fenced blocks of the card tagged `tag`, each split into its
/// statements, which a blank line separates.
fn blocks(tag: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut lines = CARD.lines();
    while let Some(line) = lines.next() {
        if line.trim() != format!("```{tag}") {
            continue;
        }
        let body: Vec<&str> = lines.by_ref().take_while(|line| line.trim() != "```").collect();
        found.extend(
            body.join("\n")
                .split("\n\n")
                .map(|statement| statement.trim().to_string())
                .filter(|statement| !statement.is_empty()),
        );
    }
    found
}

/// The card's own schema, with a few rows of each collection.
fn database() -> (TempDir, QueryExecutor<RustStore>) {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let schema = blocks("oql").remove(0);
    assert!(schema.starts_with("schema people"), "the card's first block is its schema");
    let rows = r#"
        people.insert [
          { id: "kim", name: "Kim", level: 4 }, { id: "lee", name: "Lee", level: 3 },
          { id: "ada", name: "Ada", level: 2, manager: "kim" }, { id: "ben", name: "Ben", level: 1, manager: "kim" },
          { id: "cy", name: "Cy", level: 1, manager: "lee" }
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
          { id: "t1", project: "apollo", state: "open", tags: ["api", "db"], notes: [{ kind: "call", mins: 5 }] },
          { id: "t2", project: "apollo", state: "done", tags: [], notes: [] },
          { id: "t3", project: "gemini", state: "open", tags: ["ui"], notes: [{ kind: "call", mins: 7 }, { kind: "mail", mins: 2 }] },
          { id: "t4", project: "gemini", state: "open", tags: [], notes: [] }
        ]
        links.insert [
          { id: "l1", blocker: "t1", blocked: "t2", kind: "hard" },
          { id: "l2", blocker: "t2", blocked: "t3", kind: "hard" },
          { id: "l3", blocker: "t3", blocked: "t1", kind: "soft" },
          { id: "l4", blocker: "t3", blocked: "t4", kind: "hard" }
        ]
    "#;
    let source = format!("{schema}\n{rows}");
    let report = execute_oql_file(&parse_oql_file(&source).unwrap(), &executor).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    (directory, executor)
}

#[test]
fn every_example_on_the_card_is_accepted_and_runs() {
    let (_directory, executor) = database();
    let examples = blocks("oql");
    assert!(examples.len() > 30, "the card has {} examples", examples.len());
    let mut failures = Vec::new();
    for example in examples.iter().skip(1) {
        if let Err(problem) = assist::check(&executor, example) {
            failures.push(format!("check refused: {}\n{example}", problem.message));
            continue;
        }
        if let Err(error) = executor.execute_query(&parse_oql(example).unwrap()) {
            failures.push(format!("did not run: {error}\n{example}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_mistake_on_the_card_is_refused_without_running() {
    let (_directory, executor) = database();
    let mistakes = blocks("oql-error");
    assert!(mistakes.len() >= 5);
    for mistake in mistakes {
        // Refused when run, and refused by the check, which runs nothing.
        let ran = parse_oql(&mistake)
            .map_err(|error| error.to_string())
            .and_then(|query| executor.execute_query(&query).map_err(|error| error.to_string()));
        assert!(ran.is_err(), "the card calls this a mistake, and it ran:\n{mistake}");
        assert!(
            assist::check(&executor, &mistake).is_err(),
            "the card calls this a mistake, and the check passed it:\n{mistake}"
        );
    }
}
