//! What a program that writes OQL needs from the database, as data.
//!
//! A person reads a schema file and an error message. A program that
//! writes queries, a language model above all, needs the same three
//! things in a form it can act on:
//!
//! - [`catalogue`]: what is in the database. Every collection, its fields
//!   and their types, which field refers to which collection and by what
//!   name the rows that refer to it are reached, the indexes, how many
//!   rows, and the functions that can be called.
//! - [`check`]: whether a statement is one the database would accept,
//!   without running it.
//! - [`Problem`]: what was wrong when it is not: a code to branch on, the
//!   message a person would read, where in the text, the name that was
//!   probably meant, and the names that do exist where one did not.

use crate::error::OQLError;
use crate::executor::QueryExecutor;
use crate::schema::{FieldType, Schema};
use crate::storage::DocumentStore;
use serde::Serialize;
use std::collections::BTreeMap;

/// Why a statement was not accepted, or did not finish.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    /// What kind of problem, for a program to branch on:
    ///
    /// | Code | The statement |
    /// |---|---|
    /// | `parse_error` | is not OQL |
    /// | `unknown_collection` | names a collection that is not there |
    /// | `unknown_name` | reads a field or relationship that is not there |
    /// | `unknown_function` | calls a function that is not defined |
    /// | `type_mismatch` | compares kinds that could never be equal |
    /// | `read_only` | would write, where only reads are taken |
    /// | `over_budget` | was stopped at its budget of time or rows |
    /// | `rejected` | was refused for another reason; see the message |
    pub code: &'static str,
    /// What a person would be told.
    pub message: String,
    /// Where the statement stops being OQL, counted from one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    /// The name that was probably meant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// For each collection the message names: the fields and
    /// relationships it does have.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, Vec<String>>,
}

impl Problem {
    /// A problem with a code and a message and nothing else known.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            line: None,
            column: None,
            suggestion: None,
            names: BTreeMap::new(),
        }
    }

    /// The problem a statement's text has, when it does not parse.
    pub fn of_parse(text: &str, message: impl Into<String>) -> Self {
        let mut problem = Self::new("parse_error", message);
        if let Some((line, column)) = crate::parser::parse_error_position(text) {
            problem.line = Some(line);
            problem.column = Some(column);
        }
        problem
    }

    /// The problem an error of the engine's is, with what `catalogue`
    /// knows about the names in it.
    pub fn of_error(error: &OQLError, catalogue: &Catalogue) -> Self {
        let message = match error {
            // The kind of error is the code; the text is the message.
            OQLError::SchemaValidation { message }
            | OQLError::QueryExecution { message }
            | OQLError::Generic { message }
            | OQLError::BudgetExceeded { message }
            | OQLError::FunctionError { message }
            | OQLError::ParseError { message } => message.clone(),
            other => other.to_string(),
        };
        let code = match error {
            OQLError::BudgetExceeded { .. } => "over_budget",
            _ if message.contains("is not defined: no collection") => "unknown_collection",
            _ if message.contains("Unknown function") || message.contains("cannot be used:") => {
                "unknown_function"
            }
            _ if message.contains("' has no ") || message.contains("has no field") => "unknown_name",
            _ if message.contains("could never hold") => "type_mismatch",
            _ if message.contains("read-only") => "read_only",
            _ => "rejected",
        };
        let mut problem = Self::new(code, message);
        problem.suggestion = quoted_after(&problem.message, "Did you mean ");
        if matches!(code, "unknown_name" | "unknown_collection") {
            for word in quoted(&problem.message) {
                if let Some(collection) = catalogue.collection(&word) {
                    problem.names.insert(word, collection.names());
                }
            }
        }
        if code == "unknown_collection" {
            problem.names.insert(
                "collections".to_string(),
                catalogue.collections.iter().map(|collection| collection.name.clone()).collect(),
            );
        }
        problem
    }
}

/// Every `'word'` in a message.
fn quoted(message: &str) -> Vec<String> {
    message.split('\'').skip(1).step_by(2).map(str::to_string).collect()
}

/// The `'word'` that follows `lead` in a message.
fn quoted_after(message: &str, lead: &str) -> Option<String> {
    let rest = &message[message.find(lead)? + lead.len()..];
    let rest = rest.strip_prefix('\'')?;
    Some(rest[..rest.find('\'')?].to_string())
}

/// What is known of a statement that passed the check.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Checked {
    /// The collection the statement starts from.
    pub collection: String,
    /// Whether running it would change the database.
    pub writes: bool,
    /// The `$name` parameters it needs values for.
    pub parameters: Vec<String>,
}

