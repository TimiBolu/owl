// Scan Operators
// Sequential scan, filter, project, sort, and limit operators

use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::PhysicalOperatorExec;
use crate::parser::{BinaryOperator, ComparisonOperator, Expression, FieldAccess, Literal};
use crate::planner::types::{FilterCondition, ProjectionField, SortSpec};
use crate::storage::Storage;
use std::cmp::Ordering;

/// Sequential scan operator
///
/// Scans all documents in a collection sequentially.
pub struct SeqScanExec {
    schema: String,
    storage: Option<Storage>,
    documents: Vec<Document>,
    position: usize,
}

impl SeqScanExec {
    pub fn new(schema: String) -> Self {
        Self {
            schema,
            storage: None,
            documents: Vec::new(),
            position: 0,
        }
    }
}

impl PhysicalOperatorExec for SeqScanExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.storage = Some(storage.clone());
        self.documents = storage.scan_documents(&self.schema)?;
        self.position = 0;
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        if self.position < self.documents.len() {
            let doc = self.documents[self.position].clone();
            self.position += 1;
            Ok(Some(doc))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> OQLResult<()> {
        self.documents.clear();
        self.storage = None;
        Ok(())
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.position = 0;
        Ok(())
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
        match &self.condition {
            FilterCondition::Comparison {
                left,
                operator,
                right,
            } => {
                let left_val = self.evaluate_expression(doc, left)?;
                let right_val = self.evaluate_expression(doc, right)?;
                Ok(self.compare_values(&left_val, operator, &right_val))
            }
            FilterCondition::And(left, right) => {
                let left_result = self.evaluate_filter_condition(doc, left)?;
                let right_result = self.evaluate_filter_condition(doc, right)?;
                Ok(left_result && right_result)
            }
            FilterCondition::Or(left, right) => {
                let left_result = self.evaluate_filter_condition(doc, left)?;
                let right_result = self.evaluate_filter_condition(doc, right)?;
                Ok(left_result || right_result)
            }
            FilterCondition::Not(inner) => {
                let result = self.evaluate_filter_condition(doc, inner)?;
                Ok(!result)
            }
        }
    }

    fn evaluate_filter_condition(
        &self,
        doc: &Document,
        condition: &FilterCondition,
    ) -> OQLResult<bool> {
        match condition {
            FilterCondition::Comparison {
                left,
                operator,
                right,
            } => {
                let left_val = self.evaluate_expression(doc, left)?;
                let right_val = self.evaluate_expression(doc, right)?;
                Ok(self.compare_values(&left_val, operator, &right_val))
            }
            FilterCondition::And(left, right) => {
                let left_result = self.evaluate_filter_condition(doc, left)?;
                let right_result = self.evaluate_filter_condition(doc, right)?;
                Ok(left_result && right_result)
            }
            FilterCondition::Or(left, right) => {
                let left_result = self.evaluate_filter_condition(doc, left)?;
                let right_result = self.evaluate_filter_condition(doc, right)?;
                Ok(left_result || right_result)
            }
            FilterCondition::Not(inner) => {
                let result = self.evaluate_filter_condition(doc, inner)?;
                Ok(!result)
            }
        }
    }

    fn evaluate_expression(&self, doc: &Document, expr: &Expression) -> OQLResult<Value> {
        match expr {
            Expression::Literal(lit) => Ok(Self::literal_to_value(lit)),
            Expression::FieldAccess(field_access) => self.evaluate_field_access(doc, field_access),
            Expression::BinaryOp {
                left,
                operator,
                right,
            } => {
                let left_val = self.evaluate_expression(doc, left)?;
                let right_val = self.evaluate_expression(doc, right)?;
                self.evaluate_binary_op(&left_val, operator, &right_val)
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

        let mut current_val = doc
            .fields
            .get(&field_access.path[0])
            .cloned()
            .unwrap_or(Value::Null);

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
        &self,
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

    fn compare_values(&self, left: &Value, operator: &ComparisonOperator, right: &Value) -> bool {
        match operator {
            ComparisonOperator::Equal => left == right,
            ComparisonOperator::NotEqual => left != right,
            ComparisonOperator::LessThan => self.value_less_than(left, right),
            ComparisonOperator::LessEqual => left == right || self.value_less_than(left, right),
            ComparisonOperator::GreaterThan => self.value_less_than(right, left),
            ComparisonOperator::GreaterEqual => left == right || self.value_less_than(right, left),
            ComparisonOperator::Contains
            | ComparisonOperator::StartsWith
            | ComparisonOperator::EndsWith
            | ComparisonOperator::In => {
                // String operations - simplified for now
                false
            }
        }
    }

    fn value_less_than(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => a < b,
            (Value::Float(a), Value::Float(b)) => a < b,
            (Value::String(a), Value::String(b)) => a < b,
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

        Ok(Document {
            id: doc.id.clone(),
            schema: doc.schema.clone(),
            fields: projected_fields,
            created_at: doc.created_at,
            updated_at: doc.updated_at,
            version: doc.version,
        })
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

        let mut current_val = doc
            .fields
            .get(&field_access.path[0])
            .cloned()
            .unwrap_or(Value::Null);

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
            Expression::FieldAccess(field_access) => field_access.path.join("."),
            _ => "expr".to_string(),
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
                    Ok(doc
                        .fields
                        .get(&field_access.path[0])
                        .cloned()
                        .unwrap_or(Value::Null))
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
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Less,
            (_, Value::Null) => Ordering::Greater,
            _ => Ordering::Equal,
        }
    }
}

impl PhysicalOperatorExec for SortExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)?;

        // Collect all documents
        self.sorted_documents.clear();
        while let Some(doc) = self.input.next()? {
            self.sorted_documents.push(doc);
        }

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

    fn close(&mut self) -> OQLResult<()> {
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.count = 0;
        self.input.reset()
    }
}
