//! Public storage contract and engine-neutral metadata.
//!
//! Owl has one storage implementation: [`RustStore`]. The storage boundary is
//! intentionally Rust-native so allocation, WAL ordering, object identity,
//! indexing, checkpointing, and recovery can share one correctness model.

use crate::distributed::GlobalObjRef;
use crate::document::{Document, DocumentId, ObjRef, Value};
use crate::error::{OQLError, OQLResult};
use crate::parser::ComparisonOperator;
use crate::schema::{DocumentSystemField, Schema};
use crate::storage_engine::{TypedCell, TypedSegment};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

pub use crate::storage_engine::{
    AutoCheckpointPolicy, GroupCommitter, MaintenanceStatus, RustStore, StorageLayoutStats,
    WriteOperation,
};

/// A planner-proven top-level scalar predicate that a storage scan may apply
/// before materializing a document.
#[derive(Debug, Clone)]
pub enum ScanPredicate {
    Comparison {
        field: String,
        operator: ComparisonOperator,
        value: crate::document::Value,
    },
    And(Box<ScanPredicate>, Box<ScanPredicate>),
    Or(Box<ScanPredicate>, Box<ScanPredicate>),
}

/// Delayed-materialization output from a storage-owned projected scan.
///
/// A live scan retains immutable shared rows until the executor reaches an
/// operator or API boundary that requires public documents. The owned
/// column-major representation supports storage implementations that already
/// produce values directly. In its columns, `None` is a missing field and
/// `Some(Value::Null)` is an explicitly present null.
#[derive(Debug)]
pub struct ProjectedRowBatch {
    schema: String,
    shared_rows: Option<Vec<Arc<Document>>>,
    typed_rows: Option<TypedProjectedRows>,
    ids: Vec<DocumentId>,
    created_at: Vec<DateTime<Utc>>,
    updated_at: Vec<DateTime<Utc>>,
    versions: Vec<u64>,
    columns: Vec<ProjectedColumn>,
}

#[derive(Debug)]
struct TypedProjectedRows {
    segment: Arc<TypedSegment>,
    indices: Vec<usize>,
    field_ids: Vec<Option<u16>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProjectedRowRef<'a> {
    batch: &'a ProjectedRowBatch,
    index: usize,
}

impl<'a> ProjectedRowRef<'a> {
    fn shared(&self) -> Option<&'a Document> {
        self.batch
            .shared_rows
            .as_ref()
            .map(|rows| rows[self.index].as_ref())
    }

    fn typed(&self) -> Option<(&'a TypedProjectedRows, usize)> {
        self.batch
            .typed_rows
            .as_ref()
            .map(|rows| (rows, rows.indices[self.index]))
    }

    pub fn id(&self) -> &'a str {
        if let Some(document) = self.shared() {
            document.id.as_str()
        } else if let Some((rows, row)) = self.typed() {
            rows.segment.document_ids[row].as_str()
        } else {
            self.batch.ids[self.index].as_str()
        }
    }

    pub fn schema(&self) -> &'a str {
        self.shared().map_or_else(
            || self.batch.schema.as_str(),
            |document| document.schema.as_str(),
        )
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        if let Some(document) = self.shared() {
            document.created_at
        } else if let Some((rows, row)) = self.typed() {
            rows.segment.created_at[row]
        } else {
            self.batch.created_at[self.index]
        }
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        if let Some(document) = self.shared() {
            document.updated_at
        } else if let Some((rows, row)) = self.typed() {
            rows.segment.updated_at[row]
        } else {
            self.batch.updated_at[self.index]
        }
    }

    pub fn version(&self) -> u64 {
        if let Some(document) = self.shared() {
            document.version
        } else if let Some((rows, row)) = self.typed() {
            rows.segment.document_versions[row]
        } else {
            self.batch.versions[self.index]
        }
    }

    pub fn fields(&self) -> ProjectedFieldIter<'a> {
        ProjectedFieldIter {
            row: *self,
            column: 0,
        }
    }
}

