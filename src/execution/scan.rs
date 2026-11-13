// Scan Operators
// Sequential scan, filter, project, sort, and limit operators

use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::{
    DataChunk, PhysicalOperatorCollect, PhysicalOperatorExec, DEFAULT_CHUNK_ROWS,
};
use crate::parser::{BinaryOperator, ComparisonOperator, Expression, FieldAccess, Literal};
use crate::planner::types::{FilterCondition, ProjectionField, SortSpec};
use crate::schema::DocumentSystemField;
use crate::storage::{ScanPredicate, Storage};
use std::cmp::Ordering;
use std::collections::VecDeque;

/// Sequential scan operator
///
/// Scans all documents in a collection sequentially.
pub struct SeqScanExec {
    schema: String,
    projection: Option<Vec<String>>,
    filters: Vec<ScanPredicate>,
    storage: Option<Storage>,
    chunks: VecDeque<DataChunk>,
    pending_documents: VecDeque<Document>,
    columnar_batches: u64,
    columnar_rows: u64,
}

impl SeqScanExec {
    pub fn new(schema: String) -> Self {
        Self {
            schema,
            projection: None,
            filters: Vec::new(),
            storage: None,
            chunks: VecDeque::new(),
            pending_documents: VecDeque::new(),
            columnar_batches: 0,
            columnar_rows: 0,
        }
    }

    pub fn with_projection(mut self, fields: Vec<String>) -> Self {
        self.projection = Some(fields);
        self
    }

    pub fn with_filters(mut self, filters: Vec<ScanPredicate>) -> Self {
        self.filters = filters;
        self
    }
}

impl PhysicalOperatorExec for SeqScanExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.storage = Some(storage.clone());
        self.chunks.clear();
        self.pending_documents.clear();
        self.columnar_batches = 0;
        self.columnar_rows = 0;
        if let Some(fields) = &self.projection {
            if let Some(batches) = storage.scan_projected_batches(
                &self.schema,
                &self.filters,
                fields,
                DEFAULT_CHUNK_ROWS,
            )? {
                self.columnar_batches = batches.len() as u64;
                self.columnar_rows = batches.iter().map(|batch| batch.len() as u64).sum();
                self.chunks
                    .extend(batches.into_iter().map(DataChunk::projected));
                return Ok(());
            }
        }
        let documents = match storage.scan_documents_filtered_projection(
            &self.schema,
            &self.filters,
            self.projection.as_deref(),
        )? {
            Some(documents) => documents,
            None => match &self.projection {
                Some(fields) => storage.scan_documents_projection(&self.schema, fields)?,
                None => storage.scan_documents(&self.schema)?,
            },
        };
        let mut documents = documents.into_iter();
        loop {
            let chunk = documents
                .by_ref()
                .take(DEFAULT_CHUNK_ROWS)
                .collect::<Vec<_>>();
            if chunk.is_empty() {
                break;
            }
            self.chunks.push_back(DataChunk::new(chunk));
        }
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        if let Some(document) = self.pending_documents.pop_front() {
            return Ok(Some(document));
        }
        let Some(chunk) = self.chunks.pop_front() else {
            return Ok(None);
        };
        self.pending_documents = chunk.into_documents().into();
        Ok(self.pending_documents.pop_front())
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        let target_rows = target_rows.max(1);
        if !self.pending_documents.is_empty() {
            let take = target_rows.min(self.pending_documents.len());
            return Ok(Some(DataChunk::new(
                self.pending_documents.drain(..take).collect(),
            )));
        }
        let Some(mut chunk) = self.chunks.pop_front() else {
            return Ok(None);
        };
        if chunk.len() > target_rows {
            let remainder = chunk.split_off(target_rows);
            self.chunks.push_front(remainder);
        }
        Ok(Some(chunk))
    }

    fn close(&mut self) -> OQLResult<()> {
        self.chunks.clear();
        self.pending_documents.clear();
        self.storage = None;
        Ok(())
    }

    fn reset(&mut self) -> OQLResult<()> {
        let storage = self
            .storage
            .clone()
            .ok_or_else(|| OQLError::generic("sequential scan is not open"))?;
        self.open(storage)
    }

    fn runtime_metrics(&self) -> std::collections::BTreeMap<String, u64> {
        std::collections::BTreeMap::from([
            ("columnar_batches".to_string(), self.columnar_batches),
            ("columnar_rows".to_string(), self.columnar_rows),
        ])
    }
}

/// Filter operator
///
/// Filters documents based on a condition.
pub struct FilterExec {
    condition: FilterCondition,
    input: Box<dyn PhysicalOperatorExec>,
}

