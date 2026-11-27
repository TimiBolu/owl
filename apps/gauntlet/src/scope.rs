//! Queries about names and scope, made at random and held to three answers.
//!
//! The hand-written queries ask hard questions. These ask one question in
//! many shapes: what does a name mean here? The row a block starts from,
//! written bare, with a dot, or by its collection's name; a row captured
//! with `in`, optional or not, read in a filter, a select and a nested
//! query; a name given with `=`; the row a path step started from.
//!
//! Each query is written twice from one description, as OQL and as SQL, and
//! must give:
//!
//! - the same rows from Owl as from SQLite, in any order;
//! - the same rows in the same order from Owl whether a block's conditions
//!   are arranged by the planner or run exactly as written.
//!
//! The same seed always makes the same queries.

use crate::data::{Cell, Table};
use crate::Loaded;
use anyhow::Result;
use serde_json::Value as Json;
use storefront::engines::first_difference;

/// SplitMix64, as the data generator uses.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, options: &'a [T]) -> &'a T {
        &options[self.below(options.len())]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Int,
    Float,
}

/// A plain field that always has a value.
struct Field {
    collection: &'static str,
    name: &'static str,
    kind: Kind,
}

const FIELDS: &[Field] = &[
    Field { collection: "orgs", name: "plan", kind: Kind::Text },
    Field { collection: "orgs", name: "region", kind: Kind::Text },
    Field { collection: "units", name: "budget", kind: Kind::Float },
    Field { collection: "people", name: "level", kind: Kind::Int },
    Field { collection: "people", name: "salary", kind: Kind::Float },
    Field { collection: "projects", name: "state", kind: Kind::Text },
    Field { collection: "projects", name: "budget", kind: Kind::Float },
    Field { collection: "assignments", name: "role", kind: Kind::Text },
    Field { collection: "assignments", name: "hours", kind: Kind::Int },
    Field { collection: "tasks", name: "state", kind: Kind::Text },
    Field { collection: "tasks", name: "points", kind: Kind::Int },
    Field { collection: "links", name: "kind", kind: Kind::Text },
    Field { collection: "events", name: "kind", kind: Kind::Text },
    Field { collection: "events", name: "amount", kind: Kind::Int },
];

/// One step between two collections, as OQL names it and as SQL joins it.
struct Edge {
    from: &'static str,
    /// The step's name after a dot.
    step: &'static str,
    to: &'static str,
    /// The column that holds the reference.
    column: &'static str,
    /// Whether the column is on the row the step starts from.
    forward: bool,
    /// Whether a row may have nothing at the end of a forward step.
    optional: bool,
}

const fn forward(from: &'static str, step: &'static str, to: &'static str, optional: bool) -> Edge {
    Edge { from, step, to, column: step, forward: true, optional }
}

const fn back(from: &'static str, step: &'static str, to: &'static str, column: &'static str) -> Edge {
    Edge { from, step, to, column, forward: false, optional: false }
}

const EDGES: &[Edge] = &[
    forward("units", "org", "orgs", false),
    forward("units", "parent", "units", true),
    forward("people", "org", "orgs", false),
    forward("people", "unit", "units", false),
    forward("people", "manager", "people", true),
    forward("projects", "org", "orgs", false),
    forward("projects", "owner", "people", false),
    forward("assignments", "person", "people", false),
    forward("assignments", "project", "projects", false),
    forward("tasks", "project", "projects", false),
    forward("tasks", "assignee", "people", true),
    forward("tasks", "parent", "tasks", true),
    forward("links", "blocker", "tasks", false),
    forward("links", "blocked", "tasks", false),
    forward("events", "person", "people", false),
    // The way back, by the name the schema gives it or by the collection's.
    back("units", "children", "units", "parent"),
    back("people", "reports", "people", "manager"),
    back("people", "owned", "projects", "owner"),
    back("people", "assigned", "tasks", "assignee"),
    back("tasks", "subtasks", "tasks", "parent"),
    back("tasks", "blocks", "links", "blocker"),
    back("tasks", "blocked_by", "links", "blocked"),
    back("orgs", "units", "units", "org"),
    back("orgs", "projects", "projects", "org"),
    back("units", "people", "people", "unit"),
    back("people", "assignments", "assignments", "person"),
    back("projects", "assignments", "assignments", "project"),
    back("projects", "tasks", "tasks", "project"),
    back("people", "events", "events", "person"),
];

impl Edge {
    /// The SQL condition that joins `to` to `from`, by their aliases.
    fn joined(&self, from: &str, to: &str) -> String {
        match self.forward {
            true => format!("{to}.id = {from}.{}", self.column),
            false => format!("{to}.{} = {from}.id", self.column),
        }
    }
}

/// The collections a block may start from: small enough that every pair of
/// their rows is still a quick answer.
const ROOTS: &[&str] = &["people", "projects", "units", "tasks", "orgs"];

fn edges_from(collection: &str) -> Vec<&'static Edge> {
    EDGES.iter().filter(|edge| edge.from == collection).collect()
}

fn fields_of(collection: &str) -> Vec<&'static Field> {
    FIELDS.iter().filter(|field| field.collection == collection).collect()
}

/// A value some row of the collection has in the field, written for both.
fn literal(random: &mut Random, tables: &[Table], field: &Field) -> Option<String> {
    let table = tables.iter().find(|table| table.name == field.collection)?;
    let row = &table.rows[random.below(table.rows.len())];
    let (_, cell) = row.iter().find(|(name, _)| *name == field.name)?;
    Some(match cell {
        Cell::Text(text) => format!("\"{text}\""),
        Cell::Int(number) => number.to_string(),
        Cell::Float(number) => format!("{number:?}"),
        _ => return None,
    })
}