pub struct ProjectedFieldIter<'a> {
    row: ProjectedRowRef<'a>,
    column: usize,
}

impl<'a> Iterator for ProjectedFieldIter<'a> {
    type Item = (&'a str, ProjectedValueRef<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(column) = self.row.batch.columns.get(self.column) {
            self.column += 1;
            if let Some(document) = self.row.shared() {
                if let Some(value) = document.fields.get(&column.name) {
                    return Some((column.name.as_str(), ProjectedValueRef::Value(value)));
                }
                continue;
            }
            if let Some((rows, row)) = self.row.typed() {
                let Some(field_id) = rows.field_ids[self.column - 1] else {
                    continue;
                };
                let cell = rows
                    .segment
                    .column(field_id)
                    .expect("typed projection columns were prevalidated")
                    .cell(row)
                    .expect("validated typed projection row");
                if let Some(value) = ProjectedValueRef::from_typed(cell) {
                    return Some((column.name.as_str(), value));
                }
                continue;
            }
            if let Some(value) = column.values[self.row.index].as_ref() {
                return Some((column.name.as_str(), ProjectedValueRef::Value(value)));
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProjectedValueRef<'a> {
    Value(&'a Value),
    Null,
    String(&'a str),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Reference(ObjRef),
}

impl<'a> ProjectedValueRef<'a> {
    fn from_typed(value: TypedCell<'a>) -> Option<Self> {
        match value {
            TypedCell::Missing => None,
            TypedCell::Null => Some(Self::Null),
            TypedCell::String(value) => Some(Self::String(value)),
            TypedCell::Integer(value) => Some(Self::Integer(value)),
            TypedCell::Float(value) => Some(Self::Float(value)),
            TypedCell::Boolean(value) => Some(Self::Boolean(value)),
            TypedCell::DateTime(value) => Some(Self::DateTime(value)),
            TypedCell::Reference(value) => Some(Self::Reference(value)),
        }
    }

    pub fn to_owned(self) -> Value {
        match self {
            Self::Value(value) => value.clone(),
            Self::Null => Value::Null,
            Self::String(value) => Value::String(value.to_owned()),
            Self::Integer(value) => Value::Integer(value),
            Self::Float(value) => Value::Float(value),
            Self::Boolean(value) => Value::Boolean(value),
            Self::DateTime(value) => Value::DateTime(value),
            Self::Reference(value) => Value::Reference(value),
        }
    }
}

pub struct ProjectedRows<'a> {
    batch: &'a ProjectedRowBatch,
    index: usize,
}

impl<'a> Iterator for ProjectedRows<'a> {
    type Item = ProjectedRowRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let row = (self.index < self.batch.len()).then_some(ProjectedRowRef {
            batch: self.batch,
            index: self.index,
        });
        self.index += usize::from(row.is_some());
        row
    }
}

#[derive(Debug)]
struct ProjectedColumn {
    name: String,
    values: Vec<Option<crate::document::Value>>,
}

impl ProjectedRowBatch {
    pub fn with_capacity(schema: String, fields: &[String], capacity: usize) -> Self {
        Self {
            schema,
            shared_rows: None,
            typed_rows: None,
            ids: Vec::with_capacity(capacity),
            created_at: Vec::with_capacity(capacity),
            updated_at: Vec::with_capacity(capacity),
            versions: Vec::with_capacity(capacity),
            columns: fields
                .iter()
                .map(|name| ProjectedColumn {
                    name: name.clone(),
                    values: Vec::with_capacity(capacity),
                })
                .collect(),
        }
    }

    pub fn shared_with_capacity(schema: String, fields: &[String], capacity: usize) -> Self {
        let mut batch = Self::with_capacity(schema, fields, 0);
        batch.shared_rows = Some(Vec::with_capacity(capacity));
        batch
    }

    pub(crate) fn typed(
        schema: String,
        fields: &[String],
        field_ids: Vec<Option<u16>>,
        segment: Arc<TypedSegment>,
        capacity: usize,
    ) -> Self {
        debug_assert_eq!(fields.len(), field_ids.len());
        let mut batch = Self::with_capacity(schema, fields, 0);
        batch.typed_rows = Some(TypedProjectedRows {
            segment,
            indices: Vec::with_capacity(capacity),
            field_ids,
        });
        batch
    }

    pub(crate) fn push_typed(&mut self, row: usize) {
        self.typed_rows
            .as_mut()
            .expect("typed row batch was not initialized")
            .indices
            .push(row);
    }

    pub fn push(&mut self, document: &Document) {
        debug_assert!(self.shared_rows.is_none());
        self.ids.push(document.id.clone());
        self.created_at.push(document.created_at);
        self.updated_at.push(document.updated_at);
        self.versions.push(document.version);
        for column in &mut self.columns {
            column
                .values
                .push(document.fields.get(&column.name).cloned());
        }
    }

    pub fn push_shared(&mut self, document: Arc<Document>) {
        self.shared_rows
            .as_mut()
            .expect("shared row batch was not initialized for shared ownership")
            .push(document);
    }

    pub fn len(&self) -> usize {
        self.shared_rows.as_ref().map_or_else(
            || {
                self.typed_rows
                    .as_ref()
                    .map_or(self.ids.len(), |rows| rows.indices.len())
            },
            Vec::len,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn rows(&self) -> ProjectedRows<'_> {
        ProjectedRows {
            batch: self,
            index: 0,
        }
    }

    pub fn truncate(&mut self, len: usize) {
        if let Some(rows) = &mut self.shared_rows {
            rows.truncate(len);
            return;
        }
        if let Some(rows) = &mut self.typed_rows {
            rows.indices.truncate(len);
            return;
        }
        self.ids.truncate(len);
        self.created_at.truncate(len);
        self.updated_at.truncate(len);
        self.versions.truncate(len);
        for column in &mut self.columns {
            column.values.truncate(len);
        }
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        if let Some(rows) = &mut self.shared_rows {
            return Self {
                schema: self.schema.clone(),
                shared_rows: Some(rows.split_off(at)),
                typed_rows: None,
                ids: Vec::new(),
                created_at: Vec::new(),
                updated_at: Vec::new(),
                versions: Vec::new(),
                columns: self
                    .columns
                    .iter()
                    .map(|column| ProjectedColumn {
                        name: column.name.clone(),
                        values: Vec::new(),
                    })
                    .collect(),
            };
        }
        if let Some(rows) = &mut self.typed_rows {
            return Self {
                schema: self.schema.clone(),
                shared_rows: None,
                typed_rows: Some(TypedProjectedRows {
                    segment: rows.segment.clone(),
                    indices: rows.indices.split_off(at),
                    field_ids: rows.field_ids.clone(),
                }),
                ids: Vec::new(),
                created_at: Vec::new(),
                updated_at: Vec::new(),
                versions: Vec::new(),
                columns: self
                    .columns
                    .iter()
                    .map(|column| ProjectedColumn {
                        name: column.name.clone(),
                        values: Vec::new(),
                    })
                    .collect(),
            };
        }
        Self {
            schema: self.schema.clone(),
            shared_rows: None,
            typed_rows: None,
            ids: self.ids.split_off(at),
            created_at: self.created_at.split_off(at),
            updated_at: self.updated_at.split_off(at),
            versions: self.versions.split_off(at),
            columns: self
                .columns
                .iter_mut()
                .map(|column| ProjectedColumn {
                    name: column.name.clone(),
                    values: column.values.split_off(at),
                })
                .collect(),
        }
    }

    pub fn into_documents(self) -> Vec<Document> {
        if let Some(rows) = self.shared_rows {
            return rows
                .into_iter()
                .map(|document| {
                    let projected = self
                        .columns
                        .iter()
                        .filter_map(|column| {
                            document
                                .fields
                                .get(&column.name)
                                .cloned()
                                .map(|value| (column.name.clone(), value))
                        })
                        .collect();
                    document.with_projected_fields(projected)
                })
                .collect();
        }
        if self.typed_rows.is_some() {
            return self
                .rows()
                .map(|row| Document {
                    id: row.id().to_owned(),
                    schema: row.schema().to_owned(),
                    fields: row
                        .fields()
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
                        .collect(),
                    created_at: row.created_at(),
                    updated_at: row.updated_at(),
                    version: row.version(),
                })
                .collect();
        }
        let row_count = self.ids.len();
        let mut documents = self
            .ids
            .into_iter()
            .zip(self.created_at)
            .zip(self.updated_at)
            .zip(self.versions)
            .map(|(((id, created_at), updated_at), version)| Document {
                id,
                schema: self.schema.clone(),
                fields: HashMap::with_capacity(self.columns.len()),
                created_at,
                updated_at,
                version,
            })
            .collect::<Vec<_>>();
        debug_assert_eq!(documents.len(), row_count);
        for column in self.columns {
            for (document, value) in documents.iter_mut().zip(column.values) {
                if let Some(value) = value {
                    document.fields.insert(column.name.clone(), value);
                }
            }
        }
        documents
    }
}

#[cfg(test)]
mod projected_batch_tests {
    use super::*;
    use crate::document::Value;

    fn row(id: &str, include_optional: bool) -> Arc<Document> {
        let mut fields = HashMap::from([(
            "message".to_string(),
            Value::String(format!("message-{id}")),
        )]);
        if include_optional {
            fields.insert("optional".to_string(), Value::Null);
        }
        Arc::new(Document::with_id(
            id.to_string(),
            "events".to_string(),
            fields,
        ))
    }

    #[test]
    fn shared_projection_preserves_missing_and_explicit_null() {
        let fields = vec!["message".to_string(), "optional".to_string()];
        let first = row("one", false);
        let second = row("two", true);
        let first_created_at = first.created_at;

        let mut batch = ProjectedRowBatch::shared_with_capacity("events".into(), &fields, 2);
        batch.push_shared(first);
        batch.push_shared(second);

        let tail = batch.split_off(1);
        let head = batch.into_documents();
        let tail = tail.into_documents();

        assert_eq!(head[0].id, "one");
        assert_eq!(head[0].created_at, first_created_at);
        assert!(!head[0].fields.contains_key("optional"));
        assert_eq!(tail[0].fields.get("optional"), Some(&Value::Null));
        assert_eq!(
            tail[0].fields.get("message"),
            Some(&Value::String("message-two".into()))
        );
    }

    #[test]
    fn owned_projection_truncates_metadata_and_columns_together() {
        let fields = vec!["message".to_string(), "optional".to_string()];
        let first = row("one", false);
        let second = row("two", true);
        let mut batch = ProjectedRowBatch::with_capacity("events".into(), &fields, 2);
        batch.push(&first);
        batch.push(&second);
        batch.truncate(1);

        let documents = batch.into_documents();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].id, "one");
        assert_eq!(
            documents[0].fields.get("message"),
            Some(&Value::String("message-one".into()))
        );
        assert!(!documents[0].fields.contains_key("optional"));
    }
}

pub trait DocumentStore: Send + Sync + 'static {
    /// Whether this store can authoritatively evaluate pushed scalar boolean
    /// predicates and materialize only requested result fields.
    fn supports_storage_owned_scan(&self) -> bool {
        false
    }