impl FilterExec {
    pub fn new(condition: FilterCondition, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self { condition, input }
    }

    fn evaluate_condition(&self, doc: &Document) -> OQLResult<bool> {
        Self::evaluate_filter_condition(doc, &self.condition)
    }

    pub(crate) fn matches(doc: &Document, condition: &FilterCondition) -> OQLResult<bool> {
        Self::evaluate_filter_condition(doc, condition)
    }

    fn evaluate_filter_condition(doc: &Document, condition: &FilterCondition) -> OQLResult<bool> {
        match condition {
            FilterCondition::Comparison {
                left,
                operator,
                right,
            } => {
                let left_val = Self::evaluate_expression(doc, left)?;
                let right_val = Self::evaluate_expression(doc, right)?;
                Ok(Self::compare_values(&left_val, operator, &right_val))
            }
            FilterCondition::And(left, right) => {
                let left_result = Self::evaluate_filter_condition(doc, left)?;
                let right_result = Self::evaluate_filter_condition(doc, right)?;
                Ok(left_result && right_result)
            }
            FilterCondition::Or(left, right) => {
                let left_result = Self::evaluate_filter_condition(doc, left)?;
                let right_result = Self::evaluate_filter_condition(doc, right)?;
                Ok(left_result || right_result)
            }
            FilterCondition::Not(inner) => {
                let result = Self::evaluate_filter_condition(doc, inner)?;
                Ok(!result)
            }
        }
    }

    fn evaluate_expression(doc: &Document, expr: &Expression) -> OQLResult<Value> {
        match expr {
            Expression::Literal(lit) => Ok(Self::literal_to_value(lit)),
            Expression::FieldAccess(field_access) => Self::evaluate_field_access(doc, field_access),
            Expression::BinaryOp {
                left,
                operator,
                right,
            } => {
                let left_val = Self::evaluate_expression(doc, left)?;
                let right_val = Self::evaluate_expression(doc, right)?;
                Self::evaluate_binary_op(&left_val, operator, &right_val)
            }
            _ => Err(OQLError::generic("Unsupported expression in filter")),
        }
    }

    fn literal_to_value(lit: &Literal) -> Value {
        match lit {
            Literal::String(s) => Value::String(s.clone()),
            Literal::Integer(i) => Value::Integer(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::Boolean(b) => Value::Boolean(*b),
            Literal::Null => Value::Null,
        }
    }

    fn evaluate_field_access(doc: &Document, field_access: &FieldAccess) -> OQLResult<Value> {
        if field_access.path.is_empty() {
            return Ok(Value::Null);
        }

        let mut current_val = document_field_value(doc, &field_access.path[0]);

        for segment in &field_access.path[1..] {
            match &current_val {
                Value::Object(obj) => {
                    current_val = obj.get(segment).cloned().unwrap_or(Value::Null);
                }
                _ => return Ok(Value::Null),
            }
        }

        Ok(current_val)
    }

    fn evaluate_binary_op(
        left: &Value,
        operator: &BinaryOperator,
        right: &Value,
    ) -> OQLResult<Value> {
        match operator {
            BinaryOperator::Add => match (left, right) {
                (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a + b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                _ => Err(OQLError::generic("Type mismatch in addition")),
            },
            BinaryOperator::Subtract => match (left, right) {
                (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a - b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                _ => Err(OQLError::generic("Type mismatch in subtraction")),
            },
            BinaryOperator::Multiply => match (left, right) {
                (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a * b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                _ => Err(OQLError::generic("Type mismatch in multiplication")),
            },
            BinaryOperator::Divide => match (left, right) {
                (Value::Integer(a), Value::Integer(b)) => {
                    if *b == 0 {
                        Err(OQLError::generic("Division by zero"))
                    } else {
                        Ok(Value::Integer(a / b))
                    }
                }
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                _ => Err(OQLError::generic("Type mismatch in division")),
            },
            BinaryOperator::Modulo => match (left, right) {
                (Value::Integer(a), Value::Integer(b)) => {
                    if *b == 0 {
                        Err(OQLError::generic("Modulo by zero"))
                    } else {
                        Ok(Value::Integer(a % b))
                    }
                }
                _ => Err(OQLError::generic("Type mismatch in modulo")),
            },
        }
    }

    pub(crate) fn compare_values(
        left: &Value,
        operator: &ComparisonOperator,
        right: &Value,
    ) -> bool {
        match operator {
            ComparisonOperator::Equal => left == right,
            ComparisonOperator::NotEqual => left != right,
            ComparisonOperator::LessThan => Self::value_less_than(left, right),
            ComparisonOperator::LessEqual => left == right || Self::value_less_than(left, right),
            ComparisonOperator::GreaterThan => Self::value_less_than(right, left),
            ComparisonOperator::GreaterEqual => left == right || Self::value_less_than(right, left),
            ComparisonOperator::Contains
            | ComparisonOperator::StartsWith
            | ComparisonOperator::EndsWith
            | ComparisonOperator::In => {
                // String operations - simplified for now
                false
            }
        }
    }

    fn value_less_than(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => a < b,
            (Value::Float(a), Value::Float(b)) => a < b,
            (Value::Integer(a), Value::Float(b)) => (*a as f64) < *b,
            (Value::Float(a), Value::Integer(b)) => *a < (*b as f64),
            (Value::String(a), Value::String(b)) => a < b,
            (Value::DateTime(a), Value::DateTime(b)) => a < b,
            (Value::DateTime(a), Value::String(b)) => a.to_rfc3339() < *b,
            (Value::String(a), Value::DateTime(b)) => *a < b.to_rfc3339(),
            (Value::Boolean(a), Value::Boolean(b)) => !a && *b,
            _ => false,
        }
    }
}

impl PhysicalOperatorExec for FilterExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        loop {
            match self.input.next()? {
                Some(doc) => {
                    if self.evaluate_condition(&doc)? {
                        return Ok(Some(doc));
                    }
                    // Continue to next document
                }
                None => return Ok(None),
            }
        }
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        let target_rows = target_rows.max(1);
        loop {
            let Some(input) = self.input.next_batch(target_rows)? else {
                return Ok(None);
            };
            let documents = input.into_documents();
            let mut output = DataChunk::with_capacity(documents.len());
            for document in documents {
                if self.evaluate_condition(&document)? {
                    output.push(document);
                }
            }
            if !output.is_empty() {
                return Ok(Some(output));
            }
        }
    }

    fn close(&mut self) -> OQLResult<()> {
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.input.reset()
    }
}

/// Project operator
///
/// Projects specific fields from documents.
pub struct ProjectExec {
    projections: Vec<ProjectionField>,
    input: Box<dyn PhysicalOperatorExec>,
}

impl ProjectExec {
    pub fn new(projections: Vec<ProjectionField>, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self { projections, input }
    }