/// The same literal in SQL: text is in single quotes there.
fn sql_literal(written: &str) -> String {
    match written.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        Some(text) => format!("'{}'", text.replace('\'', "''")),
        None => written.to_string(),
    }
}

/// An OQL comparison in SQL. In OQL an absent value is unequal to every
/// value, where SQL calls the comparison unknown and drops the row; `IS
/// NOT` is SQL's spelling of OQL's `!=`.
fn sql_operator(operator: &str) -> &str {
    match operator {
        "==" => "=",
        "!=" => "IS NOT",
        other => other,
    }
}

/// A name in a block: the row it starts from, or a captured row.
struct Name {
    /// As OQL writes it; empty for the starting row.
    oql: String,
    /// Its alias in the SQL.
    sql: String,
    collection: &'static str,
    /// Whether the name may be empty: an optional capture, or one made
    /// from one.
    may_be_empty: bool,
}

/// A field of a name, as each language writes it. The starting row's field
/// is written in one of the three ways OQL has for it.
fn read(random: &mut Random, root: &str, name: &Name, field: &str) -> (String, String) {
    let oql = match name.oql.is_empty() {
        false => format!("{}.{field}", name.oql),
        true => match random.below(3) {
            0 => field.to_string(),
            1 => format!(".{field}"),
            _ => format!("{root}.{field}"),
        },
    };
    (oql, format!("{}.{field}", name.sql))
}

/// One condition on the names so far, in both languages.
fn condition(random: &mut Random, tables: &[Table], root: &str, names: &[Name]) -> Option<(String, String)> {
    let at = random.below(names.len());
    let name = &names[at];
    let fields = fields_of(name.collection);
    // Two rows compared with one another, when there are two of a kind.
    if random.chance(30) {
        let others: Vec<usize> = (0..names.len())
            .filter(|other| *other != at && names[*other].collection == name.collection)
            .collect();
        if let Some(&other) = others.first() {
            let operator = *random.pick(&["<", ">", "!=", "=="]);
            let field = match (fields.is_empty(), random.chance(50)) {
                (false, true) => random.pick(&fields).name,
                _ => "id",
            };
            let (left, left_sql) = read(random, root, name, field);
            let (right, right_sql) = read(random, root, &names[other], field);
            return Some((
                format!("{left} {operator} {right}"),
                format!("{left_sql} {} {right_sql}", sql_operator(operator)),
            ));
        }
    }
    if fields.is_empty() {
        return None;
    }
    let field = *random.pick(&fields);
    let value = literal(random, tables, field)?;
    let operator = match field.kind {
        Kind::Text => *random.pick(&["==", "==", "!="]),
        // An exact match of a number is nearly always no rows.
        _ => *random.pick(&["<", ">", "<", ">", "==", "!="]),
    };
    let (left, left_sql) = read(random, root, name, field.name);
    Some((
        format!("{left} {operator} {value}"),
        format!("{left_sql} {} {}", sql_operator(operator), sql_literal(&value)),
    ))
}

pub struct Case {
    pub shape: &'static str,
    pub oql: String,
    pub sql: String,
    /// Whether the rows must also come in SQLite's order: the query's
    /// order is one the language defines and the SQL spells out.
    pub ordered: bool,
    /// For a write: what to read afterwards, on each engine. The write is
    /// made on both, and the two are then held to the same rows.
    pub then: Option<(String, String)>,
}

/// A block of captures: each names the rows a path reaches from the
/// starting row or from an earlier capture.
fn block(random: &mut Random, tables: &[Table]) -> Option<Case> {
    let root = *random.pick(ROOTS);
    let mut names = vec![Name { oql: String::new(), sql: "r0".to_string(), collection: root, may_be_empty: false }];
    let mut entries: Vec<String> = Vec::new();
    let mut joins = String::new();
    let mut filters: Vec<String> = Vec::new();
    // Conditions not yet written into the block: they go in one filter
    // at its end.
    let mut waiting: Vec<String> = Vec::new();

    for number in 1..=1 + random.below(3) {
        let base_at = random.below(names.len());
        let (base_oql, base_sql, base_collection, base_empty) = {
            let base = &names[base_at];
            (base.oql.clone(), base.sql.clone(), base.collection, base.may_be_empty)
        };
        let steps = 1 + random.below(2);
        let mut path = String::new();
        let mut collection = base_collection;
        let mut from = base_sql.clone();
        let mut pieces: Vec<(String, String, String)> = Vec::new();
        for step in 0..steps {
            let edges = edges_from(collection);
            if edges.is_empty() {
                break;
            }
            let edge = *random.pick(&edges);
            let alias = match step + 1 == steps {
                true => format!("c{number}"),
                false => format!("c{number}m"),
            };
            path.push('.');
            path.push_str(edge.step);
            pieces.push((edge.to.to_string(), alias.clone(), edge.joined(&from, &alias)));
            collection = edge.to;
            from = alias;
        }
        let Some((_, last, _)) = pieces.last() else {
            continue;
        };
        // The last alias is always the capture's own.
        let alias = format!("c{number}");
        if *last != alias {
            return None;
        }
        let optional = random.chance(25);
        let name = format!("c{number}");
        entries.push(format!(
            "  {name}{} in {}{path}",
            if optional { "?" } else { "" },
            if base_oql.is_empty() { String::new() } else { base_oql.clone() },
        ));
        match (optional, pieces.as_slice()) {
            (false, pieces) => {
                for (table, alias, on) in pieces {
                    joins.push_str(&format!("\n  JOIN {table} AS {alias} ON {on}"));
                }
            }
            (true, [(table, alias, on)]) => joins.push_str(&format!("\n  LEFT JOIN {table} AS {alias} ON {on}")),
            (true, [(first, first_alias, first_on), (second, second_alias, second_on)]) => joins.push_str(&format!(
                "\n  LEFT JOIN ({first} AS {first_alias} JOIN {second} AS {second_alias} ON {second_on}) ON {first_on}"
            )),
            _ => return None,
        }
        names.push(Name { oql: name, sql: alias, collection, may_be_empty: optional || base_empty });

        // A condition, written here or left for later.
        if random.chance(60) {
            if let Some((oql, sql)) = condition(random, tables, root, &names) {
                filters.push(sql);
                match random.chance(50) {
                    true => entries.push(format!("  where {{ {oql} }}")),
                    false => waiting.push(oql),
                }
            }
        }
    }
    if names.len() == 1 {
        return None;
    }
    if !waiting.is_empty() {
        entries.push(format!("  where {{ {} }}", waiting.join(" && ")));
    }

    let counted = random.chance(25);
    let (select, columns) = match counted {
        true => ("  count()".to_string(), "count(*) AS count".to_string()),
        false => {
            let mut shown = vec!["id".to_string()];
            let mut columns = vec!["r0.id AS id".to_string()];
            for name in &names[1..] {
                shown.push(format!("{0}: {0}.id", name.oql));
                columns.push(format!("{}.id AS {}", name.sql, name.oql));
            }
            // One more field, of any name.
            let extra = &names[random.below(names.len())];
            if let Some(field) = fields_of(extra.collection).first() {
                let (oql, sql) = read(random, root, extra, field.name);
                shown.push(format!("extra: {oql}"));
                columns.push(format!("{sql} AS extra"));
            }
            (format!("  select {{ {} }}", shown.join(", ")), columns.join(", "))
        }
    };
    let filter = match filters.is_empty() {
        true => String::new(),
        false => format!("\nWHERE {}", filters.join(" AND ")),
    };
    Some(Case {
        shape: "block of captures",
        ordered: false,
        then: None,
        oql: format!("{root} {{\n{}\n{select}\n}}", entries.join("\n")),
        sql: format!("SELECT {columns}\nFROM {root} AS r0{joins}{filter}"),
    })
}

