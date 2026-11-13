use anyhow::Result;
use owl_db::storage::{ScalarIndexDefinition, WriteOperation};
use owl_db::storage_engine::RustStore;
use owl_db::{DocumentStore, FieldType, Schema};
use std::collections::HashMap;

pub fn bootstrap(store: &RustStore) -> Result<()> {
    let indexes = indexes();
    if store.get_schema("events")?.is_none() {
        let mut operations = vec![
            WriteOperation::PutSchema(Schema::new("events".into(), event_fields())),
            WriteOperation::PutSchema(Schema::new("issues".into(), issue_fields())),
        ];
        operations.extend(indexes.into_iter().map(WriteOperation::PutScalarIndex));
        store.write_batch(operations)?;
    } else {
        for index in indexes {
            if !store
                .list_scalar_indexes(Some(&index.schema))?
                .iter()
                .any(|item| item == &index)
            {
                store.upsert_scalar_index(index)?;
            }
        }
    }
    Ok(())
}

fn optional(inner: FieldType) -> FieldType {
    FieldType::Optional(Box::new(inner))
}

fn event_fields() -> HashMap<String, FieldType> {
    HashMap::from([
        ("application".into(), FieldType::String),
        ("environment".into(), FieldType::String),
        ("occurred_at".into(), FieldType::DateTime),
        ("received_at".into(), FieldType::DateTime),
        ("level".into(), FieldType::String),
        ("level_rank".into(), FieldType::Integer),
        ("kind".into(), FieldType::String),
        ("message".into(), FieldType::String),
        ("exception_type".into(), optional(FieldType::String)),
        ("fingerprint".into(), optional(FieldType::String)),
        ("trace_id".into(), optional(FieldType::String)),
        ("span_id".into(), optional(FieldType::String)),
        ("release".into(), optional(FieldType::String)),
        ("stack".into(), FieldType::List(Box::new(FieldType::Object))),
        ("attributes".into(), FieldType::Object),
    ])
}

fn issue_fields() -> HashMap<String, FieldType> {
    HashMap::from([
        ("application".into(), FieldType::String),
        ("environment".into(), FieldType::String),
        ("fingerprint".into(), FieldType::String),
        ("exception_type".into(), optional(FieldType::String)),
        ("title".into(), FieldType::String),
        ("status".into(), FieldType::String),
        ("first_seen".into(), FieldType::DateTime),
        ("last_seen".into(), FieldType::DateTime),
        ("occurrences".into(), FieldType::Integer),
        ("resolved_at".into(), optional(FieldType::DateTime)),
        ("sample_event_id".into(), FieldType::String),
        ("latest_event_id".into(), FieldType::String),
        ("first_release".into(), optional(FieldType::String)),
        ("last_release".into(), optional(FieldType::String)),
    ])
}

fn indexes() -> Vec<ScalarIndexDefinition> {
    [
        (
            "events_by_application_time",
            "events",
            &[
                "application",
                "environment",
                "occurred_at",
                "level_rank",
                "id",
            ][..],
            false,
        ),
        (
            "events_by_level_time",
            "events",
            &[
                "application",
                "environment",
                "level_rank",
                "occurred_at",
                "id",
            ][..],
            false,
        ),
        (
            "events_by_fingerprint_time",
            "events",
            &[
                "application",
                "environment",
                "fingerprint",
                "occurred_at",
                "id",
            ][..],
            false,
        ),
        (
            "events_by_trace_time",
            "events",
            &["trace_id", "occurred_at", "id"][..],
            false,
        ),
        (
            "issues_by_status_activity",
            "issues",
            &["application", "environment", "status", "last_seen", "id"][..],
            false,
        ),
        (
            "issues_by_fingerprint",
            "issues",
            &["application", "environment", "fingerprint"][..],
            true,
        ),
    ]
    .into_iter()
    .map(|(name, schema, fields, unique)| ScalarIndexDefinition {
        name: name.into(),
        schema: schema.into(),
        fields: fields.iter().map(|field| (*field).into()).collect(),
        unique,
    })
    .collect()
}
