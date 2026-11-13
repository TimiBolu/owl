use chrono::{DateTime, Utc};
use owl_db::{Document, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackFrame {
    #[serde(default)]
    pub module: Option<String>,
    pub function: String,
    pub file: String,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub column: Option<u32>,
    #[serde(default)]
    pub in_app: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestEvent {
    #[serde(default)]
    pub id: Option<String>,
    pub application: String,
    pub environment: String,
    #[serde(default)]
    pub occurred_at: Option<DateTime<Utc>>,
    pub level: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub exception_type: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub release: Option<String>,
    #[serde(default)]
    pub stack: Vec<StackFrame>,
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

fn default_kind() -> String {
    "log".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestResult {
    pub id: String,
    pub fingerprint: Option<String>,
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResponse<T> {
    pub data: Vec<T>,
    pub next_cursor: Option<String>,
    pub elapsed_ms: f64,
    pub access_path: String,
    pub oql: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventSummary {
    pub id: String,
    pub application: String,
    pub environment: String,
    pub occurred_at: DateTime<Utc>,
    pub level: String,
    pub message: String,
    pub fingerprint: Option<String>,
    pub trace_id: Option<String>,
    pub release: Option<String>,
}

pub fn event_summary(document: &Document) -> Option<EventSummary> {
    Some(EventSummary {
        id: document.id.clone(),
        application: string(document, "application")?.to_string(),
        environment: string(document, "environment")?.to_string(),
        occurred_at: datetime(document, "occurred_at")?,
        level: string(document, "level")?.to_string(),
        message: string(document, "message")?.to_string(),
        fingerprint: optional_string(document, "fingerprint"),
        trace_id: optional_string(document, "trace_id"),
        release: optional_string(document, "release"),
    })
}

pub fn string<'a>(document: &'a Document, name: &str) -> Option<&'a str> {
    document.fields.get(name).and_then(Value::as_string)
}

pub fn optional_string(document: &Document, name: &str) -> Option<String> {
    string(document, name).map(str::to_string)
}

pub fn datetime(document: &Document, name: &str) -> Option<DateTime<Utc>> {
    match document.fields.get(name) {
        Some(Value::DateTime(value)) => Some(*value),
        _ => None,
    }
}
