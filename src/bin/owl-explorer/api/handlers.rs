use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use lazy_static::lazy_static;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use owl_db::distributed::GlobalObjRef;
use owl_db::index_manager::{IndexBackendKind, IndexManager};
use owl_db::parser::{
    parse_oql, parse_oql_file, resolve_query_with_bindings, schema_definition_to_schema,
    OQLFileStatement,
};
use owl_db::planner::physical::Statistics as PlannerStatistics;
use owl_db::planner::{LogicalPlanner, PhysicalPlanner};
use owl_db::schema::{CheckOperator, FieldType, Schema, SchemaConstraint, SchemaRegistry};
use owl_db::simulation::RealClock;
use owl_db::storage::{
    create_storage, zig_adapter::ZigStore, DocumentStore, VectorIndexDefinition, VectorIndexState,
    VectorIndexType, VectorMetricType,
};
use owl_db::{Document, QueryExecutor, Value};

use crate::api::models::*;
use crate::api::utils::{
    collect_storage_size_bytes, db_size_bytes, resolve_db_path, RESPONSE_COUNTER,
};

#[derive(Debug, Default, Clone)]
pub struct RuntimeStats {
    pub total_queries: u64,
    pub successful_queries: u64,
    pub total_query_time_ms: f64,
    pub last_query_time_ms: f64,
    pub total_errors: u64,
}

impl RuntimeStats {
    pub fn record_query(&mut self, success: bool, elapsed_ms: f64) {
        self.total_queries += 1;
        self.last_query_time_ms = elapsed_ms;
        if success {
            self.successful_queries += 1;
        } else {
            self.total_errors += 1;
        }
        self.total_query_time_ms += elapsed_ms;
    }
}

lazy_static! {
    pub static ref SERVER_STARTED_AT: Instant = Instant::now();
    pub static ref RUNTIME_STATS: Mutex<RuntimeStats> = Mutex::new(RuntimeStats::default());
}

// --- Helper Functions from original file ---

fn json_to_value(raw: &serde_json::Value) -> Result<Value, String> {
    match raw {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(v) => Ok(Value::Boolean(*v)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float(f))
            } else {
                Err("Unsupported JSON number".to_string())
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(s.clone())),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_value(item)?);
            }
            Ok(Value::List(out))
        }
        serde_json::Value::Object(obj) => {
            let mut out = std::collections::HashMap::new();
            for (k, v) in obj {
                out.insert(k.clone(), json_to_value(v)?);
            }
            Ok(Value::Object(out))
        }
    }
}

fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::String(s) => json!(s),
        Value::Integer(i) => json!(i),
        Value::Float(f) => json!(f),
        Value::Boolean(b) => json!(b),
        Value::DateTime(dt) => json!(dt.to_rfc3339()),
        Value::Reference(r) => json!(r.to_string_id()),
        Value::GlobalReference(r) => json!(r.to_string_id()),
        Value::List(items) => json!(items.iter().map(value_to_json).collect::<Vec<_>>()),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), value_to_json(v));
            }
            serde_json::Value::Object(out)
        }
        Value::Null => serde_json::Value::Null,
    }
}

fn document_to_json(doc: &Document) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    for (k, v) in &doc.fields {
        fields.insert(k.clone(), value_to_json(v));
    }
    json!({
        "id": doc.id,
        "schema": doc.schema,
        "version": doc.version,
        "created_at": doc.created_at.to_rfc3339(),
        "updated_at": doc.updated_at.to_rfc3339(),
        "fields": fields,
    })
}

fn parse_datetime_from_json(value: &serde_json::Value) -> Result<chrono::DateTime<Utc>, String> {
    let s = value
        .as_str()
        .ok_or_else(|| "DateTime must be a string".to_string())?;
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    let datetime_formats = [
        "%Y-%m-%d %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
    ];
    for fmt in datetime_formats {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(Utc.from_utc_datetime(&naive));
        }
    }
    let date_formats = ["%Y-%m-%d", "%d-%m-%Y", "%m/%d/%Y", "%d/%m/%Y"];
    for fmt in date_formats {
        if let Ok(date) = NaiveDate::parse_from_str(s, fmt) {
            let naive = date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| format!("Invalid date '{}'", s))?;
            return Ok(Utc.from_utc_datetime(&naive));
        }
    }
    Err(format!("Unable to parse datetime '{}'", s))
}

fn parse_field_type(input: &str) -> Result<FieldType, String> {
    owl_db::type_syntax::parse_declared_field_type(input, |_| false).map_err(|e| e.to_string())
}

fn format_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::String => "String".to_string(),
        FieldType::Integer => "Integer".to_string(),
        FieldType::Float => "Float".to_string(),
        FieldType::Boolean => "Boolean".to_string(),
        FieldType::DateTime => "DateTime".to_string(),
        FieldType::Object => "Object".to_string(),
        FieldType::Vector { dim } => format!("Vector({})", dim),
        FieldType::Reference(reference) => format!("Ref({})", reference.target_schema),
        FieldType::List(inner) => format!("List({})", format_field_type(inner)),
        FieldType::Optional(inner) => format!("Optional({})", format_field_type(inner)),
    }
}

fn parse_vector_metric(raw: &str) -> Result<VectorMetricType, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "cosine" => Ok(VectorMetricType::Cosine),
        "l2" | "euclidean" => Ok(VectorMetricType::L2),
        "dot" | "dot_product" => Ok(VectorMetricType::Dot),
        other => Err(format!("unsupported vector metric '{}'", other)),
    }
}

fn parse_vector_index_type(raw: &str) -> Result<VectorIndexType, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "exact" => Ok(VectorIndexType::Exact),
        "hnsw" => Ok(VectorIndexType::Hnsw),
        other => Err(format!("unsupported vector index type '{}'", other)),
    }
}

fn parse_vector_index_state(raw: Option<&str>) -> Result<VectorIndexState, String> {
    match raw.unwrap_or("ready").trim().to_ascii_lowercase().as_str() {
        "creating" => Ok(VectorIndexState::Creating),
        "ready" => Ok(VectorIndexState::Ready),
        "rebuilding" => Ok(VectorIndexState::Rebuilding),
        "degraded" => Ok(VectorIndexState::Degraded),
        "failed" => Ok(VectorIndexState::Failed),
        other => Err(format!("unsupported vector index state '{}'", other)),
    }
}

fn vector_index_to_json(index: &VectorIndexDefinition) -> serde_json::Value {
    let metric = match index.metric {
        VectorMetricType::Cosine => "cosine",
        VectorMetricType::L2 => "l2",
        VectorMetricType::Dot => "dot",
    };
    let index_type = match index.index_type {
        VectorIndexType::Exact => "exact",
        VectorIndexType::Hnsw => "hnsw",
    };
    let state = match index.state {
        VectorIndexState::Creating => "creating",
        VectorIndexState::Ready => "ready",
        VectorIndexState::Rebuilding => "rebuilding",
        VectorIndexState::Degraded => "degraded",
        VectorIndexState::Failed => "failed",
    };
    json!({
        "name": index.name,
        "schema": index.schema,
        "field": index.field,
        "dimension": index.dimension,
        "metric": metric,
        "index_type": index_type,
        "quantization": index.quantization,
        "state": state,
    })
}