    fn project_document(&self, doc: &Document) -> OQLResult<Document> {
        let mut projected_fields = std::collections::HashMap::new();

        for proj in &self.projections {
            let value = self.evaluate_expression(doc, &proj.expression)?;
            let field_name = proj
                .alias
                .clone()
                .unwrap_or_else(|| self.expression_to_field_name(&proj.expression));
            projected_fields.insert(field_name, value);
        }

        Ok(doc.with_projected_fields(projected_fields))
    }

    fn evaluate_expression(&self, doc: &Document, expr: &Expression) -> OQLResult<Value> {
        match expr {
            Expression::Literal(lit) => Ok(FilterExec::literal_to_value(lit)),
            Expression::FieldAccess(field_access) => self.evaluate_field_access(doc, field_access),
            _ => Err(OQLError::generic("Unsupported expression in projection")),
        }
    }

    fn evaluate_field_access(
        &self,
        doc: &Document,
        field_access: &FieldAccess,
    ) -> OQLResult<Value> {
        if field_access.path.is_empty() {
            return Ok(Value::Null);
        }

        let mut current_val = document_field_value(doc, &field_access.path[0]);

        for segment in &field_access.path[1..] {
            match &current_val {
                Value::Object(obj) => {
                    current_val = obj.get(segment).cloned().unwrap_or(Value::Null);
                }
                _ => return Ok(Value::Null),
            }
        }

        Ok(current_val)
    }

    fn expression_to_field_name(&self, expr: &Expression) -> String {
        match expr {
            Expression::FieldAccess(field_access) => field_access
                .path
                .last()
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            _ => "computed".to_string(),
        }
    }
}

impl PhysicalOperatorExec for ProjectExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        match self.input.next()? {
            Some(doc) => Ok(Some(self.project_document(&doc)?)),
            None => Ok(None),
        }
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        let Some(input) = self.input.next_batch(target_rows.max(1))? else {
            return Ok(None);
        };
        let documents = input.into_documents();
        let mut output = DataChunk::with_capacity(documents.len());
        for document in documents {
            output.push(self.project_document(&document)?);
        }
        Ok(Some(output))
    }

    fn close(&mut self) -> OQLResult<()> {
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.input.reset()
    }
}

