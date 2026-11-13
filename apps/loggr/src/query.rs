use crate::model::{event_summary, EventSummary, QueryResponse};
use anyhow::{anyhow, Result};
use owl_db::storage::{
    ProjectedRowRef, ProjectedValueRef, ScalarIndexBound, ScalarIndexCursor, ScalarIndexDirection,
    ScalarIndexRange,
};
use owl_db::storage_engine::RustStore;
use owl_db::{parse_oql, Document, DocumentStore, Operation, QueryExecutor, Value};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;
use std::time::Instant;

#[derive(Debug, Clone, Deserialize)]
pub struct EventFilter {
    pub application: Option<String>,
    pub environment: Option<String>,
    pub level: Option<String>,
    pub fingerprint: Option<String>,
    pub trace_id: Option<String>,
    pub text: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssueFilter {
    pub application: String,
    pub environment: String,
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub fingerprint: String,
    pub last_seen: String,
    pub occurrences: i64,
    pub latest_event_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OqlRequest {
    pub query: String,
}

const STREAM_FIELDS: &[&str] = &[
    "application",
    "environment",
    "occurred_at",
    "level",
    "message",
    "fingerprint",
    "trace_id",
    "release",
];

pub fn events(store: &RustStore, filter: EventFilter) -> Result<QueryResponse<EventSummary>> {
    let started = Instant::now();
    let limit = filter.limit.unwrap_or(200).clamp(1, 200);
    let (index, prefix, lower) = event_access_path(&filter)?;
    let cursor = filter
        .cursor
        .as_deref()
        .map(ScalarIndexCursor::decode)
        .transpose()?;
    let upper = ScalarIndexBound {
        values: prefix.clone(),
        inclusive: false,
        prefix_end: true,
    };
    let lower = ScalarIndexBound {
        values: lower.unwrap_or_else(|| prefix.clone()),
        inclusive: true,
        prefix_end: false,
    };
    let range = ScalarIndexRange {
        lower: Some(lower),
        upper: Some(upper),
        limit: Some(if filter.text.is_some() {
            limit.saturating_mul(10)
        } else {
            limit
        }),
        after: cursor,
        direction: ScalarIndexDirection::Descending,
    };
    let page = store
        .scan_scalar_index("events", index, &range)?
        .ok_or_else(|| anyhow!("Owl does not expose a scalar index scan for {index}"))?;
    let fields = STREAM_FIELDS
        .iter()
        .map(|field| (*field).to_string())
        .collect::<Vec<_>>();
    let needle = filter.text.as_deref().map(str::to_lowercase);
    let mut data = page
        .object_refs
        .iter()
        .filter_map(|object_ref| {
            store
                .navigate_reference_projection(object_ref, &fields)
                .ok()
                .flatten()
        })
        .filter_map(|document| event_summary(&document))
        .filter(|event| {
            needle
                .as_ref()
                .is_none_or(|needle| event.message.to_lowercase().contains(needle))
        })
        .collect::<Vec<_>>();
    data.sort_by(|left, right| right.occurred_at.cmp(&left.occurred_at));
    data.truncate(limit);
    Ok(QueryResponse {
        data,
        next_cursor: page.next_cursor.map(|cursor| cursor.encode()).transpose()?,
        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        access_path: format!("typed scalar index · {index} · projection 8/15 fields"),
        oql: event_oql(&filter, limit),
    })
}

pub fn issues(store: &RustStore, filter: IssueFilter) -> Result<QueryResponse<IssueSummary>> {
    let started = Instant::now();
    let limit = filter.limit.unwrap_or(50).clamp(1, 100);
    let status = filter.status.unwrap_or_else(|| "unresolved".into());
    let prefix = vec![
        Value::String(filter.application.clone()),
        Value::String(filter.environment.clone()),
        Value::String(status.clone()),
    ];
    let range = ScalarIndexRange {
        lower: Some(ScalarIndexBound {
            values: prefix.clone(),
            inclusive: true,
            prefix_end: false,
        }),
        upper: Some(ScalarIndexBound {
            values: prefix,
            inclusive: false,
            prefix_end: true,
        }),
        limit: Some(limit),
        after: filter
            .cursor
            .as_deref()
            .map(ScalarIndexCursor::decode)
            .transpose()?,
        direction: ScalarIndexDirection::Descending,
    };
    let page = store
        .scan_scalar_index("issues", "issues_by_status_activity", &range)?
        .ok_or_else(|| anyhow!("issues index is unavailable"))?;
    let data = page
        .object_refs
        .iter()
        .filter_map(|object_ref| store.navigate_reference(object_ref).ok().flatten())
        .filter_map(issue_summary)
        .collect();
    Ok(QueryResponse {
        data,
        next_cursor: page.next_cursor.map(|cursor| cursor.encode()).transpose()?,
        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        access_path: "typed scalar index · issues_by_status_activity".into(),
        oql: format!("issues.where {{ .application == {:?} && .environment == {:?} && .status == {:?} }}.sort {{ .last_seen desc }}.limit {limit}", filter.application, filter.environment, status),
    })
}

pub fn event_detail(store: &RustStore, id: &str) -> Result<Option<serde_json::Value>> {
    Ok(store
        .get_document("events", &id.to_string())?
        .map(document_json))
}

pub fn issue_detail(store: &RustStore, fingerprint: &str) -> Result<Option<serde_json::Value>> {
    Ok(store
        .get_document("issues", &fingerprint.to_string())?
        .map(document_json))
}

pub fn execute_oql(store: std::sync::Arc<RustStore>, source: String) -> Result<Vec<u8>> {
    if source.len() > 64 * 1024 {
        return Err(anyhow!("OQL query exceeds 64 KiB"));
    }
    let query = parse_oql(&source).map_err(|error| anyhow!(error.to_string()))?;
    if query.operations.iter().any(|operation| {
        matches!(
            operation,
            Operation::Insert(_)
                | Operation::InsertMany(_)
                | Operation::Update(_)
                | Operation::Delete
                | Operation::FunctionDef(_)
        )
    }) {
        return Err(anyhow!("the Loggr workbench accepts read-only OQL queries"));
    }
    let started = Instant::now();
    let executor = QueryExecutor::new_with_real_clock(store);
    let execution = executor.execute_query_batches(&query)?;
    let planning_ms = execution.planning_elapsed.as_secs_f64() * 1_000.0;
    let execution_ms = execution.execution_elapsed.as_secs_f64() * 1_000.0;
    let count = execution.len();
    let physical_plan = execution
        .physical_plan
        .unwrap_or_else(|| "general OQL executor".into());
    let access_path = summarize_plan(&physical_plan);
    let serialization_started = Instant::now();
    let data = serde_json::to_vec(&BatchRows(&execution.chunks))?;
    let serialization_ms = serialization_started.elapsed().as_secs_f64() * 1_000.0;
    let metadata = json!({
        "count": count,
        "elapsed_ms": started.elapsed().as_secs_f64() * 1_000.0,
        "planning_ms": planning_ms,
        "execution_ms": execution_ms,
        "materialization_ms": 0.0,
        "serialization_ms": serialization_ms,
        "access_path": access_path,
        "plan": physical_plan,
        "profile": serde_json::Value::Null,
        "oql": source,
    });
    let metadata = serde_json::to_vec(&metadata)?;
    let mut response = Vec::with_capacity(data.len() + metadata.len() + 9);
    response.extend_from_slice(br#"{"data":"#);
    response.extend_from_slice(&data);
    response.push(b',');
    response.extend_from_slice(&metadata[1..]);
    Ok(response)
}

struct BatchRows<'a>(&'a [owl_db::execution::DataChunk]);

impl Serialize for BatchRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let row_count = self.0.iter().map(|chunk| chunk.len()).sum();
        let mut sequence = serializer.serialize_seq(Some(row_count))?;
        for chunk in self.0 {
            if let Some(documents) = chunk.documents() {
                for document in documents {
                    sequence.serialize_element(&DocumentRef(document))?;
                }
            } else if let Some(rows) = chunk.projected_rows() {
                for row in rows {
                    sequence.serialize_element(&ProjectedDocumentRef(row))?;
                }
            }
        }
        sequence.end()
    }
}