fn parse_check_operator(raw: &str) -> Result<CheckOperator, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "eq" | "==" => Ok(CheckOperator::Eq),
        "ne" | "!=" => Ok(CheckOperator::Ne),
        "gt" | ">" => Ok(CheckOperator::Gt),
        "gte" | ">=" => Ok(CheckOperator::Gte),
        "lt" | "<" => Ok(CheckOperator::Lt),
        "lte" | "<=" => Ok(CheckOperator::Lte),
        other => Err(format!("unsupported check operator '{}'", other)),
    }
}

fn parse_constraint_defs(
    defs: Option<Vec<ConstraintDef>>,
) -> Result<Vec<SchemaConstraint>, String> {
    let Some(defs) = defs else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(defs.len());
    for def in defs {
        let kind = def.kind.trim().to_ascii_lowercase();
        let parsed = match kind.as_str() {
            "unique" => {
                let fields = def.fields.unwrap_or_default();
                if fields.is_empty() {
                    return Err("unique constraint requires non-empty fields".to_string());
                }
                SchemaConstraint::Unique {
                    fields,
                    name: def.name,
                }
            }
            "check" => {
                let field = def
                    .field
                    .ok_or_else(|| "check constraint requires 'field'".to_string())?;
                let op_raw = def
                    .op
                    .ok_or_else(|| "check constraint requires 'op'".to_string())?;
                let value_raw = def
                    .value
                    .ok_or_else(|| "check constraint requires 'value'".to_string())?;
                SchemaConstraint::Check {
                    field,
                    op: parse_check_operator(&op_raw)?,
                    value: json_to_value(&value_raw)?,
                    name: def.name,
                }
            }
            "ref" | "ref_integrity" | "reference" => {
                let field = def
                    .field
                    .ok_or_else(|| "ref constraint requires 'field'".to_string())?;
                let target_schema = def
                    .target_schema
                    .ok_or_else(|| "ref constraint requires 'target_schema'".to_string())?;
                SchemaConstraint::RefIntegrity {
                    field,
                    target_schema,
                    target_field: def.target_field,
                    required: def.required.unwrap_or(true),
                    name: def.name,
                }
            }
            _ => {
                return Err(format!(
                    "unknown constraint kind '{}' (expected unique|check|ref)",
                    def.kind
                ));
            }
        };
        out.push(parsed);
    }
    Ok(out)
}

fn constraint_to_json(constraint: &SchemaConstraint) -> serde_json::Value {
    match constraint {
        SchemaConstraint::Unique { fields, name } => json!({
            "kind": "unique",
            "name": name,
            "fields": fields,
        }),
        SchemaConstraint::Check {
            field,
            op,
            value,
            name,
        } => {
            let op = match op {
                CheckOperator::Eq => "eq",
                CheckOperator::Ne => "ne",
                CheckOperator::Gt => "gt",
                CheckOperator::Gte => "gte",
                CheckOperator::Lt => "lt",
                CheckOperator::Lte => "lte",
            };
            json!({
                "kind": "check",
                "name": name,
                "field": field,
                "op": op,
                "value": value_to_json(value),
            })
        }
        SchemaConstraint::RefIntegrity {
            field,
            target_schema,
            target_field,
            required,
            name,
        } => json!({
            "kind": "ref",
            "name": name,
            "field": field,
            "target_schema": target_schema,
            "target_field": target_field,
            "required": required,
        }),
    }
}

fn extract_field_path_value(
    fields: &std::collections::HashMap<String, Value>,
    path: &str,
) -> Option<Value> {
    let mut parts = path.split('.');
    let first = parts.next()?;
    let mut current = fields.get(first)?.clone();
    for part in parts {
        match current {
            Value::Object(map) => current = map.get(part)?.clone(),
            _ => return None,
        }
    }
    Some(current)
}