/// A name given with `=` and read inside a nested query on another
/// collection: it must still be what it was for the block's row.
fn named_value(random: &mut Random, _tables: &[Table]) -> Option<Case> {
    let root = *random.pick(&["people", "projects", "units"]);
    let fields = fields_of(root);
    let field = *random.pick(&fields);
    // The value: the field bare, with a dot, with arithmetic, or one
    // reached through a reference that every row has.
    let (value, value_sql, kind) = match random.below(4) {
        0 => (field.name.to_string(), format!("r0.{}", field.name), field.kind),
        1 => (format!(".{}", field.name), format!("r0.{}", field.name), field.kind),
        2 if field.kind != Kind::Text => {
            (format!("{} + 1", field.name), format!("(r0.{} + 1)", field.name), field.kind)
        }
        _ => {
            let edges: Vec<&Edge> = edges_from(root).into_iter().filter(|edge| edge.forward && !edge.optional).collect();
            let edge = *random.pick(&edges);
            let far = fields_of(edge.to);
            let far = *random.pick(&far);
            (
                format!(".{}.{}", edge.step, far.name),
                format!("(SELECT f.{} FROM {} AS f WHERE f.id = r0.{})", far.name, edge.to, edge.column),
                far.kind,
            )
        }
    };
    // Somewhere to ask about it: a field of the same kind.
    let asked: Vec<&Field> = FIELDS.iter().filter(|other| other.kind == kind).collect();
    let asked = *random.pick(&asked);
    let operator = match kind {
        Kind::Text => "==",
        _ => *random.pick(&["==", "<", ">"]),
    };
    let sql_operator = if operator == "==" { "=" } else { operator };
    Some(Case {
        shape: "name read in a nested query",
        ordered: false,
        then: None,
        oql: format!(
            "{root} {{\n  n = {value}\n  select {{ id, k: {}.where {{ .{} {operator} n }}.count() }}\n}}",
            asked.collection, asked.name
        ),
        sql: format!(
            "SELECT r0.id AS id, (SELECT count(*) FROM {} AS s WHERE s.{} {sql_operator} {value_sql}) AS k\nFROM {root} AS r0",
            asked.collection, asked.name
        ),
    })
}

/// A captured row read inside a nested query.
fn capture_in_nested(random: &mut Random, _tables: &[Table]) -> Option<Case> {
    let root = *random.pick(ROOTS);
    let edges = edges_from(root);
    let edge = *random.pick(&edges);
    let fields = fields_of(edge.to);
    if fields.is_empty() {
        return None;
    }
    let field = *random.pick(&fields);
    let asked: Vec<&Field> = FIELDS.iter().filter(|other| other.kind == field.kind).collect();
    let asked = *random.pick(&asked);
    Some(Case {
        shape: "captured row read in a nested query",
        ordered: false,
        then: None,
        oql: format!(
            "{root} {{\n  c in .{}\n  select {{ id, c: c.id, k: {}.where {{ .{} == c.{} }}.count() }}\n}}",
            edge.step, asked.collection, asked.name, field.name
        ),
        sql: format!(
            "SELECT r0.id AS id, c.id AS c, (SELECT count(*) FROM {} AS s WHERE s.{} = c.{}) AS k\nFROM {root} AS r0\n  JOIN {} AS c ON {}",
            asked.collection,
            asked.name,
            field.name,
            edge.to,
            edge.joined("r0", "c")
        ),
    })
}