struct DocumentRef<'a>(&'a Document);

impl Serialize for DocumentRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let document = self.0;
        let mut map = serializer.serialize_map(Some(6))?;
        map.serialize_entry("id", &document.id)?;
        map.serialize_entry("schema", &document.schema)?;
        map.serialize_entry("version", &document.version)?;
        map.serialize_entry("created_at", &document.created_at)?;
        map.serialize_entry("updated_at", &document.updated_at)?;
        map.serialize_entry("fields", &DocumentFields(document))?;
        map.end()
    }
}

struct ProjectedDocumentRef<'a>(ProjectedRowRef<'a>);

impl Serialize for ProjectedDocumentRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let row = self.0;
        let mut map = serializer.serialize_map(Some(6))?;
        map.serialize_entry("id", row.id())?;
        map.serialize_entry("schema", row.schema())?;
        map.serialize_entry("version", &row.version())?;
        map.serialize_entry("created_at", &row.created_at())?;
        map.serialize_entry("updated_at", &row.updated_at())?;
        map.serialize_entry("fields", &ProjectedFields(row))?;
        map.end()
    }
}

struct DocumentFields<'a>(&'a Document);

impl Serialize for DocumentFields<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.fields.len()))?;
        for (name, value) in &self.0.fields {
            map.serialize_entry(name, &CleanValue(value))?;
        }
        map.end()
    }
}