/// Check one statement without running it.
///
/// It is parsed, and then given the checks every statement gets before it
/// runs: its collection, the names it reads, the functions it calls, and
/// comparisons between kinds that could never be equal. Nothing is read
/// and nothing is written, so the cost does not depend on the data.
///
/// A statement that passes can still fail when run, on a value: a division
/// by zero, a constraint, a budget.
pub fn check<S: DocumentStore>(executor: &QueryExecutor<S>, text: &str) -> Result<Checked, Problem> {
    let query = crate::parser::parse_oql(text).map_err(|error| Problem::of_parse(text, error.to_string()))?;
    executor.check_query(&query).map_err(|error| {
        // Only a refusal pays for the catalogue.
        let known = catalogue(executor).unwrap_or_default();
        Problem::of_error(&error, &known)
    })?;
    Ok(Checked {
        collection: query.collection.clone(),
        writes: QueryExecutor::<S>::writes(&query),
        parameters: crate::parser::query_parameters(&query).into_iter().collect(),
    })
}

/// What is in a database.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Catalogue {
    pub collections: Vec<Collection>,
    /// Functions written in OQL and kept in this database.
    pub functions: Vec<Function>,
    /// Functions every database has: the engine's own and the standard
    /// library's. Names only; they are the same everywhere.
    pub builtin: Vec<String>,
}

/// One collection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Collection {
    pub name: String,
    /// How many rows it holds, where the store can say without reading
    /// them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
    /// Its declared fields, `id` first and the rest by name.
    pub fields: Vec<Field>,
    /// The rows of other collections that refer to a row of this one,
    /// and the name each set is reached by.
    pub relationships: Vec<Relationship>,
    pub indexes: Vec<Index>,
}

impl Collection {
    /// Every name that can follow a `.` on a row of this collection.
    pub fn names(&self) -> Vec<String> {
        self.fields
            .iter()
            .map(|field| field.name.clone())
            .chain(self.relationships.iter().map(|relationship| relationship.name.clone()))
            .collect()
    }
}

/// One field of a collection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Field {
    pub name: String,
    /// The type as a schema declares it: `String`, `Optional(Ref(people))`,
    /// `List({ kind: String, mins: Integer })`.
    #[serde(rename = "type")]
    pub declared: String,
    /// The collection a reference, or each reference in a list, points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refers_to: Option<String>,
}

/// The rows of `from` whose field `by` refers to a row of this collection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Relationship {
    pub name: String,
    pub from: String,
    pub by: String,
}

/// An index on a collection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Index {
    pub fields: Vec<String>,
    /// `unique`, `ordered`, or the metric of an index for `nearest`.
    pub kind: String,
}

/// A function written in OQL.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Function {
    pub name: String,
    /// As it is called: `discount(price, rate: Float)`.
    pub signature: String,
}