fn compare_numeric(left: &Value, right: &Value) -> Option<f64> {
    fn as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Integer(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    Some(as_f64(left)? - as_f64(right)?)
}

fn check_constraint_holds(observed: &Value, op: &CheckOperator, expected: &Value) -> bool {
    match op {
        CheckOperator::Eq => observed == expected,
        CheckOperator::Ne => observed != expected,
        CheckOperator::Gt => compare_numeric(observed, expected)
            .map(|delta| delta > 0.0)
            .unwrap_or(false),
        CheckOperator::Gte => compare_numeric(observed, expected)
            .map(|delta| delta >= 0.0)
            .unwrap_or(false),
        CheckOperator::Lt => compare_numeric(observed, expected)
            .map(|delta| delta < 0.0)
            .unwrap_or(false),
        CheckOperator::Lte => compare_numeric(observed, expected)
            .map(|delta| delta <= 0.0)
            .unwrap_or(false),
    }
}

fn reference_matches_document(store: &ZigStore, observed: &Value, target: &Document) -> bool {
    match observed {
        Value::String(id) => &target.id == id || target.fields.get("id") == Some(observed),
        Value::Reference(objref) => store
            .get_objref(&target.schema, &target.id)
            .ok()
            .flatten()
            .map(|candidate| candidate == *objref)
            .unwrap_or(false),
        Value::GlobalReference(_) => false,
        _ => false,
    }
}

fn validate_document_constraints_for_store(
    store: &ZigStore,
    schema: &Schema,
    document: &Document,
    current_document_id: Option<&String>,
) -> Result<(), String> {
    for constraint in &schema.constraints {
        match constraint {
            SchemaConstraint::Unique { fields, name } => {
                let docs = store
                    .scan_documents(&schema.name)
                    .map_err(|e| e.to_string())?;
                for existing in docs {
                    if let Some(current_id) = current_document_id {
                        if &existing.id == current_id {
                            continue;
                        }
                    }
                    let mut same = true;
                    for field in fields {
                        let left = extract_field_path_value(&document.fields, field);
                        let right = extract_field_path_value(&existing.fields, field);
                        if left != right {
                            same = false;
                            break;
                        }
                    }
                    if same {
                        return Err(format!(
                            "Unique constraint '{}' violated on schema '{}' fields {:?}",
                            name.clone().unwrap_or_else(|| "unnamed_unique".to_string()),
                            schema.name,
                            fields
                        ));
                    }
                }
            }
            SchemaConstraint::Check {
                field,
                op,
                value,
                name,
            } => {
                let observed =
                    extract_field_path_value(&document.fields, field).ok_or_else(|| {
                        format!(
                            "Check constraint '{}' failed: field '{}' missing",
                            name.clone().unwrap_or_else(|| "unnamed_check".to_string()),
                            field
                        )
                    })?;
                if !check_constraint_holds(&observed, op, value) {
                    return Err(format!(
                        "Check constraint '{}' failed for field '{}'",
                        name.clone().unwrap_or_else(|| "unnamed_check".to_string()),
                        field
                    ));
                }
            }
            SchemaConstraint::RefIntegrity {
                field,
                target_schema,
                target_field,
                required,
                name,
            } => {
                let observed = extract_field_path_value(&document.fields, field);
                let Some(observed) = observed else {
                    if *required {
                        return Err(format!(
                            "Ref constraint '{}' failed: field '{}' missing",
                            name.clone().unwrap_or_else(|| "unnamed_ref".to_string()),
                            field
                        ));
                    }
                    continue;
                };

                if observed.is_null() {
                    if *required {
                        return Err(format!(
                            "Ref constraint '{}' failed: '{}' is null",
                            name.clone().unwrap_or_else(|| "unnamed_ref".to_string()),
                            field
                        ));
                    }
                    continue;
                }

                let target_docs = store
                    .scan_documents(target_schema)
                    .map_err(|e| e.to_string())?;
                let target_key = target_field.clone().unwrap_or_else(|| "id".to_string());
                let exists = target_docs.iter().any(|target| {
                    if target_key == "id" {
                        return reference_matches_document(store, &observed, target);
                    }
                    target
                        .fields
                        .get(&target_key)
                        .map(|v| *v == observed)
                        .unwrap_or(false)
                });
                if !exists {
                    return Err(format!(
                        "Ref constraint '{}' failed: '{}' does not match target {}.{}",
                        name.clone().unwrap_or_else(|| "unnamed_ref".to_string()),
                        field,
                        target_schema,
                        target_key
                    ));
                }
            }
        }
    }
    Ok(())
}

fn coerce_json_value_to_field_type(
    value: &serde_json::Value,
    field_type: &FieldType,
) -> Result<Value, String> {
    match field_type {
        FieldType::String => value
            .as_str()
            .map(|s| Value::String(s.to_string()))
            .ok_or_else(|| "Expected string".to_string()),
        FieldType::Integer => {
            if let Some(v) = value.as_i64() {
                Ok(Value::Integer(v))
            } else if let Some(v) = value.as_f64() {
                Ok(Value::Integer(v as i64))
            } else if let Some(v) = value.as_str() {
                v.parse::<i64>()
                    .map(Value::Integer)
                    .map_err(|_| "Expected integer".to_string())
            } else {
                Err("Expected integer".to_string())
            }
        }
        FieldType::Float => {
            if let Some(v) = value.as_f64() {
                Ok(Value::Float(v))
            } else if let Some(v) = value.as_i64() {
                Ok(Value::Float(v as f64))
            } else if let Some(v) = value.as_str() {
                v.parse::<f64>()
                    .map(Value::Float)
                    .map_err(|_| "Expected float".to_string())
            } else {
                Err("Expected float".to_string())
            }
        }
        FieldType::Boolean => {
            if let Some(v) = value.as_bool() {
                Ok(Value::Boolean(v))
            } else if let Some(v) = value.as_str() {
                match v.trim().to_ascii_lowercase().as_str() {
                    "true" => Ok(Value::Boolean(true)),
                    "false" => Ok(Value::Boolean(false)),
                    _ => Err("Expected boolean".to_string()),
                }
            } else {
                Err("Expected boolean".to_string())
            }
        }
        FieldType::DateTime => parse_datetime_from_json(value).map(Value::DateTime),
        FieldType::Object => {
            if value.is_object() {
                json_to_value(value)
            } else {
                Err("Expected object".to_string())
            }
        }
        FieldType::Vector { dim } => {
            let arr = value
                .as_array()
                .ok_or_else(|| "Expected vector array".to_string())?;
            if arr.len() != *dim as usize {
                return Err(format!(
                    "Expected vector dimension {}, got {}",
                    dim,
                    arr.len()
                ));
            }
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                if let Some(v) = item.as_f64() {
                    out.push(Value::Float(v));
                } else if let Some(v) = item.as_i64() {
                    out.push(Value::Float(v as f64));
                } else {
                    return Err("Vector elements must be numeric".to_string());
                }
            }
            Ok(Value::List(out))
        }
        FieldType::Reference(_) => {
            if value.is_null() {
                return Ok(Value::Null);
            }
            let s = value
                .as_str()
                .ok_or_else(|| "Expected reference string".to_string())?;
            if let Some(obj) = GlobalObjRef::from_string(s) {
                return Ok(Value::GlobalReference(obj));
            }
            let obj = owl_db::document::ObjRef::from_string(s)
                .ok_or_else(|| "Invalid reference value".to_string())?;
            Ok(Value::Reference(obj))
        }
        FieldType::List(inner) => {
            let arr = value
                .as_array()
                .ok_or_else(|| "Expected list/array".to_string())?;
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(coerce_json_value_to_field_type(item, inner)?);
            }
            Ok(Value::List(out))
        }
        FieldType::Optional(inner) => {
            if value.is_null() {
                Ok(Value::Null)
            } else {
                coerce_json_value_to_field_type(value, inner)
            }
        }
    }
}

fn normalize_script_input(input: &str) -> String {
    let mut out = String::new();
    let mut saw_statement = false;
    let mut prev_nonempty = false;
    for raw_line in input.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            prev_nonempty = false;
            continue;
        }

        let statement_start = is_statement_start(trimmed);
        if saw_statement && statement_start && prev_nonempty {
            out.push('\n');
        }

        out.push_str(raw_line);
        out.push('\n');
        saw_statement = true;
        prev_nonempty = true;
    }
    out
}

#[derive(Clone)]
struct ScriptStoreSnapshot {
    schemas: Vec<Schema>,
    documents_by_schema: std::collections::HashMap<String, Vec<Document>>,
}

fn snapshot_script_store<S: DocumentStore>(store: &S) -> Result<ScriptStoreSnapshot, String> {
    let schema_names = store.list_schemas().map_err(|e| e.to_string())?;
    let mut schemas = Vec::new();
    let mut documents_by_schema = std::collections::HashMap::new();
    for name in schema_names {
        if let Some(schema) = store.get_schema(&name).map_err(|e| e.to_string())? {
            let docs = store.scan_documents(&name).map_err(|e| e.to_string())?;
            schemas.push(schema);
            documents_by_schema.insert(name, docs);
        }
    }
    Ok(ScriptStoreSnapshot {
        schemas,
        documents_by_schema,
    })
}