/// A chain of path steps whose `select` names the collection it began at:
/// that is the row each row was reached from.
fn anchor_after_steps(random: &mut Random, tables: &[Table]) -> Option<Case> {
    let root = *random.pick(ROOTS);
    let fields = fields_of(root);
    let mut chain = root.to_string();
    let mut filter = String::new();
    if random.chance(50) {
        let field = *random.pick(&fields);
        let value = literal(random, tables, field)?;
        chain.push_str(&format!(".where {{ .{} == {value} }}", field.name));
        filter = format!("\nWHERE r0.{} = {}", field.name, sql_literal(&value));
    }
    let mut collection = root;
    let mut from = "r0".to_string();
    let mut joins = String::new();
    for step in 1..=1 + random.below(2) {
        let edges = edges_from(collection);
        if edges.is_empty() {
            break;
        }
        let edge = *random.pick(&edges);
        let alias = format!("s{step}");
        chain.push('.');
        chain.push_str(edge.step);
        joins.push_str(&format!("\n  JOIN {} AS {alias} ON {}", edge.to, edge.joined(&from, &alias)));
        collection = edge.to;
        from = alias.clone();
        // A filter between two steps, on the rows the first reached.
        if random.chance(30) {
            if let Some(between) = fields_of(collection).first() {
                if let Some(value) = literal(random, tables, between) {
                    let (operator, sql_operator) = match between.kind {
                        Kind::Text => ("==", "="),
                        _ => (">", ">"),
                    };
                    chain.push_str(&format!(".where {{ .{} {operator} {value} }}", between.name));
                    filter.push_str(&format!(
                        "\n{} {alias}.{} {sql_operator} {}",
                        if filter.is_empty() { "WHERE" } else { "  AND" },
                        between.name,
                        sql_literal(&value)
                    ));
                }
            }
        }
    }
    if from == "r0" {
        return None;
    }
    let field = fields.first()?;
    Some(Case {
        shape: "row a path began at",
        ordered: false,
        then: None,
        oql: format!("{chain}.select {{ from: {root}.id, of: {root}.{}, id }}", field.name),
        sql: format!(
            "SELECT r0.id AS \"from\", r0.{} AS \"of\", {from}.id AS id\nFROM {root} AS r0{joins}{filter}",
            field.name
        ),
    })
}

/// A step that leads back into its own collection, to walk.
struct Loop {
    collection: &'static str,
    /// The path the walk follows.
    path: &'static str,
    /// The SQL that joins `n`, the next row, to `m`.
    next: &'static str,
}

const LOOPS: &[Loop] = &[
    Loop { collection: "units", path: ".children", next: "JOIN units AS n ON n.parent = m.id" },
    Loop { collection: "units", path: ".parent", next: "JOIN units AS n ON n.id = m.parent" },
    Loop { collection: "people", path: ".manager", next: "JOIN people AS n ON n.id = m.manager" },
    Loop { collection: "people", path: ".reports", next: "JOIN people AS n ON n.manager = m.id" },
    Loop { collection: "tasks", path: ".parent", next: "JOIN tasks AS n ON n.id = m.parent" },
    Loop { collection: "tasks", path: ".subtasks", next: "JOIN tasks AS n ON n.parent = m.id" },
    Loop {
        collection: "tasks",
        path: ".blocked_by.blocker",
        next: "JOIN links AS l ON l.blocked = m.id JOIN tasks AS n ON n.id = l.blocker",
    },
    Loop {
        collection: "tasks",
        path: ".blocks.blocked",
        next: "JOIN links AS l ON l.blocker = m.id JOIN tasks AS n ON n.id = l.blocked",
    },
];

/// An id some row of the collection has.
fn some_id(random: &mut Random, tables: &[Table], collection: &str) -> Option<String> {
    let table = tables.iter().find(|table| table.name == collection)?;
    let row = &table.rows[random.below(table.rows.len())];
    match row.iter().find(|(name, _)| *name == "id") {
        Some((_, Cell::Text(id))) => Some(id.clone()),
        _ => None,
    }
}

/// Every row a walk reaches, each once for the row it started from: as a
/// capture in a block, or as a chain that names the collection it began at.
fn walked(random: &mut Random, tables: &[Table]) -> Option<Case> {
    let walk = random.pick(LOOPS);
    let root = walk.collection;
    // From a few rows, since every row a walk can reach from every row is
    // a great many.
    let ids: Vec<String> = (0..1 + random.below(3)).filter_map(|_| some_id(random, tables, root)).collect();
    let listed = |quote: char| {
        ids.iter().map(|id| format!("{quote}{id}{quote}")).collect::<Vec<_>>().join(", ")
    };
    let starts = format!("id in [{}]", listed('"'));
    let field = fields_of(root).first()?.name;
    let oql = match random.below(3) {
        0 => format!(
            "{root} {{\n  where {{ {starts} }}\n  far in .walk({})\n  select {{ id, of: {field}, far: far.id }}\n}}",
            walk.path
        ),
        1 => format!(
            "{root} {{\n  where {{ {starts} }}\n  near in {}\n  far in near.walk({})\n  select {{ id, of: {root}.{field}, far: far.id }}\n}}",
            walk.path, walk.path
        ),
        _ => format!(
            "{root}.where {{ .{starts} }}.walk({}).select {{ id: {root}.id, of: {root}.{field}, far: id }}",
            walk.path
        ),
    };
    // The second form starts its walk one step out, once for each row at
    // that step, so a row reached by two of them is given twice.
    let from_near = oql.contains("near in");
    let sql = match from_near {
        false => format!(
            "WITH RECURSIVE reach(start, id) AS (\n  SELECT m.id, n.id FROM {root} AS m {next} WHERE m.id IN ({ids})\n  UNION\n  SELECT reach.start, n.id FROM reach JOIN {root} AS m ON m.id = reach.id {next}\n)\nSELECT r0.id AS id, r0.{field} AS \"of\", reach.id AS far\nFROM reach JOIN {root} AS r0 ON r0.id = reach.start",
            next = walk.next,
            ids = listed('\''),
        ),
        true => format!(
            "WITH RECURSIVE near(root, id, line) AS (\n  SELECT m.id, n.id, row_number() OVER () FROM {root} AS m {next} WHERE m.id IN ({ids})\n),\nreach(start, id) AS (\n  SELECT m.id, n.id FROM {root} AS m {next} WHERE m.id IN (SELECT id FROM near)\n  UNION\n  SELECT reach.start, n.id FROM reach JOIN {root} AS m ON m.id = reach.id {next}\n)\nSELECT r0.id AS id, r0.{field} AS \"of\", reach.id AS far\nFROM near JOIN reach ON reach.start = near.id JOIN {root} AS r0 ON r0.id = near.root",
            next = walk.next,
            ids = listed('\''),
        ),
    };
    Some(Case { shape: "rows of a walk", ordered: false, then: None, oql, sql })
}

