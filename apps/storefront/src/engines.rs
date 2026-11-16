//! The two engines behind one small interface: load a dataset, run a query,
//! return rows as JSON so results can be compared.

use crate::data::Dataset;
use anyhow::{anyhow, Context, Result};
use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, QueryExecutor, Value};
use rusqlite::types::ValueRef;
use serde_json::{json, Map, Value as Json};
use std::path::Path;
use std::sync::Arc;

/// Rows written per durable commit, for both engines.
pub const BATCH: usize = 500;

pub struct Owl {
    store: Arc<RustStore>,
    executor: QueryExecutor<RustStore>,
}

impl Owl {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, include_str!("../schema.oql"))
    }

    /// Open a database and declare `schema`, a script of schema statements.
    pub fn open_with(path: &Path, schema: &str) -> Result<Self> {
        let store = Arc::new(
            RustStore::new(path, 64 << 30).map_err(|error| anyhow!("open Owl: {error}"))?,
        );
        let executor = QueryExecutor::new_with_real_clock(store.clone());
        let owl = Self { store, executor };
        let schema =
            parse_oql_file(schema).map_err(|error| anyhow!("schema: {error}"))?;
        let report = execute_oql_file(&schema, &owl.executor)
            .map_err(|error| anyhow!("schema: {error}"))?;
        if !report.errors.is_empty() {
            return Err(anyhow!("schema: {}", report.errors.join("; ")));
        }
        Ok(owl)
    }

    /// Run one statement and return its rows.
    pub fn query(&self, source: &str) -> Result<Vec<Json>> {
        let query = parse_oql(source).map_err(|error| anyhow!("{error}"))?;
        let rows = self
            .executor
            .execute_query(&query)
            .map_err(|error| anyhow!("{error}"))?;
        Ok(rows
            .into_iter()
            .map(|row| {
                Json::Object(
                    row.fields
                        .iter()
                        .filter(|(name, _)| !name.starts_with('_'))
                        .map(|(name, value)| (name.clone(), owl_json(value)))
                        .collect(),
                )
            })
            .collect())
    }

    /// How Owl would run a query: the `path` reported by `.explain()`.
    pub fn explain(&self, source: &str) -> Result<String> {
        let rows = self.query(&format!("{}\n.explain()", source.trim_end()))?;
        let row = rows.first().context("explain returned no row")?;
        Ok(match (row.get("path"), row.get("uses_index")) {
            (Some(Json::String(path)), Some(Json::Bool(true))) => format!("{path}, index"),
            (Some(Json::String(path)), _) => path.clone(),
            _ => "unknown".to_string(),
        })
    }

    /// Rewrite everything into a fresh base generation of column segments.
    pub fn compact(&self) -> Result<()> {
        self.store
            .compact()
            .map(|_| ())
            .map_err(|error| anyhow!("compact: {error}"))
    }

    /// How many of a collection's rows are held as decoded documents, and
    /// how many live only in column segments.
    pub fn residency(&self, collection: &str) -> Result<(usize, usize, usize)> {
        let stats = self
            .store
            .storage_layout_stats(collection)
            .map_err(|error| anyhow!("layout: {error}"))?;
        Ok((stats.live_rows, stats.resident_documents, stats.lazy_typed_rows))
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.store
            .checkpoint()
            .map(|_| ())
            .map_err(|error| anyhow!("checkpoint: {error}"))
    }

    /// Load with prepared inserts: each batch is one call that hands Owl
    /// field values, which it types, validates and commits with one durable
    /// write. This is what SQLite's prepared statements are compared with.
    pub fn load(&self, data: &Dataset) -> Result<()> {
        let text = |value: &str| Value::String(value.to_string());
        let roots: Vec<_> = data.categories.iter().filter(|c| c.parent.is_none()).collect();
        let leaves: Vec<_> = data.categories.iter().filter(|c| c.parent.is_some()).collect();
        self.insert("categories", &roots, |c| {
            vec![("id", text(&c.id)), ("name", text(&c.name))]
        })?;
        self.insert("categories", &leaves, |c| {
            vec![
                ("id", text(&c.id)),
                ("name", text(&c.name)),
                ("parent", text(c.parent.as_deref().unwrap_or_default())),
            ]
        })?;
        self.insert("products", &data.products.iter().collect::<Vec<_>>(), |p| {
            vec![
                ("id", text(&p.id)),
                ("name", text(&p.name)),
                ("category", text(&p.category)),
                ("price", Value::Float(p.price)),
                ("stock", Value::Integer(p.stock as i64)),
                (
                    "embedding",
                    Value::List(
                        p.embedding
                            .iter()
                            .map(|component| Value::Float(f64::from(*component)))
                            .collect(),
                    ),
                ),
            ]
        })?;
        self.insert("customers", &data.customers.iter().collect::<Vec<_>>(), |c| {
            vec![
                ("id", text(&c.id)),
                ("name", text(&c.name)),
                ("email", text(&c.email)),
                ("tier", text(c.tier)),
                ("country", text(c.country)),
                ("joined", text(&c.joined)),
                (
                    "address",
                    Value::Object(
                        [("city", text(&c.city)), ("zip", text(&c.zip))]
                            .into_iter()
                            .map(|(name, value)| (name.to_string(), value))
                            .collect(),
                    ),
                ),
            ]
        })?;
        self.insert("orders", &data.orders.iter().collect::<Vec<_>>(), |o| {
            vec![
                ("id", text(&o.id)),
                ("customer", text(&o.customer)),
                ("status", text(o.status)),
                ("placed", text(&o.placed)),
                ("total", Value::Float(o.total)),
                (
                    "items",
                    Value::List(
                        o.items
                            .iter()
                            .map(|item| {
                                Value::Object(
                                    [
                                        ("product", text(&item.product)),
                                        ("qty", Value::Integer(item.qty as i64)),
                                        ("price", Value::Float(item.price)),
                                    ]
                                    .into_iter()
                                    .map(|(name, value)| (name.to_string(), value))
                                    .collect(),
                                )
                            })
                            .collect(),
                    ),
                ),
            ]
        })?;
        self.insert("payments", &data.payments.iter().collect::<Vec<_>>(), |p| {
            vec![
                ("id", text(&p.id)),
                ("order", text(&p.order)),
                ("method", text(p.method)),
                ("amount", Value::Float(p.amount)),
                ("paid_at", text(&p.paid_at)),
            ]
        })?;
        self.insert("reviews", &data.reviews.iter().collect::<Vec<_>>(), |r| {
            vec![
                ("id", text(&r.id)),
                ("product", text(&r.product)),
                ("customer", text(&r.customer)),
                ("rating", Value::Integer(r.rating as i64)),
                ("body", text(&r.body)),
                ("written", text(&r.written)),
            ]
        })?;
        self.insert("tickets", &data.tickets.iter().collect::<Vec<_>>(), |t| {
            let mut fields = vec![
                ("id", text(&t.id)),
                ("customer", text(&t.customer)),
                ("status", text(t.status)),
                ("priority", Value::Integer(t.priority as i64)),
                ("opened", text(&t.opened)),
            ];
            if let Some(order) = &t.order {
                fields.push(("order", text(order)));
            }
            fields
        })
    }

    /// Insert rows of field values as one durable commit.
    pub fn insert_rows(
        &self,
        collection: &str,
        rows: Vec<std::collections::HashMap<String, Value>>,
    ) -> Result<()> {
        self.executor
            .insert_documents(collection, rows)
            .map(|_| ())
            .map_err(|error| anyhow!("loading {collection}: {error}"))
    }

    fn insert<T>(
        &self,
        collection: &str,
        rows: &[&T],
        fields: impl Fn(&T) -> Vec<(&'static str, Value)>,
    ) -> Result<()> {
        for batch in rows.chunks(BATCH) {
            let documents = batch
                .iter()
                .map(|row| {
                    fields(row)
                        .into_iter()
                        .map(|(name, value)| (name.to_string(), value))
                        .collect()
                })
                .collect();
            self.executor
                .insert_documents(collection, documents)
                .map_err(|error| anyhow!("loading {collection}: {error}"))?;
        }
        Ok(())
    }
}