    /// Whether this store has transaction-local writes that are not yet
    /// represented in committed access paths.
    fn has_pending_writes(&self) -> OQLResult<bool> {
        Ok(false)
    }

    fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>>;
    fn put_document(&self, document: &Document) -> OQLResult<()>;
    /// Persist a document batch. Backends may override this to provide one
    /// atomic durability boundary; the default preserves compatibility.
    fn put_documents(&self, documents: &[Document]) -> OQLResult<()> {
        for document in documents {
            self.put_document(document)?;
        }
        Ok(())
    }
    fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool>;
    fn scan_documents(&self, schema: &str) -> OQLResult<Vec<Document>>;
    /// Scan documents while materializing only the requested top-level user
    /// fields. Document identity and system metadata are always preserved.
    ///
    /// The default keeps storage implementations compatible. Engines with a
    /// decoded object directory or projection-aware record format should
    /// override this to avoid cloning or decoding unneeded values.
    fn scan_documents_projection(
        &self,
        schema: &str,
        fields: &[String],
    ) -> OQLResult<Vec<Document>> {
        let documents = self.scan_documents(schema)?;
        Ok(documents
            .into_iter()
            .map(|document| {
                let projected = fields
                    .iter()
                    .filter_map(|field| {
                        document
                            .fields
                            .get(field)
                            .cloned()
                            .map(|value| (field.clone(), value))
                    })
                    .collect();
                document.with_projected_fields(projected)
            })
            .collect())
    }
    /// Attempt a storage-local filtered scan. `None` asks the executor to use
    /// the ordinary scan plus filter operators. Implementations must preserve
    /// the same comparison semantics and transaction visibility as OQL.
    fn scan_documents_filtered_projection(
        &self,
        _schema: &str,
        _filters: &[ScanPredicate],
        _fields: Option<&[String]>,
    ) -> OQLResult<Option<Vec<Document>>> {
        Ok(None)
    }
    /// Attempt the same pushed scan while retaining projected fields in
    /// column-major batches. Stores should return batches in authoritative scan
    /// order and keep each batch reasonably cache-sized.
    fn scan_projected_batches(
        &self,
        _schema: &str,
        _filters: &[ScanPredicate],
        _fields: &[String],
        _batch_rows: usize,
    ) -> OQLResult<Option<Vec<ProjectedRowBatch>>> {
        Ok(None)
    }
    fn get_schema(&self, name: &str) -> OQLResult<Option<Schema>>;
    fn put_schema(&self, schema: &Schema) -> OQLResult<()>;