/// The block's row named inside a query inside a query.
fn two_deep(random: &mut Random, _tables: &[Table]) -> Option<Case> {
    let root = *random.pick(&["people", "projects", "units"]);
    let fields = fields_of(root);
    let field = *random.pick(&fields);
    let middle: Vec<&Field> = FIELDS.iter().filter(|other| other.collection != root).collect();
    let middle = *random.pick(&middle);
    let inner: Vec<&Field> = FIELDS
        .iter()
        .filter(|other| other.kind == field.kind && other.collection != root && other.collection != middle.collection)
        .collect();
    if inner.is_empty() {
        return None;
    }
    let inner = *random.pick(&inner);
    let (operator, sql_operator) = match field.kind {
        Kind::Text => ("==", "="),
        _ => (*random.pick(&["<", ">"]), ""),
    };
    let sql_operator = if sql_operator.is_empty() { operator } else { sql_operator };
    // Written by the collection's name, or as a name given with `=`.
    let (given, named) = match random.chance(50) {
        true => (String::new(), format!("{root}.{}", field.name)),
        false => (format!("  n = .{}\n", field.name), "n".to_string()),
    };
    Some(Case {
        shape: "name read two queries deep",
        ordered: false,
        then: None,
        oql: format!(
            "{root} {{\n{given}  select {{ id, k: {}.where {{ {}.where {{ .{} {operator} {named} }}.count() > 2 }}.count() }}\n}}",
            middle.collection, inner.collection, inner.name
        ),
        sql: format!(
            "SELECT r0.id AS id, (SELECT count(*) FROM {} AS s WHERE (SELECT count(*) FROM {} AS t WHERE t.{} {sql_operator} r0.{}) > 2) AS k\nFROM {root} AS r0",
            middle.collection, inner.collection, inner.name, field.name
        ),
    })
}

/// The same, with the middle query a path from the block's row, or a query
/// on the block's own collection: there the name means the block's row in
/// the middle query and the middle row in the query inside it.
fn two_deep_from_a_row(random: &mut Random, _tables: &[Table]) -> Option<Case> {
    let root = *random.pick(&["people", "projects", "units", "tasks"]);
    let fields = fields_of(root);
    let field = *random.pick(&fields);
    let backs: Vec<&Edge> = edges_from(root).into_iter().filter(|edge| !edge.forward).collect();
    if backs.is_empty() {
        return None;
    }
    let edge = *random.pick(&backs);
    let (operator, sql_operator) = match field.kind {
        Kind::Text => ("==", "="),
        _ => (">", ">"),
    };
    match random.chance(50) {
        // `.assignments.where { units.where { .budget > people.salary } .. }`
        true => {
            let inner: Vec<&Field> = FIELDS
                .iter()
                .filter(|other| other.kind == field.kind && other.collection != root && other.collection != edge.to)
                .collect();
            if inner.is_empty() {
                return None;
            }
            let inner = *random.pick(&inner);
            Some(Case {
                shape: "name read two deep, through a path",
                ordered: false,
                then: None,
                oql: format!(
                    "{root} {{\n  select {{ id, k: .{}.where {{ {}.where {{ .{} {operator} {root}.{} }}.count() > 2 }}.count() }}\n}}",
                    edge.step, inner.collection, inner.name, field.name
                ),
                sql: format!(
                    "SELECT r0.id AS id, (SELECT count(*) FROM {} AS s WHERE s.{} = r0.id AND (SELECT count(*) FROM {} AS t WHERE t.{} {sql_operator} r0.{}) > 2) AS k\nFROM {root} AS r0",
                    edge.to, edge.column, inner.collection, inner.name, field.name
                ),
            })
        }
        // `people.where { .level > people.level && projects.where { .owner == people.id } .. }`
        false => {
            if edge.to == root {
                return None;
            }
            Some(Case {
                shape: "name taken by a nearer row",
                ordered: false,
                then: None,
                oql: format!(
                    "{root} {{\n  select {{ id, k: {root}.where {{ .{0} {operator} {root}.{0} && {1}.where {{ .{2} == {root}.id }}.count() > 0 }}.count() }}\n}}",
                    field.name, edge.to, edge.column
                ),
                sql: format!(
                    "SELECT r0.id AS id, (SELECT count(*) FROM {root} AS m WHERE m.{0} {sql_operator} r0.{0} AND (SELECT count(*) FROM {1} AS t WHERE t.{2} = m.id) > 0) AS k\nFROM {root} AS r0",
                    field.name, edge.to, edge.column
                ),
            })
        }
    }
}