fn owl_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Boolean(value) => json!(value),
        Value::Integer(value) => json!(value),
        Value::Float(value) => json!(value),
        Value::String(value) => json!(value),
        Value::DateTime(instant) => json!(instant.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        Value::List(items) => Json::Array(items.iter().map(owl_json).collect()),
        Value::Object(fields) => Json::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), owl_json(value)))
                .collect(),
        ),
        Value::Reference(_) | Value::GlobalReference(_) => json!("<reference>"),
    }
}

pub struct Sqlite {
    connection: rusqlite::Connection,
}

impl Sqlite {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, include_str!("../schema.sql"))
    }

    /// Open a database and run `schema`, a batch of `CREATE` statements.
    pub fn open_with(path: &Path, schema: &str) -> Result<Self> {
        let connection = rusqlite::Connection::open(path)?;
        // The same promise Owl makes: a commit is on disk before it returns.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(schema)?;
        Ok(Self { connection })
    }

    /// The connection, for a loader that prepares its own statements.
    pub fn connection(&mut self) -> &mut rusqlite::Connection {
        &mut self.connection
    }

    pub fn query(&self, source: &str) -> Result<Vec<Json>> {
        let mut statement = self.connection.prepare(source)?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut rows = statement.query([])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let mut object = Map::new();
            for (index, name) in names.iter().enumerate() {
                let cell = match row.get_ref(index)? {
                    ValueRef::Null => Json::Null,
                    ValueRef::Integer(value) => json!(value),
                    ValueRef::Real(value) => json!(value),
                    ValueRef::Text(bytes) => {
                        let value = String::from_utf8_lossy(bytes);
                        // Nested results arrive as JSON text.
                        match value.as_bytes().first() {
                            Some(b'[' | b'{') => {
                                serde_json::from_str(&value).unwrap_or_else(|_| json!(value))
                            }
                            _ => json!(value),
                        }
                    }
                    ValueRef::Blob(_) => json!("<blob>"),
                };
                object.insert(name.clone(), cell);
            }
            result.push(Json::Object(object));
        }
        Ok(result)
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.connection
            // Give the planner its statistics, as any deployment would.
            .execute_batch("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Load with prepared statements, one transaction per batch.
    pub fn load(&mut self, data: &Dataset) -> Result<()> {
        use rusqlite::params;
        macro_rules! load {
            ($rows:expr, $sql:expr, |$row:ident| $params:expr) => {
                for batch in $rows.chunks(BATCH) {
                    let transaction = self.connection.transaction()?;
                    {
                        let mut statement = transaction.prepare_cached($sql)?;
                        for $row in batch {
                            statement.execute($params)?;
                        }
                    }
                    transaction.commit()?;
                }
            };
        }

        let mut categories: Vec<_> = data.categories.iter().collect();
        categories.sort_by_key(|category| category.parent.is_some());
        load!(categories, "INSERT INTO categories VALUES (?1, ?2, ?3)", |c| params![
            c.id, c.name, c.parent
        ]);
        load!(data.products, "INSERT INTO products VALUES (?1, ?2, ?3, ?4, ?5)", |p| params![
            p.id, p.name, p.category, p.price, p.stock
        ]);
        load!(
            data.customers,
            "INSERT INTO customers VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            |c| params![c.id, c.name, c.email, c.tier, c.country, c.joined, c.city, c.zip]
        );
        // An order and its line items commit together, as they do in Owl
        // where the items are part of the order.
        for batch in data.orders.chunks(BATCH) {
            let transaction = self.connection.transaction()?;
            {
                let mut order = transaction
                    .prepare_cached("INSERT INTO orders VALUES (?1, ?2, ?3, ?4, ?5)")?;
                let mut item = transaction
                    .prepare_cached("INSERT INTO order_items VALUES (?1, ?2, ?3, ?4, ?5)")?;
                for o in batch {
                    order.execute(params![o.id, o.customer, o.status, o.placed, o.total])?;
                    for (line, i) in o.items.iter().enumerate() {
                        item.execute(params![o.id, line as i64, i.product, i.qty, i.price])?;
                    }
                }
            }
            transaction.commit()?;
        }
        load!(data.payments, "INSERT INTO payments VALUES (?1, ?2, ?3, ?4, ?5)", |p| params![
            p.id, p.order, p.method, p.amount, p.paid_at
        ]);
        load!(data.reviews, "INSERT INTO reviews VALUES (?1, ?2, ?3, ?4, ?5, ?6)", |r| params![
            r.id, r.product, r.customer, r.rating, r.body, r.written
        ]);
        load!(data.tickets, "INSERT INTO tickets VALUES (?1, ?2, ?3, ?4, ?5, ?6)", |t| params![
            t.id, t.customer, t.order, t.status, t.priority, t.opened
        ]);
        Ok(())
    }
}