/// Sort operator
///
/// Sorts documents based on sort specifications.
pub struct SortExec {
    sort_specs: Vec<SortSpec>,
    input: Box<dyn PhysicalOperatorExec>,
    sorted_documents: Vec<Document>,
    position: usize,
}

impl SortExec {
    pub fn new(sort_specs: Vec<SortSpec>, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self {
            sort_specs,
            input,
            sorted_documents: Vec::new(),
            position: 0,
        }
    }

    fn eval_expr_static(doc: &Document, expr: &Expression) -> OQLResult<Value> {
        match expr {
            Expression::Literal(lit) => Ok(FilterExec::literal_to_value(lit)),
            Expression::FieldAccess(field_access) => {
                if field_access.path.is_empty() {
                    Ok(Value::Null)
                } else {
                    Ok(document_field_value(doc, &field_access.path[0]))
                }
            }
            _ => Err(OQLError::generic("Unsupported expression in sort")),
        }
    }

    fn compare_values_static(a: &Value, b: &Value) -> Ordering {
        match (a, b) {
            (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
            (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
            (Value::String(x), Value::String(y)) => x.cmp(y),
            (Value::Boolean(x), Value::Boolean(y)) => x.cmp(y),
            (Value::DateTime(x), Value::DateTime(y)) => x.cmp(y),
            (Value::Integer(x), Value::Float(y)) => {
                (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
            }
            (Value::Float(x), Value::Integer(y)) => {
                x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
            }
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Less,
            (_, Value::Null) => Ordering::Greater,
            _ => Ordering::Equal,
        }
    }
}

fn document_field_value(doc: &Document, field: &str) -> Value {
    match DocumentSystemField::from_name(field) {
        Some(system_field) => system_field.value(doc),
        None => doc.fields.get(field).cloned().unwrap_or(Value::Null),
    }
}

impl PhysicalOperatorExec for SortExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)?;

        // Collect all documents
        self.sorted_documents = self.input.collect_all()?;

        // Sort them - clone sort_specs to avoid borrow issues
        let sort_specs = self.sort_specs.clone();
        self.sorted_documents.sort_by(|a, b| {
            for spec in &sort_specs {
                let a_val = SortExec::eval_expr_static(a, &spec.expression).unwrap_or(Value::Null);
                let b_val = SortExec::eval_expr_static(b, &spec.expression).unwrap_or(Value::Null);

                let cmp = SortExec::compare_values_static(&a_val, &b_val);
                if cmp != Ordering::Equal {
                    return match spec.direction {
                        crate::parser::SortDirection::Asc => cmp,
                        crate::parser::SortDirection::Desc => cmp.reverse(),
                    };
                }
            }
            Ordering::Equal
        });
        self.position = 0;

        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        if self.position < self.sorted_documents.len() {
            let doc = self.sorted_documents[self.position].clone();
            self.position += 1;
            Ok(Some(doc))
        } else {
            Ok(None)
        }
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        let remaining = self.sorted_documents.len().saturating_sub(self.position);
        if remaining == 0 {
            return Ok(None);
        }
        let take = remaining.min(target_rows.max(1));
        let end = self.position + take;
        let chunk = self.sorted_documents[self.position..end].to_vec();
        self.position = end;
        Ok(Some(DataChunk::new(chunk)))
    }

    fn close(&mut self) -> OQLResult<()> {
        self.sorted_documents.clear();
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.position = 0;
        Ok(())
    }
}

/// Limit operator
///
/// Limits the number of documents returned.
pub struct LimitExec {
    limit: u64,
    input: Box<dyn PhysicalOperatorExec>,
    count: u64,
}

impl LimitExec {
    pub fn new(limit: u64, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self {
            limit,
            input,
            count: 0,
        }
    }
}

impl PhysicalOperatorExec for LimitExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)?;
        self.count = 0;
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        if self.count >= self.limit {
            return Ok(None);
        }

        match self.input.next()? {
            Some(doc) => {
                self.count += 1;
                Ok(Some(doc))
            }
            None => Ok(None),
        }
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        let remaining = self.limit.saturating_sub(self.count) as usize;
        if remaining == 0 {
            return Ok(None);
        }
        let Some(mut chunk) = self.input.next_batch(target_rows.max(1).min(remaining))? else {
            return Ok(None);
        };
        if chunk.len() > remaining {
            chunk.truncate(remaining);
        }
        self.count = self.count.saturating_add(chunk.len() as u64);
        Ok(Some(chunk))
    }

    fn close(&mut self) -> OQLResult<()> {
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.count = 0;
        self.input.reset()
    }
}