    fn delete_schema(&self, _name: &str) -> OQLResult<bool> {
        Ok(false)
    }

    fn list_schemas(&self) -> OQLResult<Vec<String>>;

    fn upsert_scalar_index(&self, _index: ScalarIndexDefinition) -> OQLResult<()> {
        Err(OQLError::generic(
            "scalar indexes are not supported by this store",
        ))
    }

    fn delete_scalar_index(&self, _schema: &str, _name: &str) -> OQLResult<bool> {
        Err(OQLError::generic(
            "scalar indexes are not supported by this store",
        ))
    }

    fn list_scalar_indexes(&self, _schema: Option<&str>) -> OQLResult<Vec<ScalarIndexDefinition>> {
        Ok(Vec::new())
    }

    /// Scan a native typed scalar index. `None` means this storage backend has
    /// no native execution path and the caller may use a compatibility path.
    fn scan_scalar_index(
        &self,
        _schema: &str,
        _name: &str,
        _range: &ScalarIndexRange,
    ) -> OQLResult<Option<ScalarIndexScanPage>> {
        Ok(None)
    }

    fn list_references_to(
        &self,
        _target_schema: &str,
        _target_id: &DocumentId,
        _source_schema: Option<&str>,
        _field_path: Option<&str>,
    ) -> OQLResult<Vec<ReverseReferenceEntry>> {
        Ok(Vec::new())
    }