fn restore_script_store<S: DocumentStore>(
    store: &S,
    snapshot: &ScriptStoreSnapshot,
) -> Result<(), String> {
    let existing_schemas = store.list_schemas().map_err(|e| e.to_string())?;
    for schema_name in &existing_schemas {
        let docs = store
            .scan_documents(schema_name)
            .map_err(|e| e.to_string())?;
        for doc in docs {
            let _ = store
                .delete_document(schema_name, &doc.id)
                .map_err(|e| e.to_string())?;
        }
    }
    for schema_name in existing_schemas {
        let _ = store
            .delete_schema(&schema_name)
            .map_err(|e| e.to_string())?;
    }

    for schema in &snapshot.schemas {
        store.put_schema(schema).map_err(|e| e.to_string())?;
    }
    for docs in snapshot.documents_by_schema.values() {
        for doc in docs {
            store.put_document(doc).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn is_statement_start(trimmed: &str) -> bool {
    if trimmed.starts_with("schema ") || trimmed.starts_with("fn ") {
        return true;
    }
    let head = trimmed
        .split_whitespace()
        .next()
        .unwrap_or(trimmed)
        .trim_end_matches('{');
    let Some((collection, op)) = head.split_once('.') else {
        return false;
    };
    if collection.is_empty() {
        return false;
    }
    if !collection
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return false;
    }
    matches!(
        op,
        "insert" | "where" | "select" | "update" | "delete" | "group_by" | "sort" | "limit"
    )
}

// --- Handlers ---

pub fn create_database(
    req: CreateDbRequest,
    registry: &mut Registry,
    registry_file: &Path,
) -> Result<(), String> {
    if req.name.trim().is_empty() {
        return Err("database name is required".to_string());
    }
    if req.path.trim().is_empty() {
        return Err("database path is required".to_string());
    }

    let size_mb = req.size_mb.max(1);
    let size = db_size_bytes(size_mb);
    let db_path = resolve_db_path(&req.path);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("failed to create parent dir: {}", e))?;
    }

    let db_path_str = db_path.to_string_lossy().to_string();
    let _ = create_storage(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    registry.upsert(DatabaseEntry {
        name: req.name,
        path: req.path,
        size_mb,
    });
    registry.save(registry_file)?;
    Ok(())
}

pub fn delete_database(
    req: DeleteDatabaseRequest,
    registry: &mut Registry,
    registry_file: &Path,
) -> Result<serde_json::Value, String> {
    let removed = registry
        .remove(&req.name)
        .ok_or_else(|| format!("database '{}' not found", req.name))?;
    registry.save(registry_file)?;

    let db_path = resolve_db_path(&removed.path);
    let wal_path = PathBuf::from(format!("{}.wal", db_path.display()));
    let superblock_path = PathBuf::from(format!("{}.sb", db_path.display()));

    let mut deleted_files = 0usize;
    for p in [&db_path, &wal_path, &superblock_path] {
        if p.exists() {
            fs::remove_file(p).map_err(|e| format!("failed removing '{}': {}", p.display(), e))?;
            deleted_files += 1;
        }
    }

    Ok(json!({
        "name": removed.name,
        "deleted_files": deleted_files,
        "path": removed.path,
    }))
}

pub fn execute_query(db: &DatabaseEntry, query: &str) -> Result<serde_json::Value, String> {
    let started = Instant::now();
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = Arc::new(
        ZigStore::new(&db_path_str, size)
            .map_err(|e| format!("{} (path: {})", e, db_path.display()))?,
    );
    let executor = QueryExecutor::new_with_real_clock(store);
    match parse_oql(query) {
        Ok(parsed) => {
            let scanned_count = executor
                .store()
                .scan_documents(&parsed.collection)
                .map(|docs| docs.len())
                .unwrap_or(0);
            let docs = executor.execute_query(&parsed).map_err(|e| e.to_string())?;
            if let Ok(mut stats) = RUNTIME_STATS.lock() {
                stats.record_query(true, started.elapsed().as_secs_f64() * 1000.0);
            }
            Ok(json!({
                "ok": true,
                "mode": "single",
                "count": docs.len(),
                "documents": docs.iter().map(document_to_json).collect::<Vec<_>>(),
                "meta": {
                    "execution_time_ms": started.elapsed().as_secs_f64() * 1000.0,
                    "documents_scanned": scanned_count,
                    "documents_returned": docs.len(),
                }
            }))
        }
        Err(single_err) => {
            let mut script_executor = QueryExecutor::new_with_real_clock(executor.store().clone());
            let file = parse_oql_file(query)
                .or_else(|_| {
                    let normalized = normalize_script_input(query);
                    parse_oql_file(&normalized)
                })
                .map_err(|file_err| {
                    format!(
                        "single-query parse failed: {}; script parse failed: {}",
                        single_err, file_err
                    )
                })?;
            let schema_registry = SchemaRegistry::new();
            for stmt in &file.statements {
                if let OQLFileStatement::Schema(def) = stmt {
                    let empty = Schema::new(def.name.clone(), std::collections::HashMap::new());
                    let _ = schema_registry.register(empty);
                }
            }
            for stmt in &file.statements {
                if let OQLFileStatement::Schema(def) = stmt {
                    if let Ok(schema) = schema_definition_to_schema(def, &schema_registry) {
                        let _ = schema_registry.register(schema.clone());
                        let _ = script_executor.store().put_schema(&schema);
                    }
                }
            }

            let mut summary = Vec::new();
            let mut query_results = Vec::new();
            let mut errors = Vec::new();
            let mut schema_count = 0usize;
            let mut query_count = 0usize;
            let mut function_count = 0usize;
            let mut binding_count = 0usize;
            let mut success_count = 0usize;
            let mut total_scanned = 0usize;
            let mut query_bindings: std::collections::HashMap<String, owl_db::parser::OQLQuery> =
                std::collections::HashMap::new();

            for stmt in &file.statements {
                match stmt {
                    OQLFileStatement::Schema(def) => {
                        schema_count += 1;
                        success_count += 1;
                        summary.push(json!({"type": "schema", "value": def.name}));
                    }
                    OQLFileStatement::FunctionDef(def) => {
                        function_count += 1;
                        match script_executor.register_user_function(def.clone()) {
                            Ok(()) => {
                                success_count += 1;
                                summary.push(json!({"type": "function", "value": def.name}));
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                summary.push(json!({"type": "error", "value": msg}));
                                errors.push(msg);
                            }
                        }
                    }
                    OQLFileStatement::QueryBinding(binding) => {
                        binding_count += 1;
                        query_bindings.insert(binding.name.clone(), binding.query.clone());
                        success_count += 1;
                        summary.push(json!({"type": "binding", "value": binding.name}));
                    }
                    OQLFileStatement::Query(q) => {
                        query_count += 1;
                        let resolved_query = match resolve_query_with_bindings(q, &query_bindings) {
                            Ok(rq) => rq,
                            Err(e) => {
                                let msg = format!("Binding resolution failed: {}", e);
                                summary.push(json!({"type": "error", "value": msg}));
                                errors.push(msg);
                                continue;
                            }
                        };
                        total_scanned += executor
                            .store()
                            .scan_documents(&resolved_query.collection)
                            .map(|docs| docs.len())
                            .unwrap_or(0);
                        match script_executor.execute_query(&resolved_query) {
                            Ok(docs) => {
                                success_count += 1;
                                summary.push(json!({"type": "query", "value": docs.len()}));
                                let docs_json: Vec<_> = docs.iter().map(document_to_json).collect();
                                query_results.push(json!({
                                    "count": docs.len(),
                                    "documents": docs_json,
                                }));
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                summary.push(json!({"type": "error", "value": msg}));
                                errors.push(msg);
                            }
                        }
                    }
                    OQLFileStatement::Transaction(tx_statements) => {
                        let snapshot = snapshot_script_store(script_executor.store().as_ref())?;
                        let mut active_txn_id = None;
                        if let Some(zig_store) = script_executor
                            .store()
                            .as_any()
                            .downcast_ref::<owl_db::storage::zig_adapter::ZigStore>(
                        ) {
                            let tx_id = zig_store
                                .begin_local_transaction()
                                .map_err(|e| e.to_string())?;
                            active_txn_id = Some(tx_id);
                        }
                        let mut tx_errors = Vec::new();
                        let mut tx_summary = Vec::new();
                        let mut tx_query_results = Vec::new();
                        let mut tx_bindings = query_bindings.clone();
                        let mut tx_scanned = 0usize;

                        for tx_stmt in tx_statements {
                            match tx_stmt {
                                OQLFileStatement::Schema(def) => {
                                    schema_count += 1;
                                    success_count += 1;
                                    tx_summary.push(json!({"type": "schema", "value": def.name}));
                                }
                                OQLFileStatement::FunctionDef(def) => {
                                    function_count += 1;
                                    match script_executor.register_user_function(def.clone()) {
                                        Ok(()) => {
                                            success_count += 1;
                                            tx_summary.push(
                                                json!({"type": "function", "value": def.name}),
                                            );
                                        }
                                        Err(e) => tx_errors.push(e.to_string()),
                                    }
                                }
                                OQLFileStatement::QueryBinding(binding) => {
                                    binding_count += 1;
                                    tx_bindings.insert(binding.name.clone(), binding.query.clone());
                                    success_count += 1;
                                    tx_summary
                                        .push(json!({"type": "binding", "value": binding.name}));
                                }
                                OQLFileStatement::Query(q) => {
                                    query_count += 1;
                                    let resolved_query =
                                        match resolve_query_with_bindings(q, &tx_bindings) {
                                            Ok(rq) => rq,
                                            Err(e) => {
                                                tx_errors.push(format!(
                                                    "Binding resolution failed: {}",
                                                    e
                                                ));
                                                continue;
                                            }
                                        };
                                    tx_scanned += executor
                                        .store()
                                        .scan_documents(&resolved_query.collection)
                                        .map(|docs| docs.len())
                                        .unwrap_or(0);
                                    match script_executor.execute_query(&resolved_query) {
                                        Ok(docs) => {
                                            success_count += 1;
                                            tx_summary.push(
                                                json!({"type": "query", "value": docs.len()}),
                                            );
                                            let docs_json: Vec<_> =
                                                docs.iter().map(document_to_json).collect();
                                            tx_query_results.push(json!({
                                                "count": docs.len(),
                                                "documents": docs_json,
                                            }));
                                        }
                                        Err(e) => tx_errors.push(e.to_string()),
                                    }
                                }
                                OQLFileStatement::Transaction(_) => {
                                    tx_errors.push(
                                        "Nested transaction blocks are not supported".to_string(),
                                    );
                                }
                            }
                        }

                        if tx_errors.is_empty() {
                            if let Some(tx_id) = active_txn_id {
                                if let Some(zig_store) = script_executor
                                    .store()
                                    .as_any()
                                    .downcast_ref::<owl_db::storage::zig_adapter::ZigStore>(
                                ) {
                                    zig_store
                                        .commit_local_transaction(tx_id)
                                        .map_err(|e| e.to_string())?;
                                }
                            }
                            query_bindings = tx_bindings;
                            total_scanned += tx_scanned;
                            summary.extend(tx_summary);
                            query_results.extend(tx_query_results);
                            success_count += 1;
                            summary.push(json!({"type": "transaction", "value": "committed"}));
                        } else {
                            if let Some(tx_id) = active_txn_id {
                                if let Some(zig_store) = script_executor
                                    .store()
                                    .as_any()
                                    .downcast_ref::<owl_db::storage::zig_adapter::ZigStore>(
                                ) {
                                    let _ = zig_store.rollback_local_transaction(tx_id);
                                }
                            }
                            restore_script_store(script_executor.store().as_ref(), &snapshot)?;
                            let tx_msg = format!(
                                "Transaction rolled back due to {} error(s)",
                                tx_errors.len()
                            );
                            summary.push(json!({"type": "transaction", "value": "rolled_back"}));
                            summary.push(json!({"type": "error", "value": tx_msg.clone()}));
                            errors.push(tx_msg);
                            errors.extend(tx_errors);
                        }
                    }
                }
            }

            let last_query_documents = query_results
                .last()
                .and_then(|v| v.get("documents"))
                .cloned()
                .unwrap_or_else(|| json!([]));
            let returned_count = last_query_documents
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0);
            let ok = errors.is_empty();
            let error_message = if ok {
                serde_json::Value::Null
            } else {
                json!(format!(
                    "Script execution failed with {} error(s)",
                    errors.len()
                ))
            };

            if let Ok(mut stats) = RUNTIME_STATS.lock() {
                stats.record_query(ok, started.elapsed().as_secs_f64() * 1000.0);
            }
            Ok(json!({
                "ok": ok,
                "mode": "script",
                "schema_count": schema_count,
                "binding_count": binding_count,
                "query_count": query_count,
                "function_count": function_count,
                "success_count": success_count,
                "errors": errors,
                "error": error_message,
                "results": summary,
                "query_results": query_results,
                "last_query_documents": last_query_documents,
                "meta": {
                    "execution_time_ms": started.elapsed().as_secs_f64() * 1000.0,
                    "documents_scanned": total_scanned,
                    "documents_returned": returned_count,
                }
            }))
        }
    }
}

