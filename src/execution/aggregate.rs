// Aggregate Operators
// Group by, having, and aggregation operators

use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::PhysicalOperatorExec;
use crate::planner::types::{AggregateFunction, AggregateSpec, FilterCondition};
use crate::storage::Storage;
use std::collections::HashMap;

/// Group by operator
///
/// Groups documents by specified fields.
pub struct GroupByExec {
    group_by_fields: Vec<String>,
    input: Box<dyn PhysicalOperatorExec>,
    groups: HashMap<String, Vec<Document>>,
    group_keys: Vec<String>,
    current_index: usize,
}

impl GroupByExec {
    pub fn new(group_by_fields: Vec<String>, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self {
            group_by_fields,
            input,
            groups: HashMap::new(),
            group_keys: Vec::new(),
            current_index: 0,
        }
    }

    fn build_groups(&mut self) -> OQLResult<()> {
        // Collect all documents and group them
        while let Some(doc) = self.input.next()? {
            let group_key = self.extract_group_key(&doc)?;
            self.groups
                .entry(group_key)
                .or_insert_with(Vec::new)
                .push(doc);
        }

        // Store group keys for iteration
        self.group_keys = self.groups.keys().cloned().collect();
        self.group_keys.sort(); // Deterministic order
        self.current_index = 0;

        Ok(())
    }

    fn extract_group_key(&self, doc: &Document) -> OQLResult<String> {
        let mut key_parts = Vec::new();

        for field in &self.group_by_fields {
            let value = doc.fields.get(field).cloned().unwrap_or(Value::Null);
            key_parts.push(value.to_string_repr());
        }

        Ok(key_parts.join("|"))
    }

    fn create_group_document(&self, group_key: &str, documents: &[Document]) -> Document {
        let mut fields = HashMap::new();

        // Add group key fields
        for (_i, field_name) in self.group_by_fields.iter().enumerate() {
            if let Some(first_doc) = documents.first() {
                if let Some(value) = first_doc.fields.get(field_name) {
                    fields.insert(field_name.clone(), value.clone());
                }
            }
        }

        // Add group metadata
        fields.insert(
            "_group_count".to_string(),
            Value::Integer(documents.len() as i64),
        );
        fields.insert(
            "_group_key".to_string(),
            Value::String(group_key.to_string()),
        );

        Document {
            id: format!("group_{}", group_key),
            schema: "grouped".to_string(),
            fields,
            created_at: documents
                .first()
                .map(|d| d.created_at)
                .unwrap_or_else(chrono::Utc::now),
            updated_at: documents
                .first()
                .map(|d| d.updated_at)
                .unwrap_or_else(chrono::Utc::now),
            version: 1,
        }
    }
}

impl PhysicalOperatorExec for GroupByExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)?;
        self.build_groups()?;
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        if self.current_index < self.group_keys.len() {
            let group_key = &self.group_keys[self.current_index];
            let documents = self.groups.get(group_key).unwrap();
            self.current_index += 1;
            Ok(Some(self.create_group_document(group_key, documents)))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> OQLResult<()> {
        self.groups.clear();
        self.group_keys.clear();
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.current_index = 0;
        Ok(())
    }
}

/// Having operator
///
/// Filters grouped documents based on aggregate conditions.
pub struct HavingExec {
    condition: FilterCondition,
    input: Box<dyn PhysicalOperatorExec>,
}

impl HavingExec {
    pub fn new(condition: FilterCondition, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self { condition, input }
    }

    fn evaluate_condition(&self, doc: &Document) -> OQLResult<bool> {
        // Simplified evaluation - in practice, this would evaluate aggregate functions
        match &self.condition {
            FilterCondition::Comparison {
                left: _,
                operator: _,
                right: _,
            } => {
                // For now, just check if group count meets condition
                if let Some(Value::Integer(count)) = doc.fields.get("_group_count") {
                    // Simplified: assume condition is "count() > N"
                    Ok(*count > 0)
                } else {
                    Ok(true)
                }
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
            FilterCondition::Comparison { .. } => self.evaluate_condition(doc),
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
}

impl PhysicalOperatorExec for HavingExec {
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

/// Aggregate operator
///
/// Computes aggregate functions (count, sum, avg, min, max).
pub struct AggregateExec {
    aggregates: Vec<AggregateSpec>,
    input: Box<dyn PhysicalOperatorExec>,
    result: Option<Document>,
    returned: bool,
}

impl AggregateExec {
    pub fn new(aggregates: Vec<AggregateSpec>, input: Box<dyn PhysicalOperatorExec>) -> Self {
        Self {
            aggregates,
            input,
            result: None,
            returned: false,
        }
    }

    fn compute_aggregates(&mut self) -> OQLResult<()> {
        let mut count: i64 = 0;
        let mut sums: HashMap<String, f64> = HashMap::new();
        let mut mins: HashMap<String, Value> = HashMap::new();
        let mut maxs: HashMap<String, Value> = HashMap::new();

        // Collect all documents and compute aggregates
        while let Some(doc) = self.input.next()? {
            count += 1;

            for spec in &self.aggregates {
                if let Some(field_access) = &spec.field {
                    let field_key = field_access.path.join(".");
                    if let Some(value) = doc.fields.get(&field_access.path[0]) {
                        match spec.function {
                            AggregateFunction::Sum => {
                                let num = self.value_to_number(value)?;
                                *sums.entry(field_key.clone()).or_insert(0.0) += num;
                            }
                            AggregateFunction::Avg => {
                                let num = self.value_to_number(value)?;
                                *sums.entry(field_key.clone()).or_insert(0.0) += num;
                            }
                            AggregateFunction::Min => {
                                let current_min = mins.get(&field_key);
                                if current_min.is_none()
                                    || self.value_less_than(value, current_min.unwrap())
                                {
                                    mins.insert(field_key.clone(), value.clone());
                                }
                            }
                            AggregateFunction::Max => {
                                let current_max = maxs.get(&field_key);
                                if current_max.is_none()
                                    || self.value_less_than(current_max.unwrap(), value)
                                {
                                    maxs.insert(field_key.clone(), value.clone());
                                }
                            }
                            AggregateFunction::Count | AggregateFunction::Exists => {
                                // Count is handled separately
                            }
                        }
                    }
                }
            }
        }

        // Build result document
        let mut fields = HashMap::new();

        for spec in &self.aggregates {
            let result_name = spec
                .alias
                .clone()
                .unwrap_or_else(|| format!("{:?}", spec.function));

            let value = match spec.function {
                AggregateFunction::Count => Value::Integer(count),
                AggregateFunction::Exists => Value::Boolean(count > 0),
                AggregateFunction::Sum => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        let sum = sums.get(&field_key).copied().unwrap_or(0.0);
                        Value::Float(sum)
                    } else {
                        Value::Null
                    }
                }
                AggregateFunction::Avg => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        let sum = sums.get(&field_key).copied().unwrap_or(0.0);
                        let avg = if count > 0 { sum / count as f64 } else { 0.0 };
                        Value::Float(avg)
                    } else {
                        Value::Null
                    }
                }
                AggregateFunction::Min => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        mins.get(&field_key).cloned().unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                }
                AggregateFunction::Max => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        maxs.get(&field_key).cloned().unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                }
            };