    fn scan_referencing_documents(
        &self,
        target_schema: &str,
        target_id: &DocumentId,
        source_schema: Option<&str>,
        field_path: Option<&str>,
    ) -> OQLResult<Vec<Document>> {
        let edges = self.list_references_to(target_schema, target_id, source_schema, field_path)?;
        let mut seen = HashSet::new();
        let mut documents = Vec::new();
        for edge in edges {
            let key = (edge.source_schema, edge.source_id);
            if seen.insert(key.clone()) {
                if let Some(document) = self.get_document(&key.0, &key.1)? {
                    documents.push(document);
                }
            }
        }
        Ok(documents)
    }

    fn upsert_vector_index(&self, _index: VectorIndexDefinition) -> OQLResult<()> {
        Err(OQLError::generic(
            "vector indexes are not supported by this store",
        ))
    }

    fn delete_vector_index(&self, _schema: &str, _name: &str) -> OQLResult<bool> {
        Err(OQLError::generic(
            "vector indexes are not supported by this store",
        ))
    }

    fn list_vector_indexes(&self, _schema: Option<&str>) -> OQLResult<Vec<VectorIndexDefinition>> {
        Ok(Vec::new())
    }

    fn vector_ann_search(
        &self,
        _index: &VectorIndexDefinition,
        _documents: &[Document],
        _query: &[f32],
        _k: usize,
        _num_candidates: usize,
    ) -> OQLResult<Option<Vec<VectorAnnHit>>> {
        Ok(None)
    }