/// A name for a collection of the block's row, counted, filtered, and read
/// inside a nested query.
fn named_collection(random: &mut Random, tables: &[Table]) -> Option<Case> {
    let root = *random.pick(ROOTS);
    let edges: Vec<&Edge> = edges_from(root).into_iter().filter(|edge| !edge.forward).collect();
    if edges.is_empty() {
        return None;
    }
    let edge = *random.pick(&edges);
    let fields = fields_of(edge.to);
    let field = *random.pick(&fields);
    let value = literal(random, tables, field)?;
    let (operator, sql_operator) = match field.kind {
        Kind::Text => ("==", "="),
        _ => (">", ">"),
    };
    let asked: Vec<&Field> = FIELDS.iter().filter(|other| other.kind == field.kind).collect();
    let asked = *random.pick(&asked);
    let mine = format!("{} AS m WHERE m.{} = r0.id", edge.to, edge.column);
    Some(Case {
        shape: "name for a collection",
        ordered: false,
        then: None,
        oql: format!(
            "{root} {{\n  mine = .{}\n  select {{\n    id,\n    all: mine.count(),\n    some: mine.where {{ .{} {operator} {value} }}.count(),\n    like: {}.where {{ .{} in mine.{} }}.count()\n  }}\n}}",
            edge.step, field.name, asked.collection, asked.name, field.name
        ),
        sql: format!(
            "SELECT r0.id AS id,\n  (SELECT count(*) FROM {mine}) AS \"all\",\n  (SELECT count(*) FROM {mine} AND m.{} {sql_operator} {}) AS \"some\",\n  (SELECT count(*) FROM {} AS s WHERE s.{} IN (SELECT m.{} FROM {mine})) AS \"like\"\nFROM {root} AS r0",
            field.name,
            sql_literal(&value),
            asked.collection,
            asked.name,
            field.name
        ),
    })
}

/// Captures, then a grouping or a sort and a limit over the combinations.
fn after_captures(random: &mut Random, tables: &[Table]) -> Option<Case> {
    let root = *random.pick(ROOTS);
    let first = *random.pick(&edges_from(root));
    let seconds = edges_from(first.to);
    let second = *random.pick(&seconds);
    let filter = match random.chance(50) {
        false => None,
        true => {
            let field = *fields_of(root).first()?;
            let value = literal(random, tables, field)?;
            let (operator, sql_operator) = match field.kind {
                Kind::Text => ("==", "="),
                _ => (">", ">"),
            };
            Some((format!("  where {{ {} {operator} {value} }}\n", field.name), format!("\nWHERE r0.{} {sql_operator} {}", field.name, sql_literal(&value))))
        }
    };
    let (filter, filter_sql) = filter.unwrap_or_default();
    let joins = format!(
        "\n  JOIN {} AS a ON {}\n  JOIN {} AS b ON {}",
        first.to,
        first.joined("r0", "a"),
        second.to,
        second.joined("a", "b")
    );
    let key = *fields_of(second.to).first()?;
    match random.chance(50) {
        true => Some(Case {
            shape: "grouping after captures",
            ordered: false,
            then: None,
            oql: format!(
                "{root} {{\n  a in .{}\n  b in a.{}\n{filter}  group_by {{ b.{} }}\n  select {{ {}, rows: count() }}\n}}",
                first.step, second.step, key.name, key.name
            ),
            sql: format!(
                "SELECT b.{0} AS {0}, count(*) AS rows\nFROM {root} AS r0{joins}{filter_sql}\nGROUP BY b.{0}",
                key.name
            ),
        }),
        false => {
            let keep = 1 + random.below(40);
            Some(Case {
                shape: "sort and limit after captures",
                ordered: false,
                then: None,
                oql: format!(
                    "{root} {{\n  a in .{}\n  b in a.{}\n{filter}  sort {{ b.id.desc, a.id.asc, id.asc }}\n  limit({keep})\n  select {{ id, a: a.id, b: b.id }}\n}}",
                    first.step, second.step
                ),
                sql: format!(
                    "SELECT DISTINCT r0.id AS id, a.id AS a, b.id AS b\nFROM {root} AS r0{joins}{filter_sql}\nORDER BY b.id DESC, a.id ASC, r0.id ASC\nLIMIT {keep}"
                ),
            })
        }
    }
}

/// A filtered read, and a sorted one, in the order the language gives them:
/// a filter keeps the order of a scan, which is by id, whether or not an
/// index serves it; a sort keeps that order among rows it finds equal.
fn in_order(random: &mut Random, tables: &[Table]) -> Option<Case> {
    let root = *random.pick(ROOTS);
    let fields = fields_of(root);
    let mut chain = root.to_string();
    let mut filters: Vec<String> = Vec::new();
    for _ in 0..random.below(3) {
        let field = *random.pick(&fields);
        let value = literal(random, tables, field)?;
        let (operator, sql_operator) = match field.kind {
            Kind::Text => ("==", "="),
            _ => *random.pick(&[("<", "<"), (">", ">"), ("==", "=")]),
        };
        chain.push_str(&format!(".where {{ .{} {operator} {value} }}", field.name));
        filters.push(format!("r0.{} {sql_operator} {}", field.name, sql_literal(&value)));
    }
    let shown = *random.pick(&fields);
    let mut order = "r0.id".to_string();
    let sorted = random.chance(40);
    if sorted {
        let by = *random.pick(&fields);
        let (direction, sql_direction) = *random.pick(&[("asc", "ASC"), ("desc", "DESC")]);
        chain.push_str(&format!(".sort {{ .{}.{direction} }}", by.name));
        order = format!("r0.{} {sql_direction}, r0.id", by.name);
    }
    let mut taken = String::new();
    if random.chance(50) {
        let keep = 1 + random.below(30);
        chain.push_str(&format!(".limit({keep})"));
        taken = format!("\nLIMIT {keep}");
    }
    chain.push_str(&format!(".select {{ id, shown: .{} }}", shown.name));
    let filter = match filters.is_empty() {
        true => String::new(),
        false => format!("\nWHERE {}", filters.join(" AND ")),
    };
    Some(Case {
        shape: if sorted { "sorted read, in order" } else { "filtered read, in order" },
        ordered: true,
        then: None,
        oql: chain,
        sql: format!("SELECT r0.id AS id, r0.{} AS shown\nFROM {root} AS r0{filter}\nORDER BY {order}{taken}", shown.name),
    })
}