/// Read the catalogue of the database `executor` is on. It reads schemas,
/// index definitions and row counts, and no rows.
pub fn catalogue<S: DocumentStore>(executor: &QueryExecutor<S>) -> Result<Catalogue, OQLError> {
    let store = executor.store();
    let mut names = store.list_schemas()?;
    names.retain(|name| !name.starts_with('$') && !name.starts_with('_'));
    names.sort();
    let mut schemas = Vec::new();
    for name in &names {
        if let Some(schema) = store.get_schema(name)? {
            schemas.push(schema);
        }
    }
    let scalar = store.list_scalar_indexes(None)?;
    let vector = store.list_vector_indexes(None)?;

    let mut collections = Vec::new();
    for schema in &schemas {
        let mut fields: Vec<Field> = schema
            .fields
            .iter()
            .map(|(name, declared)| Field {
                name: name.clone(),
                declared: declared_type(schema, name, declared),
                refers_to: target_of(declared).map(str::to_string),
            })
            .collect();
        fields.sort_by(|left, right| (left.name != "id", &left.name).cmp(&(right.name != "id", &right.name)));

        // Named with `as` where the reference was declared; otherwise the
        // referring collection's own name, when that is not a field here.
        let mut relationships = Vec::new();
        for source in &schemas {
            let mut referring: Vec<(&String, &FieldType)> = source
                .fields
                .iter()
                .filter(|(_, declared)| target_of(declared) == Some(schema.name.as_str()))
                .collect();
            referring.sort_by_key(|(field, _)| field.as_str());
            let mut by_collection_name = false;
            for (field, _) in &referring {
                match source.reverse_name(field) {
                    Some(name) => relationships.push(Relationship {
                        name: name.to_string(),
                        from: source.name.clone(),
                        by: (*field).clone(),
                    }),
                    None => by_collection_name = true,
                }
            }
            if by_collection_name && !schema.fields.contains_key(&source.name) {
                let by: Vec<&str> = referring
                    .iter()
                    .filter(|(field, _)| source.reverse_name(field).is_none())
                    .map(|(field, _)| field.as_str())
                    .collect();
                relationships.push(Relationship {
                    name: source.name.clone(),
                    from: source.name.clone(),
                    by: by.join(" or "),
                });
            }
        }
        relationships.sort_by(|left, right| left.name.cmp(&right.name));

        let mut indexes: Vec<Index> = scalar
            .iter()
            .filter(|index| index.schema == schema.name)
            .map(|index| Index {
                fields: index.fields.clone(),
                kind: if index.unique { "unique" } else { "ordered" }.to_string(),
            })
            .chain(vector.iter().filter(|index| index.schema == schema.name).map(|index| Index {
                fields: vec![index.field.clone()],
                kind: format!("{:?}", index.metric).to_lowercase(),
            }))
            .collect();
        indexes.sort_by(|left, right| left.fields.cmp(&right.fields));

        collections.push(Collection {
            name: schema.name.clone(),
            rows: store.collection_len(&schema.name)?,
            fields,
            relationships,
            indexes,
        });
    }

    let registry = executor.functions();
    let mut functions = Vec::new();
    let mut builtin = registry.builtin_names();
    for function in registry.oql_functions() {
        if function.library {
            builtin.push(function.definition.name.clone());
            continue;
        }
        let parameters: Vec<String> = function
            .definition
            .parameters
            .iter()
            .map(|parameter| match parameter.param_type.as_str() {
                "Any" => parameter.name.clone(),
                declared => format!("{}: {declared}", parameter.name),
            })
            .collect();
        functions.push(Function {
            name: function.definition.name.clone(),
            signature: format!("{}({})", function.definition.name, parameters.join(", ")),
        });
    }
    builtin.sort();
    builtin.dedup();
    Ok(Catalogue { collections, functions, builtin })
}

/// The collection a field refers to, through `Optional` and `List`.
fn target_of(declared: &FieldType) -> Option<&str> {
    match declared {
        FieldType::Reference(reference) => Some(&reference.target_schema),
        FieldType::Optional(inner) | FieldType::List(inner) => target_of(inner),
        _ => None,
    }
}

/// A field's type as a schema would declare it, with the shape of an
/// embedded object where one was declared.
fn declared_type(schema: &Schema, path: &str, declared: &FieldType) -> String {
    match declared {
        FieldType::Optional(inner) => format!("Optional({})", declared_type(schema, path, inner)),
        FieldType::List(inner) => format!("List({})", declared_type(schema, &format!("{path}[]"), inner)),
        FieldType::Reference(reference) => format!("Ref({})", reference.target_schema),
        FieldType::Vector { dim } => format!("Vector({dim})"),
        FieldType::Object => match schema.shape(path) {
            Some(fields) => {
                let inside: Vec<String> = fields
                    .iter()
                    .map(|(name, field)| format!("{name}: {}", declared_type(schema, &format!("{path}.{name}"), field)))
                    .collect();
                format!("{{ {} }}", inside.join(", "))
            }
            None => "Object".to_string(),
        },
        other => other.type_name().to_string(),
    }
}

impl Catalogue {
    /// The collection of that name.
    pub fn collection(&self, name: &str) -> Option<&Collection> {
        self.collections.iter().find(|collection| collection.name == name)
    }

    /// The catalogue as text short enough to sit in a prompt: a schema for
    /// each collection, with what a schema does not say written as
    /// comments.
    pub fn as_text(&self) -> String {
        let mut text = String::new();
        for collection in &self.collections {
            let rows = match collection.rows {
                Some(1) => "  // 1 row".to_string(),
                Some(rows) => format!("  // {rows} rows"),
                None => String::new(),
            };
            text.push_str(&format!("schema {} {{{rows}\n", collection.name));
            for field in &collection.fields {
                text.push_str(&format!("  {}: {}\n", field.name, field.declared));
            }
            for relationship in &collection.relationships {
                text.push_str(&format!(
                    "  // .{}: the {} whose .{} is this row\n",
                    relationship.name, relationship.from, relationship.by
                ));
            }
            for index in &collection.indexes {
                text.push_str(&format!("  // index, {}: {}\n", index.kind, index.fields.join(", ")));
            }
            text.push_str("}\n");
        }
        if !self.functions.is_empty() {
            let signatures: Vec<&str> = self.functions.iter().map(|function| function.signature.as_str()).collect();
            text.push_str(&format!("// functions defined here: {}\n", signatures.join(", ")));
        }
        text
    }
}