    fn build_vector_index(
        &self,
        _schema: &str,
        _name: &str,
        _force_rebuild: bool,
    ) -> OQLResult<VectorIndexBuildReport> {
        Err(OQLError::generic("vector index build is not implemented"))
    }

    fn vector_index_status(
        &self,
        _schema: &str,
        _name: &str,
    ) -> OQLResult<Option<VectorIndexStatus>> {
        Ok(None)
    }

    fn navigate_reference(
        &self,
        _object_ref: &crate::document::ObjRef,
    ) -> OQLResult<Option<Document>> {
        Ok(None)
    }

    fn navigate_references(
        &self,
        object_refs: &[crate::document::ObjRef],
    ) -> OQLResult<Vec<Option<Document>>> {
        object_refs
            .iter()
            .map(|object_ref| self.navigate_reference(object_ref))
            .collect()
    }

    fn navigate_reference_projection(
        &self,
        object_ref: &crate::document::ObjRef,
        fields: &[String],
    ) -> OQLResult<Option<Document>> {
        let Some(mut document) = self.navigate_reference(object_ref)? else {
            return Ok(None);
        };
        let selected = fields.iter().collect::<HashSet<_>>();
        document.fields.retain(|name, _| selected.contains(name));
        for field in fields {
            if let Some(system_field) = DocumentSystemField::from_name(field) {
                let value = system_field.value(&document);
                document.fields.insert(field.clone(), value);
            }
        }
        Ok(Some(document))
    }

    /// Resolve a group of stable references with one projection description.
    ///
    /// Backends can override this to amortize catalog lookups, locks, and
    /// projection compilation across an execution chunk.
    fn navigate_references_projection(
        &self,
        object_refs: &[crate::document::ObjRef],
        fields: &[String],
    ) -> OQLResult<Vec<Option<Document>>> {
        object_refs
            .iter()
            .map(|object_ref| self.navigate_reference_projection(object_ref, fields))
            .collect()
    }

    fn navigate_global_reference(&self, _object_ref: &GlobalObjRef) -> OQLResult<Option<Document>> {
        Ok(None)
    }

    fn get_objref(
        &self,
        _schema: &str,
        _id: &DocumentId,
    ) -> OQLResult<Option<crate::document::ObjRef>> {
        Ok(None)
    }

    fn get_global_objref(
        &self,
        _schema: &str,
        _id: &DocumentId,
    ) -> OQLResult<Option<GlobalObjRef>> {
        Ok(None)
    }

    fn as_any(&self) -> &dyn std::any::Any;
}

