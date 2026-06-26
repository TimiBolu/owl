// Parallel Aggregation Operator
// Parallel computation of aggregate functions

use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::PhysicalOperatorExec;
use crate::execution::thread_pool::ThreadPool;
use crate::planner::types::{AggregateFunction, AggregateSpec};
use crate::storage::Storage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Partial aggregate result from a single thread
#[derive(Debug, Clone)]
struct PartialAggregate {
    count: i64,
    sums: HashMap<String, f64>,
    mins: HashMap<String, Value>,
    maxs: HashMap<String, Value>,
}

impl PartialAggregate {
    fn new() -> Self {
        Self {
            count: 0,
            sums: HashMap::new(),
            mins: HashMap::new(),
            maxs: HashMap::new(),
        }
    }

    fn merge(&mut self, other: PartialAggregate) {
        self.count += other.count;

        // Merge sums
        for (key, value) in other.sums {
            *self.sums.entry(key).or_insert(0.0) += value;
        }

        // Merge mins
        for (key, value) in other.mins {
            let should_update = if let Some(current) = self.mins.get(&key) {
                Self::value_less_than(&value, current)
            } else {
                true
            };

            if should_update {
                self.mins.insert(key, value);
            }
        }

        // Merge maxs
        for (key, value) in other.maxs {
            let should_update = if let Some(current) = self.maxs.get(&key) {
                Self::value_less_than(current, &value)
            } else {
                true
            };

            if should_update {
                self.maxs.insert(key, value);
            }
        }
    }

    fn value_less_than(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => a < b,
            (Value::Float(a), Value::Float(b)) => a < b,
            (Value::String(a), Value::String(b)) => a < b,
            _ => false,
        }
    }
}

/// Parallel aggregate operator
///
/// Computes aggregate functions in parallel by dividing input into chunks,
/// computing partial aggregates, and then merging the results.
pub struct ParallelAggregateExec {
    aggregates: Vec<AggregateSpec>,
    input: Box<dyn PhysicalOperatorExec>,
    thread_pool: Arc<ThreadPool>,
    chunk_size: usize,
    result: Option<Document>,
    returned: bool,
}

impl ParallelAggregateExec {
    pub fn new(
        aggregates: Vec<AggregateSpec>,
        input: Box<dyn PhysicalOperatorExec>,
        thread_pool: Arc<ThreadPool>,
        chunk_size: usize,
    ) -> Self {
        let chunk_size = if chunk_size == 0 {
            100 / thread_pool.num_workers().max(1)
        } else {
            chunk_size
        };

        Self {
            aggregates,
            input,
            thread_pool,
            chunk_size: chunk_size.max(10),
            result: None,
            returned: false,
        }
    }

    fn compute_aggregates(&mut self) -> OQLResult<()> {
        // Collect all input documents
        let mut all_documents = Vec::new();
        while let Some(doc) = self.input.next()? {
            all_documents.push(doc);
        }

        let total_docs = all_documents.len();
        if total_docs == 0 {
            // Return empty result
            self.result = Some(self.create_result_document(PartialAggregate::new()));
            return Ok(());
        }

        // For small datasets, don't bother with parallelism
        if total_docs < self.chunk_size * 2 {
            let partial = Self::compute_partial_aggregate(&all_documents, &self.aggregates);
            self.result = Some(self.create_result_document(partial));
            return Ok(());
        }

        // Divide into chunks
        let num_chunks = (total_docs + self.chunk_size - 1) / self.chunk_size;
        let chunks: Vec<Vec<Document>> = (0..num_chunks)
            .map(|i| {
                let start = i * self.chunk_size;
                let end = ((i + 1) * self.chunk_size).min(total_docs);
                all_documents[start..end].to_vec()
            })
            .collect();

        // Compute partial aggregates in parallel
        let partial_results = Arc::new(Mutex::new(Vec::new()));
        let aggregates = self.aggregates.clone();

        for chunk in chunks {
            let partial_results_clone = Arc::clone(&partial_results);
            let aggregates_clone = aggregates.clone();

            self.thread_pool.execute(move || {
                let partial = Self::compute_partial_aggregate(&chunk, &aggregates_clone);
                partial_results_clone.lock().unwrap().push(partial);
            })?;
        }

        // Wait for all chunks to complete
        self.thread_pool.wait_for_completion();

        // Merge partial results
        let partial_results = partial_results
            .lock()
            .map_err(|_| OQLError::generic("Failed to lock partial results"))?;
        let mut final_aggregate = PartialAggregate::new();
        for partial in partial_results.iter() {
            final_aggregate.merge(partial.clone());
        }

        // Create result document
        self.result = Some(self.create_result_document(final_aggregate));

        Ok(())
    }