/// Compare two result sets. Numbers match within a relative tolerance (the
/// engines sum floats in different orders), and `true`/`false` match
/// SQLite's `1`/`0`. Returns a description of the first difference.
pub fn first_difference(owl: &[Json], sqlite: &[Json]) -> Option<String> {
    if owl.len() != sqlite.len() {
        return Some(format!(
            "Owl returned {} row(s), SQLite {}",
            owl.len(),
            sqlite.len()
        ));
    }
    owl.iter()
        .zip(sqlite)
        .enumerate()
        .find_map(|(index, (left, right))| {
            differs(left, right).map(|detail| format!("row {index}: {detail}"))
        })
}

fn differs(owl: &Json, sqlite: &Json) -> Option<String> {
    let number = |value: &Json| match value {
        Json::Number(number) => number.as_f64(),
        Json::Bool(flag) => Some(f64::from(u8::from(*flag))),
        _ => None,
    };
    match (owl, sqlite) {
        (Json::Object(left), Json::Object(right)) => {
            if left.len() != right.len() {
                let names = |object: &Map<String, Json>| {
                    let mut names: Vec<_> = object.keys().cloned().collect();
                    names.sort();
                    names.join(", ")
                };
                return Some(format!(
                    "fields differ: Owl has [{}], SQLite has [{}]",
                    names(left),
                    names(right)
                ));
            }
            left.iter().find_map(|(name, value)| match right.get(name) {
                Some(other) => differs(value, other).map(|detail| format!("{name}: {detail}")),
                None => Some(format!("SQLite has no field '{name}'")),
            })
        }
        (Json::Array(left), Json::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!(
                    "lists of {} and {} element(s)",
                    left.len(),
                    right.len()
                ));
            }
            left.iter()
                .zip(right)
                .enumerate()
                .find_map(|(index, (a, b))| differs(a, b).map(|detail| format!("[{index}] {detail}")))
        }
        // An empty nested list: SQLite's aggregate over no rows is `[]`.
        (Json::Null, Json::Array(right)) if right.is_empty() => None,
        _ => match (number(owl), number(sqlite)) {
            (Some(left), Some(right)) => {
                let scale = left.abs().max(right.abs()).max(1.0);
                ((left - right).abs() > 1e-6 * scale).then(|| format!("Owl {left}, SQLite {right}"))
            }
            _ => (owl != sqlite).then(|| format!("Owl {owl}, SQLite {sqlite}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::first_difference;
    use serde_json::json;

    #[test]
    fn equal_results_have_no_difference() {
        let owl = [json!({"id": "a", "total": 10.0, "ok": true, "items": [{"n": 1}]})];
        let sqlite = [json!({"id": "a", "total": 10.000000001, "ok": 1, "items": [{"n": 1.0}]})];
        assert_eq!(first_difference(&owl, &sqlite), None);
        assert_eq!(
            first_difference(&[json!({"items": null})], &[json!({"items": []})]),
            None
        );
    }

    #[test]
    fn every_kind_of_difference_is_reported() {
        let base = [json!({"id": "a", "total": 10.0, "items": [{"n": 1}]})];
        for other in [
            json!({"id": "b", "total": 10.0, "items": [{"n": 1}]}),
            json!({"id": "a", "total": 10.01, "items": [{"n": 1}]}),
            json!({"id": "a", "total": 10.0, "items": [{"n": 2}]}),
            json!({"id": "a", "total": 10.0, "items": [{"n": 1}, {"n": 1}]}),
            json!({"id": "a", "total": 10.0, "items": [{"m": 1}]}),
            json!({"id": "a", "total": 10.0}),
            json!({"id": "a", "total": null, "items": [{"n": 1}]}),
        ] {
            assert!(first_difference(&base, &[other.clone()]).is_some(), "{other}");
        }
        assert!(first_difference(&base, &[]).is_some());
    }
}