pub type Storage = Arc<dyn DocumentStore>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScalarIndexDefinition {
    pub name: String,
    pub schema: String,
    pub fields: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScalarIndexBound {
    pub values: Vec<crate::document::Value>,
    pub inclusive: bool,
    /// Advance to the first key after this encoded component prefix. This is
    /// used for composite-prefix and range bounds without inventing sentinels.
    pub prefix_end: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ScalarIndexDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScalarIndexRange {
    pub lower: Option<ScalarIndexBound>,
    pub upper: Option<ScalarIndexBound>,
    pub limit: Option<usize>,
    pub after: Option<ScalarIndexCursor>,
    pub direction: ScalarIndexDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScalarIndexCursor {
    pub version: u8,
    pub schema: String,
    pub index_name: String,
    pub key: Vec<u8>,
    pub object_ref: crate::document::ObjRef,
    pub direction: ScalarIndexDirection,
}

impl ScalarIndexCursor {
    pub fn encode(&self) -> OQLResult<String> {
        use base64::Engine;
        let bytes = bincode::serialize(self).map_err(|error| {
            OQLError::serialization_error(&format!("cannot encode index cursor: {error}"), None)
        })?;
        let mut framed = bytes;
        let checksum = crc32fast::hash(&framed);
        framed.extend_from_slice(&checksum.to_le_bytes());
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(framed))
    }

    pub fn decode(encoded: &str) -> OQLResult<Self> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|error| OQLError::generic(&format!("invalid index cursor: {error}")))?;
        if bytes.len() < 4 {
            return Err(OQLError::generic("index cursor is truncated"));
        }
        let (payload, checksum) = bytes.split_at(bytes.len() - 4);
        if crc32fast::hash(payload) != u32::from_le_bytes(checksum.try_into().unwrap()) {
            return Err(OQLError::generic("index cursor checksum mismatch"));
        }
        let cursor: Self = bincode::deserialize(payload).map_err(|error| {
            OQLError::serialization_error(&format!("cannot decode index cursor: {error}"), None)
        })?;
        if cursor.version != 2 || cursor.key.is_empty() || cursor.object_ref.is_null() {
            return Err(OQLError::generic("invalid index cursor contents"));
        }
        Ok(cursor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarIndexScanPage {
    pub object_refs: Vec<crate::document::ObjRef>,
    /// Encoded typed key corresponding one-for-one with `object_refs`.
    pub encoded_keys: Vec<Vec<u8>>,
    pub next_cursor: Option<ScalarIndexCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReverseReferenceEntry {
    pub source_schema: String,
    pub source_id: DocumentId,
    pub field_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorIndexState {
    Creating,
    Ready,
    Rebuilding,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorIndexType {
    Exact,
    Hnsw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorMetricType {
    Cosine,
    L2,
    Dot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorIndexDefinition {
    pub name: String,
    pub schema: String,
    pub field: String,
    pub dimension: u32,
    pub metric: VectorMetricType,
    pub index_type: VectorIndexType,
    pub quantization: Option<String>,
    pub state: VectorIndexState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorAnnHit {
    pub index: usize,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorIndexBuildReport {
    pub indexed_documents: usize,
    pub skipped_documents: usize,
    pub total_documents: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorIndexStatus {
    pub schema: String,
    pub name: String,
    pub state: VectorIndexState,
    pub file_exists: bool,
    pub file_size_bytes: Option<u64>,
    pub file_mtime_unix_seconds: Option<u64>,
}

pub fn create_storage_with_fs(
    path: &str,
    size: usize,
    filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
) -> OQLResult<Storage> {
    Ok(Arc::new(RustStore::new_with_fs(path, size, filesystem)?))
}

pub fn create_storage(path: &str, size: usize) -> OQLResult<Storage> {
    Ok(Arc::new(RustStore::new(path, size)?))
}