    fn compute_partial_aggregate(
        documents: &[Document],
        aggregates: &[AggregateSpec],
    ) -> PartialAggregate {
        let mut partial = PartialAggregate::new();

        for doc in documents {
            partial.count += 1;

            for spec in aggregates {
                if let Some(field_access) = &spec.field {
                    let field_key = field_access.path.join(".");
                    if let Some(value) = doc.fields.get(&field_access.path[0]) {
                        match spec.function {
                            AggregateFunction::Sum | AggregateFunction::Avg => {
                                if let Ok(num) = Self::value_to_number(value) {
                                    *partial.sums.entry(field_key.clone()).or_insert(0.0) += num;
                                }
                            }
                            AggregateFunction::Min => {
                                let should_update =
                                    if let Some(current) = partial.mins.get(&field_key) {
                                        PartialAggregate::value_less_than(value, current)
                                    } else {
                                        true
                                    };

                                if should_update {
                                    partial.mins.insert(field_key.clone(), value.clone());
                                }
                            }
                            AggregateFunction::Max => {
                                let should_update =
                                    if let Some(current) = partial.maxs.get(&field_key) {
                                        PartialAggregate::value_less_than(current, value)
                                    } else {
                                        true
                                    };

                                if should_update {
                                    partial.maxs.insert(field_key.clone(), value.clone());
                                }
                            }
                            AggregateFunction::Count => {
                                // Count is handled by partial.count
                            }
                        }
                    }
                }
            }
        }

        partial
    }

    fn create_result_document(&self, aggregate: PartialAggregate) -> Document {
        let mut fields = HashMap::new();

        for spec in &self.aggregates {
            let result_name = spec
                .alias
                .clone()
                .unwrap_or_else(|| format!("{:?}", spec.function));

            let value = match spec.function {
                AggregateFunction::Count => Value::Integer(aggregate.count),
                AggregateFunction::Sum => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        let sum = aggregate.sums.get(&field_key).copied().unwrap_or(0.0);
                        Value::Float(sum)
                    } else {
                        Value::Null
                    }
                }
                AggregateFunction::Avg => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        let sum = aggregate.sums.get(&field_key).copied().unwrap_or(0.0);
                        let avg = if aggregate.count > 0 {
                            sum / aggregate.count as f64
                        } else {
                            0.0
                        };
                        Value::Float(avg)
                    } else {
                        Value::Null
                    }
                }
                AggregateFunction::Min => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        aggregate
                            .mins
                            .get(&field_key)
                            .cloned()
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                }
                AggregateFunction::Max => {
                    if let Some(field) = &spec.field {
                        let field_key = field.path.join(".");
                        aggregate
                            .maxs
                            .get(&field_key)
                            .cloned()
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                }
            };

            fields.insert(result_name, value);
        }

        Document {
            id: "aggregate_result".to_string(),
            schema: "aggregate".to_string(),
            fields,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        }
    }

    fn value_to_number(value: &Value) -> OQLResult<f64> {
        match value {
            Value::Integer(i) => Ok(*i as f64),
            Value::Float(f) => Ok(*f),
            _ => Err(OQLError::generic("Cannot convert value to number")),
        }
    }
}

impl PhysicalOperatorExec for ParallelAggregateExec {
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