struct ProjectedFields<'a>(ProjectedRowRef<'a>);

impl Serialize for ProjectedFields<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (name, value) in self.0.fields() {
            map.serialize_entry(name, &CleanProjectedValue(value))?;
        }
        map.end()
    }
}

struct CleanProjectedValue<'a>(ProjectedValueRef<'a>);

impl Serialize for CleanProjectedValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            ProjectedValueRef::Value(value) => CleanValue(value).serialize(serializer),
            ProjectedValueRef::Null => serializer.serialize_none(),
            ProjectedValueRef::String(value) => serializer.serialize_str(value),
            ProjectedValueRef::Integer(value) => serializer.serialize_i64(value),
            ProjectedValueRef::Float(value) => serializer.serialize_f64(value),
            ProjectedValueRef::Boolean(value) => serializer.serialize_bool(value),
            ProjectedValueRef::DateTime(value) => value.serialize(serializer),
            ProjectedValueRef::Reference(value) => serializer.serialize_str(&value.to_string_id()),
        }
    }
}

struct CleanValue<'a>(&'a Value);

impl Serialize for CleanValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::String(value) => serializer.serialize_str(value),
            Value::Integer(value) => serializer.serialize_i64(*value),
            Value::Float(value) => serializer.serialize_f64(*value),
            Value::Boolean(value) => serializer.serialize_bool(*value),
            Value::DateTime(value) => value.serialize(serializer),
            Value::Reference(value) => serializer.serialize_str(&value.to_string_id()),
            Value::GlobalReference(value) => serializer.serialize_str(&value.to_string_id()),
            Value::List(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&CleanValue(value))?;
                }
                sequence.end()
            }
            Value::Object(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (name, value) in values {
                    map.serialize_entry(name, &CleanValue(value))?;
                }
                map.end()
            }
            Value::Null => serializer.serialize_none(),
        }
    }
}

fn summarize_plan(plan: &str) -> String {
    if plan == "general OQL executor" {
        return plan.into();
    }
    let index = plan
        .split_whitespace()
        .find_map(|part| part.strip_prefix("index="))
        .unwrap_or("native scan");
    let direction = plan
        .split_whitespace()
        .find_map(|part| part.strip_prefix("direction="))
        .unwrap_or("planned");
    format!("Owl physical plan · {index} · {}", direction.to_lowercase())
}

fn event_access_path(
    filter: &EventFilter,
) -> Result<(&'static str, Vec<Value>, Option<Vec<Value>>)> {
    if let Some(trace_id) = &filter.trace_id {
        return Ok((
            "events_by_trace_time",
            vec![Value::String(trace_id.clone())],
            None,
        ));
    }
    let application = filter
        .application
        .as_ref()
        .ok_or_else(|| anyhow!("application is required"))?;
    let environment = filter
        .environment
        .as_ref()
        .ok_or_else(|| anyhow!("environment is required"))?;
    let base = vec![
        Value::String(application.clone()),
        Value::String(environment.clone()),
    ];
    if let Some(fingerprint) = &filter.fingerprint {
        let mut prefix = base;
        prefix.push(Value::String(fingerprint.clone()));
        return Ok(("events_by_fingerprint_time", prefix, None));
    }
    if let Some(level) = &filter.level {
        let mut lower = base.clone();
        lower.push(Value::Integer(level_rank(level)?));
        return Ok(("events_by_level_time", base, Some(lower)));
    }
    Ok(("events_by_application_time", base, None))
}