pub fn explain_query(db: &DatabaseEntry, query: &str) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = Arc::new(
        ZigStore::new(&db_path_str, size)
            .map_err(|e| format!("{} (path: {})", e, db_path.display()))?,
    );

    let parsed = parse_oql(query).map_err(|e| format!("parse failed: {}", e))?;

    let registry = SchemaRegistry::new();
    let mut stats = PlannerStatistics::new();
    let schema_names = store.list_schemas().map_err(|e| e.to_string())?;
    for schema_name in schema_names {
        if let Some(schema) = store.get_schema(&schema_name).map_err(|e| e.to_string())? {
            let _ = registry.register(schema);
        }
        let row_count = store
            .scan_documents(&schema_name)
            .map(|docs| docs.len() as u64)
            .unwrap_or(0);
        stats.set_table_size(schema_name.clone(), row_count.max(1));
    }

    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let logical_plan = logical_planner.plan(&parsed).map_err(|e| e.to_string())?;
    let physical_planner =
        PhysicalPlanner::with_schema_registry(registry.clone()).with_statistics(stats);
    let physical_plan = physical_planner
        .plan(&logical_plan)
        .map_err(|e| e.to_string())?;
    let estimated_cost = physical_plan.estimate_cost();
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    let reverse_ref_prefilter = executor
        .explain_reverse_reference_prefilter(&parsed)
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "query": query,
        "collection": parsed.collection,
        "logical_plan": logical_plan.root,
        "physical_plan": format!("{:?}", physical_plan.root),
        "estimated_cost": estimated_cost,
        "graph_native_prefilter": {
            "eligible": !reverse_ref_prefilter.is_empty(),
            "plans": reverse_ref_prefilter,
        }
    }))
}

pub fn load_schemas(db: &DatabaseEntry) -> Result<Vec<String>, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    store.list_schemas().map_err(|e| e.to_string())
}

pub fn load_schema_detail(db: &DatabaseEntry, schema: &str) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let schema_obj = store
        .get_schema(schema)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("schema '{}' not found", schema))?;

    let mut fields = Vec::new();
    for (name, ty) in schema_obj.fields {
        fields.push(json!({
            "name": name,
            "type": format_field_type(&ty),
        }));
    }
    fields.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        an.cmp(bn)
    });

    let constraints = schema_obj
        .constraints
        .iter()
        .map(constraint_to_json)
        .collect::<Vec<_>>();

    Ok(json!({
        "name": schema_obj.name,
        "fields": fields,
        "constraints": constraints,
    }))
}

pub fn load_documents(
    db: &DatabaseEntry,
    schema: &str,
    page: usize,
    page_size: usize,
) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let mut docs = store.scan_documents(schema).map_err(|e| e.to_string())?;

    for doc in &mut docs {
        let direct = store
            .get_objref(schema, &doc.id)
            .map_err(|e| e.to_string())?
            .is_some();
        if !direct {
            if let Some(canonical_id) = store
                .resolve_index_id_for_payload_id(schema, &doc.id)
                .map_err(|e| e.to_string())?
            {
                doc.id = canonical_id;
            }
        }
    }
    docs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let total = docs.len();
    let page_size = page_size.max(1).min(200);
    let total_pages = if total == 0 {
        1
    } else {
        (total + page_size - 1) / page_size
    };
    let page = page.max(1).min(total_pages);
    let start = (page - 1) * page_size;
    let end = (start + page_size).min(total);
    let page_docs = if start < total {
        &docs[start..end]
    } else {
        &[]
    };

    Ok(json!({
        "schema": schema,
        "page": page,
        "page_size": page_size,
        "total": total,
        "total_pages": total_pages,
        "documents": page_docs.iter().map(document_to_json).collect::<Vec<_>>(),
    }))
}