    fn create_test_document(schema: &str, id: &str, value: i64) -> Document {
        let mut fields = HashMap::new();
        fields.insert("value".to_string(), Value::Integer(value));

        Document::with_id(id.to_string(), schema.to_string(), fields)
    }

    #[test]
    fn test_parallel_aggregate_count() {
        let storage = create_storage("test_parallel_agg_count.owl", 10 * 1024 * 1024).unwrap();

        // Create test documents
        for i in 0..100 {
            let doc = create_test_document("items", &format!("item{}", i), i);
            storage.put_document(&doc).unwrap();
        }

        // Create parallel aggregate
        let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
        let input = Box::new(SeqScanExec::new("items".to_string()));
        let aggregates = vec![AggregateSpec {
            function: AggregateFunction::Count,
            field: None,
            alias: Some("total_count".to_string()),
        }];
        let mut agg = ParallelAggregateExec::new(aggregates, input, thread_pool, 25);

        // Execute
        agg.open(storage.clone()).unwrap();

        let result = agg.next().unwrap().unwrap();
        assert_eq!(result.fields.get("total_count"), Some(&Value::Integer(100)));

        agg.close().unwrap();
        std::fs::remove_file("test_parallel_agg_count.owl").ok();
    }

    #[test]
    fn test_parallel_aggregate_sum_avg() {
        let storage = create_storage("test_parallel_agg_sum.owl", 10 * 1024 * 1024).unwrap();

        // Create test documents (0 to 99)
        for i in 0..100 {
            let doc = create_test_document("items", &format!("item{}", i), i);
            storage.put_document(&doc).unwrap();
        }

        // Create parallel aggregate
        let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
        let input = Box::new(SeqScanExec::new("items".to_string()));
        let aggregates = vec![
            AggregateSpec {
                function: AggregateFunction::Sum,
                field: Some(FieldAccess {
                    path: vec!["value".to_string()],
                }),
                alias: Some("total_sum".to_string()),
            },
            AggregateSpec {
                function: AggregateFunction::Avg,
                field: Some(FieldAccess {
                    path: vec!["value".to_string()],
                }),
                alias: Some("avg_value".to_string()),
            },
        ];
        let mut agg = ParallelAggregateExec::new(aggregates, input, thread_pool, 25);

        // Execute
        agg.open(storage.clone()).unwrap();

        let result = agg.next().unwrap().unwrap();

        // Sum of 0..99 = 4950
        assert_eq!(result.fields.get("total_sum"), Some(&Value::Float(4950.0)));

        // Average = 4950 / 100 = 49.5
        assert_eq!(result.fields.get("avg_value"), Some(&Value::Float(49.5)));

        agg.close().unwrap();
        std::fs::remove_file("test_parallel_agg_sum.owl").ok();
    }

    #[test]
    fn test_parallel_aggregate_min_max() {
        let storage = create_storage("test_parallel_agg_minmax.owl", 10 * 1024 * 1024).unwrap();

        // Create test documents
        for i in 0..100 {
            let doc = create_test_document("items", &format!("item{}", i), i);
            storage.put_document(&doc).unwrap();
        }

        // Create parallel aggregate
        let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
        let input = Box::new(SeqScanExec::new("items".to_string()));
        let aggregates = vec![
            AggregateSpec {
                function: AggregateFunction::Min,
                field: Some(FieldAccess {
                    path: vec!["value".to_string()],
                }),
                alias: Some("min_value".to_string()),
            },
            AggregateSpec {
                function: AggregateFunction::Max,
                field: Some(FieldAccess {
                    path: vec!["value".to_string()],
                }),
                alias: Some("max_value".to_string()),
            },
        ];
        let mut agg = ParallelAggregateExec::new(aggregates, input, thread_pool, 25);

        // Execute
        agg.open(storage.clone()).unwrap();

        let result = agg.next().unwrap().unwrap();

        assert_eq!(result.fields.get("min_value"), Some(&Value::Integer(0)));
        assert_eq!(result.fields.get("max_value"), Some(&Value::Integer(99)));

        agg.close().unwrap();
        std::fs::remove_file("test_parallel_agg_minmax.owl").ok();
    }
}