fn level_rank(level: &str) -> Result<i64> {
    match level.to_lowercase().as_str() {
        "trace" => Ok(10),
        "debug" => Ok(20),
        "info" => Ok(30),
        "warn" | "warning" => Ok(40),
        "error" => Ok(50),
        "fatal" => Ok(60),
        _ => Err(anyhow!("unknown level '{level}'")),
    }
}

fn event_oql(filter: &EventFilter, limit: usize) -> String {
    let mut clauses = Vec::new();
    if let Some(value) = &filter.application {
        clauses.push(format!(".application == {:?}", value));
    }
    if let Some(value) = &filter.environment {
        clauses.push(format!(".environment == {:?}", value));
    }
    if let Some(value) = &filter.level {
        if let Ok(rank) = level_rank(value) {
            clauses.push(format!(".level_rank >= {rank}"));
        }
    }
    if let Some(value) = &filter.fingerprint {
        clauses.push(format!(".fingerprint == {:?}", value));
    }
    if let Some(value) = &filter.trace_id {
        clauses.push(format!(".trace_id == {:?}", value));
    }
    format!("events.where {{ {} }}.sort {{ .occurred_at desc }}.select {{ id, occurred_at, level, message, fingerprint, trace_id, release }}.limit {limit}", clauses.join(" && "))
}

fn issue_summary(document: Document) -> Option<IssueSummary> {
    Some(IssueSummary {
        id: document.id,
        title: document.fields.get("title")?.as_string()?.to_string(),
        status: document.fields.get("status")?.as_string()?.to_string(),
        fingerprint: document.fields.get("fingerprint")?.as_string()?.to_string(),
        last_seen: match document.fields.get("last_seen")? {
            Value::DateTime(value) => value.to_rfc3339(),
            _ => return None,
        },
        occurrences: document.fields.get("occurrences")?.as_integer()?,
        latest_event_id: document
            .fields
            .get("latest_event_id")?
            .as_string()?
            .to_string(),
    })
}

pub fn document_json(document: Document) -> serde_json::Value {
    let fields = document
        .fields
        .into_iter()
        .map(|(key, value)| (key, value_json(value)))
        .collect::<serde_json::Map<_, _>>();
    json!({ "id": document.id, "schema": document.schema, "version": document.version, "created_at": document.created_at, "updated_at": document.updated_at, "fields": fields })
}

fn value_json(value: Value) -> serde_json::Value {
    match value {
        Value::String(value) => json!(value),
        Value::Integer(value) => json!(value),
        Value::Float(value) => json!(value),
        Value::Boolean(value) => json!(value),
        Value::DateTime(value) => json!(value),
        Value::Reference(value) => json!(value.to_string_id()),
        Value::GlobalReference(value) => json!(value.to_string_id()),
        Value::List(values) => {
            serde_json::Value::Array(values.into_iter().map(value_json).collect())
        }
        Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, value_json(value)))
                .collect(),
        ),
        Value::Null => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use owl_db::{FieldType, Schema};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn batch_oql_response_preserves_clean_json_shape_and_projection() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            RustStore::new(
                directory.path().join("batch-response.owl"),
                16 * 1024 * 1024,
            )
            .unwrap(),
        );
        store
            .put_schema(&Schema::new(
                "events".into(),
                HashMap::from([
                    ("message".into(), FieldType::String),
                    ("sequence".into(), FieldType::Integer),
                    (
                        "optional".into(),
                        FieldType::Optional(Box::new(FieldType::String)),
                    ),
                ]),
            ))
            .unwrap();
        store
            .put_document(&Document::with_id(
                "event-1".into(),
                "events".into(),
                HashMap::from([
                    ("message".into(), Value::String("boom".into())),
                    ("sequence".into(), Value::Integer(1)),
                    ("optional".into(), Value::Null),
                ]),
            ))
            .unwrap();

        let response = execute_oql(
            store,
            "events.where { .sequence >= 0 }.select { message, optional }".into(),
        )
        .unwrap();
        let response: serde_json::Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["count"], 1);
        assert_eq!(response["data"][0]["id"], "event-1");
        assert_eq!(response["data"][0]["schema"], "events");
        assert_eq!(response["data"][0]["fields"]["message"], "boom");
        assert!(response["data"][0]["fields"]["optional"].is_null());
        assert!(response["data"][0]["fields"].get("sequence").is_none());
        assert_eq!(response["materialization_ms"], 0.0);
        assert!(response["serialization_ms"].is_number());
    }
}