pub fn upsert_collection_schema(
    db: &DatabaseEntry,
    req: UpsertCollectionRequest,
) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    let collection_name = req.name.trim();
    if collection_name.is_empty() {
        return Err("collection name is required".to_string());
    }
    if req.fields.is_empty() {
        return Err("collection schema must include at least one field".to_string());
    }

    let mut next_fields = std::collections::HashMap::new();
    for field in &req.fields {
        let name = field.name.trim();
        if name.is_empty() {
            return Err("field name cannot be empty".to_string());
        }
        let parsed = parse_field_type(&field.r#type)?;
        next_fields.insert(name.to_string(), parsed);
    }

    let strategy = req
        .strategy
        .unwrap_or_else(|| "replace".to_string())
        .to_ascii_lowercase();
    let incoming_constraints = parse_constraint_defs(req.constraints)?;

    let schema_to_store = if strategy == "merge" {
        let existing = store
            .get_schema(collection_name)
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| {
                Schema::new(
                    collection_name.to_string(),
                    std::collections::HashMap::new(),
                )
            });

        let mut merged = existing.fields;
        for (k, v) in next_fields {
            merged.insert(k, v);
        }

        let constraints = if incoming_constraints.is_empty() {
            existing.constraints
        } else {
            incoming_constraints
        };
        Schema::new_with_constraints(collection_name.to_string(), merged, constraints)
    } else {
        Schema::new_with_constraints(
            collection_name.to_string(),
            next_fields,
            incoming_constraints,
        )
    };

    store
        .put_schema(&schema_to_store)
        .map_err(|e| e.to_string())?;
    load_schema_detail(db, collection_name)
}

pub fn load_collection_summaries(db: &DatabaseEntry) -> Result<Vec<serde_json::Value>, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let mut schema_names = store.list_schemas().map_err(|e| e.to_string())?;
    schema_names.sort();

    let mut out = Vec::with_capacity(schema_names.len());
    for name in schema_names {
        let document_count = store
            .scan_documents(&name)
            .map(|docs| docs.len())
            .unwrap_or(0);
        out.push(json!({
            "name": name,
            "document_count": document_count,
        }));
    }
    Ok(out)
}

pub fn rebuild_indexes(
    db: &DatabaseEntry,
    req: RebuildIndexesRequest,
) -> Result<serde_json::Value, String> {
    if req.database != db.name {
        return Err(format!(
            "database mismatch: route resolved '{}' but payload requested '{}'",
            db.name, req.database
        ));
    }

    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = create_storage(&db_path_str, size).map_err(|e| e.to_string())?;
    let catalog_path = PathBuf::from(format!("{}.indexes.json", db_path.display()));
    if !catalog_path.exists() {
        return Err(format!(
            "index catalog '{}' does not exist",
            catalog_path.display()
        ));
    }

    let backend_kind = match req
        .backend
        .unwrap_or_else(|| "memory".to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        "zig" => IndexBackendKind::Zig,
        "memory" | "inmemory" => IndexBackendKind::InMemory,
        other => {
            return Err(format!(
                "unsupported backend '{}'; use zig or memory",
                other
            ));
        }
    };

    let manager = IndexManager::load_catalog_with_backend(
        &catalog_path,
        Arc::new(RealClock::new()),
        backend_kind,
    )
    .map_err(|e| e.to_string())?;

    let report = manager
        .rebuild_from_store(store.as_ref())
        .map_err(|e| e.to_string())?;
    manager
        .save_catalog(&catalog_path)
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "ok": true,
        "database": db.name,
        "catalog": catalog_path.display().to_string(),
        "schemas_processed": report.schemas_processed,
        "documents_scanned": report.documents_scanned,
        "entries_indexed": report.entries_indexed,
        "documents_skipped_missing_objref": report.documents_skipped_missing_objref,
    }))
}

pub fn list_vector_indexes(
    db: &DatabaseEntry,
    req: ListVectorIndexesRequest,
) -> Result<serde_json::Value, String> {
    if req.database != db.name {
        return Err(format!(
            "database mismatch: route resolved '{}' but payload requested '{}'",
            db.name, req.database
        ));
    }
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let indexes = store
        .list_vector_indexes(req.schema.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": true,
        "database": db.name,
        "count": indexes.len(),
        "indexes": indexes.iter().map(vector_index_to_json).collect::<Vec<_>>(),
    }))
}

pub fn upsert_vector_index(
    db: &DatabaseEntry,
    req: UpsertVectorIndexRequest,
) -> Result<serde_json::Value, String> {
    if req.database != db.name {
        return Err(format!(
            "database mismatch: route resolved '{}' but payload requested '{}'",
            db.name, req.database
        ));
    }
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    let schema = store
        .get_schema(&req.schema)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("schema '{}' not found", req.schema))?;
    match schema.fields.get(&req.field) {
        Some(FieldType::Vector { dim }) => {
            if *dim != req.dimension {
                return Err(format!(
                    "dimension mismatch for field '{}': schema has {}, request has {}",
                    req.field, dim, req.dimension
                ));
            }
        }
        Some(other) => {
            return Err(format!(
                "field '{}' must be Vector type, got {}",
                req.field,
                other.type_name()
            ));
        }
        None => {
            return Err(format!(
                "field '{}' not found in schema '{}'",
                req.field, req.schema
            ))
        }
    }

    let index = VectorIndexDefinition {
        name: req.name,
        schema: req.schema,
        field: req.field,
        dimension: req.dimension,
        metric: parse_vector_metric(&req.metric)?,
        index_type: parse_vector_index_type(&req.index_type)?,
        quantization: req.quantization,
        state: parse_vector_index_state(req.state.as_deref())?,
    };
    store
        .upsert_vector_index(index.clone())
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": true,
        "database": db.name,
        "index": vector_index_to_json(&index),
    }))
}

pub fn delete_vector_index(
    db: &DatabaseEntry,
    req: DeleteVectorIndexRequest,
) -> Result<serde_json::Value, String> {
    if req.database != db.name {
        return Err(format!(
            "database mismatch: route resolved '{}' but payload requested '{}'",
            db.name, req.database
        ));
    }
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let deleted = store
        .delete_vector_index(&req.schema, &req.name)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": deleted,
        "deleted": deleted,
        "database": db.name,
        "schema": req.schema,
        "name": req.name,
    }))
}

pub fn vector_index_status(
    db: &DatabaseEntry,
    req: VectorIndexStatusRequest,
) -> Result<serde_json::Value, String> {
    if req.database != db.name {
        return Err(format!(
            "database mismatch: route resolved '{}' but payload requested '{}'",
            db.name, req.database
        ));
    }
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    let status = store
        .vector_index_status(&req.schema, &req.name)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": true,
        "database": db.name,
        "status": status.as_ref().map(|s| json!({
            "schema": s.schema,
            "name": s.name,
            "state": match s.state {
                VectorIndexState::Creating => "creating",
                VectorIndexState::Ready => "ready",
                VectorIndexState::Rebuilding => "rebuilding",
                VectorIndexState::Degraded => "degraded",
                VectorIndexState::Failed => "failed",
            },
            "file_exists": s.file_exists,
            "file_size_bytes": s.file_size_bytes,
            "file_mtime_unix_seconds": s.file_mtime_unix_seconds,
        })),
    }))
}