/// A list of text a row holds, and the SQLite table that holds it.
const TEXT_LISTS: &[(&str, &str)] = &[("people", "skills"), ("projects", "tags"), ("tasks", "labels")];

/// The members of a list a row holds, named one by one beside the row.
fn list_members(random: &mut Random, tables: &[Table]) -> Option<Case> {
    if random.chance(40) {
        // A list of objects: each task's work log, whose entries hold a
        // reference of their own.
        let least = random.below(120);
        let condition = format!("minutes > {least}");
        let (oql, shape) = match random.below(3) {
            0 => (
                format!(
                    "tasks {{\n  w in .worklog\n  where {{ w.{condition} }}\n  select {{ id, minutes: w.minutes, state: w.state, level: w.author.level }}\n}}"
                ),
                "members of a list of objects",
            ),
            1 => (
                format!(
                    "tasks {{\n  w? in .worklog.where {{ .{condition} }}\n  select {{ id, minutes: w.minutes, state: w.state, level: w.author.level }}\n}}"
                ),
                "members of a list of objects, or none",
            ),
            _ => (
                format!(
                    "tasks.worklog.where {{ .{condition} }}.select {{ id: tasks.id, minutes, state, level: .author.level }}"
                ),
                "row a list member came from",
            ),
        };
        let join = if shape.ends_with("or none") { "LEFT JOIN" } else { "JOIN" };
        let (on, filter) = match shape.ends_with("or none") {
            true => (format!(" AND w.{condition}"), String::new()),
            false => (String::new(), format!("\nWHERE w.{condition}")),
        };
        return Some(Case {
            shape,
            ordered: false,
            then: None,
            oql,
            sql: format!(
                "SELECT r0.id AS id, w.minutes AS minutes, w.state AS state, p.level AS level\nFROM tasks AS r0\n  {join} tasks_worklog AS w ON w.parent = r0.id{on}\n  LEFT JOIN people AS p ON p.id = w.author{filter}"
            ),
        });
    }
    let (root, list) = *random.pick(TEXT_LISTS);
    let table = tables.iter().find(|table| table.name == root)?;
    let value = (0..20).find_map(|_| {
        let row = &table.rows[random.below(table.rows.len())];
        match row.iter().find(|(name, _)| *name == list) {
            Some((_, Cell::Texts(values))) if !values.is_empty() => Some(values[random.below(values.len())].clone()),
            _ => None,
        }
    })?;
    let field = *fields_of(root).first()?;
    let (entry, join, filter) = match random.below(3) {
        0 => (format!("  x in .{list}"), "JOIN", String::new()),
        1 => (format!("  x? in .{list}"), "LEFT JOIN", String::new()),
        _ => (
            format!("  x in .{list}\n  where {{ x == \"{value}\" }}"),
            "JOIN",
            format!("\nWHERE l.value = '{value}'"),
        ),
    };
    Some(Case {
        shape: "members of a list of text",
        ordered: false,
        then: None,
        oql: format!("{root} {{\n{entry}\n  select {{ id, of: {}, x }}\n}}", field.name),
        sql: format!(
            "SELECT r0.id AS id, r0.{} AS \"of\", l.value AS x\nFROM {root} AS r0\n  {join} {root}_{list} AS l ON l.parent = r0.id{filter}",
            field.name
        ),
    })
}

/// A row's place among the rows of its partition, and a total that runs.
fn placed(random: &mut Random, _tables: &[Table]) -> Option<Case> {
    let (root, by, measure) = *random.pick(&[
        ("people", "unit", "level"),
        ("tasks", "project", "points"),
        ("assignments", "person", "hours"),
        ("assignments", "project", "hours"),
        ("events", "person", "amount"),
    ]);
    let (direction, sql_direction) = *random.pick(&[("asc", "ASC"), ("desc", "DESC")]);
    let window = format!("PARTITION BY r0.{by} ORDER BY r0.{measure} {sql_direction}, r0.id");
    let inner = format!(
        "SELECT r0.id AS id, row_number() OVER ({window}) AS place, sum(r0.{measure}) OVER ({window} ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running\nFROM {root} AS r0"
    );
    let chain = format!(
        "{root}\n  .sort {{ .{measure}.{direction}, .id.asc }}\n  .partition_by {{ .{by} }}\n  .select {{ id, place: row_number(), running: so_far().{measure}.sum() }}"
    );
    let keep = 1 + random.below(3);
    match random.chance(50) {
        true => Some(Case { shape: "place in a partition", ordered: false, then: None, oql: chain, sql: inner }),
        false => Some(Case {
            shape: "first places in a partition",
            ordered: false,
            then: None,
            oql: format!("{chain}\n  .where {{ .place <= {keep} }}"),
            sql: format!("SELECT * FROM (\n{inner}\n) WHERE place <= {keep}"),
        }),
    }
}

