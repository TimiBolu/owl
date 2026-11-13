use crate::fingerprint::fingerprint_v1;
use crate::model::{IngestEvent, IngestResult, StackFrame};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use owl_db::storage::WriteOperation;
use owl_db::storage_engine::RustStore;
use owl_db::{Document, DocumentStore, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use std::time::{Duration as StdDuration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

const QUEUE_CAPACITY: usize = 1_024;
pub const MAX_BATCH_EVENTS: usize = 256;

struct Command {
    events: Vec<PreparedEvent>,
    response: oneshot::Sender<Result<Vec<IngestResult>, String>>,
}

#[derive(Clone)]
pub struct IngestService {
    sender: mpsc::SyncSender<Command>,
}

#[derive(Debug, Clone)]
struct PreparedEvent {
    id: String,
    application: String,
    environment: String,
    occurred_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
    level: String,
    level_rank: i64,
    kind: String,
    message: String,
    exception_type: Option<String>,
    fingerprint: Option<String>,
    trace_id: Option<String>,
    span_id: Option<String>,
    release: Option<String>,
    stack: Vec<StackFrame>,
    attributes: HashMap<String, serde_json::Value>,
}

#[derive(Debug)]
pub enum SubmitError {
    Invalid(String),
    Saturated,
    Unavailable(String),
}

impl IngestService {
    pub fn start(store: Arc<RustStore>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Command>(QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("loggr-writer".into())
            .spawn(move || writer_loop(store, receiver))
            .context("start Loggr writer")?;
        Ok(Self { sender })
    }

    pub async fn submit(&self, events: Vec<IngestEvent>) -> Result<Vec<IngestResult>, SubmitError> {
        if events.is_empty() || events.len() > MAX_BATCH_EVENTS {
            return Err(SubmitError::Invalid(format!(
                "a batch must contain 1..={MAX_BATCH_EVENTS} events"
            )));
        }
        let prepared = events
            .into_iter()
            .map(PreparedEvent::try_from)
            .collect::<Result<Vec<_>>>()
            .map_err(|error| SubmitError::Invalid(error.to_string()))?;
        let (response, wait) = oneshot::channel();
        self.sender
            .try_send(Command {
                events: prepared,
                response,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => SubmitError::Saturated,
                mpsc::TrySendError::Disconnected(_) => {
                    SubmitError::Unavailable("ingest writer stopped".into())
                }
            })?;
        wait.await
            .map_err(|_| SubmitError::Unavailable("ingest writer stopped".into()))?
            .map_err(SubmitError::Unavailable)
    }
}

impl TryFrom<IngestEvent> for PreparedEvent {
    type Error = anyhow::Error;

    fn try_from(input: IngestEvent) -> Result<Self> {
        let application = bounded_required("application", input.application, 128)?;
        let environment = bounded_required("environment", input.environment, 64)?;
        let message = bounded_required("message", input.message, 32_768)?;
        let kind = bounded_required("kind", input.kind, 32)?.to_lowercase();
        if kind != "log" && kind != "exception" {
            return Err(anyhow!("kind must be log or exception"));
        }
        let (level, level_rank) = normalize_level(&input.level)?;
        let occurred_at = input.occurred_at.unwrap_or_else(Utc::now);
        if occurred_at > Utc::now() + Duration::hours(24) {
            return Err(anyhow!(
                "occurred_at may not be more than 24 hours in the future"
            ));
        }
        if input.stack.len() > 256 {
            return Err(anyhow!("stack may contain at most 256 frames"));
        }
        if input.attributes.len() > 256 {
            return Err(anyhow!("attributes may contain at most 256 keys"));
        }
        for value in input.attributes.values() {
            validate_json_depth(value, 0)?;
        }
        let exception_type = bounded_optional("exception_type", input.exception_type, 256)?;
        let computed = (kind == "exception" || exception_type.is_some()).then(|| {
            fingerprint_v1(
                &application,
                &environment,
                exception_type.as_deref(),
                &message,
                &input.stack,
            )
        });
        Ok(Self {
            id: input
                .id
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            application,
            environment,
            occurred_at,
            received_at: Utc::now(),
            level,
            level_rank,
            kind,
            message,
            exception_type,
            fingerprint: bounded_optional("fingerprint", input.fingerprint, 256)?.or(computed),
            trace_id: bounded_optional("trace_id", input.trace_id, 256)?,
            span_id: bounded_optional("span_id", input.span_id, 256)?,
            release: bounded_optional("release", input.release, 256)?,
            stack: input.stack,
            attributes: input.attributes,
        })
    }
}

fn writer_loop(store: Arc<RustStore>, receiver: mpsc::Receiver<Command>) {
    let mut pending = None;
    loop {
        let first = match pending.take().or_else(|| receiver.recv().ok()) {
            Some(command) => command,
            None => break,
        };
        let deadline = Instant::now() + StdDuration::from_millis(5);
        let mut event_count = first.events.len();
        let mut commands = vec![first];
        while event_count < MAX_BATCH_EVENTS {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match receiver.recv_timeout(remaining) {
                Ok(command) if event_count + command.events.len() <= MAX_BATCH_EVENTS => {
                    event_count += command.events.len();
                    commands.push(command);
                }
                Ok(command) => {
                    pending = Some(command);
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let events = commands
            .iter()
            .flat_map(|command| command.events.iter().cloned())
            .collect::<Vec<_>>();
        match commit_events(&store, &events) {
            Ok(results) => {
                let mut offset = 0;
                for command in commands {
                    let end = offset + command.events.len();
                    let _ = command.response.send(Ok(results[offset..end].to_vec()));
                    offset = end;
                }
            }
            Err(error) => {
                let message = error.to_string();
                for command in commands {
                    let _ = command.response.send(Err(message.clone()));
                }
            }
        }
    }
}

fn commit_events(store: &RustStore, events: &[PreparedEvent]) -> Result<Vec<IngestResult>> {
    let mut results = Vec::with_capacity(events.len());
    let mut operations = Vec::new();
    let mut issue_updates: HashMap<String, Document> = HashMap::new();
    let mut accepted_ids = HashSet::new();

    for event in events {
        let duplicate = !accepted_ids.insert(event.id.clone())
            || store.get_document("events", &event.id)?.is_some();
        results.push(IngestResult {
            id: event.id.clone(),
            fingerprint: event.fingerprint.clone(),
            duplicate,
        });
        if duplicate {
            continue;
        }

        operations.push(WriteOperation::PutDocument(event_document(event)?));
        if let Some(fingerprint) = &event.fingerprint {
            let issue = if let Some(current) = issue_updates.remove(fingerprint) {
                advance_issue(current, event)
            } else if let Some(existing) = store.get_document("issues", fingerprint)? {
                advance_issue(existing, event)
            } else {
                new_issue(event, fingerprint)
            };
            issue_updates.insert(fingerprint.clone(), issue);
        }
    }
    operations.extend(issue_updates.into_values().map(WriteOperation::PutDocument));
    if !operations.is_empty() {
        store.write_batch(operations)?;
    }
    Ok(results)
}

fn event_document(event: &PreparedEvent) -> Result<Document> {
    let stack = event.stack.iter().map(frame_value).collect();
    let attributes = event
        .attributes
        .iter()
        .map(|(key, value)| Ok((key.clone(), json_value(value)?)))
        .collect::<Result<HashMap<_, _>>>()?;
    Ok(Document::with_id(
        event.id.clone(),
        "events".into(),
        HashMap::from([
            (
                "application".into(),
                Value::String(event.application.clone()),
            ),
            (
                "environment".into(),
                Value::String(event.environment.clone()),
            ),
            ("occurred_at".into(), Value::DateTime(event.occurred_at)),
            ("received_at".into(), Value::DateTime(event.received_at)),
            ("level".into(), Value::String(event.level.clone())),
            ("level_rank".into(), Value::Integer(event.level_rank)),
            ("kind".into(), Value::String(event.kind.clone())),
            ("message".into(), Value::String(event.message.clone())),
            (
                "exception_type".into(),
                optional_value(&event.exception_type),
            ),
            ("fingerprint".into(), optional_value(&event.fingerprint)),
            ("trace_id".into(), optional_value(&event.trace_id)),
            ("span_id".into(), optional_value(&event.span_id)),
            ("release".into(), optional_value(&event.release)),
            ("stack".into(), Value::List(stack)),
            ("attributes".into(), Value::Object(attributes)),
        ]),
    ))
}

fn new_issue(event: &PreparedEvent, fingerprint: &str) -> Document {
    Document::with_id(
        fingerprint.into(),
        "issues".into(),
        HashMap::from([
            (
                "application".into(),
                Value::String(event.application.clone()),
            ),
            (
                "environment".into(),
                Value::String(event.environment.clone()),
            ),
            ("fingerprint".into(), Value::String(fingerprint.into())),
            (
                "exception_type".into(),
                optional_value(&event.exception_type),
            ),
            ("title".into(), Value::String(event.message.clone())),
            ("status".into(), Value::String("unresolved".into())),
            ("first_seen".into(), Value::DateTime(event.occurred_at)),
            ("last_seen".into(), Value::DateTime(event.occurred_at)),
            ("occurrences".into(), Value::Integer(1)),
            ("resolved_at".into(), Value::Null),
            ("sample_event_id".into(), Value::String(event.id.clone())),
            ("latest_event_id".into(), Value::String(event.id.clone())),
            ("first_release".into(), optional_value(&event.release)),
            ("last_release".into(), optional_value(&event.release)),
        ]),
    )
}

fn advance_issue(issue: Document, event: &PreparedEvent) -> Document {
    let occurrences = issue
        .fields
        .get("occurrences")
        .and_then(Value::as_integer)
        .unwrap_or(0)
        + 1;
    let first_seen = match issue.fields.get("first_seen") {
        Some(Value::DateTime(value)) => (*value).min(event.occurred_at),
        _ => event.occurred_at,
    };
    let previous_last_seen = match issue.fields.get("last_seen") {
        Some(Value::DateTime(value)) => *value,
        _ => event.occurred_at,
    };
    let is_latest = event.occurred_at >= previous_last_seen;
    let resolved = matches!(issue.fields.get("resolved_at"), Some(Value::DateTime(at)) if event.occurred_at > *at);
    let mut updates = HashMap::from([
        ("first_seen".into(), Value::DateTime(first_seen)),
        (
            "last_seen".into(),
            Value::DateTime(previous_last_seen.max(event.occurred_at)),
        ),
        ("occurrences".into(), Value::Integer(occurrences)),
        (
            "status".into(),
            Value::String(
                if resolved {
                    "regressed"
                } else {
                    issue
                        .fields
                        .get("status")
                        .and_then(Value::as_string)
                        .unwrap_or("unresolved")
                }
                .into(),
            ),
        ),
    ]);
    if is_latest {
        updates.insert("latest_event_id".into(), Value::String(event.id.clone()));
        updates.insert("last_release".into(), optional_value(&event.release));
    }
    issue.new_version_with_fields(updates)
}

fn frame_value(frame: &StackFrame) -> Value {
    Value::Object(HashMap::from([
        (
            "module".into(),
            frame
                .module
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        ("function".into(), Value::String(frame.function.clone())),
        ("file".into(), Value::String(frame.file.clone())),
        (
            "line".into(),
            frame
                .line
                .map(|v| Value::Integer(v as i64))
                .unwrap_or(Value::Null),
        ),
        (
            "column".into(),
            frame
                .column
                .map(|v| Value::Integer(v as i64))
                .unwrap_or(Value::Null),
        ),
        ("in_app".into(), Value::Boolean(frame.in_app)),
    ]))
}

fn optional_value(value: &Option<String>) -> Value {
    value.clone().map(Value::String).unwrap_or(Value::Null)
}

fn json_value(value: &serde_json::Value) -> Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(value) => Value::Boolean(*value),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(Value::Integer)
            .or_else(|| value.as_f64().map(Value::Float))
            .ok_or_else(|| anyhow!("unsupported JSON number"))?,
        serde_json::Value::String(value) => Value::String(value.clone()),
        serde_json::Value::Array(values) => {
            Value::List(values.iter().map(json_value).collect::<Result<_>>()?)
        }
        serde_json::Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), json_value(value)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

fn bounded_required(name: &str, value: String, max: usize) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{name} is required"));
    }
    if value.len() > max {
        return Err(anyhow!("{name} exceeds {max} bytes"));
    }
    Ok(trimmed.to_string())
}

fn bounded_optional(name: &str, value: Option<String>, max: usize) -> Result<Option<String>> {
    value
        .map(|value| bounded_required(name, value, max))
        .transpose()
}

fn normalize_level(level: &str) -> Result<(String, i64)> {
    let level = level.trim().to_lowercase();
    let rank = match level.as_str() {
        "trace" => 10,
        "debug" => 20,
        "info" => 30,
        "warn" | "warning" => 40,
        "error" => 50,
        "fatal" => 60,
        _ => return Err(anyhow!("unknown level '{level}'")),
    };
    Ok((
        if level == "warning" {
            "warn".into()
        } else {
            level
        },
        rank,
    ))
}

fn validate_json_depth(value: &serde_json::Value, depth: usize) -> Result<()> {
    if depth > 8 {
        return Err(anyhow!("attributes nesting exceeds 8 levels"));
    }
    match value {
        serde_json::Value::Object(object) => {
            for value in object.values() {
                validate_json_depth(value, depth + 1)?;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                validate_json_depth(item, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap;

    #[tokio::test]
    async fn duplicate_id_does_not_increment_issue() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            Arc::new(RustStore::new(dir.path().join("loggr.owl"), 32 * 1024 * 1024).unwrap());
        bootstrap::bootstrap(&store).unwrap();
        let service = IngestService::start(store.clone()).unwrap();
        let event = IngestEvent {
            id: Some("same".into()),
            application: "api".into(),
            environment: "prod".into(),
            occurred_at: None,
            level: "error".into(),
            kind: "exception".into(),
            message: "boom 123".into(),
            exception_type: Some("Boom".into()),
            fingerprint: None,
            trace_id: None,
            span_id: None,
            release: None,
            stack: vec![],
            attributes: HashMap::new(),
        };
        assert!(!service.submit(vec![event.clone()]).await.unwrap()[0].duplicate);
        assert!(service.submit(vec![event]).await.unwrap()[0].duplicate);
        let issues = store.scan_documents("issues").unwrap();
        assert_eq!(issues[0].fields["occurrences"], Value::Integer(1));
    }
}