pub fn build_vector_index(
    db: &DatabaseEntry,
    req: BuildVectorIndexRequest,
) -> Result<serde_json::Value, String> {
    if req.database != db.name {
        return Err(format!(
            "database mismatch: route resolved '{}' but payload requested '{}'",
            db.name, req.database
        ));
    }
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let force = req.force.unwrap_or(false);
    let report = store
        .build_vector_index(&req.schema, &req.name, force)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": true,
        "database": db.name,
        "schema": req.schema,
        "name": req.name,
        "force": force,
        "report": {
            "indexed_documents": report.indexed_documents,
            "skipped_documents": report.skipped_documents,
            "total_documents": report.total_documents,
        }
    }))
}

pub fn update_document(
    db: &DatabaseEntry,
    req: UpdateDocumentRequest,
) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    let schema = store
        .get_schema(&req.schema)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("schema '{}' not found", req.schema))?;
    let mut doc = store
        .get_document(&req.schema, &req.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("document '{}' not found", req.id))?;

    let replace = req.replace.unwrap_or(false);
    let mut converted_fields = std::collections::HashMap::new();
    for (field_name, raw) in req.fields {
        let field_type = schema.fields.get(&field_name).ok_or_else(|| {
            format!(
                "field '{}' not defined in schema '{}'",
                field_name, req.schema
            )
        })?;
        let value = coerce_json_value_to_field_type(&raw, field_type)
            .map_err(|e| format!("Invalid value for '{}': {}", field_name, e))?;
        converted_fields.insert(field_name, value);
    }

    if replace {
        doc.fields = converted_fields;
        doc.increment_version();
    } else {
        for (field_name, value) in converted_fields {
            doc.set_field(field_name, value);
        }
    }

    schema
        .validate_document(&doc.fields)
        .map_err(|e| e.to_string())?;
    validate_document_constraints_for_store(&store, &schema, &doc, Some(&doc.id))?;
    store.put_document(&doc).map_err(|e| e.to_string())?;
    Ok(document_to_json(&doc))
}

pub fn duplicate_document(
    db: &DatabaseEntry,
    req: DuplicateDocumentRequest,
) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    let source = store
        .get_document(&req.schema, &req.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("document '{}' not found", req.id))?;

    let mut fields = source.fields.clone();
    if let Some(Value::String(id_value)) = fields.get_mut("id") {
        let suffix = Utc::now().timestamp_millis();
        *id_value = format!("{}_copy_{}", id_value, suffix);
    }

    let duplicated = Document::new(source.schema.clone(), fields);
    let schema = store
        .get_schema(&req.schema)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("schema '{}' not found", req.schema))?;
    schema
        .validate_document(&duplicated.fields)
        .map_err(|e| e.to_string())?;
    validate_document_constraints_for_store(&store, &schema, &duplicated, None)?;
    store.put_document(&duplicated).map_err(|e| e.to_string())?;
    Ok(document_to_json(&duplicated))
}

pub fn delete_document(db: &DatabaseEntry, req: DeleteDocumentRequest) -> Result<bool, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let deleted = store
        .delete_document(&req.schema, &req.id)
        .map_err(|e| e.to_string())?;
    if deleted {
        return Ok(true);
    }

    if let Some(canonical_id) = store
        .resolve_index_id_for_payload_id(&req.schema, &req.id)
        .map_err(|e| e.to_string())?
    {
        return store
            .delete_document(&req.schema, &canonical_id)
            .map_err(|e| e.to_string());
    }

    Ok(false)
}

pub fn delete_all_documents(
    db: &DatabaseEntry,
    req: DeleteAllDocumentsRequest,
) -> Result<usize, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;
    let docs = store
        .scan_documents(&req.schema)
        .map_err(|e| e.to_string())?;
    let mut deleted = 0usize;
    for doc in docs {
        let mut removed = store
            .delete_document(&req.schema, &doc.id)
            .map_err(|e| e.to_string())?;
        if !removed {
            if let Some(canonical_id) = store
                .resolve_index_id_for_payload_id(&req.schema, &doc.id)
                .map_err(|e| e.to_string())?
            {
                removed = store
                    .delete_document(&req.schema, &canonical_id)
                    .map_err(|e| e.to_string())?;
            }
        }
        if removed {
            deleted += 1;
        }
    }
    Ok(deleted)
}

pub fn delete_collection(
    db: &DatabaseEntry,
    req: DeleteCollectionRequest,
) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    if store
        .get_schema(&req.name)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Err(format!("collection '{}' not found", req.name));
    }

    let docs = store.scan_documents(&req.name).map_err(|e| e.to_string())?;
    let mut deleted_docs = 0usize;
    for doc in docs {
        let mut removed = store
            .delete_document(&req.name, &doc.id)
            .map_err(|e| e.to_string())?;
        if !removed {
            if let Some(canonical_id) = store
                .resolve_index_id_for_payload_id(&req.name, &doc.id)
                .map_err(|e| e.to_string())?
            {
                removed = store
                    .delete_document(&req.name, &canonical_id)
                    .map_err(|e| e.to_string())?;
            }
        }
        if removed {
            deleted_docs += 1;
        }
    }

    let schema_deleted = store.delete_schema(&req.name).map_err(|e| e.to_string())?;
    Ok(json!({
        "collection": req.name,
        "deleted_documents": deleted_docs,
        "deleted_schema": schema_deleted,
    }))
}