/// A write, made on both engines, and what the collection holds after it.
/// A row is updated through a filter on itself or on a row it refers to;
/// rows nothing refers to are deleted.
fn written(random: &mut Random, tables: &[Table]) -> Option<Case> {
    let (root, measure) = *random.pick(&[("people", "level"), ("tasks", "points"), ("assignments", "hours"), ("events", "amount")]);
    let fields = fields_of(root);
    // Which rows: by a field of the row, or of a row it refers to.
    let (filter, filter_sql) = match random.chance(60) {
        true => {
            let field = *random.pick(&fields);
            let value = literal(random, tables, field)?;
            let operator = match field.kind {
                Kind::Text => "==",
                _ => *random.pick(&["<", ">", "=="]),
            };
            (
                format!(".{} {operator} {value}", field.name),
                format!("{root}.{} {} {}", field.name, sql_operator(operator), sql_literal(&value)),
            )
        }
        false => {
            let edges: Vec<&Edge> = edges_from(root).into_iter().filter(|edge| edge.forward && !edge.optional).collect();
            let edge = *random.pick(&edges);
            let far = *random.pick(&fields_of(edge.to));
            let value = literal(random, tables, far)?;
            let operator = match far.kind {
                Kind::Text => "==",
                _ => *random.pick(&["<", ">"]),
            };
            (
                format!(".{}.{} {operator} {value}", edge.step, far.name),
                format!(
                    "(SELECT f.{} FROM {} AS f WHERE f.id = {root}.{}) {} {}",
                    far.name,
                    edge.to,
                    edge.column,
                    sql_operator(operator),
                    sql_literal(&value)
                ),
            )
        }
    };
    let then = Some((
        format!("{root}.select {{ id, {measure} }}"),
        format!("SELECT id, {measure} FROM {root} ORDER BY id"),
    ));
    // Nothing refers to an event, an assignment or a link, so those may go.
    let removable = matches!(root, "events" | "assignments");
    match removable && random.chance(30) {
        true => Some(Case {
            shape: "rows deleted",
            ordered: true,
            then,
            oql: format!("{root}.where {{ {filter} && .{measure} > 100000 || {filter} && .id > \"zz\" }}.delete()"),
            sql: format!("DELETE FROM {root} WHERE ({filter_sql} AND {root}.{measure} > 100000) OR ({filter_sql} AND {root}.id > 'zz')"),
        }),
        false => {
            let by = 1 + random.below(3);
            let (set, set_sql) = match random.chance(50) {
                true => (format!("{measure}: .{measure} + {by}"), format!("{measure} = {measure} + {by}")),
                false => (format!("{measure} += {by}"), format!("{measure} = {measure} + {by}")),
            };
            Some(Case {
                shape: "rows updated",
                ordered: true,
                then,
                oql: format!("{root}.where {{ {filter} }}.update {{ {set} }}"),
                sql: format!("UPDATE {root} SET {set_sql} WHERE {filter_sql}"),
            })
        }
    }
}

/// The cases of one seed.
pub fn cases(seed: u64, count: usize, tables: &[Table]) -> Vec<Case> {
    let mut random = Random(seed);
    let mut made = Vec::with_capacity(count);
    let mut tries = 0;
    while made.len() < count && tries < count * 20 {
        tries += 1;
        let case = match random.below(28) {
            18..=20 => in_order(&mut random, tables),
            21 | 22 => list_members(&mut random, tables),
            23 | 24 => placed(&mut random, tables),
            25..=27 => written(&mut random, tables),
            0..=5 => block(&mut random, tables),
            6 => named_value(&mut random, tables),
            7 => capture_in_nested(&mut random, tables),
            8 | 9 => anchor_after_steps(&mut random, tables),
            10 | 11 => walked(&mut random, tables),
            12 => two_deep(&mut random, tables),
            13 => named_collection(&mut random, tables),
            14 | 15 => two_deep_from_a_row(&mut random, tables),
            _ => after_captures(&mut random, tables),
        };
        made.extend(case);
    }
    made
}

/// Rows in an order that does not depend on the engine.
fn sorted(mut rows: Vec<Json>) -> Vec<Json> {
    rows.sort_by_cached_key(|row| row.to_string());
    rows
}

pub enum Verdict {
    Same(usize),
    /// Owl could not run it.
    Failed(String),
    /// Owl and SQLite disagree.
    Differs(String),
    /// Owl disagrees with itself: arranged against as written.
    Rearranged(String),
}

pub fn check(loaded: &Loaded, case: &Case) -> Result<Verdict> {
    if let Some((read, read_sql)) = &case.then {
        // The write on each engine, then the same read of both.
        loaded.sqlite.query(&case.sql).map_err(|error| anyhow::anyhow!("{error}\n{}", case.sql))?;
        if let Err(error) = loaded.owl.query(&case.oql) {
            return Ok(Verdict::Failed(error.to_string()));
        }
        let sqlite = loaded.sqlite.query(read_sql).map_err(|error| anyhow::anyhow!("{error}\n{read_sql}"))?;
        let owl = match loaded.owl.query(read) {
            Ok(rows) => rows,
            Err(error) => return Ok(Verdict::Failed(format!("reading after the write: {error}"))),
        };
        return Ok(match first_difference(&owl, &sqlite) {
            None => Verdict::Same(owl.len()),
            Some(difference) => Verdict::Differs(format!("after the write: {difference}")),
        });
    }
    let sqlite = loaded.sqlite.query(&case.sql).map_err(|error| anyhow::anyhow!("{error}\n{}", case.sql))?;
    owl_db::executor::set_capture_planning(true);
    let arranged = match loaded.owl.query(&case.oql) {
        Ok(rows) => rows,
        Err(error) => return Ok(Verdict::Failed(error.to_string())),
    };
    owl_db::executor::set_capture_planning(false);
    let written = loaded.owl.query(&case.oql);
    owl_db::executor::set_capture_planning(true);
    let written = match written {
        Ok(rows) => rows,
        Err(error) => return Ok(Verdict::Failed(format!("as written: {error}"))),
    };
    if let Some(difference) = first_difference(&arranged, &written) {
        return Ok(Verdict::Rearranged(difference));
    }
    let rows = arranged.len();
    if case.ordered {
        return Ok(match first_difference(&arranged, &sqlite) {
            None => Verdict::Same(rows),
            Some(difference) => Verdict::Differs(format!("in order: {difference}")),
        });
    }
    Ok(match first_difference(&sorted(arranged), &sorted(sqlite)) {
        None => Verdict::Same(rows),
        Some(difference) => Verdict::Differs(difference),
    })
}