pub struct OffsetExec {
    offset: u64,
    skipped: u64,
    input: Box<dyn PhysicalOperatorExec>,
}

impl OffsetExec {
    pub fn new(offset: u64, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self {
            offset,
            skipped: 0,
            input,
        }
    }
}

impl PhysicalOperatorExec for OffsetExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)?;
        self.skipped = 0;
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        while self.skipped < self.offset {
            if self.input.next()?.is_none() {
                self.skipped = self.offset;
                return Ok(None);
            }
            self.skipped += 1;
        }
        self.input.next()
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        while self.skipped < self.offset {
            let remaining = (self.offset - self.skipped) as usize;
            let Some(chunk) = self
                .input
                .next_batch(remaining.min(DEFAULT_CHUNK_ROWS).max(1))?
            else {
                self.skipped = self.offset;
                return Ok(None);
            };
            self.skipped += chunk.len() as u64;
        }
        self.input.next_batch(target_rows)
    }

    fn close(&mut self) -> OQLResult<()> {
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.skipped = 0;
        self.input.reset()
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use crate::schema::{FieldType, Schema};
    use crate::storage::{DocumentStore, RustStore};
    use std::collections::{HashMap, VecDeque};
    use std::sync::Arc;

    struct BatchOnlySource {
        chunks: VecDeque<DataChunk>,
    }

    impl PhysicalOperatorExec for BatchOnlySource {
        fn open(&mut self, _storage: Storage) -> OQLResult<()> {
            Ok(())
        }

        fn next(&mut self) -> OQLResult<Option<Document>> {
            panic!("batch-native pipeline fell back to row execution")
        }

        fn next_batch(&mut self, _target_rows: usize) -> OQLResult<Option<DataChunk>> {
            Ok(self.chunks.pop_front())
        }

        fn close(&mut self) -> OQLResult<()> {
            Ok(())
        }

        fn reset(&mut self) -> OQLResult<()> {
            Ok(())
        }
    }

    fn document(id: &str, level: i64, message: &str) -> Document {
        Document::with_id(
            id.to_string(),
            "events".to_string(),
            HashMap::from([
                ("level".to_string(), Value::Integer(level)),
                ("message".to_string(), Value::String(message.to_string())),
            ]),
        )
    }

    #[test]
    fn filter_project_and_limit_stay_batch_native() {
        let source = BatchOnlySource {
            chunks: VecDeque::from([
                DataChunk::new(vec![
                    document("one", 10, "ignored"),
                    document("two", 50, "failure"),
                ]),
                DataChunk::new(vec![document("three", 60, "timeout")]),
            ]),
        };
        let filter = FilterExec::new(
            FilterCondition::Comparison {
                left: Expression::FieldAccess(FieldAccess {
                    path: vec!["level".to_string()],
                }),
                operator: ComparisonOperator::GreaterEqual,
                right: Expression::Literal(Literal::Integer(50)),
            },
            Box::new(source),
        );
        let project = ProjectExec::new(
            vec![ProjectionField {
                expression: Expression::FieldAccess(FieldAccess {
                    path: vec!["message".to_string()],
                }),
                alias: None,
                field_type: None,
            }],
            Box::new(filter),
        );
        let mut limit = LimitExec::new(2, Box::new(project));

        let documents = limit.collect_all().expect("batch pipeline");
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].fields.len(), 1);
        assert_eq!(
            documents[0].fields["message"],
            Value::String("failure".to_string())
        );
        assert_eq!(
            documents[1].fields["message"],
            Value::String("timeout".to_string())
        );
    }

    #[test]
    fn sequential_scan_reset_reloads_moved_rows() {
        let directory = tempfile::TempDir::new().unwrap();
        let store = Arc::new(
            RustStore::new(directory.path().join("scan-reset.owl"), 16 * 1024 * 1024).unwrap(),
        );
        store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([("message".to_string(), FieldType::String)]),
            ))
            .unwrap();
        for id in ["one", "two", "three"] {
            store
                .put_document(&Document::with_id(
                    id.to_string(),
                    "events".to_string(),
                    HashMap::from([("message".to_string(), Value::String(id.to_string()))]),
                ))
                .unwrap();
        }

        let storage: Storage = store;
        let mut scan = SeqScanExec::new("events".to_string());
        scan.open(storage).unwrap();
        assert_eq!(scan.collect_all().unwrap().len(), 3);
        scan.reset().unwrap();
        assert_eq!(scan.collect_all().unwrap().len(), 3);
        scan.close().unwrap();
    }
}