pub fn import_data(
    db: &DatabaseEntry,
    req: ImportDataRequest,
) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = ZigStore::new(&db_path_str, size)
        .map_err(|e| format!("{} (path: {})", e, db_path.display()))?;

    let schema_name = req.schema.trim();
    if schema_name.is_empty() {
        return Err("schema is required".to_string());
    }
    if req.fields.is_empty() {
        return Err("import requires at least one schema field".to_string());
    }

    let mut schema_fields = std::collections::HashMap::new();
    for field in &req.fields {
        let name = field.name.trim();
        if name.is_empty() {
            return Err("field name cannot be empty".to_string());
        }
        schema_fields.insert(name.to_string(), parse_field_type(&field.r#type)?);
    }
    let constraints = parse_constraint_defs(req.constraints)?;
    let schema = Schema::new_with_constraints(schema_name.to_string(), schema_fields, constraints);

    let strategy = req
        .strategy
        .unwrap_or_else(|| "merge".to_string())
        .to_ascii_lowercase();
    if strategy == "replace" {
        let docs = store
            .scan_documents(schema_name)
            .map_err(|e| e.to_string())?;
        for doc in docs {
            let _ = store.delete_document(schema_name, &doc.id);
        }
    }

    store.put_schema(&schema).map_err(|e| e.to_string())?;

    let mut imported = 0usize;
    let mut errors = Vec::new();
    for (idx, doc_raw) in req.documents.iter().enumerate() {
        let mut row_errors = Vec::new();
        let mut fields = std::collections::HashMap::new();
        for (field_name, field_type) in &schema.fields {
            let Some(value_raw) = doc_raw.get(field_name) else {
                if !field_type.is_optional() {
                    row_errors.push(format!(
                        "row {} missing required field '{}'",
                        idx + 1,
                        field_name
                    ));
                }
                continue;
            };
            match coerce_json_value_to_field_type(value_raw, field_type) {
                Ok(v) => {
                    fields.insert(field_name.clone(), v);
                }
                Err(e) => {
                    row_errors.push(format!("row {} invalid '{}': {}", idx + 1, field_name, e))
                }
            }
        }
        if !row_errors.is_empty() {
            errors.extend(row_errors);
            continue;
        }
        let doc = Document::new(schema_name.to_string(), fields);
        match validate_document_constraints_for_store(&store, &schema, &doc, None)
            .and_then(|_| store.put_document(&doc).map_err(|e| e.to_string()))
        {
            Ok(_) => imported += 1,
            Err(e) => errors.push(format!("row {} store error: {}", idx + 1, e)),
        }
    }

    Ok(json!({
        "schema": schema_name,
        "imported": imported,
        "errors": errors,
    }))
}

pub fn seed_feature_demo(db: &DatabaseEntry) -> Result<serde_json::Value, String> {
    let size = db_size_bytes(db.size_mb);
    let db_path = resolve_db_path(&db.path);
    let db_path_str = db_path.to_string_lossy().to_string();
    let store = Arc::new(
        ZigStore::new(&db_path_str, size)
            .map_err(|e| format!("{} (path: {})", e, db_path.display()))?,
    );
    let executor = QueryExecutor::new_with_real_clock(store.clone());

    let mut user_fields = std::collections::HashMap::new();
    user_fields.insert("id".to_string(), FieldType::String);
    user_fields.insert("name".to_string(), FieldType::String);
    user_fields.insert("profile".to_string(), FieldType::Object);
    user_fields.insert(
        "tags".to_string(),
        FieldType::List(Box::new(FieldType::String)),
    );
    store
        .put_schema(&Schema::new("users".to_string(), user_fields))
        .map_err(|e| e.to_string())?;

    let mut order_fields = std::collections::HashMap::new();
    order_fields.insert("id".to_string(), FieldType::String);
    order_fields.insert("user".to_string(), parse_field_type("Ref(users)")?);
    order_fields.insert("amount".to_string(), FieldType::Float);
    order_fields.insert("meta".to_string(), FieldType::Object);
    store
        .put_schema(&Schema::new("orders".to_string(), order_fields))
        .map_err(|e| e.to_string())?;

    let users_insert = r#"
users.insert [
  { id: "u1", name: "Ada", profile: { city: "Lagos", prefs: { theme: "light", alerts: true } }, tags: ["pro", "beta"] },
  { id: "u2", name: "Mina", profile: { city: "Abuja", prefs: { theme: "dark", alerts: false } }, tags: ["starter"] }
]
"#;
    let q1 = parse_oql(users_insert).map_err(|e| e.to_string())?;
    executor.execute_query(&q1).map_err(|e| e.to_string())?;

    let orders_insert = r#"
orders.insert [
  { id: "o1", user: "u1", amount: 320.5, meta: { channel: "web", item_count: 3 } },
  { id: "o2", user: "u1", amount: 120.0, meta: { channel: "mobile", item_count: 1 } },
  { id: "o3", user: "u2", amount: 80.0, meta: { channel: "web", item_count: 2 } }
]
"#;
    let q2 = parse_oql(orders_insert).map_err(|e| e.to_string())?;
    executor.execute_query(&q2).map_err(|e| e.to_string())?;

    Ok(json!({
        "ok": true,
        "seeded_collections": ["users", "orders"],
        "notes": [
            "batch insert via insert [ {...}, {...} ]",
            "references via Ref(users) and user: \"u1\" conversion",
            "nested objects via FieldType::Object and object literals"
        ]
    }))
}

pub fn build_metrics_payload(registry: &Registry, target_db: Option<&str>) -> serde_json::Value {
    let uptime = SERVER_STARTED_AT.elapsed().as_secs_f64();
    let storage_size_bytes = collect_storage_size_bytes(registry, target_db);
    let stats = RUNTIME_STATS.lock().map(|s| s.clone()).unwrap_or_default();
    let total_queries = stats.total_queries;
    let successful_queries = stats.successful_queries;
    let failed_queries = total_queries.saturating_sub(successful_queries);
    let qps = if uptime > 0.0 {
        total_queries as f64 / uptime
    } else {
        0.0
    };
    let avg_ms = if total_queries > 0 {
        stats.total_query_time_ms / total_queries as f64
    } else {
        0.0
    };
    let success_rate = if total_queries > 0 {
        successful_queries as f64 / total_queries as f64
    } else {
        1.0
    };
    let error_rate = if total_queries > 0 {
        failed_queries as f64 / total_queries as f64
    } else {
        0.0
    };
    let active_alerts = if error_rate > 0.05 {
        vec![json!({
            "id": "high-error-rate",
            "severity": "Warning",
            "title": "Elevated query error rate",
            "message": format!("Error rate is {:.2}% over process lifetime", error_rate * 100.0),
            "timestamp": Utc::now().to_rfc3339(),
            "resolved": false
        })]
    } else {
        Vec::new()
    };

    json!({
        "system_metrics": {
            "timestamp": Utc::now().to_rfc3339(),
            "active_connections": 1,
            "queries_per_second": qps,
            "avg_query_time_ms": avg_ms,
            "storage_size_bytes": storage_size_bytes,
            "memory_usage_bytes": storage_size_bytes.saturating_div(2),
            "cache_hit_rate": 0.0,
            "error_rate": error_rate,
            "uptime_seconds": uptime as u64
        },
        "query_stats": {
            "total_queries": total_queries,
            "successful_queries": successful_queries,
            "avg_execution_time_ms": avg_ms,
            "success_rate": success_rate,
            "cache_hit_rate": 0.0,
            "queries_per_second": qps
        },
        "active_alerts": active_alerts
    })
}

pub fn build_cluster_topology_payload(
    registry: &Registry,
    target_db: Option<&str>,
) -> serde_json::Value {
    let metrics = build_metrics_payload(registry, target_db);
    let cpu_load = metrics["system_metrics"]["queries_per_second"]
        .as_f64()
        .unwrap_or(0.0)
        * 2.0;
    let ram_load = if metrics["system_metrics"]["storage_size_bytes"]
        .as_u64()
        .unwrap_or(0)
        == 0
    {
        5.0
    } else {
        25.0
    };
    let nodes = vec![json!({
        "id": "owl-node-local",
        "label": "owl-node-local",
        "role": "primary",
        "status": "healthy",
        "shardCount": 1,
        "metrics": {
            "cpu": cpu_load.clamp(0.0, 100.0),
            "ram": ram_load
        }
    })];
    json!({
        "nodes": nodes,
        "edges": []
    })
}

pub fn ai_chat_response(messages: &[AIChatMessage]) -> serde_json::Value {
    let last = messages
        .last()
        .map(|m| m.content.to_lowercase())
        .unwrap_or_default();
    let (content, suggested_actions) = if last.contains("users") || last.contains("find") {
        (
            "I prepared a query for users. You can run it directly.",
            vec![json!({
                "label": "Run Query",
                "action": "run-query",
                "payload": "users.where { .age > 25 }"
            })],
        )
    } else if last.contains("orders") {
        (
            "I prepared an orders query for you.",
            vec![json!({
                "label": "Run Query",
                "action": "run-query",
                "payload": "orders.select { id, amount, status }"
            })],
        )
    } else {
        (
            "I can help with OQL. Ask for filters, projections, inserts, or schema operations.",
            Vec::new(),
        )
    };
    json!({
        "message": {
            "id": format!("ai-{}", RESPONSE_COUNTER.fetch_add(1, Ordering::Relaxed)),
            "role": "assistant",
            "content": content,
            "timestamp": Utc::now().timestamp_millis(),
            "steps": [
                {
                    "id": "step-1",
                    "type": "analysis",
                    "content": "Analyzed prompt and generated backend response.",
                    "timestamp": Utc::now().timestamp_millis()
                }
            ]
        },
        "suggestedActions": suggested_actions
    })
}