            fields.insert(result_name, value);
        }

        self.result = Some(Document {
            id: "aggregate_result".to_string(),
            schema: "aggregate".to_string(),
            fields,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        });

        Ok(())
    }

    fn value_to_number(&self, value: &Value) -> OQLResult<f64> {
        match value {
            Value::Integer(i) => Ok(*i as f64),
            Value::Float(f) => Ok(*f),
            _ => Err(OQLError::generic("Cannot convert value to number")),
        }
    }

    fn value_less_than(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => a < b,
            (Value::Float(a), Value::Float(b)) => a < b,
            (Value::String(a), Value::String(b)) => a < b,
            _ => false,
        }
    }
}

impl PhysicalOperatorExec for AggregateExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)?;
        self.compute_aggregates()?;
        self.returned = false;
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        if !self.returned {
            self.returned = true;
            Ok(self.result.clone())
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> OQLResult<()> {
        self.result = None;
        self.input.close()
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.returned = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::scan::SeqScanExec;
    use crate::parser::FieldAccess;
    use crate::storage::create_storage;

    fn create_test_document(schema: &str, id: &str, age: i64, score: f64) -> Document {
        let mut fields = HashMap::new();
        fields.insert("age".to_string(), Value::Integer(age));
        fields.insert("score".to_string(), Value::Float(score));

        Document::with_id(id.to_string(), schema.to_string(), fields)
    }

    #[test]
    fn test_aggregate_count() {
        let storage = create_storage("test_aggregate_count.owl", 10 * 1024 * 1024).unwrap();

        // Create test documents
        for i in 0..5 {
            let doc = create_test_document("users", &format!("user{}", i), 20 + i, 80.0 + i as f64);
            storage.put_document(&doc).unwrap();
        }

        // Create aggregate
        let input = Box::new(SeqScanExec::new("users".to_string()));
        let aggregates = vec![AggregateSpec {
            function: AggregateFunction::Count,
            field: None,
            alias: Some("total_count".to_string()),
        }];
        let mut agg = AggregateExec::new(aggregates, input);

        // Execute
        agg.open(storage.clone()).unwrap();

        let result = agg.next().unwrap().unwrap();
        assert_eq!(result.fields.get("total_count"), Some(&Value::Integer(5)));

        agg.close().unwrap();
        std::fs::remove_file("test_aggregate_count.owl").ok();
    }

    #[test]
    fn test_aggregate_sum_avg() {
        let storage = create_storage("test_aggregate_sum_avg.owl", 10 * 1024 * 1024).unwrap();

        // Create test documents
        for i in 0..5 {
            let doc = create_test_document("users", &format!("user{}", i), 20 + i, 80.0 + i as f64);
            storage.put_document(&doc).unwrap();
        }

        // Create aggregate
        let input = Box::new(SeqScanExec::new("users".to_string()));
        let aggregates = vec![
            AggregateSpec {
                function: AggregateFunction::Sum,
                field: Some(FieldAccess {
                    path: vec!["age".to_string()],
                }),
                alias: Some("total_age".to_string()),
            },
            AggregateSpec {
                function: AggregateFunction::Avg,
                field: Some(FieldAccess {
                    path: vec!["score".to_string()],
                }),
                alias: Some("avg_score".to_string()),
            },
        ];
        let mut agg = AggregateExec::new(aggregates, input);

        // Execute
        agg.open(storage.clone()).unwrap();

        let result = agg.next().unwrap().unwrap();

        // Sum of ages: 20 + 21 + 22 + 23 + 24 = 110
        assert_eq!(result.fields.get("total_age"), Some(&Value::Float(110.0)));

        // Avg of scores: (80 + 81 + 82 + 83 + 84) / 5 = 82.0
        assert_eq!(result.fields.get("avg_score"), Some(&Value::Float(82.0)));

        agg.close().unwrap();
        std::fs::remove_file("test_aggregate_sum_avg.owl").ok();
    }
}
