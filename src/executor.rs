// OQL Query Executor - Document scanning and filtering

use crate::distributed::GlobalObjRef;
use crate::document::{Document, ObjRef, Value};
use crate::error::{OQLError, OQLResult};
use crate::execution::{
    build_profiled_read_executor, build_read_executor, DataChunk, OperatorExecutionStats,
    PhysicalOperatorCollect,
};
use crate::functions::FunctionRegistry;
use crate::hot_path_optimizations::optimized_get_field;
use crate::profile_call;
// use crate::optimizer::QueryOptimizer; // TODO: Re-enable after optimizer module
use crate::parser::{
    ComparisonOperator, Condition, Expression, FieldAccess, GroupByBlock, HavingBlock, InsertBlock,
    Literal, NearestBlock, OQLQuery, Operation, SelectBlock, SortBlock, UpdateBlock, UpsertBlock,
    VectorMetric, WhereBlock,
};
use crate::planner::operators::{IndexAccessKind, PhysicalOperator};
use crate::planner::{LogicalPlanner, PhysicalPlan, PhysicalPlanner};
use crate::schema::DocumentSystemField;
use crate::schema::SchemaRegistry;
use crate::schema::{CheckOperator, FieldType, Schema, SchemaConstraint};
use crate::simulation::Clock;
use crate::storage::DocumentStore;
use crate::storage::{VectorIndexState, VectorIndexType, VectorMetricType};
use crate::streaming::{
    PaginatedResult, QueryStream, StreamConfig, StreamCursor, StreamingExecutor,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Query execution engine for OQL with dependency injection
#[allow(dead_code)]
pub struct QueryExecutor<S: DocumentStore> {
    store: Arc<S>,
    clock: Arc<dyn Clock>,
    functions: FunctionRegistry,
    // optimizer: QueryOptimizer, // TODO: Re-enable after optimizer module
    streaming_executor: StreamingExecutor<S>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReverseReferenceClause {
    target_schema: String,
    target_id: String,
    steps: Vec<ReverseReferenceStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReverseReferenceStep {
    source_schema: String,
    field_path: String,
}

#[derive(Debug, Clone, PartialEq)]
enum PathState {
    Present(Value),
    Null,
    Missing,
    BrokenReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReverseReferenceExplainStep {
    pub source_schema: String,
    pub field_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReverseReferenceExplainPlan {
    pub collection: String,
    pub target_schema: String,
    pub target_id_literal: String,
    pub steps: Vec<ReverseReferenceExplainStep>,
}

/// Documents returned by a query together with the physical plan that was
/// actually executed. A missing plan means the general OQL executor handled
/// the query.
#[derive(Debug)]
pub struct QueryExecution {
    pub documents: Vec<Document>,
    pub physical_plan: Option<String>,
    pub operator_profile: Option<OperatorExecutionStats>,
    pub planning_elapsed: Duration,
    /// Time spent in storage and physical operators, excluding conversion of
    /// column-major result batches into public `Document` values.
    pub execution_elapsed: Duration,
    pub materialization_elapsed: Duration,
}

/// Batch-native query output.
///
/// Safe physical reads preserve projected storage batches here instead of
/// constructing one `HashMap`-backed public document per row. Call
/// [`BatchQueryExecution::into_documents`] only at compatibility boundaries
/// that explicitly require the legacy document representation.
#[derive(Debug)]
pub struct BatchQueryExecution {
    pub chunks: Vec<DataChunk>,
    pub physical_plan: Option<String>,
    pub planning_elapsed: Duration,
    pub execution_elapsed: Duration,
}

impl BatchQueryExecution {
    pub fn len(&self) -> usize {
        self.chunks.iter().map(DataChunk::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(DataChunk::is_empty)
    }

    pub fn projected_chunk_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| chunk.is_projected())
            .count()
    }

    pub fn into_documents(self) -> Vec<Document> {
        let capacity = self.len();
        let mut documents = Vec::with_capacity(capacity);
        for chunk in self.chunks {
            documents.extend(chunk.into_documents());
        }
        documents
    }
}

impl<S: DocumentStore> QueryExecutor<S> {
    pub fn evaluate_script_expression(&self, expression: &Expression) -> OQLResult<Value> {
        self.evaluate_expression(
            &Document::with_id("$script".to_string(), "$script".to_string(), HashMap::new()),
            expression,
        )
    }
    /// Create a new query executor with dependency injection
    pub fn new(store: Arc<S>, clock: Arc<dyn Clock>) -> Self {
        let streaming_executor = StreamingExecutor::new_with_clock(store.clone(), clock.clone());
        Self {
            store: store.clone(),
            clock,
            functions: FunctionRegistry::new(),
            // optimizer: QueryOptimizer::new(), // TODO: Re-enable after optimizer module
            streaming_executor,
        }
    }

    /// Create a new query executor with production clock (for backward compatibility)
    pub fn new_with_real_clock(store: Arc<S>) -> Self {
        use crate::simulation::RealClock;
        Self::new(store, Arc::new(RealClock::new()))
    }

    pub fn explain_reverse_reference_prefilter(
        &self,
        query: &OQLQuery,
    ) -> OQLResult<Vec<ReverseReferenceExplainPlan>> {
        let Some(Operation::Where(where_block)) = query.operations.first() else {
            return Ok(Vec::new());
        };

        let clauses =
            self.collect_reverse_reference_clauses(&query.collection, &where_block.condition)?;
        Ok(clauses
            .into_iter()
            .map(|clause| ReverseReferenceExplainPlan {
                collection: query.collection.clone(),
                target_schema: clause.target_schema,
                target_id_literal: clause.target_id,
                steps: clause
                    .steps
                    .into_iter()
                    .map(|step| ReverseReferenceExplainStep {
                        source_schema: step.source_schema,
                        field_path: step.field_path,
                    })
                    .collect(),
            })
            .collect())
    }

    /// Register a user-defined function
    pub fn register_user_function(
        &mut self,
        func_def: crate::parser::FunctionDefinition,
    ) -> OQLResult<()> {
        self.functions.register_user_function(func_def)
    }

    /// Get access to the underlying store (for testing)
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Get function names (for testing)
    pub fn function_names(&self) -> Vec<String> {
        self.functions.function_names()
    }

    // TODO: Re-enable after optimizer module
    // /// Get access to the query optimizer
    // pub fn optimizer(&self) -> &QueryOptimizer {
    //     &self.optimizer
    // }
    //
    // /// Get query execution statistics
    // pub fn get_query_stats(&self) -> OQLResult<HashMap<String, crate::optimizer::QueryStats>> {
    //     self.optimizer.get_query_stats()
    // }
    //
    // /// Get index recommendations
    // pub fn get_index_recommendations(&self) -> OQLResult<Vec<crate::optimizer::IndexRecommendation>> {
    //     self.optimizer.get_index_recommendations()
    // }

    /// Clear query result cache
    /// TODO: Re-enable after optimizer module
    pub fn clear_query_cache(&self) -> OQLResult<()> {
        // self.optimizer.clear_cache()
        Ok(())
    }

    /// Execute a query as a stream for memory-efficient processing
    pub fn execute_query_stream(
        &self,
        query: &OQLQuery,
        config: StreamConfig,
    ) -> OQLResult<QueryStream<S>> {
        self.streaming_executor
            .execute_stream(query.clone(), config)
    }

    /// Execute a paginated query
    pub fn execute_paginated_query(
        &self,
        query: &OQLQuery,
        page_size: usize,
        cursor: Option<StreamCursor>,
    ) -> OQLResult<PaginatedResult> {
        self.streaming_executor
            .execute_paginated(query.clone(), page_size, cursor)
    }

    /// Count total documents for a query
    pub fn count_query_results(&self, query: &OQLQuery) -> OQLResult<usize> {
        self.streaming_executor.count_documents(query)
    }

    /// Execute an OQL query and return matching documents
    pub fn execute_query(&self, query: &OQLQuery) -> OQLResult<Vec<Document>> {
        profile_call!("execute_query", {
            if let Some(plan) = self.try_plan_safe_read(query)? {
                return self.execute_read_plan(&plan);
            }

            self.execute_general_query(query)
        })
    }

    /// Execute a read while retaining Owl's batch-native result representation.
    ///
    /// This is the preferred boundary for broad projections and streaming
    /// consumers. Queries outside the safe physical-read subset remain fully
    /// supported by wrapping their established document output in chunks.
    pub fn execute_query_batches(&self, query: &OQLQuery) -> OQLResult<BatchQueryExecution> {
        let planning_started = Instant::now();
        let plan = self.try_plan_safe_read(query)?;
        let planning_elapsed = planning_started.elapsed();

        if let Some(plan) = plan {
            let physical_plan = plan.root.explain();
            let execution_started = Instant::now();
            let chunks = self.execute_read_plan_batches(&plan)?;
            return Ok(BatchQueryExecution {
                chunks,
                physical_plan: Some(physical_plan),
                planning_elapsed,
                execution_elapsed: execution_started.elapsed(),
            });
        }

        let execution_started = Instant::now();
        let documents = self.execute_general_query(query)?;
        Ok(BatchQueryExecution {
            chunks: (!documents.is_empty())
                .then(|| DataChunk::new(documents))
                .into_iter()
                .collect(),
            physical_plan: None,
            planning_elapsed,
            execution_elapsed: execution_started.elapsed(),
        })
    }

    /// Execute a query and report the physical plan that was actually used.
    /// Planning and execution share one plan, so observability does not cause
    /// a second planner pass or risk describing a different execution.
    pub fn execute_query_with_plan(&self, query: &OQLQuery) -> OQLResult<QueryExecution> {
        profile_call!("execute_query_with_plan", {
            let planning_started = Instant::now();
            let plan = self.try_plan_safe_read(query)?;
            let planning_elapsed = planning_started.elapsed();

            if let Some(plan) = plan {
                let physical_plan = plan.root.explain();
                let execution_started = Instant::now();
                let (documents, operator_profile, materialization_elapsed) =
                    self.execute_read_plan_profiled(&plan)?;
                let total_execution = execution_started.elapsed();
                return Ok(QueryExecution {
                    documents,
                    physical_plan: Some(physical_plan),
                    operator_profile: Some(operator_profile),
                    planning_elapsed,
                    execution_elapsed: total_execution.saturating_sub(materialization_elapsed),
                    materialization_elapsed,
                });
            }

            let execution_started = Instant::now();
            Ok(QueryExecution {
                documents: self.execute_general_query(query)?,
                physical_plan: None,
                operator_profile: None,
                planning_elapsed,
                execution_elapsed: execution_started.elapsed(),
                materialization_elapsed: Duration::ZERO,
            })
        })
    }

    fn execute_general_query(&self, query: &OQLQuery) -> OQLResult<Vec<Document>> {
        let mut documents = Vec::new();
        let mut is_insert_only = false;

        // Check if this is an INSERT-only operation
        if query.operations.len() == 1 {
            match &query.operations[0] {
                Operation::Insert(_) | Operation::InsertMany(_) => {
                    is_insert_only = true;
                }
                _ => {}
            }
        }

        // For non-INSERT operations, start by scanning the collection
        if !is_insert_only {
            let scanned_docs =
                if let Some(prefiltered) = self.prefilter_documents_for_query(query)? {
                    prefiltered
                } else {
                    self.scan_collection(&query.collection)?
                };
            documents.extend(scanned_docs);
        }

        // Apply operations in sequence
        let mut grouped = false;
        for operation in &query.operations {
            // Grouping an empty collection yields no groups, so there is no
            // group row to project or filter. Without this, an aggregate
            // `select` would see no rows and summarise the whole (empty)
            // collection into one phantom result.
            if grouped
                && documents.is_empty()
                && matches!(operation, Operation::Select(_) | Operation::Having(_))
            {
                continue;
            }
            let temp_docs = std::mem::take(&mut documents);
            documents = self.apply_operation(temp_docs, operation, &query.collection)?;
            grouped |= matches!(operation, Operation::GroupBy(_));
        }

        Ok(documents)
    }

    /// Explain the native indexed read plan selected for this query. `None`
    /// means the query remains on the established general-purpose executor.
    pub fn explain_indexed_read(&self, query: &OQLQuery) -> OQLResult<Option<String>> {
        Ok(self.try_plan_safe_read(query)?.and_then(|plan| {
            Self::contains_native_index_scan(&plan.root).then(|| plan.root.explain())
        }))
    }

    fn execute_read_plan(&self, plan: &PhysicalPlan) -> OQLResult<Vec<Document>> {
        let mut executor = build_read_executor(&plan.root)?;
        let storage: crate::storage::Storage = self.store.clone();
        executor.open(storage)?;
        let result = executor.collect_all();
        let close_result = executor.close();
        match (result, close_result) {
            (Ok(documents), Ok(())) => Ok(documents),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn execute_read_plan_batches(&self, plan: &PhysicalPlan) -> OQLResult<Vec<DataChunk>> {
        let mut executor = build_read_executor(&plan.root)?;
        let storage: crate::storage::Storage = self.store.clone();
        executor.open(storage)?;
        let result = executor.collect_chunks();
        let close_result = executor.close();
        match (result, close_result) {
            (Ok(chunks), Ok(())) => Ok(chunks),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn execute_read_plan_profiled(
        &self,
        plan: &PhysicalPlan,
    ) -> OQLResult<(Vec<Document>, OperatorExecutionStats, Duration)> {
        let (mut executor, profile) = build_profiled_read_executor(&plan.root)?;
        let storage: crate::storage::Storage = self.store.clone();
        executor.open(storage)?;
        let result = executor.collect_all_timed();
        let close_result = executor.close();
        match (result, close_result) {
            (Ok((documents, materialization)), Ok(())) => {
                Ok((documents, profile.snapshot(), materialization))
            }
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn try_plan_safe_read(&self, query: &OQLQuery) -> OQLResult<Option<PhysicalPlan>> {
        if !Self::is_pipeline_safe_read(query) {
            return Ok(None);
        }
        // Committed scalar indexes do not contain transaction-local writes.
        // The general executor overlays them and is therefore authoritative
        // until commit publishes both records and index maintenance.
        if self.store.has_pending_writes()? {
            return Ok(None);
        }

        // Requiring a schema gives the logical planner enough information to
        // reject comparisons whose literal type does not match the indexed
        // field. That avoids changing OQL's coercion behavior at this boundary.
        let Some(schema) = self.store.get_schema(&query.collection)? else {
            return Ok(None);
        };
        // The general executor defines what a query means. Hand the planner
        // only predicates whose literal already has the field's stored type,
        // so index keys and storage-side comparisons cannot coerce
        // differently; anything else stays on the general executor.
        let Some(query) = Self::normalize_pipeline_literals(query, &schema) else {
            return Ok(None);
        };
        let query = &query;
        let registry = SchemaRegistry::new();
        registry.register(schema)?;
        let logical = match LogicalPlanner::with_schema_registry(registry).plan(query) {
            Ok(plan) => plan,
            Err(_) => return Ok(None),
        };
        let physical = PhysicalPlanner::new()
            .with_storage_catalog(self.store.as_ref())?
            .plan(&logical)?;

        Ok(Some(physical))
    }

    /// Rewrite numeric literals to the compared field's declared type, or
    /// return `None` when a predicate has no exact typed equivalent.
    ///
    /// `.price >= 10` on a `Float` field becomes `.price >= 10.0`, which is
    /// the same predicate under [`crate::value_semantics`]. `.age == 2.5` on
    /// an `Integer` field has no integer form, so the query is left to the
    /// general executor instead of being approximated.
    fn normalize_pipeline_literals(query: &OQLQuery, schema: &Schema) -> Option<OQLQuery> {
        fn scalar_type(field_type: &FieldType) -> &FieldType {
            match field_type {
                FieldType::Optional(inner) => scalar_type(inner),
                other => other,
            }
        }

        fn normalize(condition: &Condition, schema: &Schema) -> Option<Condition> {
            match condition {
                Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess { path }),
                    operator,
                    right: Expression::Literal(literal),
                } if path.len() == 1 => {
                    let literal = match (schema.fields.get(&path[0]).map(scalar_type), literal) {
                        (Some(FieldType::Float), Literal::Integer(value)) => {
                            let widened = *value as f64;
                            // Beyond 2^53 the float cannot represent the
                            // integer, so the predicate would change.
                            if widened as i64 != *value {
                                return None;
                            }
                            Literal::Float(widened)
                        }
                        (Some(FieldType::Integer), Literal::Float(value)) => {
                            if value.fract() != 0.0
                                || *value < -9_007_199_254_740_992.0
                                || *value > 9_007_199_254_740_992.0
                            {
                                return None;
                            }
                            Literal::Integer(*value as i64)
                        }
                        // The pipeline has no timestamp literal; the general
                        // executor compares timestamps with strings by
                        // instant.
                        (Some(FieldType::DateTime), Literal::String(_)) => return None,
                        (_, literal) => literal.clone(),
                    };
                    Some(Condition::Comparison {
                        left: Expression::FieldAccess(FieldAccess { path: path.clone() }),
                        operator: operator.clone(),
                        right: Expression::Literal(literal),
                    })
                }
                Condition::And(left, right) => Some(Condition::And(
                    Box::new(normalize(left, schema)?),
                    Box::new(normalize(right, schema)?),
                )),
                Condition::Or(left, right) => Some(Condition::Or(
                    Box::new(normalize(left, schema)?),
                    Box::new(normalize(right, schema)?),
                )),
                _ => None,
            }
        }

        let mut operations = Vec::with_capacity(query.operations.len());
        for operation in &query.operations {
            operations.push(match operation {
                Operation::Where(block) => Operation::Where(WhereBlock {
                    condition: normalize(&block.condition, schema)?,
                }),
                other => other.clone(),
            });
        }
        Some(OQLQuery {
            collection: query.collection.clone(),
            operations,
        })
    }

    fn is_pipeline_safe_read(query: &OQLQuery) -> bool {
        let mut has_where = false;
        let mut bounded = false;
        for operation in &query.operations {
            match operation {
                // The pipeline pushes filters down to the scan. That is only
                // the same query while every filter precedes every
                // limit/offset: `limit(5).where {..}` filters the first five
                // rows, it does not return five matching rows.
                Operation::Where(_) if bounded => return false,
                Operation::Limit(_) | Operation::Offset(_) | Operation::First => bounded = true,
                _ => {}
            }
            match operation {
                Operation::Where(block) => {
                    has_where = true;
                    if !Self::is_pipeline_safe_condition(&block.condition) {
                        return false;
                    }
                }
                Operation::Select(block) => {
                    if block
                        .fields
                        .iter()
                        .any(|field| !Self::is_pipeline_safe_expression(&field.expression))
                    {
                        return false;
                    }
                }
                Operation::Sort(block) => {
                    if block
                        .fields
                        .iter()
                        .any(|field| !Self::is_pipeline_safe_expression(&field.expression))
                    {
                        return false;
                    }
                }
                Operation::Limit(_) => {}
                Operation::Offset(_) | Operation::First => {}
                _ => return false,
            }
        }
        has_where
    }

    fn is_pipeline_safe_expression(expression: &Expression) -> bool {
        match expression {
            Expression::Literal(_) => true,
            Expression::FieldAccess(FieldAccess { path }) => path.len() == 1,
            _ => false,
        }
    }

    fn is_pipeline_safe_condition(condition: &Condition) -> bool {
        match condition {
            Condition::Comparison {
                left: Expression::FieldAccess(FieldAccess { path }),
                operator,
                right: Expression::Literal(_),
            } => {
                path.len() == 1
                    && matches!(
                        operator,
                        ComparisonOperator::Equal
                            | ComparisonOperator::LessThan
                            | ComparisonOperator::LessEqual
                            | ComparisonOperator::GreaterThan
                            | ComparisonOperator::GreaterEqual
                    )
            }
            Condition::And(left, right) => {
                Self::is_pipeline_safe_condition(left) && Self::is_pipeline_safe_condition(right)
            }
            Condition::Or(left, right) => {
                Self::is_pipeline_safe_condition(left) && Self::is_pipeline_safe_condition(right)
            }
            _ => false,
        }
    }

    fn contains_native_index_scan(operator: &PhysicalOperator) -> bool {
        match operator {
            PhysicalOperator::IndexScan(scan) => scan.access == IndexAccessKind::NativeTyped,
            PhysicalOperator::IndexUnion(union) => {
                union.inputs.iter().all(Self::contains_native_index_scan)
            }
            PhysicalOperator::Filter(operator) => Self::contains_native_index_scan(&operator.input),
            PhysicalOperator::Project(operator) => {
                Self::contains_native_index_scan(&operator.input)
            }
            PhysicalOperator::Sort(operator) => Self::contains_native_index_scan(&operator.input),
            PhysicalOperator::Limit(operator) => Self::contains_native_index_scan(&operator.input),
            PhysicalOperator::Offset(operator) => Self::contains_native_index_scan(&operator.input),
            _ => false,
        }
    }

    /// Scan all documents in a collection
    fn scan_collection(&self, collection: &str) -> OQLResult<Vec<Document>> {
        let documents = self.store.scan_documents(collection)?;

        Ok(documents)
    }

    fn prefilter_documents_for_query(&self, query: &OQLQuery) -> OQLResult<Option<Vec<Document>>> {
        let Some(Operation::Where(where_block)) = query.operations.first() else {
            return Ok(None);
        };

        let clauses =
            self.collect_reverse_reference_clauses(&query.collection, &where_block.condition)?;
        if clauses.is_empty() {
            return Ok(None);
        }

        let mut candidate_documents: Option<Vec<Document>> = None;
        let mut seen_ids: Option<HashSet<String>> = None;

        for clause in clauses {
            let target_id =
                match self.resolve_canonical_target_id(&clause.target_schema, &clause.target_id)? {
                    Some(id) => id,
                    None => return Ok(Some(Vec::new())),
                };
            let docs = self.resolve_reverse_reference_clause_documents(&clause, &target_id)?;

            let docs_by_id: HashMap<String, Document> =
                docs.into_iter().map(|doc| (doc.id.clone(), doc)).collect();
            let ids: HashSet<String> = docs_by_id.keys().cloned().collect();

            match (&mut candidate_documents, &mut seen_ids) {
                (None, None) => {
                    candidate_documents = Some(docs_by_id.into_values().collect());
                    seen_ids = Some(ids);
                }
                (Some(current_docs), Some(current_ids)) => {
                    current_docs.retain(|doc| ids.contains(&doc.id));
                    current_ids.retain(|id| ids.contains(id));
                }
                _ => {}
            }
        }

        Ok(candidate_documents)
    }

    fn resolve_reverse_reference_clause_documents(
        &self,
        clause: &ReverseReferenceClause,
        canonical_target_id: &str,
    ) -> OQLResult<Vec<Document>> {
        let mut current_targets = vec![(
            clause.target_schema.clone(),
            canonical_target_id.to_string(),
        )];
        let mut current_documents = Vec::new();

        for step in clause.steps.iter().rev() {
            let mut next_documents = Vec::new();
            let mut seen = HashSet::new();

            for (target_schema, target_id) in &current_targets {
                let docs = self.store.scan_referencing_documents(
                    target_schema,
                    target_id,
                    Some(&step.source_schema),
                    Some(&step.field_path),
                )?;
                for doc in docs {
                    let key = (doc.schema.clone(), doc.id.clone());
                    if seen.insert(key) {
                        next_documents.push(doc);
                    }
                }
            }

            current_targets = next_documents
                .iter()
                .map(|doc| (doc.schema.clone(), doc.id.clone()))
                .collect();
            current_documents = next_documents;

            if current_targets.is_empty() {
                break;
            }
        }

        Ok(current_documents)
    }

    fn collect_reverse_reference_clauses(
        &self,
        source_collection: &str,
        condition: &Condition,
    ) -> OQLResult<Vec<ReverseReferenceClause>> {
        let mut clauses = Vec::new();
        self.collect_reverse_reference_clauses_into(source_collection, condition, &mut clauses)?;
        Ok(clauses)
    }

    fn collect_reverse_reference_clauses_into(
        &self,
        source_collection: &str,
        condition: &Condition,
        out: &mut Vec<ReverseReferenceClause>,
    ) -> OQLResult<()> {
        match condition {
            Condition::And(left, right) => {
                self.collect_reverse_reference_clauses_into(source_collection, left, out)?;
                self.collect_reverse_reference_clauses_into(source_collection, right, out)?;
            }
            Condition::Comparison {
                left,
                operator: ComparisonOperator::Equal,
                right,
            } => {
                if let Some(clause) =
                    self.reverse_reference_clause_from_comparison(source_collection, left, right)?
                {
                    out.push(clause);
                } else if let Some(clause) =
                    self.reverse_reference_clause_from_comparison(source_collection, right, left)?
                {
                    out.push(clause);
                }
            }
            Condition::Comparison { .. } | Condition::Or(_, _) | Condition::Not(_) => {}
        }
        Ok(())
    }

    fn reverse_reference_clause_from_comparison(
        &self,
        source_collection: &str,
        field_expr: &Expression,
        literal_expr: &Expression,
    ) -> OQLResult<Option<ReverseReferenceClause>> {
        let Expression::FieldAccess(field_access) = field_expr else {
            return Ok(None);
        };
        let target_literal = match literal_expr {
            Expression::Literal(Literal::String(value)) => value.clone(),
            _ => return Ok(None),
        };
        if field_access.path.len() < 2 || field_access.path.last().map(String::as_str) != Some("id")
        {
            return Ok(None);
        }
        let mut current_schema = source_collection.to_string();
        let mut steps = Vec::new();

        for segment in &field_access.path[..field_access.path.len() - 1] {
            let Some(schema) = self.store.get_schema(&current_schema)? else {
                return Ok(None);
            };
            let Some(field_type) = schema.get_field_type(segment) else {
                return Ok(None);
            };
            let Some(target_schema) = field_type.referenced_schema() else {
                return Ok(None);
            };
            steps.push(ReverseReferenceStep {
                source_schema: current_schema.clone(),
                field_path: segment.clone(),
            });
            current_schema = target_schema.to_string();
        }

        Ok(Some(ReverseReferenceClause {
            target_schema: current_schema,
            target_id: target_literal,
            steps,
        }))
    }

    fn resolve_canonical_target_id(
        &self,
        target_schema: &str,
        observed_id: &str,
    ) -> OQLResult<Option<String>> {
        if self
            .store
            .get_document(target_schema, &observed_id.to_string())?
            .is_some()
        {
            return Ok(Some(observed_id.to_string()));
        }

        let docs = self.store.scan_documents(target_schema)?;
        Ok(docs.into_iter().find_map(|doc| {
            if doc.id == observed_id {
                Some(doc.id)
            } else {
                match doc.fields.get("id") {
                    Some(Value::String(logical_id)) if logical_id == observed_id => Some(doc.id),
                    _ => None,
                }
            }
        }))
    }

    /// Apply a single operation to a set of documents
    fn apply_operation(
        &self,
        documents: Vec<Document>,
        operation: &Operation,
        collection: &str,
    ) -> OQLResult<Vec<Document>> {
        match operation {
            Operation::Where(where_block) => {
                self.apply_where_filter(documents, &where_block.condition)
            }
            Operation::Select(select_block) => {
                self.apply_select_projection(documents, select_block)
            }
            Operation::Insert(insert_block) => {
                self.apply_insert_operation(collection, insert_block)
            }
            Operation::InsertMany(insert_blocks) => {
                self.apply_insert_many_operation(collection, insert_blocks)
            }
            Operation::Upsert(upsert_block) => {
                self.apply_upsert_operation(documents, collection, upsert_block)
            }
            Operation::Update(update_block) => self.apply_update_operation(documents, update_block),
            Operation::Delete => self.apply_delete_operation(documents),
            Operation::GroupBy(group_by_block) => {
                self.apply_group_by_operation(documents, group_by_block)
            }
            Operation::Nearest(nearest_block) => {
                self.apply_nearest_operation(documents, nearest_block)
            }
            Operation::Having(having_block) => self.apply_having_operation(documents, having_block),
            Operation::Count => self.apply_count_operation(documents),
            Operation::Sum(field_access) => self.apply_sum_operation(documents, field_access),
            Operation::Avg(field_access) => self.apply_avg_operation(documents, field_access),
            Operation::Min(field_access) => self.apply_min_operation(documents, field_access),
            Operation::Max(field_access) => self.apply_max_operation(documents, field_access),
            Operation::Sort(sort_block) => self.apply_sort_operation(documents, sort_block),
            Operation::Limit(limit_value) => self.apply_limit_operation(documents, *limit_value),
            Operation::Offset(offset) => Ok(documents.into_iter().skip(*offset as usize).collect()),
            Operation::First => Ok(documents.into_iter().take(1).collect()),
            Operation::Exists => {
                let exists = !documents.is_empty();
                Ok(vec![Document::with_id(
                    "exists_result".to_string(),
                    "aggregate".to_string(),
                    HashMap::from([("exists".to_string(), Value::Boolean(exists))]),
                )])
            }
            Operation::FunctionDef(_) => {
                let context = crate::error::ErrorContext::new("apply_operation")
                    .with_location("query_executor")
                    .with_info("operation_type", "function_definition");

                Err(OQLError::EnhancedQueryExecution {
                    message: "Function definitions cannot be executed as query operations"
                        .to_string(),
                    context: Some(context),
                    severity: crate::error::ErrorSeverity::Error,
                    recovery: Some(crate::error::RecoverySuggestion {
                        suggestion: "Define functions separately from queries".to_string(),
                        example: Some("fn my_function() { ... } then use in queries".to_string()),
                        auto_recoverable: false,
                    }),
                })
            }
        }
    }

    /// Apply WHERE clause filtering to documents
    fn apply_where_filter(
        &self,
        documents: Vec<Document>,
        condition: &Condition,
    ) -> OQLResult<Vec<Document>> {
        let mut filtered = Vec::new();

        for document in documents {
            if self.evaluate_condition(&document, condition)? {
                filtered.push(document);
            }
        }

        Ok(filtered)
    }

    /// Apply nearest-neighbor vector search to the current document set.
    ///
    /// Returns top-k documents sorted by descending score and injects `_score`.
    fn apply_nearest_operation(
        &self,
        documents: Vec<Document>,
        nearest_block: &NearestBlock,
    ) -> OQLResult<Vec<Document>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let query_vector = self.resolve_query_vector(&nearest_block.query)?;
        if query_vector.is_empty() {
            return Err(OQLError::TypeError {
                expected: "non-empty numeric query vector".to_string(),
                actual: "empty vector".to_string(),
            });
        }

        let field_name = nearest_block
            .field
            .path
            .first()
            .ok_or_else(|| OQLError::generic("nearest.field cannot be empty"))?;

        // Enforce schema vector dimension/type when available.
        if let Some(schema_name) = documents.first().map(|d| d.schema.clone()) {
            if let Ok(Some(schema)) = self.store.get_schema(&schema_name) {
                if let Some(field_type) = schema.get_field_type(field_name) {
                    match field_type {
                        crate::schema::FieldType::Vector { dim } => {
                            if query_vector.len() != *dim as usize {
                                return Err(OQLError::TypeError {
                                    expected: format!("Vector({})", dim),
                                    actual: format!("Vector({})", query_vector.len()),
                                });
                            }
                        }
                        other => {
                            return Err(OQLError::TypeError {
                                expected: "Vector".to_string(),
                                actual: other.type_name().to_string(),
                            });
                        }
                    }
                }
            }
        }

        let mut full_vectors: Vec<Vec<f32>> = Vec::new();
        let mut row_to_doc_idx: Vec<usize> = Vec::new();
        full_vectors.reserve(documents.len());
        row_to_doc_idx.reserve(documents.len());

        for (doc_idx, document) in documents.iter().enumerate() {
            let value = self.get_field_value(document, &nearest_block.field)?;
            if let Some(vector) = Self::value_to_numeric_vector(&value) {
                if vector.len() == query_vector.len() {
                    full_vectors.push(vector);
                    row_to_doc_idx.push(doc_idx);
                }
            }
        }

        if full_vectors.is_empty() {
            return Ok(Vec::new());
        }

        let k = (nearest_block.k as usize).min(full_vectors.len());
        if k == 0 {
            return Ok(Vec::new());
        }

        let schema_name = documents
            .first()
            .map(|d| d.schema.as_str())
            .unwrap_or_default()
            .to_string();
        let field_name = field_name.to_string();
        let index_catalog = self
            .store
            .list_vector_indexes(Some(&schema_name))
            .unwrap_or_default();
        let matched_index = index_catalog.into_iter().find(|idx| {
            idx.schema == schema_name
                && idx.field == field_name
                && idx.state == VectorIndexState::Ready
                && Self::metric_matches(&nearest_block.metric, idx.metric)
        });

        let ann_enabled = nearest_block.exact.unwrap_or(false) == false
            && matched_index
                .as_ref()
                .map(|idx| idx.index_type == VectorIndexType::Hnsw)
                .unwrap_or(false);
        let num_candidates = nearest_block
            .num_candidates
            .map(|v| v as usize)
            .unwrap_or_else(|| (k * 8).max(k))
            .min(full_vectors.len());

        let topk = if ann_enabled {
            if let Some(index_def) = matched_index.as_ref() {
                match self.store.vector_ann_search(
                    index_def,
                    &documents,
                    &query_vector,
                    k,
                    num_candidates,
                ) {
                    Ok(Some(hits)) => hits
                        .into_iter()
                        .map(|h| crate::storage_engine::vector::VectorHit {
                            index: h.index as u32,
                            score: h.score,
                        })
                        .collect(),
                    Ok(None) => {
                        let candidate_indices = if num_candidates < full_vectors.len() {
                            self.approximate_candidate_indices(
                                &full_vectors,
                                &query_vector,
                                &nearest_block.metric,
                                num_candidates,
                            )?
                        } else {
                            (0..full_vectors.len()).collect()
                        };
                        self.exact_rerank_on_candidates(
                            &full_vectors,
                            &query_vector,
                            &nearest_block.metric,
                            &candidate_indices,
                            k,
                        )?
                    }
                    Err(_) => {
                        let candidate_indices = if num_candidates < full_vectors.len() {
                            self.approximate_candidate_indices(
                                &full_vectors,
                                &query_vector,
                                &nearest_block.metric,
                                num_candidates,
                            )?
                        } else {
                            (0..full_vectors.len()).collect()
                        };
                        self.exact_rerank_on_candidates(
                            &full_vectors,
                            &query_vector,
                            &nearest_block.metric,
                            &candidate_indices,
                            k,
                        )?
                    }
                }
            } else {
                let candidate_indices = if num_candidates < full_vectors.len() {
                    self.approximate_candidate_indices(
                        &full_vectors,
                        &query_vector,
                        &nearest_block.metric,
                        num_candidates,
                    )?
                } else {
                    (0..full_vectors.len()).collect()
                };
                self.exact_rerank_on_candidates(
                    &full_vectors,
                    &query_vector,
                    &nearest_block.metric,
                    &candidate_indices,
                    k,
                )?
            }
        } else {
            let candidate_indices: Vec<usize> = (0..full_vectors.len()).collect();
            self.exact_rerank_on_candidates(
                &full_vectors,
                &query_vector,
                &nearest_block.metric,
                &candidate_indices,
                k,
            )?
        };

        let mut output = Vec::with_capacity(topk.len());
        for hit in topk {
            let source_idx = *row_to_doc_idx
                .get(hit.index as usize)
                .ok_or_else(|| OQLError::generic("vector top-k returned invalid index"))?;
            let mut document = documents[source_idx].clone();
            document
                .fields
                .insert("_score".to_string(), Value::Float(hit.score as f64));
            output.push(document);
        }

        Ok(output)
    }

    fn resolve_query_vector(&self, query_expression: &Expression) -> OQLResult<Vec<f32>> {
        let temp_doc = Document::new("__vector_query__".to_string(), HashMap::new());
        let query_value = self.evaluate_expression(&temp_doc, query_expression)?;
        Self::value_to_numeric_vector(&query_value).ok_or_else(|| OQLError::TypeError {
            expected: "numeric list".to_string(),
            actual: query_value.type_name().to_string(),
        })
    }

    fn value_to_numeric_vector(value: &Value) -> Option<Vec<f32>> {
        match value {
            Value::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Value::Float(v) => out.push(*v as f32),
                        Value::Integer(v) => out.push(*v as f32),
                        _ => return None,
                    }
                }
                Some(out)
            }
            _ => None,
        }
    }

    fn metric_to_ffi(metric: &VectorMetric) -> crate::storage_engine::vector::VectorMetric {
        match metric {
            VectorMetric::Cosine => crate::storage_engine::vector::VectorMetric::Cosine,
            VectorMetric::L2 => crate::storage_engine::vector::VectorMetric::L2,
            VectorMetric::Dot => crate::storage_engine::vector::VectorMetric::Dot,
        }
    }

    fn metric_matches(metric: &VectorMetric, index_metric: VectorMetricType) -> bool {
        matches!(
            (metric, index_metric),
            (VectorMetric::Cosine, VectorMetricType::Cosine)
                | (VectorMetric::L2, VectorMetricType::L2)
                | (VectorMetric::Dot, VectorMetricType::Dot)
        )
    }

    fn approximate_candidate_indices(
        &self,
        full_vectors: &[Vec<f32>],
        query_vector: &[f32],
        metric: &VectorMetric,
        num_candidates: usize,
    ) -> OQLResult<Vec<usize>> {
        let projection_dim = full_vectors.first().map(|v| v.len().min(8)).unwrap_or(0);
        if projection_dim == 0 {
            return Ok(Vec::new());
        }

        let mut projected_rows = Vec::with_capacity(full_vectors.len() * projection_dim);
        for vec in full_vectors {
            projected_rows.extend_from_slice(&vec[..projection_dim]);
        }
        let projected_query = query_vector[..projection_dim].to_vec();
        let coarse = crate::storage_engine::vector::vector_topk_f32(
            &projected_rows,
            full_vectors.len(),
            projection_dim,
            &projected_query,
            Self::metric_to_ffi(metric),
            num_candidates,
        )
        .map_err(|e| {
            OQLError::storage_error(&format!("vector coarse top-k failed: {}", e), None)
        })?;

        let mut candidate_indices = Vec::with_capacity(coarse.len());
        for hit in coarse {
            candidate_indices.push(hit.index as usize);
        }
        Ok(candidate_indices)
    }

    fn exact_rerank_on_candidates(
        &self,
        full_vectors: &[Vec<f32>],
        query_vector: &[f32],
        metric: &VectorMetric,
        candidate_indices: &[usize],
        k: usize,
    ) -> OQLResult<Vec<crate::storage_engine::vector::VectorHit>> {
        if candidate_indices.is_empty() {
            return Ok(Vec::new());
        }
        let dim = query_vector.len();
        let mut candidate_rows = Vec::with_capacity(candidate_indices.len() * dim);
        for &idx in candidate_indices {
            let vec = full_vectors
                .get(idx)
                .ok_or_else(|| OQLError::generic("invalid candidate index"))?;
            candidate_rows.extend_from_slice(vec);
        }
        let reranked = crate::storage_engine::vector::vector_topk_f32(
            &candidate_rows,
            candidate_indices.len(),
            dim,
            query_vector,
            Self::metric_to_ffi(metric),
            k.min(candidate_indices.len()),
        )
        .map_err(|e| {
            OQLError::storage_error(&format!("vector exact rerank failed: {}", e), None)
        })?;

        let mut remapped = Vec::with_capacity(reranked.len());
        for hit in reranked {
            let original = *candidate_indices
                .get(hit.index as usize)
                .ok_or_else(|| OQLError::generic("invalid rerank index"))?;
            remapped.push(crate::storage_engine::vector::VectorHit {
                index: original as u32,
                score: hit.score,
            });
        }
        Ok(remapped)
    }

    /// Evaluate a condition against a document
    fn evaluate_condition(&self, document: &Document, condition: &Condition) -> OQLResult<bool> {
        match condition {
            Condition::Comparison {
                left,
                operator,
                right,
            } => {
                let left_value = self.evaluate_expression(document, left)?;
                let right_value = self.evaluate_expression(document, right)?;
                self.compare_values(&left_value, operator, &right_value)
            }
            // Both connectives short-circuit, so a guard such as
            // `.kind == "a" && expensive_or_failing(.x)` never evaluates its
            // right side for rows the left side already decided.
            Condition::And(left, right) => Ok(self.evaluate_condition(document, left)?
                && self.evaluate_condition(document, right)?),
            Condition::Or(left, right) => Ok(self.evaluate_condition(document, left)?
                || self.evaluate_condition(document, right)?),
            Condition::Not(inner) => {
                let result = self.evaluate_condition(document, inner)?;
                Ok(!result)
            }
        }
    }

    /// Evaluate an expression against a document
    fn evaluate_expression(
        &self,
        document: &Document,
        expression: &Expression,
    ) -> OQLResult<Value> {
        match expression {
            Expression::Identifier(name) => {
                // A bare name is a function parameter when one is in scope,
                // then a query parameter, and otherwise a field of the
                // current row.
                if let Some(value) = CallFrame::value(name) {
                    return Ok(value);
                }
                if let Some(row) = CallFrame::row(name) {
                    return Ok(Self::row_as_value(&row));
                }
                if name.starts_with('$') {
                    return Err(OQLError::Generic {
                        message: format!(
                            "query parameter {name} has no value; bind it before executing"
                        ),
                    });
                }
                self.get_field_value(
                    document,
                    &FieldAccess {
                        path: vec![name.clone()],
                    },
                )
            }
            Expression::FieldAccess(field_access) => self.get_field_value(document, field_access),
            Expression::Literal(literal) => Ok(self.literal_to_value(literal)),
            Expression::FunctionCall { name, args } => {
                self.evaluate_function_call(document, name, args)
            }
            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => {
                let condition_result = self.evaluate_condition(document, condition)?;
                if condition_result {
                    self.evaluate_expression(document, then_expr)
                } else {
                    self.evaluate_expression(document, else_expr)
                }
            }
            Expression::Match { expr, arms } => {
                let expr_value = self.evaluate_expression(document, expr)?;
                self.evaluate_match_expression(document, &expr_value, arms)
            }
            Expression::LogicalAnd(left, right) => {
                let left_value = self.evaluate_expression(document, left)?;
                let left_bool = self.value_to_bool(&left_value)?;

                if !left_bool {
                    Ok(Value::Boolean(false))
                } else {
                    let right_value = self.evaluate_expression(document, right)?;
                    let right_bool = self.value_to_bool(&right_value)?;
                    Ok(Value::Boolean(right_bool))
                }
            }
            Expression::LogicalOr(left, right) => {
                // In a value position `a || b` yields the first operand that
                // is present and truthy, so `.nickname || .name` is a
                // fallback. With boolean operands this is ordinary OR.
                let left_value = self.evaluate_expression(document, left)?;
                if self.value_to_bool(&left_value)? {
                    Ok(left_value)
                } else {
                    self.evaluate_expression(document, right)
                }
            }
            Expression::LogicalNot(expr) => {
                let value = self.evaluate_expression(document, expr)?;
                let bool_value = self.value_to_bool(&value)?;
                Ok(Value::Boolean(!bool_value))
            }
            Expression::BinaryOp {
                left,
                operator,
                right,
            } => {
                let left_value = self.evaluate_expression(document, left)?;
                let right_value = self.evaluate_expression(document, right)?;
                self.evaluate_binary_operation(&left_value, operator, &right_value)
            }
            Expression::ArrayAccess { array, index } => {
                let array_value = self.evaluate_expression(document, array)?;
                let index_value = self.evaluate_expression(document, index)?;
                self.evaluate_array_access(&array_value, &index_value)
            }
            Expression::ChainedAccess { base, field } => {
                // `param.path` on a row parameter is ordinary path access on
                // that row, with reference traversal and null-safety.
                if let Some((row, path)) = Self::row_parameter_path(expression) {
                    return self.get_field_value(&row, &FieldAccess { path });
                }
                let base_value = self.evaluate_expression(document, base)?;
                self.get_field_from_value(&base_value, field)
            }
            Expression::Block(expressions) => {
                // Evaluate all expressions in the block and return the last one
                let mut last_value = Value::Null;
                for expr in expressions {
                    last_value = self.evaluate_expression(document, expr)?;
                }
                Ok(last_value)
            }
            Expression::ArrayLiteral(elements) => {
                let mut values = Vec::new();
                for element in elements {
                    let value = self.evaluate_expression(document, element)?;
                    values.push(value);
                }
                Ok(Value::List(values))
            }
            Expression::ObjectLiteral(fields) => {
                let mut object_fields = std::collections::HashMap::new();
                for field in fields {
                    let value = self.evaluate_expression(document, &field.value)?;
                    object_fields.insert(field.name.clone(), value);
                }
                Ok(Value::Object(object_fields))
            }
            Expression::ConditionExpr(condition) => Ok(Value::Boolean(
                self.evaluate_condition(document, condition)?,
            )),
            Expression::QueryExpression { query } => {
                self.evaluate_query_expression(document, query)
            }
            Expression::CollectionExpression { source, operations } => {
                self.evaluate_collection_expression(document, source, operations)
            }
        }
    }

    fn evaluate_collection_expression(
        &self,
        scope_document: &Document,
        source: &Expression,
        operations: &[Operation],
    ) -> OQLResult<Value> {
        let reverse_documents = match source {
            Expression::FieldAccess(field) => {
                self.resolve_reverse_collection_source(scope_document, &field.path)?
            }
            _ => None,
        };
        let collection_scope = match source {
            Expression::FieldAccess(field) => field
                .path
                .last()
                .cloned()
                .unwrap_or_else(|| "$collection".to_string()),
            Expression::Identifier(name) => name.clone(),
            _ => "$collection".to_string(),
        };
        let (mut documents, source_was_object) = if let Some(documents) = reverse_documents {
            (documents, false)
        } else {
            let source_value = self.evaluate_expression(scope_document, source)?;
            let source_was_object = matches!(source_value, Value::Object(_));
            let values = match source_value {
                Value::List(values) => values,
                Value::Object(object) => vec![Value::Object(object)],
                Value::Null => Vec::new(),
                scalar => vec![scalar],
            };
            let mut documents = Vec::with_capacity(values.len());
            for (index, value) in values.into_iter().enumerate() {
                let fields = match value {
                    Value::Object(fields) => fields,
                    scalar => HashMap::from([("value".to_string(), scalar)]),
                };
                documents.push(Document::with_id(
                    index.to_string(),
                    collection_scope.clone(),
                    fields,
                ));
            }
            (documents, source_was_object)
        };

        for operation in operations {
            let materialized = self.materialize_operation_with_scope(operation, scope_document)?;
            documents = self.apply_operation(documents, &materialized, &collection_scope)?;
        }

        let aggregate_field = match operations.last() {
            Some(Operation::Count) => Some("count"),
            Some(Operation::Sum(_)) => Some("sum"),
            Some(Operation::Avg(_)) => Some("avg"),
            Some(Operation::Min(_)) => Some("min"),
            Some(Operation::Max(_)) => Some("max"),
            Some(Operation::Exists) => Some("exists"),
            _ => None,
        };
        if let Some(field) = aggregate_field {
            return Ok(documents
                .first()
                .and_then(|document| document.fields.get(field))
                .cloned()
                .unwrap_or(Value::Null));
        }

        let mut rows = documents
            .into_iter()
            .map(|document| {
                if document.fields.len() == 1 {
                    if let Some(value) = document.fields.get("value") {
                        return value.clone();
                    }
                }
                Value::Object(document.fields)
            })
            .collect::<Vec<_>>();
        if matches!(operations.last(), Some(Operation::First)) {
            Ok(rows.into_iter().next().unwrap_or(Value::Null))
        } else if source_was_object {
            Ok(rows.pop().unwrap_or(Value::Null))
        } else {
            Ok(Value::List(rows))
        }
    }

    fn resolve_reverse_collection_source(
        &self,
        scope_document: &Document,
        path: &[String],
    ) -> OQLResult<Option<Vec<Document>>> {
        let Some((reverse_name, forward_path)) = path.split_last() else {
            return Ok(None);
        };
        let mut target = scope_document.clone();
        for segment in forward_path {
            let Some(value) = target.get_field(segment).cloned() else {
                return Ok(Some(Vec::new()));
            };
            target = match value {
                Value::Reference(reference) => {
                    let Some(document) = self.resolve_objref_target(&reference)? else {
                        return Ok(Some(Vec::new()));
                    };
                    document
                }
                Value::GlobalReference(reference) => {
                    let Some(document) = self.store.navigate_global_reference(&reference)? else {
                        return Ok(Some(Vec::new()));
                    };
                    document
                }
                _ => return Ok(None),
            };
        }

        if target.get_field(reverse_name).is_some() {
            return Ok(None);
        }
        Ok(Some(self.store.scan_referencing_documents(
            &target.schema,
            &target.id,
            Some(reverse_name),
            None,
        )?))
    }

    fn evaluate_query_expression(
        &self,
        scope_document: &Document,
        query: &crate::parser::OQLQuery,
    ) -> OQLResult<Value> {
        let materialized_query = self.materialize_query_with_scope(query, scope_document)?;
        let docs = self.execute_query(&materialized_query)?;
        let aggregate_field = match materialized_query.operations.last() {
            Some(Operation::Count) => Some("count"),
            Some(Operation::Sum(_)) => Some("sum"),
            Some(Operation::Avg(_)) => Some("avg"),
            Some(Operation::Min(_)) => Some("min"),
            Some(Operation::Max(_)) => Some("max"),
            Some(Operation::Exists) => Some("exists"),
            _ => None,
        };
        if let Some(field) = aggregate_field {
            let value = docs
                .first()
                .and_then(|doc| doc.fields.get(field))
                .cloned()
                .unwrap_or(Value::Null);
            return Ok(value);
        }

        let mut rows = Vec::with_capacity(docs.len());
        for doc in docs {
            let mut object_fields = std::collections::HashMap::new();
            object_fields.insert("id".to_string(), Value::String(doc.id));
            object_fields.insert("schema".to_string(), Value::String(doc.schema));
            for (k, v) in doc.fields {
                object_fields.insert(k, v);
            }
            rows.push(Value::Object(object_fields));
        }
        if matches!(materialized_query.operations.last(), Some(Operation::First)) {
            Ok(rows.into_iter().next().unwrap_or(Value::Null))
        } else {
            Ok(Value::List(rows))
        }
    }

    fn materialize_query_with_scope(
        &self,
        query: &crate::parser::OQLQuery,
        scope_document: &Document,
    ) -> OQLResult<crate::parser::OQLQuery> {
        let operations = query
            .operations
            .iter()
            .map(|op| self.materialize_operation_with_scope(op, scope_document))
            .collect::<OQLResult<Vec<_>>>()?;
        Ok(crate::parser::OQLQuery {
            collection: query.collection.clone(),
            operations,
        })
    }

    fn materialize_operation_with_scope(
        &self,
        operation: &Operation,
        scope_document: &Document,
    ) -> OQLResult<Operation> {
        Ok(match operation {
            Operation::Where(block) => Operation::Where(crate::parser::WhereBlock {
                condition: self
                    .materialize_condition_with_scope(&block.condition, scope_document)?,
            }),
            Operation::Select(block) => Operation::Select(crate::parser::SelectBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|f| {
                        Ok(crate::parser::SelectField {
                            alias: f.alias.clone(),
                            expression: self
                                .materialize_expression_with_scope(&f.expression, scope_document)?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            }),
            Operation::Insert(block) => Operation::Insert(crate::parser::InsertBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|f| {
                        Ok(crate::parser::InsertField {
                            name: f.name.clone(),
                            value: self
                                .materialize_expression_with_scope(&f.value, scope_document)?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            }),
            Operation::InsertMany(blocks) => Operation::InsertMany(
                blocks
                    .iter()
                    .map(|block| {
                        Ok(crate::parser::InsertBlock {
                            fields: block
                                .fields
                                .iter()
                                .map(|f| {
                                    Ok(crate::parser::InsertField {
                                        name: f.name.clone(),
                                        value: self.materialize_expression_with_scope(
                                            &f.value,
                                            scope_document,
                                        )?,
                                    })
                                })
                                .collect::<OQLResult<Vec<_>>>()?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            ),
            Operation::Update(block) => Operation::Update(crate::parser::UpdateBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|f| {
                        Ok(crate::parser::UpdateField {
                            name: f.name.clone(),
                            value: self
                                .materialize_expression_with_scope(&f.value, scope_document)?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            }),
            Operation::Sort(block) => Operation::Sort(crate::parser::SortBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|f| {
                        Ok(crate::parser::SortField {
                            expression: self
                                .materialize_expression_with_scope(&f.expression, scope_document)?,
                            direction: f.direction.clone(),
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            }),
            Operation::Having(block) => Operation::Having(crate::parser::HavingBlock {
                condition: self
                    .materialize_condition_with_scope(&block.condition, scope_document)?,
            }),
            Operation::Nearest(block) => Operation::Nearest(crate::parser::NearestBlock {
                field: block.field.clone(),
                query: self.materialize_expression_with_scope(&block.query, scope_document)?,
                metric: block.metric.clone(),
                k: block.k,
                num_candidates: block.num_candidates,
                exact: block.exact,
            }),
            _ => operation.clone(),
        })
    }

    fn materialize_condition_with_scope(
        &self,
        condition: &Condition,
        scope_document: &Document,
    ) -> OQLResult<Condition> {
        Ok(match condition {
            Condition::Comparison {
                left,
                operator,
                right,
            } => Condition::Comparison {
                left: self.materialize_expression_with_scope(left, scope_document)?,
                operator: operator.clone(),
                right: self.materialize_expression_with_scope(right, scope_document)?,
            },
            Condition::And(left, right) => Condition::And(
                Box::new(self.materialize_condition_with_scope(left, scope_document)?),
                Box::new(self.materialize_condition_with_scope(right, scope_document)?),
            ),
            Condition::Or(left, right) => Condition::Or(
                Box::new(self.materialize_condition_with_scope(left, scope_document)?),
                Box::new(self.materialize_condition_with_scope(right, scope_document)?),
            ),
            Condition::Not(inner) => Condition::Not(Box::new(
                self.materialize_condition_with_scope(inner, scope_document)?,
            )),
        })
    }

    fn materialize_expression_with_scope(
        &self,
        expr: &Expression,
        scope_document: &Document,
    ) -> OQLResult<Expression> {
        if let Some(path) = Self::extract_scoped_path(expr, &scope_document.schema) {
            return self.scoped_path_to_literal_expression(scope_document, &path);
        }

        Ok(match expr {
            Expression::Identifier(_)
            | Expression::FieldAccess(_)
            | Expression::Literal(_)
            | Expression::QueryExpression { .. } => expr.clone(),
            Expression::CollectionExpression { source, operations } => {
                Expression::CollectionExpression {
                    source: Box::new(
                        self.materialize_expression_with_scope(source, scope_document)?,
                    ),
                    operations: operations
                        .iter()
                        .map(|operation| {
                            self.materialize_operation_with_scope(operation, scope_document)
                        })
                        .collect::<OQLResult<Vec<_>>>()?,
                }
            }
            Expression::FunctionCall { name, args } => Expression::FunctionCall {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|a| self.materialize_expression_with_scope(a, scope_document))
                    .collect::<OQLResult<Vec<_>>>()?,
            },
            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => Expression::IfThenElse {
                condition: Box::new(
                    self.materialize_condition_with_scope(condition, scope_document)?,
                ),
                then_expr: Box::new(
                    self.materialize_expression_with_scope(then_expr, scope_document)?,
                ),
                else_expr: Box::new(
                    self.materialize_expression_with_scope(else_expr, scope_document)?,
                ),
            },
            Expression::Match { expr, arms } => Expression::Match {
                expr: Box::new(self.materialize_expression_with_scope(expr, scope_document)?),
                arms: arms
                    .iter()
                    .map(|arm| {
                        Ok(crate::parser::MatchArm {
                            pattern: arm.pattern.clone(),
                            guard: arm
                                .guard
                                .as_ref()
                                .map(|g| self.materialize_condition_with_scope(g, scope_document))
                                .transpose()?,
                            body: self
                                .materialize_expression_with_scope(&arm.body, scope_document)?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            },
            Expression::LogicalAnd(left, right) => Expression::LogicalAnd(
                Box::new(self.materialize_expression_with_scope(left, scope_document)?),
                Box::new(self.materialize_expression_with_scope(right, scope_document)?),
            ),
            Expression::LogicalOr(left, right) => Expression::LogicalOr(
                Box::new(self.materialize_expression_with_scope(left, scope_document)?),
                Box::new(self.materialize_expression_with_scope(right, scope_document)?),
            ),
            Expression::LogicalNot(inner) => Expression::LogicalNot(Box::new(
                self.materialize_expression_with_scope(inner, scope_document)?,
            )),
            Expression::BinaryOp {
                left,
                operator,
                right,
            } => Expression::BinaryOp {
                left: Box::new(self.materialize_expression_with_scope(left, scope_document)?),
                operator: operator.clone(),
                right: Box::new(self.materialize_expression_with_scope(right, scope_document)?),
            },
            Expression::ArrayAccess { array, index } => Expression::ArrayAccess {
                array: Box::new(self.materialize_expression_with_scope(array, scope_document)?),
                index: Box::new(self.materialize_expression_with_scope(index, scope_document)?),
            },
            Expression::ChainedAccess { base, field } => Expression::ChainedAccess {
                base: Box::new(self.materialize_expression_with_scope(base, scope_document)?),
                field: field.clone(),
            },
            Expression::Block(expressions) => Expression::Block(
                expressions
                    .iter()
                    .map(|e| self.materialize_expression_with_scope(e, scope_document))
                    .collect::<OQLResult<Vec<_>>>()?,
            ),
            Expression::ArrayLiteral(elements) => Expression::ArrayLiteral(
                elements
                    .iter()
                    .map(|e| self.materialize_expression_with_scope(e, scope_document))
                    .collect::<OQLResult<Vec<_>>>()?,
            ),
            Expression::ObjectLiteral(fields) => Expression::ObjectLiteral(
                fields
                    .iter()
                    .map(|f| {
                        Ok(crate::parser::ObjectField {
                            name: f.name.clone(),
                            value: self
                                .materialize_expression_with_scope(&f.value, scope_document)?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            ),
            Expression::ConditionExpr(condition) => Expression::ConditionExpr(Box::new(
                self.materialize_condition_with_scope(condition, scope_document)?,
            )),
        })
    }

    fn extract_scoped_path(expr: &Expression, immediate_scope: &str) -> Option<Vec<String>> {
        match expr {
            Expression::Identifier(name) if name == immediate_scope => Some(Vec::new()),
            Expression::FieldAccess(field_access)
                if field_access.path.len() == 1 && field_access.path[0] == immediate_scope =>
            {
                Some(Vec::new())
            }
            Expression::ChainedAccess { base, field } => {
                let mut path = Self::extract_scoped_path(base, immediate_scope)?;
                path.push(field.clone());
                Some(path)
            }
            _ => None,
        }
    }

    fn scoped_path_to_literal_expression(
        &self,
        scope_document: &Document,
        path: &[String],
    ) -> OQLResult<Expression> {
        let value = if path.is_empty() {
            return Err(OQLError::FunctionError {
                message: "Scope anchor must be followed by a field path, e.g. companies.id"
                    .to_string(),
            });
        } else {
            self.get_field_value(
                scope_document,
                &crate::parser::FieldAccess {
                    path: path.to_vec(),
                },
            )?
        };

        match value {
            Value::String(s) => Ok(Expression::Literal(Literal::String(s))),
            Value::Integer(i) => Ok(Expression::Literal(Literal::Integer(i))),
            Value::Float(f) => Ok(Expression::Literal(Literal::Float(f))),
            Value::Boolean(b) => Ok(Expression::Literal(Literal::Boolean(b))),
            Value::DateTime(dt) => Ok(Expression::Literal(Literal::String(dt.to_rfc3339()))),
            Value::Reference(r) => Ok(Expression::Literal(Literal::String(r.to_string_id()))),
            Value::GlobalReference(r) => Ok(Expression::Literal(Literal::String(r.to_string_id()))),
            Value::Null => Err(OQLError::FunctionError {
                message: format!("Scope path {} resolved to Null", path.join(".")),
            }),
            other => Err(OQLError::FunctionError {
                message: format!(
                    "Scope path {} must resolve to scalar literal, got {}",
                    path.join("."),
                    other.type_name()
                ),
            }),
        }
    }

    /// The value of a whole row, as seen when a row parameter is used
    /// without a path.
    fn row_as_value(row: &Document) -> Value {
        let mut fields = row.fields.clone();
        fields
            .entry("id".to_string())
            .or_insert_with(|| Value::String(row.id.clone()));
        Value::Object(fields)
    }

    /// Recognise `param.a.b` where `param` is a row parameter of the
    /// function being evaluated.
    fn row_parameter_path(expression: &Expression) -> Option<(Document, Vec<String>)> {
        let mut path = Vec::new();
        let mut current = expression;
        while let Expression::ChainedAccess { base, field } = current {
            path.push(field.clone());
            current = base;
        }
        let Expression::Identifier(name) = current else {
            return None;
        };
        if path.is_empty() || !CallFrame::is_row(name) {
            return None;
        }
        path.reverse();
        CallFrame::row(name).map(|row| (row, path))
    }

    /// Evaluate a binary operation
    fn evaluate_binary_operation(
        &self,
        left: &Value,
        operator: &crate::parser::BinaryOperator,
        right: &Value,
    ) -> OQLResult<Value> {
        use crate::parser::BinaryOperator;

        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => {
                // Integer arithmetic is checked: a result outside the 64-bit
                // range is an error, never a silently wrapped value.
                let overflow = || OQLError::FunctionError {
                    message: format!("Integer overflow evaluating {a} {operator:?} {b}"),
                };
                match operator {
                    BinaryOperator::Add => a.checked_add(*b).map(Value::Integer).ok_or_else(overflow),
                    BinaryOperator::Subtract => {
                        a.checked_sub(*b).map(Value::Integer).ok_or_else(overflow)
                    }
                    BinaryOperator::Multiply => {
                        a.checked_mul(*b).map(Value::Integer).ok_or_else(overflow)
                    }
                    BinaryOperator::Divide if *b == 0 => Err(OQLError::FunctionError {
                        message: "Division by zero".to_string(),
                    }),
                    BinaryOperator::Divide => {
                        a.checked_div(*b).map(Value::Integer).ok_or_else(overflow)
                    }
                    BinaryOperator::Modulo if *b == 0 => Err(OQLError::FunctionError {
                        message: "Modulo by zero".to_string(),
                    }),
                    BinaryOperator::Modulo => {
                        a.checked_rem(*b).map(Value::Integer).ok_or_else(overflow)
                    }
                }
            }
            (Value::Float(a), Value::Float(b)) => match operator {
                BinaryOperator::Add => Ok(Value::Float(a + b)),
                BinaryOperator::Subtract => Ok(Value::Float(a - b)),
                BinaryOperator::Multiply => Ok(Value::Float(a * b)),
                BinaryOperator::Divide => {
                    if *b == 0.0 {
                        Err(OQLError::FunctionError {
                            message: "Division by zero".to_string(),
                        })
                    } else {
                        Ok(Value::Float(a / b))
                    }
                }
                BinaryOperator::Modulo => Ok(Value::Float(a % b)),
            },
            (Value::Integer(a), Value::Float(b)) => {
                let a_float = *a as f64;
                match operator {
                    BinaryOperator::Add => Ok(Value::Float(a_float + b)),
                    BinaryOperator::Subtract => Ok(Value::Float(a_float - b)),
                    BinaryOperator::Multiply => Ok(Value::Float(a_float * b)),
                    BinaryOperator::Divide => {
                        if *b == 0.0 {
                            Err(OQLError::FunctionError {
                                message: "Division by zero".to_string(),
                            })
                        } else {
                            Ok(Value::Float(a_float / b))
                        }
                    }
                    BinaryOperator::Modulo => Ok(Value::Float(a_float % b)),
                }
            }
            (Value::Float(a), Value::Integer(b)) => {
                let b_float = *b as f64;
                match operator {
                    BinaryOperator::Add => Ok(Value::Float(a + b_float)),
                    BinaryOperator::Subtract => Ok(Value::Float(a - b_float)),
                    BinaryOperator::Multiply => Ok(Value::Float(a * b_float)),
                    BinaryOperator::Divide => {
                        if *b == 0 {
                            Err(OQLError::FunctionError {
                                message: "Division by zero".to_string(),
                            })
                        } else {
                            Ok(Value::Float(a / b_float))
                        }
                    }
                    BinaryOperator::Modulo => Ok(Value::Float(a % b_float)),
                }
            }
            (Value::String(a), Value::String(b)) if matches!(operator, BinaryOperator::Add) => {
                Ok(Value::String(format!("{}{}", a, b)))
            }
            _ => Err(OQLError::FunctionError {
                message: format!(
                    "Invalid binary operation: {:?} {:?} {:?}",
                    left, operator, right
                ),
            }),
        }
    }

    /// Evaluate array access
    fn evaluate_array_access(&self, array_value: &Value, index_value: &Value) -> OQLResult<Value> {
        match (array_value, index_value) {
            (Value::List(list), Value::Integer(index)) => {
                let idx = *index as usize;
                if idx < list.len() {
                    Ok(list[idx].clone())
                } else {
                    Ok(Value::Null) // Return null for out-of-bounds access
                }
            }
            (Value::String(s), Value::Integer(index)) => {
                let idx = *index as usize;
                let chars: Vec<char> = s.chars().collect();
                if idx < chars.len() {
                    Ok(Value::String(chars[idx].to_string()))
                } else {
                    Ok(Value::Null) // Return null for out-of-bounds access
                }
            }
            _ => Err(OQLError::FunctionError {
                message: format!("Invalid array access: {:?}[{:?}]", array_value, index_value),
            }),
        }
    }

    /// Get a field from a value (for chained access)
    fn get_field_from_value(&self, value: &Value, field_name: &str) -> OQLResult<Value> {
        match value {
            Value::Object(obj) => Ok(obj.get(field_name).cloned().unwrap_or(Value::Null)),
            _ => Ok(Value::Null),
        }
    }

    /// Evaluate a match expression
    fn evaluate_match_expression(
        &self,
        document: &Document,
        expr_value: &Value,
        arms: &[crate::parser::MatchArm],
    ) -> OQLResult<Value> {
        for arm in arms {
            if self.pattern_matches(&arm.pattern, expr_value)? {
                // Check guard condition if present
                if let Some(guard) = &arm.guard {
                    if !self.evaluate_condition(document, guard)? {
                        continue;
                    }
                }

                return self.evaluate_expression(document, &arm.body);
            }
        }

        Err(OQLError::Generic {
            message: "No matching pattern found in match expression".to_string(),
        })
    }

    /// Check if a pattern matches a value
    fn pattern_matches(
        &self,
        pattern: &crate::parser::MatchPattern,
        value: &Value,
    ) -> OQLResult<bool> {
        match pattern {
            crate::parser::MatchPattern::Literal(literal) => {
                let pattern_value = self.literal_to_value(literal);
                Ok(self.values_equal(value, &pattern_value))
            }
            crate::parser::MatchPattern::Wildcard => Ok(true),
            crate::parser::MatchPattern::Variable(_) => Ok(true), // Variables always match
            crate::parser::MatchPattern::Guard(_condition) => Err(OQLError::Generic {
                message: "Guard pattern syntax is not supported in match patterns".to_string(),
            }),
        }
    }

    /// Convert a value to boolean for logical operations
    fn value_to_bool(&self, value: &Value) -> OQLResult<bool> {
        match value {
            Value::Boolean(b) => Ok(*b),
            Value::Integer(i) => Ok(*i != 0),
            Value::Float(f) => Ok(*f != 0.0),
            Value::String(s) => Ok(!s.is_empty()),
            Value::Null => Ok(false),
            Value::List(items) => Ok(!items.is_empty()),
            // Any other present value (object, timestamp, reference) is
            // truthy, so truthiness is defined for every value.
            _ => Ok(true),
        }
    }

    /// Get field value from document using field access path with implicit null handling
    /// This is the core implementation of OWL's migration-friendly field access
    pub fn get_field_value(
        &self,
        document: &Document,
        field_access: &FieldAccess,
    ) -> OQLResult<Value> {
        profile_call!("get_field_value", {
            if field_access.path.is_empty() {
                // Return null for empty path instead of error - graceful degradation
                return Ok(Value::Null);
            }

            // Handle single-level field access directly with optimized lookup
            if field_access.path.len() == 1 {
                let field_name = &field_access.path[0];

                // Handle special fields
                if let Some(system_field) = DocumentSystemField::from_name(field_name) {
                    return Ok(system_field.value(document));
                }
                // Use optimized field access with caching.
                return Ok(optimized_get_field(document, field_name).unwrap_or(Value::Null));
            }

            // Grouped rows flatten grouped key paths into underscore-separated fields
            // like `team_id` and `team_name`. Allow `.team.id` to resolve against
            // those synthetic fields during the subsequent select phase.
            if document.schema == "group" {
                let flattened_field_name = field_access.path.join("_");
                if let Some(value) = optimized_get_field(document, &flattened_field_name) {
                    return Ok(value);
                }
            }

            // Handle multi-level field access with implicit null handling
            self.resolve_nested_field_path(document, &field_access.path)
        })
    }

    fn resolve_path_state(&self, document: &Document, path: &[String]) -> OQLResult<PathState> {
        if path.is_empty() {
            return Ok(PathState::Missing);
        }

        let value = match DocumentSystemField::from_name(&path[0]) {
            Some(system_field) => Some(system_field.value(document)),
            None => document.get_field(&path[0]).cloned(),
        };
        let Some(value) = value else {
            return Ok(PathState::Missing);
        };
        self.resolve_value_path_state(value, &path[1..])
    }

    fn resolve_object_path_state(
        &self,
        object: &HashMap<String, Value>,
        path: &[String],
    ) -> OQLResult<PathState> {
        if path.is_empty() {
            return Ok(PathState::Present(Value::Object(object.clone())));
        }
        let Some(value) = object.get(&path[0]).cloned() else {
            return Ok(PathState::Missing);
        };
        self.resolve_value_path_state(value, &path[1..])
    }

    fn resolve_value_path_state(&self, value: Value, remaining: &[String]) -> OQLResult<PathState> {
        match value {
            Value::Null if remaining.is_empty() => Ok(PathState::Null),
            Value::Null => Ok(PathState::Missing),
            Value::Object(object) => self.resolve_object_path_state(&object, remaining),
            Value::Reference(reference) => {
                let Some(target) = self.resolve_objref_target(&reference)? else {
                    return Ok(PathState::BrokenReference);
                };
                if remaining.is_empty() {
                    Ok(PathState::Present(Value::Reference(reference)))
                } else {
                    self.resolve_path_state(&target, remaining)
                }
            }
            Value::GlobalReference(reference) => {
                if reference.is_null() {
                    return Ok(PathState::BrokenReference);
                }
                let Some(target) = self.store.navigate_global_reference(&reference)? else {
                    return Ok(PathState::BrokenReference);
                };
                if remaining.is_empty() {
                    Ok(PathState::Present(Value::GlobalReference(reference)))
                } else {
                    self.resolve_path_state(&target, remaining)
                }
            }
            other if remaining.is_empty() => Ok(PathState::Present(other)),
            _ => Ok(PathState::Missing),
        }
    }

    fn resolve_objref_target(&self, reference: &ObjRef) -> OQLResult<Option<Document>> {
        if let Some(document) = self.try_fast_navigation(reference)? {
            return Ok(Some(document));
        }

        let reference_id = reference.to_string_id();
        let Some((schema, document_id)) = reference_id.split_once('#') else {
            return Ok(None);
        };
        self.store.get_document(schema, &document_id.to_string())
    }

    /// Resolve nested field path with implicit null handling at every level
    /// This enables migration-free schema evolution by never throwing errors for missing fields
    fn resolve_nested_field_path(&self, document: &Document, path: &[String]) -> OQLResult<Value> {
        if path.is_empty() {
            return Ok(Value::Null);
        }

        let field_name = &path[0];
        let remaining_path = &path[1..];

        // Get the field value from the current document
        let field_value = match DocumentSystemField::from_name(field_name) {
            Some(system_field) => system_field.value(document),
            None => {
                // Regular field access - return null if field doesn't exist
                match document.get_field(field_name) {
                    Some(value) => value.clone(),
                    None => Value::Null, // Field doesn't exist - return null gracefully
                }
            }
        };

        // If this is the final field in the path, return its value
        if remaining_path.is_empty() {
            return Ok(field_value);
        }

        // We need to navigate deeper - handle different value types
        match field_value {
            Value::Reference(objref) => {
                // This is a reference - resolve it and continue navigation
                self.resolve_reference_and_continue(&objref, remaining_path)
            }
            Value::GlobalReference(objref) => {
                self.resolve_global_reference_and_continue(&objref, remaining_path)
            }
            Value::Object(nested_object) => {
                // This is a nested object - navigate into it
                self.resolve_object_field_path(&nested_object, remaining_path)
            }
            Value::Null => {
                // Field is null - any deeper navigation also returns null
                Ok(Value::Null)
            }
            _ => {
                // Trying to navigate into a non-reference, non-object value
                // In traditional systems this would be an error, but we return null gracefully
                Ok(Value::Null)
            }
        }
    }

    /// Resolve nested object field path with implicit null handling
    /// This enables deep navigation into nested objects like .profile.preferences.theme
    fn resolve_object_field_path(
        &self,
        object: &HashMap<String, Value>,
        path: &[String],
    ) -> OQLResult<Value> {
        if path.is_empty() {
            return Ok(Value::Null);
        }

        let field_name = &path[0];
        let remaining_path = &path[1..];

        // Get the field value from the nested object
        let field_value = match object.get(field_name) {
            Some(value) => value.clone(),
            None => Value::Null, // Field doesn't exist in nested object - return null gracefully
        };

        // If this is the final field in the path, return its value
        if remaining_path.is_empty() {
            return Ok(field_value);
        }

        // We need to navigate deeper - handle different value types
        match field_value {
            Value::Object(nested_object) => {
                // Navigate deeper into nested object
                self.resolve_object_field_path(&nested_object, remaining_path)
            }
            Value::Reference(objref) => {
                // This is a reference within a nested object - resolve it
                self.resolve_reference_and_continue(&objref, remaining_path)
            }
            Value::GlobalReference(objref) => {
                self.resolve_global_reference_and_continue(&objref, remaining_path)
            }
            Value::Null => {
                // Field is null - any deeper navigation also returns null
                Ok(Value::Null)
            }
            _ => {
                // Trying to navigate into a non-object, non-reference value
                // Return null gracefully instead of error
                Ok(Value::Null)
            }
        }
    }

    /// Resolve a reference and continue field path navigation
    ///
    /// This implements O(1) pointer-based navigation when using Zig storage.
    /// For other storage backends, falls back to document lookup.
    fn resolve_reference_and_continue(
        &self,
        objref: &ObjRef,
        remaining_path: &[String],
    ) -> OQLResult<Value> {
        // Try O(1) pointer-based navigation first (if using Zig storage)
        if let Some(referenced_doc) = self.try_fast_navigation(objref)? {
            // Fast path: Direct pointer navigation succeeded
            return self.resolve_nested_field_path(&referenced_doc, remaining_path);
        }

        // Fallback path: Convert ObjRef to string ID for compatibility
        let ref_id = objref.to_string_id();

        // Parse the reference to get schema and document ID
        let (schema, doc_id) = if ref_id.contains('#') {
            let parts: Vec<&str> = ref_id.splitn(2, '#').collect();
            (parts[0], parts[1])
        } else if ref_id.starts_with("ref_") {
            // This is a hashed reference - we can't resolve it without a mapping
            // Return null gracefully for now
            return Ok(Value::Null);
        } else {
            // If no schema prefix, we can't resolve - return null gracefully
            return Ok(Value::Null);
        };

        // Try to load the referenced document
        match self.store.get_document(schema, &doc_id.to_string()) {
            Ok(Some(referenced_doc)) => {
                // Successfully loaded - continue navigation
                self.resolve_nested_field_path(&referenced_doc, remaining_path)
            }
            Ok(None) | Err(_) => {
                // Referenced document doesn't exist or can't be loaded - return null gracefully
                Ok(Value::Null)
            }
        }
    }

    fn resolve_global_reference_and_continue(
        &self,
        objref: &GlobalObjRef,
        remaining_path: &[String],
    ) -> OQLResult<Value> {
        if objref.is_null() {
            return Ok(Value::Null);
        }

        match self.store.navigate_global_reference(objref) {
            Ok(Some(referenced_doc)) => {
                self.resolve_nested_field_path(&referenced_doc, remaining_path)
            }
            Ok(None) | Err(_) => Ok(Value::Null),
        }
    }

    /// Try fast O(1) pointer-based navigation using Zig storage
    ///
    /// This is the critical path for OQL's performance advantage:
    /// - Navigation is pointer arithmetic, not a database query
    /// - Executes in <50ns (2-3 memory loads)
    /// - No query planning overhead
    ///
    /// Returns Some(Document) if fast navigation succeeded, None otherwise.
    fn try_fast_navigation(&self, objref: &ObjRef) -> OQLResult<Option<Document>> {
        // Check if this is a null reference
        if objref.is_null() {
            return Ok(None);
        }

        // Try to use the storage engine's fast navigation if available
        // This will use direct pointer swizzling for Zig storage
        match self.store.navigate_reference(objref) {
            Ok(Some(doc)) => Ok(Some(doc)),
            Ok(None) => Ok(None),
            Err(_) => Ok(None), // Fallback to slow path on error
        }
    }

    /// Convert a literal to a Value
    fn literal_to_value(&self, literal: &Literal) -> Value {
        match literal {
            Literal::String(s) => Value::String(s.clone()),
            Literal::Integer(i) => Value::Integer(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::Boolean(b) => Value::Boolean(*b),
            Literal::Null => Value::Null,
        }
    }

    /// Compare two values using the given operator.
    ///
    /// Comparison semantics live in [`crate::value_semantics`] so that this
    /// executor and the physical read pipeline cannot disagree. A comparison
    /// never fails; an undecidable one does not match.
    fn compare_values(
        &self,
        left: &Value,
        operator: &ComparisonOperator,
        right: &Value,
    ) -> OQLResult<bool> {
        Ok(crate::value_semantics::matches(left, operator, right))
    }

    /// Check if two values are equal
    fn values_equal(&self, left: &Value, right: &Value) -> bool {
        crate::value_semantics::equal(left, right)
    }

    /// Test the natural order of two values. Unordered pairs (a null, or
    /// unrelated types) satisfy no ordering.
    fn compare_ordered_values<F>(
        &self,
        left: &Value,
        right: &Value,
        compare_fn: F,
    ) -> OQLResult<bool>
    where
        F: Fn(std::cmp::Ordering) -> bool,
    {
        Ok(crate::value_semantics::compare(left, right).is_some_and(compare_fn))
    }

    /// Apply SELECT projection to documents
    fn apply_select_projection(
        &self,
        documents: Vec<Document>,
        select_block: &SelectBlock,
    ) -> OQLResult<Vec<Document>> {
        // Check if any field is an aggregation function
        let has_aggregation = select_block
            .fields
            .iter()
            .any(|field| Self::expression_contains_aggregate(&field.expression));

        // Check if we're dealing with grouped data (documents with schema "group")
        let is_grouped_data = documents.iter().any(|doc| doc.schema == "group");

        if has_aggregation && !is_grouped_data {
            // Handle aggregation over all documents - return single result document
            let mut projected_fields = HashMap::new();

            for select_field in &select_block.fields {
                let field_name = match &select_field.alias {
                    Some(alias) => alias.clone(),
                    None => match &select_field.expression {
                        Expression::FunctionCall { name, .. } => name.clone(),
                        Expression::FieldAccess(field_access) => field_access
                            .path
                            .last()
                            .unwrap_or(&"unknown".to_string())
                            .clone(),
                        _ => "computed".to_string(),
                    },
                };

                let value = self.evaluate_collection_aggregate_expression(
                    &documents,
                    &select_field.expression,
                )?;

                projected_fields.insert(field_name, value);
            }

            let projected_doc = Document::with_id(
                "aggregation_result".to_string(),
                "aggregation".to_string(),
                projected_fields,
            );

            Ok(vec![projected_doc])
        } else {
            // Handle regular projection - return one document per input document
            let mut projected_documents = Vec::new();

            for document in documents {
                let mut projected_fields = HashMap::new();

                for select_field in &select_block.fields {
                    let field_name = match &select_field.alias {
                        Some(alias) => alias.clone(),
                        None => {
                            // Extract field name from expression
                            match &select_field.expression {
                                Expression::FieldAccess(field_access) => field_access
                                    .path
                                    .last()
                                    .unwrap_or(&"unknown".to_string())
                                    .clone(),
                                Expression::FunctionCall { name, .. } => name.clone(),
                                _ => "computed".to_string(),
                            }
                        }
                    };

                    let value = self.evaluate_expression(&document, &select_field.expression)?;

                    projected_fields.insert(field_name, value);
                }

                let projected_doc = document.with_projected_fields(projected_fields);

                projected_documents.push(projected_doc);
            }

            Ok(projected_documents)
        }
    }

    /// Apply INSERT operation
    fn apply_insert_operation(
        &self,
        collection: &str,
        insert_block: &InsertBlock,
    ) -> OQLResult<Vec<Document>> {
        // Convert insert fields to document fields
        let mut fields = HashMap::new();

        // Create a temporary empty document for expression evaluation context
        let temp_doc = Document::new(collection.to_string(), HashMap::new());

        // Get schema if it exists for type conversion
        let schema = self.store.get_schema(collection).ok().flatten();

        for insert_field in &insert_block.fields {
            let mut value = self.evaluate_expression(&temp_doc, &insert_field.value)?;

            // Convert value to match schema type if needed
            if let Some(ref schema) = schema {
                if let Some(field_type) = schema.get_field_type(&insert_field.name) {
                    value = self.convert_to_schema_type(value, field_type, collection)?;
                }
            }

            fields.insert(insert_field.name.clone(), value);
        }

        // Create new document
        let document = Document::new(collection.to_string(), fields);

        // Validate against schema if it exists
        if let Some(schema) = schema {
            schema.validate_document(&document.fields)?;
            self.validate_document_constraints(&schema, &document, None)?;
        }

        // Store the document
        self.store.put_document(&document)?;

        // Return the created document
        Ok(vec![document])
    }

    /// Apply INSERT MANY operation (batch insert)
    fn apply_insert_many_operation(
        &self,
        collection: &str,
        insert_blocks: &[InsertBlock],
    ) -> OQLResult<Vec<Document>> {
        let mut documents = Vec::with_capacity(insert_blocks.len());

        // Get schema once for all documents
        let schema = self.store.get_schema(collection).ok().flatten();

        // Create a temporary empty document for expression evaluation context
        let temp_doc = Document::new(collection.to_string(), HashMap::new());

        // Process each insert block
        for insert_block in insert_blocks {
            let mut fields = HashMap::new();

            for insert_field in &insert_block.fields {
                let mut value = self.evaluate_expression(&temp_doc, &insert_field.value)?;

                // Convert value to match schema type if needed
                if let Some(ref schema) = schema {
                    if let Some(field_type) = schema.get_field_type(&insert_field.name) {
                        value = self.convert_to_schema_type(value, field_type, collection)?;
                    }
                }

                fields.insert(insert_field.name.clone(), value);
            }

            // Create new document
            let document = Document::new(collection.to_string(), fields);

            // Validate against schema if it exists
            if let Some(ref schema) = schema {
                schema.validate_document(&document.fields)?;
            }

            documents.push(document);
        }

        if let Some(ref schema) = schema {
            self.validate_insert_batch_constraints(schema, &documents)?;
        }

        // RustStore commits this as one WAL frame and one sync. Compatibility
        // stores retain the DocumentStore default while they migrate.
        self.store.put_documents(&documents)?;

        // Return all created documents
        Ok(documents)
    }

    fn apply_upsert_operation(
        &self,
        documents: Vec<Document>,
        collection: &str,
        upsert: &UpsertBlock,
    ) -> OQLResult<Vec<Document>> {
        let insert = InsertBlock {
            fields: upsert.fields.clone(),
        };
        let schema = self.store.get_schema(collection).ok().flatten();
        let temp_doc = Document::new(collection.to_string(), HashMap::new());
        let mut values = HashMap::with_capacity(upsert.fields.len());
        for field in &upsert.fields {
            let mut value = self.evaluate_expression(&temp_doc, &field.value)?;
            if let Some(field_type) = schema
                .as_ref()
                .and_then(|schema| schema.get_field_type(&field.name))
            {
                value = self.convert_to_schema_type(value, field_type, collection)?;
            }
            values.insert(field.name.clone(), value);
        }
        let key_value = values.get(&upsert.key).ok_or_else(|| OQLError::Generic {
            message: format!("Upsert document is missing key '{}'", upsert.key),
        })?;
        let mut matches = documents.into_iter().filter(|document| {
            let existing = document
                .get_field(&upsert.key)
                .cloned()
                .or_else(|| (upsert.key == "id").then_some(Value::String(document.id.clone())));
            existing.is_some_and(|existing| self.values_equal(&existing, key_value))
        });
        let matched = matches.next();
        if matches.next().is_some() {
            return Err(OQLError::SchemaValidation {
                message: format!(
                    "Upsert key '{}={}' matched more than one '{}' document",
                    upsert.key,
                    key_value.to_string_repr(),
                    collection
                ),
            });
        }

        let Some(mut document) = matched else {
            return self.apply_insert_operation(collection, &insert);
        };
        for (name, value) in values {
            document.set_field(name, value);
        }
        if let Some(schema) = schema {
            schema.validate_document(&document.fields)?;
            self.validate_document_constraints(&schema, &document, Some(&document.id))?;
        }
        self.store.put_documents(std::slice::from_ref(&document))?;
        Ok(vec![document])
    }

    fn validate_insert_batch_constraints(
        &self,
        schema: &Schema,
        documents: &[Document],
    ) -> OQLResult<()> {
        for document in documents {
            self.validate_document_constraints(schema, document, None)?;
        }
        for constraint in &schema.constraints {
            let SchemaConstraint::Unique { fields, name } = constraint else {
                continue;
            };
            for left_index in 0..documents.len() {
                for right_index in left_index + 1..documents.len() {
                    let same = fields.iter().all(|field| {
                        self.extract_field_path_value(&documents[left_index].fields, field)
                            == self.extract_field_path_value(&documents[right_index].fields, field)
                    });
                    if same {
                        return Err(OQLError::SchemaValidation {
                            message: format!(
                                "Unique constraint '{}' violated within insert batch on schema '{}' fields {:?}",
                                name.clone().unwrap_or_else(|| "unnamed_unique".to_string()),
                                schema.name,
                                fields
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Convert a value to match the expected schema type
    fn convert_to_schema_type(
        &self,
        value: Value,
        field_type: &crate::schema::FieldType,
        _current_collection: &str,
    ) -> OQLResult<Value> {
        use crate::schema::FieldType;

        match (value, field_type) {
            // If we have a string but schema expects a reference, convert it
            (Value::String(ref_id), FieldType::Reference(ref_type)) => {
                let target_schema = &ref_type.target_schema;

                // Scan for the document with this id field value
                let documents = self.store.scan_documents(target_schema)?;

                // Find document where the "id" field matches
                for doc in documents {
                    if let Some(Value::String(doc_id)) = doc.fields.get("id") {
                        if doc_id == &ref_id {
                            // Found the document! Get its ObjRef
                            if let Some(objref) =
                                self.store.get_global_objref(target_schema, &doc.id)?
                            {
                                return Ok(Value::GlobalReference(objref));
                            }
                            if let Some(objref) = self.store.get_objref(target_schema, &doc.id)? {
                                return Ok(Value::Reference(objref));
                            }
                        }
                    }
                }

                // Document not found - return error or null based on whether reference is required
                if ref_type.required {
                    Err(OQLError::ReferenceError {
                        message: format!(
                            "Referenced document not found: {} in {}",
                            ref_id, target_schema
                        ),
                    })
                } else {
                    Ok(Value::Null)
                }
            }
            // Handle optional types
            (v, FieldType::Optional(inner_type)) => {
                if v.is_null() {
                    Ok(Value::Null)
                } else {
                    self.convert_to_schema_type(v, inner_type, _current_collection)
                }
            }
            // Pass through if types already match or no conversion needed
            (v, _) => Ok(v),
        }
    }

    /// Apply UPDATE operation to documents
    fn apply_update_operation(
        &self,
        documents: Vec<Document>,
        update_block: &UpdateBlock,
    ) -> OQLResult<Vec<Document>> {
        let updated_documents = self.materialize_updated_documents(documents, update_block)?;
        // Materialize and validate the whole update set before publishing any
        // document. RustStore persists the batch in one WAL frame.
        self.store.put_documents(&updated_documents)?;
        Ok(updated_documents)
    }

    pub fn materialize_updated_documents(
        &self,
        documents: Vec<Document>,
        update_block: &UpdateBlock,
    ) -> OQLResult<Vec<Document>> {
        let mut updated_documents = Vec::with_capacity(documents.len());
        for document in documents {
            updated_documents.push(self.materialize_updated_document(document, update_block)?);
        }
        Ok(updated_documents)
    }

    fn materialize_updated_document(
        &self,
        mut document: Document,
        update_block: &UpdateBlock,
    ) -> OQLResult<Document> {
        let schema = self.store.get_schema(&document.schema).ok().flatten();

        for update_field in &update_block.fields {
            let mut new_value = self.evaluate_expression(&document, &update_field.value)?;
            let path: Vec<&str> = update_field.name.split('.').collect();
            if path.len() == 1 {
                if let Some(ref schema) = schema {
                    if let Some(field_type) = schema.get_field_type(&update_field.name) {
                        new_value =
                            self.convert_to_schema_type(new_value, field_type, &document.schema)?;
                    }
                }
                document.set_field(update_field.name.clone(), new_value);
            } else {
                let root = path[0];
                let root_may_be_created = schema
                    .as_ref()
                    .and_then(|schema| schema.get_field_type(root))
                    .is_some_and(Self::field_type_is_object);
                Self::set_nested_update_value(
                    &mut document.fields,
                    &path,
                    new_value,
                    root_may_be_created,
                    &update_field.name,
                )?;
            }
        }

        if let Some(schema) = schema {
            schema.validate_document(&document.fields)?;
            self.validate_document_constraints(&schema, &document, Some(&document.id))?;
        }

        Ok(document)
    }

    fn field_type_is_object(field_type: &crate::schema::FieldType) -> bool {
        match field_type {
            crate::schema::FieldType::Object => true,
            crate::schema::FieldType::Optional(inner) => Self::field_type_is_object(inner),
            _ => false,
        }
    }

    fn set_nested_update_value(
        object: &mut HashMap<String, Value>,
        path: &[&str],
        value: Value,
        allow_missing_here: bool,
        full_path: &str,
    ) -> OQLResult<()> {
        let Some((head, tail)) = path.split_first() else {
            return Err(OQLError::Generic {
                message: "Update path cannot be empty".to_string(),
            });
        };
        if tail.is_empty() {
            object.insert((*head).to_string(), value);
            return Ok(());
        }

        if !object.contains_key(*head) {
            if !allow_missing_here {
                return Err(OQLError::SchemaValidation {
                    message: format!(
                        "Cannot update '{}': missing intermediate object '{}'",
                        full_path, head
                    ),
                });
            }
            object.insert((*head).to_string(), Value::Object(HashMap::new()));
        }

        match object.get_mut(*head) {
            Some(Value::Object(nested)) => {
                Self::set_nested_update_value(nested, tail, value, true, full_path)
            }
            Some(Value::Reference(_)) | Some(Value::GlobalReference(_)) => {
                Err(OQLError::SchemaValidation {
                    message: format!(
                        "Cannot update '{}': writes cannot traverse reference '{}'",
                        full_path, head
                    ),
                })
            }
            Some(other) => Err(OQLError::SchemaValidation {
                message: format!(
                    "Cannot update '{}': '{}' is {}, not an object",
                    full_path,
                    head,
                    other.type_name()
                ),
            }),
            None => unreachable!("missing nested update entry was handled above"),
        }
    }

    fn validate_document_constraints(
        &self,
        schema: &Schema,
        document: &Document,
        current_document_id: Option<&String>,
    ) -> OQLResult<()> {
        if schema.constraints.is_empty() {
            return Ok(());
        }

        for constraint in &schema.constraints {
            match constraint {
                SchemaConstraint::Unique { fields, name } => {
                    let docs = self.store.scan_documents(&schema.name)?;
                    for existing in docs {
                        if let Some(current_id) = current_document_id {
                            if &existing.id == current_id {
                                continue;
                            }
                        }
                        let mut same = true;
                        for field in fields {
                            let left = self.extract_field_path_value(&document.fields, field);
                            let right = self.extract_field_path_value(&existing.fields, field);
                            if left != right {
                                same = false;
                                break;
                            }
                        }
                        if same {
                            return Err(OQLError::SchemaValidation {
                                message: format!(
                                    "Unique constraint '{}' violated on schema '{}' fields {:?}",
                                    name.clone().unwrap_or_else(|| "unnamed_unique".to_string()),
                                    schema.name,
                                    fields
                                ),
                            });
                        }
                    }
                }
                SchemaConstraint::Check {
                    field,
                    op,
                    value,
                    name,
                } => {
                    let observed = self.extract_field_path_value(&document.fields, field);
                    let Some(observed) = observed else {
                        return Err(OQLError::SchemaValidation {
                            message: format!(
                                "Check constraint '{}' failed: field '{}' missing",
                                name.clone().unwrap_or_else(|| "unnamed_check".to_string()),
                                field
                            ),
                        });
                    };
                    if !self.check_constraint_holds(&observed, op, value) {
                        return Err(OQLError::SchemaValidation {
                            message: format!(
                                "Check constraint '{}' failed for field '{}'",
                                name.clone().unwrap_or_else(|| "unnamed_check".to_string()),
                                field
                            ),
                        });
                    }
                }
                SchemaConstraint::RefIntegrity {
                    field,
                    target_schema,
                    target_field,
                    required,
                    name,
                } => {
                    let observed = self.extract_field_path_value(&document.fields, field);
                    let Some(observed) = observed else {
                        if *required {
                            return Err(OQLError::ReferenceError {
                                message: format!(
                                    "Ref constraint '{}' failed: field '{}' missing",
                                    name.clone().unwrap_or_else(|| "unnamed_ref".to_string()),
                                    field
                                ),
                            });
                        }
                        continue;
                    };

                    if observed.is_null() {
                        if *required {
                            return Err(OQLError::ReferenceError {
                                message: format!(
                                    "Ref constraint '{}' failed: '{}' is null",
                                    name.clone().unwrap_or_else(|| "unnamed_ref".to_string()),
                                    field
                                ),
                            });
                        }
                        continue;
                    }

                    let target_docs = self.store.scan_documents(target_schema)?;
                    let target_key = target_field.clone().unwrap_or_else(|| "id".to_string());
                    let exists = target_docs.iter().any(|target| {
                        if target_key == "id" {
                            return self.reference_matches_document(&observed, target);
                        }
                        target
                            .fields
                            .get(&target_key)
                            .map(|v| *v == observed)
                            .unwrap_or(false)
                    });
                    if !exists {
                        return Err(OQLError::ReferenceError {
                            message: format!(
                                "Ref constraint '{}' failed: '{}' does not match target {}.{}",
                                name.clone().unwrap_or_else(|| "unnamed_ref".to_string()),
                                field,
                                target_schema,
                                target_key
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn extract_field_path_value(
        &self,
        fields: &HashMap<String, Value>,
        path: &str,
    ) -> Option<Value> {
        let mut parts = path.split('.');
        let first = parts.next()?;
        let mut current = fields.get(first)?.clone();
        for part in parts {
            match current {
                Value::Object(map) => {
                    current = map.get(part)?.clone();
                }
                _ => return None,
            }
        }
        Some(current)
    }

    fn check_constraint_holds(
        &self,
        observed: &Value,
        op: &CheckOperator,
        expected: &Value,
    ) -> bool {
        match op {
            CheckOperator::Eq => observed == expected,
            CheckOperator::Ne => observed != expected,
            CheckOperator::Gt => self
                .compare_numeric(observed, expected)
                .map(|o| o > 0.0)
                .unwrap_or(false),
            CheckOperator::Gte => self
                .compare_numeric(observed, expected)
                .map(|o| o >= 0.0)
                .unwrap_or(false),
            CheckOperator::Lt => self
                .compare_numeric(observed, expected)
                .map(|o| o < 0.0)
                .unwrap_or(false),
            CheckOperator::Lte => self
                .compare_numeric(observed, expected)
                .map(|o| o <= 0.0)
                .unwrap_or(false),
        }
    }

    fn compare_numeric(&self, left: &Value, right: &Value) -> Option<f64> {
        fn as_f64(v: &Value) -> Option<f64> {
            match v {
                Value::Integer(i) => Some(*i as f64),
                Value::Float(f) => Some(*f),
                _ => None,
            }
        }
        Some(as_f64(left)? - as_f64(right)?)
    }

    fn reference_matches_document(&self, observed: &Value, target: &Document) -> bool {
        match observed {
            Value::String(id) => &target.id == id || target.fields.get("id") == Some(observed),
            Value::Reference(objref) => self
                .store
                .get_objref(&target.schema, &target.id)
                .ok()
                .flatten()
                .map(|v| v == *objref)
                .unwrap_or(false),
            Value::GlobalReference(objref) => self
                .store
                .get_global_objref(&target.schema, &target.id)
                .ok()
                .flatten()
                .map(|v| v == *objref)
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Apply DELETE operation to documents
    fn apply_delete_operation(&self, documents: Vec<Document>) -> OQLResult<Vec<Document>> {
        let mut deleted_documents = Vec::new();

        for document in documents {
            // Delete the document from storage
            let was_deleted = self.store.delete_document(&document.schema, &document.id)?;

            if was_deleted {
                deleted_documents.push(document);
            }
        }

        Ok(deleted_documents)
    }

    /// Evaluate a function call expression
    fn evaluate_function_call(
        &self,
        document: &Document,
        name: &str,
        args: &[Expression],
    ) -> OQLResult<Value> {
        if document.schema == "group" {
            if let Some(field_path) = Self::aggregate_field_path(name, args)? {
                return self.execute_group_aggregation(document, name, &field_path);
            }
        }

        // Handle aggregation functions specially (they need document context)
        // But only if they have the right number of arguments for aggregation
        match name {
            "exists" | "is_null" | "is_missing" | "is_broken_ref" => {
                if args.len() != 1 {
                    return Err(OQLError::FunctionError {
                        message: format!("{} expects exactly one path argument", name),
                    });
                }
                let Expression::FieldAccess(path) = &args[0] else {
                    return Err(OQLError::FunctionError {
                        message: format!("{} expects a field path", name),
                    });
                };
                let state = self.resolve_path_state(document, &path.path)?;
                let result = match name {
                    "exists" => matches!(state, PathState::Present(_) | PathState::Null),
                    "is_null" => matches!(state, PathState::Null),
                    "is_missing" => matches!(state, PathState::Missing),
                    "is_broken_ref" => matches!(state, PathState::BrokenReference),
                    _ => unreachable!(),
                };
                Ok(Value::Boolean(result))
            }
            "count" => self.evaluate_count_function(document, args),
            "sum" if args.len() == 1 => self.evaluate_sum_function(document, args),
            "avg" if args.len() == 1 => self.evaluate_avg_function(document, args),
            "min" if args.len() == 1 => self.evaluate_min_function(document, args),
            "max" if args.len() == 1 => self.evaluate_max_function(document, args),
            _ => {
                // Check if it's a built-in function
                if let Some(func) = self.functions.get_function(name) {
                    // Evaluate arguments first
                    let mut arg_values = Vec::new();
                    for arg in args {
                        arg_values.push(self.evaluate_expression(document, arg)?);
                    }

                    // Call the built-in function
                    func(&arg_values)
                } else if let Some(user_func) = self.functions.get_user_function(name) {
                    // Handle user-defined function
                    self.evaluate_user_function(document, user_func, args)
                } else {
                    Err(OQLError::Generic {
                        message: format!("Unknown function: {}", name),
                    })
                }
            }
        }
    }

    fn aggregate_field_path(name: &str, args: &[Expression]) -> OQLResult<Option<Vec<String>>> {
        let is_aggregate = matches!(name, "count" | "sum" | "avg" | "min" | "max");
        if !is_aggregate {
            return Ok(None);
        }
        if name == "count" && args.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if args.len() != 1 {
            return Err(OQLError::FunctionError {
                message: format!(
                    "{} expects {}",
                    name,
                    if name == "count" {
                        "zero or one field argument"
                    } else {
                        "exactly one field argument"
                    }
                ),
            });
        }
        match &args[0] {
            Expression::FieldAccess(field) if field.path.len() == 1 && field.path[0] == "*" => {
                if name == "count" {
                    Ok(Some(Vec::new()))
                } else {
                    Err(OQLError::FunctionError {
                        message: format!("{} does not accept '*'", name),
                    })
                }
            }
            Expression::FieldAccess(field) => Ok(Some(field.path.clone())),
            _ => Err(OQLError::FunctionError {
                message: format!("{} expects a field path argument", name),
            }),
        }
    }

    fn expression_contains_aggregate(expression: &Expression) -> bool {
        match expression {
            Expression::FunctionCall { name, args } => {
                matches!(name.as_str(), "count" | "sum" | "avg" | "min" | "max")
                    || args.iter().any(Self::expression_contains_aggregate)
            }
            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => {
                Self::condition_contains_aggregate(condition)
                    || Self::expression_contains_aggregate(then_expr)
                    || Self::expression_contains_aggregate(else_expr)
            }
            Expression::Match { expr, arms } => {
                Self::expression_contains_aggregate(expr)
                    || arms.iter().any(|arm| {
                        arm.guard
                            .as_ref()
                            .is_some_and(Self::condition_contains_aggregate)
                            || Self::expression_contains_aggregate(&arm.body)
                    })
            }
            Expression::LogicalAnd(left, right)
            | Expression::LogicalOr(left, right)
            | Expression::BinaryOp { left, right, .. } => {
                Self::expression_contains_aggregate(left)
                    || Self::expression_contains_aggregate(right)
            }
            Expression::LogicalNot(inner) => Self::expression_contains_aggregate(inner),
            Expression::ArrayAccess { array, index } => {
                Self::expression_contains_aggregate(array)
                    || Self::expression_contains_aggregate(index)
            }
            Expression::ChainedAccess { base, .. } => Self::expression_contains_aggregate(base),
            Expression::Block(expressions) | Expression::ArrayLiteral(expressions) => {
                expressions.iter().any(Self::expression_contains_aggregate)
            }
            Expression::ObjectLiteral(fields) => fields
                .iter()
                .any(|field| Self::expression_contains_aggregate(&field.value)),
            Expression::ConditionExpr(condition) => Self::condition_contains_aggregate(condition),
            // Collection/query expressions have their own row scope. Their
            // terminal aggregates are correlated values, not aggregates over
            // the surrounding select input.
            Expression::QueryExpression { .. }
            | Expression::CollectionExpression { .. }
            | Expression::Identifier(_)
            | Expression::FieldAccess(_)
            | Expression::Literal(_) => false,
        }
    }

    fn condition_contains_aggregate(condition: &Condition) -> bool {
        match condition {
            Condition::Comparison { left, right, .. } => {
                Self::expression_contains_aggregate(left)
                    || Self::expression_contains_aggregate(right)
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                Self::condition_contains_aggregate(left)
                    || Self::condition_contains_aggregate(right)
            }
            Condition::Not(inner) => Self::condition_contains_aggregate(inner),
        }
    }

    fn evaluate_collection_aggregate_expression(
        &self,
        documents: &[Document],
        expression: &Expression,
    ) -> OQLResult<Value> {
        if let Expression::FunctionCall { name, args } = expression {
            if let Some(field_path) = Self::aggregate_field_path(name, args)? {
                return self.execute_aggregation(documents, name, &field_path);
            }
        }

        match expression {
            Expression::BinaryOp {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate_collection_aggregate_expression(documents, left)?;
                let right = self.evaluate_collection_aggregate_expression(documents, right)?;
                self.evaluate_binary_operation(&left, operator, &right)
            }
            Expression::LogicalAnd(left, right) => {
                let left = self.evaluate_collection_aggregate_expression(documents, left)?;
                if !self.value_to_bool(&left)? {
                    return Ok(Value::Boolean(false));
                }
                let right = self.evaluate_collection_aggregate_expression(documents, right)?;
                Ok(Value::Boolean(self.value_to_bool(&right)?))
            }
            Expression::LogicalOr(left, right) => {
                let left = self.evaluate_collection_aggregate_expression(documents, left)?;
                if self.value_to_bool(&left)? {
                    return Ok(Value::Boolean(true));
                }
                let right = self.evaluate_collection_aggregate_expression(documents, right)?;
                Ok(Value::Boolean(self.value_to_bool(&right)?))
            }
            Expression::LogicalNot(inner) => Ok(Value::Boolean(!self.value_to_bool(
                &self.evaluate_collection_aggregate_expression(documents, inner)?,
            )?)),
            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => {
                if self.evaluate_collection_aggregate_condition(documents, condition)? {
                    self.evaluate_collection_aggregate_expression(documents, then_expr)
                } else {
                    self.evaluate_collection_aggregate_expression(documents, else_expr)
                }
            }
            Expression::Match { expr, arms } => {
                let value = self.evaluate_collection_aggregate_expression(documents, expr)?;
                for arm in arms {
                    if self.pattern_matches(&arm.pattern, &value)?
                        && match &arm.guard {
                            Some(guard) => {
                                self.evaluate_collection_aggregate_condition(documents, guard)?
                            }
                            None => true,
                        }
                    {
                        return self.evaluate_collection_aggregate_expression(documents, &arm.body);
                    }
                }
                Err(OQLError::Generic {
                    message: "No matching pattern found in aggregate expression".to_string(),
                })
            }
            Expression::ArrayAccess { array, index } => {
                let array = self.evaluate_collection_aggregate_expression(documents, array)?;
                let index = self.evaluate_collection_aggregate_expression(documents, index)?;
                self.evaluate_array_access(&array, &index)
            }
            Expression::ChainedAccess { base, field } => {
                let base = self.evaluate_collection_aggregate_expression(documents, base)?;
                self.get_field_from_value(&base, field)
            }
            Expression::Block(expressions) => {
                let mut value = Value::Null;
                for expression in expressions {
                    value = self.evaluate_collection_aggregate_expression(documents, expression)?;
                }
                Ok(value)
            }
            Expression::ArrayLiteral(expressions) => expressions
                .iter()
                .map(|expression| {
                    self.evaluate_collection_aggregate_expression(documents, expression)
                })
                .collect::<OQLResult<Vec<_>>>()
                .map(Value::List),
            Expression::ObjectLiteral(fields) => fields
                .iter()
                .map(|field| {
                    Ok((
                        field.name.clone(),
                        self.evaluate_collection_aggregate_expression(documents, &field.value)?,
                    ))
                })
                .collect::<OQLResult<HashMap<_, _>>>()
                .map(Value::Object),
            Expression::ConditionExpr(condition) => Ok(Value::Boolean(
                self.evaluate_collection_aggregate_condition(documents, condition)?,
            )),
            Expression::FunctionCall { name, args }
                if args.iter().any(Self::expression_contains_aggregate) =>
            {
                let context = documents
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Document::new("$aggregate".to_string(), HashMap::new()));
                let materialized_args = args
                    .iter()
                    .map(|arg| {
                        self.evaluate_collection_aggregate_expression(documents, arg)
                            .and_then(Self::value_as_expression)
                    })
                    .collect::<OQLResult<Vec<_>>>()?;
                self.evaluate_function_call(&context, name, &materialized_args)
            }
            _ => match documents.first() {
                Some(document) => self.evaluate_expression(document, expression),
                None => match expression {
                    Expression::Literal(literal) => Ok(self.literal_to_value(literal)),
                    _ => Ok(Value::Null),
                },
            },
        }
    }

    fn evaluate_collection_aggregate_condition(
        &self,
        documents: &[Document],
        condition: &Condition,
    ) -> OQLResult<bool> {
        match condition {
            Condition::Comparison {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate_collection_aggregate_expression(documents, left)?;
                let right = self.evaluate_collection_aggregate_expression(documents, right)?;
                self.compare_values(&left, operator, &right)
            }
            Condition::And(left, right) => Ok(self
                .evaluate_collection_aggregate_condition(documents, left)?
                && self.evaluate_collection_aggregate_condition(documents, right)?),
            Condition::Or(left, right) => Ok(self
                .evaluate_collection_aggregate_condition(documents, left)?
                || self.evaluate_collection_aggregate_condition(documents, right)?),
            Condition::Not(inner) => {
                Ok(!self.evaluate_collection_aggregate_condition(documents, inner)?)
            }
        }
    }

    fn value_as_expression(value: Value) -> OQLResult<Expression> {
        match value {
            Value::String(value) => Ok(Expression::Literal(Literal::String(value))),
            Value::Integer(value) => Ok(Expression::Literal(Literal::Integer(value))),
            Value::Float(value) => Ok(Expression::Literal(Literal::Float(value))),
            Value::Boolean(value) => Ok(Expression::Literal(Literal::Boolean(value))),
            Value::Null => Ok(Expression::Literal(Literal::Null)),
            Value::List(values) => values
                .into_iter()
                .map(Self::value_as_expression)
                .collect::<OQLResult<Vec<_>>>()
                .map(Expression::ArrayLiteral),
            Value::Object(fields) => fields
                .into_iter()
                .map(|(name, value)| {
                    Ok(crate::parser::ObjectField {
                        name,
                        value: Self::value_as_expression(value)?,
                    })
                })
                .collect::<OQLResult<Vec<_>>>()
                .map(Expression::ObjectLiteral),
            other => Err(OQLError::TypeError {
                expected: "literal-compatible aggregate result".to_string(),
                actual: other.type_name().to_string(),
            }),
        }
    }

    /// Evaluate COUNT function
    fn evaluate_count_function(
        &self,
        _document: &Document,
        args: &[Expression],
    ) -> OQLResult<Value> {
        if args.is_empty() {
            // count() or count(*) - count all documents
            Ok(Value::Integer(1)) // For single document context, always 1
        } else if args.len() == 1 {
            // count(field) - count non-null values
            match &args[0] {
                Expression::FieldAccess(_) => {
                    let field_value = self.evaluate_expression(_document, &args[0])?;
                    if field_value.is_null() {
                        Ok(Value::Integer(0))
                    } else {
                        Ok(Value::Integer(1))
                    }
                }
                _ => Ok(Value::Integer(1)), // count of literal values is always 1
            }
        } else {
            Err(OQLError::Generic {
                message: "COUNT function expects 0 or 1 arguments".to_string(),
            })
        }
    }

    /// Evaluate SUM function
    fn evaluate_sum_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "SUM function expects exactly 1 argument".to_string(),
            });
        }

        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(i) => Ok(Value::Integer(i)),
            Value::Float(f) => Ok(Value::Float(f)),
            Value::Null => Ok(Value::Integer(0)),
            _ => Err(OQLError::TypeError {
                expected: "numeric type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }

    /// Evaluate AVG function
    fn evaluate_avg_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "AVG function expects exactly 1 argument".to_string(),
            });
        }

        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(i) => Ok(Value::Float(i as f64)),
            Value::Float(f) => Ok(Value::Float(f)),
            Value::Null => Ok(Value::Null),
            _ => Err(OQLError::TypeError {
                expected: "numeric type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }

    /// Evaluate MIN function
    fn evaluate_min_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "MIN function expects exactly 1 argument".to_string(),
            });
        }

        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(_) | Value::Float(_) | Value::String(_) | Value::DateTime(_) => {
                Ok(value)
            }
            Value::Null => Ok(Value::Null),
            _ => Err(OQLError::TypeError {
                expected: "comparable type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }

    /// Evaluate MAX function
    fn evaluate_max_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "MAX function expects exactly 1 argument".to_string(),
            });
        }

        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(_) | Value::Float(_) | Value::String(_) | Value::DateTime(_) => {
                Ok(value)
            }
            Value::Null => Ok(Value::Null),
            _ => Err(OQLError::TypeError {
                expected: "comparable type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }

    /// Execute aggregation over a collection of documents
    pub fn execute_aggregation(
        &self,
        documents: &[Document],
        function_name: &str,
        field_path: &[String],
    ) -> OQLResult<Value> {
        match function_name {
            "count" => {
                if field_path.is_empty() {
                    // count(*) - count all documents
                    Ok(Value::Integer(documents.len() as i64))
                } else {
                    // count(field) - count non-null values
                    let mut count = 0i64;
                    for doc in documents {
                        let field_access = FieldAccess {
                            path: field_path.to_vec(),
                        };
                        let value = self.get_field_value(doc, &field_access)?;
                        if !value.is_null() {
                            count += 1;
                        }
                    }
                    Ok(Value::Integer(count))
                }
            }
            "sum" => {
                let mut sum_int = 0i64;
                let mut sum_float = 0.0f64;
                let mut has_float = false;
                let mut count = 0;

                for doc in documents {
                    let field_access = FieldAccess {
                        path: field_path.to_vec(),
                    };
                    let value = self.get_field_value(doc, &field_access)?;

                    match value {
                        Value::Integer(i) => {
                            if has_float {
                                sum_float += i as f64;
                            } else {
                                sum_int = sum_int.checked_add(i).ok_or_else(|| {
                                    OQLError::FunctionError {
                                        message: "Integer overflow evaluating sum()".to_string(),
                                    }
                                })?;
                            }
                            count += 1;
                        }
                        Value::Float(f) => {
                            if !has_float {
                                sum_float = sum_int as f64 + f;
                                has_float = true;
                            } else {
                                sum_float += f;
                            }
                            count += 1;
                        }
                        Value::Null => {} // Skip null values
                        _ => {
                            return Err(OQLError::TypeError {
                                expected: "numeric type".to_string(),
                                actual: value.type_name().to_string(),
                            })
                        }
                    }
                }

                if count == 0 {
                    Ok(Value::Integer(0))
                } else if has_float {
                    Ok(Value::Float(sum_float))
                } else {
                    Ok(Value::Integer(sum_int))
                }
            }
            "avg" => {
                let mut sum_float = 0.0f64;
                let mut count = 0;

                for doc in documents {
                    let field_access = FieldAccess {
                        path: field_path.to_vec(),
                    };
                    let value = self.get_field_value(doc, &field_access)?;

                    match value {
                        Value::Integer(i) => {
                            sum_float += i as f64;
                            count += 1;
                        }
                        Value::Float(f) => {
                            sum_float += f;
                            count += 1;
                        }
                        Value::Null => {} // Skip null values
                        _ => {
                            return Err(OQLError::TypeError {
                                expected: "numeric type".to_string(),
                                actual: value.type_name().to_string(),
                            })
                        }
                    }
                }

                if count == 0 {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Float(sum_float / count as f64))
                }
            }
            "min" => {
                let mut min_value: Option<Value> = None;

                for doc in documents {
                    let field_access = FieldAccess {
                        path: field_path.to_vec(),
                    };
                    let value = self.get_field_value(doc, &field_access)?;

                    if value.is_null() {
                        continue;
                    }

                    match &min_value {
                        None => min_value = Some(value),
                        Some(current_min) => {
                            if self
                                .compare_ordered_values(&value, current_min, |ord| ord.is_lt())?
                            {
                                min_value = Some(value);
                            }
                        }
                    }
                }

                Ok(min_value.unwrap_or(Value::Null))
            }
            "max" => {
                let mut max_value: Option<Value> = None;

                for doc in documents {
                    let field_access = FieldAccess {
                        path: field_path.to_vec(),
                    };
                    let value = self.get_field_value(doc, &field_access)?;

                    if value.is_null() {
                        continue;
                    }

                    match &max_value {
                        None => max_value = Some(value),
                        Some(current_max) => {
                            if self
                                .compare_ordered_values(&value, current_max, |ord| ord.is_gt())?
                            {
                                max_value = Some(value);
                            }
                        }
                    }
                }

                Ok(max_value.unwrap_or(Value::Null))
            }
            _ => Err(OQLError::Generic {
                message: format!("Unknown aggregation function: {}", function_name),
            }),
        }
    }

    /// Apply GROUP BY operation to documents
    fn apply_group_by_operation(
        &self,
        documents: Vec<Document>,
        group_by_block: &GroupByBlock,
    ) -> OQLResult<Vec<Document>> {
        use std::collections::BTreeMap;

        // Group documents by the specified fields
        let mut groups: BTreeMap<Vec<u8>, (Vec<Value>, Vec<Document>)> = BTreeMap::new();

        for document in documents {
            // Create group key by concatenating field values
            let mut group_values = Vec::new();

            for field_access in &group_by_block.fields {
                let value = self.get_field_value(&document, field_access)?;
                group_values.push(value);
            }

            // A typed composite encoding cannot collide when strings contain a
            // delimiter and keeps Integer(1) distinct from String("1").
            // Keys group by OQL equality, under which `1` and `1.0` are the
            // same value, so whole floats are keyed as integers.
            let canonical_key: Vec<Value> = group_values
                .iter()
                .map(|value| match value {
                    Value::Float(float)
                        if float.fract() == 0.0 && float.abs() < 9_007_199_254_740_992.0 =>
                    {
                        Value::Integer(*float as i64)
                    }
                    other => other.clone(),
                })
                .collect();
            let group_key =
                bincode::serialize(&canonical_key).map_err(|error| OQLError::Generic {
                    message: format!("Failed to encode group key: {}", error),
                })?;
            groups
                .entry(group_key)
                .or_insert_with(|| (group_values, Vec::new()))
                .1
                .push(document);
        }

        // Create a document for each group
        let mut grouped_documents = Vec::new();

        for (_group_key, (group_values, group_documents)) in groups {
            let mut fields = HashMap::new();

            // Add group key fields.
            // For nested paths like `.team.id`, expose the grouped key as `team_id`
            // so shorthand projections like `select { team_id }` work naturally.
            for (i, field_access) in group_by_block.fields.iter().enumerate() {
                let field_value = group_values.get(i).cloned().unwrap_or(Value::Null);
                let joined_field_name = if field_access.path.is_empty() {
                    "group_key".to_string()
                } else {
                    field_access.path.join("_")
                };
                fields.insert(joined_field_name, field_value.clone());

                if let Some(last) = field_access.path.last() {
                    fields.entry(last.clone()).or_insert(field_value);
                }
            }

            // Add group count
            fields.insert(
                "group_count".to_string(),
                Value::Integer(group_documents.len() as i64),
            );
            let group_schema = group_documents
                .first()
                .map(|d| d.schema.clone())
                .unwrap_or_default();
            fields.insert("_group_schema".to_string(), Value::String(group_schema));

            // Carry the exact source rows with the ephemeral grouped row. This
            // avoids rescanning the source collection for every aggregate and
            // preserves read-your-writes inside transactions.
            fields.insert(
                "_group_documents".to_string(),
                Value::List(
                    group_documents
                        .iter()
                        .map(Self::encode_group_source_document)
                        .collect(),
                ),
            );

            // Create group document
            let group_doc = Document::with_id(
                format!("group_{}", grouped_documents.len()),
                "group".to_string(),
                fields,
            );

            grouped_documents.push(group_doc);
        }

        Ok(grouped_documents)
    }

    /// Apply HAVING operation to grouped documents
    fn apply_having_operation(
        &self,
        documents: Vec<Document>,
        having_block: &HavingBlock,
    ) -> OQLResult<Vec<Document>> {
        // HAVING filters grouped documents based on aggregate conditions
        // For now, we'll implement a simple version that filters based on the condition
        self.apply_where_filter(documents, &having_block.condition)
    }

    /// Apply COUNT operation to documents
    fn apply_count_operation(&self, documents: Vec<Document>) -> OQLResult<Vec<Document>> {
        let count = documents.len() as i64;
        let mut fields = HashMap::new();
        fields.insert("count".to_string(), Value::Integer(count));

        let result_doc =
            Document::with_id("count_result".to_string(), "aggregate".to_string(), fields);

        Ok(vec![result_doc])
    }

    /// A terminal aggregate such as `orders.sum(.amount)`.
    ///
    /// It is the same computation as the aggregate function inside a
    /// `select`, so both forms share [`Self::execute_aggregation`] and agree
    /// on result types, null handling, and empty input.
    fn apply_terminal_aggregate(
        &self,
        documents: Vec<Document>,
        function: &str,
        field_access: &FieldAccess,
    ) -> OQLResult<Vec<Document>> {
        let value = self.execute_aggregation(&documents, function, &field_access.path)?;
        let mut fields = HashMap::new();
        fields.insert(function.to_string(), value);
        Ok(vec![Document::with_id(
            format!("{function}_result"),
            "aggregate".to_string(),
            fields,
        )])
    }

    fn apply_sum_operation(
        &self,
        documents: Vec<Document>,
        field_access: &FieldAccess,
    ) -> OQLResult<Vec<Document>> {
        self.apply_terminal_aggregate(documents, "sum", field_access)
    }

    fn apply_avg_operation(
        &self,
        documents: Vec<Document>,
        field_access: &FieldAccess,
    ) -> OQLResult<Vec<Document>> {
        self.apply_terminal_aggregate(documents, "avg", field_access)
    }

    fn apply_min_operation(
        &self,
        documents: Vec<Document>,
        field_access: &FieldAccess,
    ) -> OQLResult<Vec<Document>> {
        self.apply_terminal_aggregate(documents, "min", field_access)
    }

    fn apply_max_operation(
        &self,
        documents: Vec<Document>,
        field_access: &FieldAccess,
    ) -> OQLResult<Vec<Document>> {
        self.apply_terminal_aggregate(documents, "max", field_access)
    }

    /// Execute aggregation over grouped documents
    pub fn execute_group_aggregation(
        &self,
        group_doc: &Document,
        function_name: &str,
        field_path: &[String],
    ) -> OQLResult<Value> {
        let documents = match group_doc.get_field("_group_documents") {
            Some(Value::List(documents)) => documents
                .iter()
                .map(Self::decode_group_source_document)
                .collect::<OQLResult<Vec<_>>>()?,
            _ => {
                return Err(OQLError::Generic {
                    message: "Group document missing source rows".to_string(),
                })
            }
        };
        self.execute_aggregation(&documents, function_name, field_path)
    }

    fn encode_group_source_document(document: &Document) -> Value {
        Value::Object(HashMap::from([
            ("id".to_string(), Value::String(document.id.clone())),
            ("schema".to_string(), Value::String(document.schema.clone())),
            ("fields".to_string(), Value::Object(document.fields.clone())),
            (
                "created_at".to_string(),
                Value::DateTime(document.created_at),
            ),
            (
                "updated_at".to_string(),
                Value::DateTime(document.updated_at),
            ),
            (
                "version".to_string(),
                Value::Integer(document.version as i64),
            ),
        ]))
    }

    fn decode_group_source_document(value: &Value) -> OQLResult<Document> {
        let Value::Object(envelope) = value else {
            return Err(OQLError::Generic {
                message: "Grouped source row is not an object".to_string(),
            });
        };
        let get_string = |name: &str| match envelope.get(name) {
            Some(Value::String(value)) => Ok(value.clone()),
            _ => Err(OQLError::Generic {
                message: format!("Grouped source row is missing '{}'", name),
            }),
        };
        let fields = match envelope.get("fields") {
            Some(Value::Object(fields)) => fields.clone(),
            _ => {
                return Err(OQLError::Generic {
                    message: "Grouped source row is missing 'fields'".to_string(),
                })
            }
        };
        let created_at = match envelope.get("created_at") {
            Some(Value::DateTime(value)) => *value,
            _ => {
                return Err(OQLError::Generic {
                    message: "Grouped source row is missing 'created_at'".to_string(),
                })
            }
        };
        let updated_at = match envelope.get("updated_at") {
            Some(Value::DateTime(value)) => *value,
            _ => {
                return Err(OQLError::Generic {
                    message: "Grouped source row is missing 'updated_at'".to_string(),
                })
            }
        };
        let version = match envelope.get("version") {
            Some(Value::Integer(value)) => {
                u64::try_from(*value).map_err(|_| OQLError::Generic {
                    message: "Grouped source row has an invalid version".to_string(),
                })?
            }
            _ => {
                return Err(OQLError::Generic {
                    message: "Grouped source row is missing 'version'".to_string(),
                })
            }
        };
        Ok(Document {
            id: get_string("id")?,
            schema: get_string("schema")?,
            fields,
            created_at,
            updated_at,
            version,
        })
    }

    /// Apply SORT operation to documents
    fn apply_sort_operation(
        &self,
        mut documents: Vec<Document>,
        sort_block: &SortBlock,
    ) -> OQLResult<Vec<Document>> {
        // Sort documents by the specified fields
        documents.sort_by(|a, b| {
            for sort_field in &sort_block.fields {
                // Evaluate the expression for both documents
                let a_value = self
                    .evaluate_expression(a, &sort_field.expression)
                    .unwrap_or(Value::Null);
                let b_value = self
                    .evaluate_expression(b, &sort_field.expression)
                    .unwrap_or(Value::Null);

                let comparison = self.compare_values_for_sort(&a_value, &b_value);

                let result = match sort_field.direction {
                    crate::parser::SortDirection::Asc => comparison,
                    crate::parser::SortDirection::Desc => comparison.reverse(),
                };

                if result != std::cmp::Ordering::Equal {
                    return result;
                }
            }
            std::cmp::Ordering::Equal
        });

        Ok(documents)
    }

    /// Apply LIMIT operation to documents
    fn apply_limit_operation(
        &self,
        documents: Vec<Document>,
        limit: u64,
    ) -> OQLResult<Vec<Document>> {
        let limit = limit as usize;
        if documents.len() <= limit {
            Ok(documents)
        } else {
            Ok(documents.into_iter().take(limit).collect())
        }
    }

    /// Compare two values for sorting purposes
    fn compare_values_for_sort(&self, left: &Value, right: &Value) -> std::cmp::Ordering {
        crate::value_semantics::sort_order(left, right)
    }

    /// Evaluate a user-defined function call.
    ///
    /// Arguments are evaluated in the caller's scope, then bound to the
    /// parameter names for the duration of the body. The body runs through
    /// the ordinary evaluator, so parameters are visible everywhere an
    /// expression can appear: nested calls, `if` conditions, match guards,
    /// boolean bodies, and nested queries.
    fn evaluate_user_function(
        &self,
        document: &Document,
        user_func: &crate::functions::UserDefinedFunction,
        args: &[Expression],
    ) -> OQLResult<Value> {
        let definition = &user_func.definition;
        if args.len() != definition.parameters.len() {
            return Err(OQLError::Generic {
                message: format!(
                    "Function '{}' expects {} arguments, got {}",
                    definition.name,
                    definition.parameters.len(),
                    args.len()
                ),
            });
        }

        let mut frame = CallFrame::default();
        for (parameter, arg) in definition.parameters.iter().zip(args) {
            if let Some(row) = self.row_argument(document, arg) {
                frame.rows.insert(parameter.name.clone(), row);
                continue;
            }
            let value = self.evaluate_expression(document, arg)?;
            let value = Self::coerce_argument(value, &parameter.param_type).map_err(|actual| {
                OQLError::TypeError {
                    expected: format!(
                        "{} for parameter '{}' of function '{}'",
                        parameter.param_type, parameter.name, definition.name
                    ),
                    actual,
                }
            })?;
            frame.values.insert(parameter.name.clone(), value);
        }

        let _guard = CallFrameGuard::enter(frame, &definition.name)?;
        self.evaluate_expression(document, &definition.body)
    }

    /// A whole-row argument: `*` for the current row, or the name of a row
    /// parameter being passed along to another function.
    fn row_argument(&self, document: &Document, arg: &Expression) -> Option<Document> {
        match arg {
            Expression::FieldAccess(FieldAccess { path }) if path.len() == 1 && path[0] == "*" => {
                Some(document.clone())
            }
            Expression::Identifier(name) => CallFrame::row(name),
            _ => None,
        }
    }

    /// Check an argument against its declared parameter type, widening an
    /// integer passed for a `Float`. Returns the offending type name on
    /// mismatch.
    fn coerce_argument(value: Value, type_name: &str) -> Result<Value, String> {
        let declared = type_name.to_ascii_lowercase();
        let accepted = match (&value, declared.as_str()) {
            (Value::Null, _) => true,
            (Value::String(_), "string") => true,
            (Value::Integer(_), "integer" | "int") => true,
            (Value::Float(_), "float" | "double") => true,
            (Value::Integer(integer), "float" | "double") => {
                return Ok(Value::Float(*integer as f64));
            }
            (Value::Boolean(_), "boolean" | "bool") => true,
            (Value::DateTime(_), "datetime" | "timestamp") => true,
            // A timestamp parameter accepts the string form that schemas
            // without a DateTime column store.
            (Value::String(_), "datetime" | "timestamp") => true,
            (
                _,
                "string" | "integer" | "int" | "float" | "double" | "boolean" | "bool"
                | "datetime" | "timestamp",
            ) => false,
            // Any other declared type names a schema or structural type
            // (`List`, `Object`, a collection). Those are not checked here.
            _ => true,
        };
        if accepted {
            Ok(value)
        } else {
            Err(value.type_name().to_string())
        }
    }
}

/// Parameter bindings of the user function currently being evaluated.
#[derive(Default)]
struct CallFrame {
    /// Parameters bound to ordinary values.
    values: HashMap<String, Value>,
    /// Parameters bound to a whole row, so `p.team.name` keeps full path
    /// semantics, including reference traversal.
    rows: HashMap<String, Document>,
}

thread_local! {
    /// One frame per active user-function call on this thread. Only the top
    /// frame is visible, which gives each call its own lexical scope.
    static CALL_FRAMES: std::cell::RefCell<Vec<CallFrame>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Calls nested deeper than this are reported as runaway recursion rather
/// than overflowing the stack.
const MAX_FUNCTION_CALL_DEPTH: usize = 48;

impl CallFrame {
    fn value(name: &str) -> Option<Value> {
        CALL_FRAMES.with(|frames| {
            frames
                .borrow()
                .last()
                .and_then(|frame| frame.values.get(name).cloned())
        })
    }

    fn row(name: &str) -> Option<Document> {
        CALL_FRAMES.with(|frames| {
            frames
                .borrow()
                .last()
                .and_then(|frame| frame.rows.get(name).cloned())
        })
    }

    fn is_row(name: &str) -> bool {
        CALL_FRAMES.with(|frames| {
            frames
                .borrow()
                .last()
                .is_some_and(|frame| frame.rows.contains_key(name))
        })
    }
}

/// Pops the call frame when the function body finishes, including on error.
struct CallFrameGuard;

impl CallFrameGuard {
    fn enter(frame: CallFrame, function: &str) -> OQLResult<Self> {
        CALL_FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            if frames.len() >= MAX_FUNCTION_CALL_DEPTH {
                return Err(OQLError::FunctionError {
                    message: format!(
                        "function '{function}' exceeded the maximum call depth of \
                         {MAX_FUNCTION_CALL_DEPTH}; check for unbounded recursion"
                    ),
                });
            }
            frames.push(frame);
            Ok(Self)
        })
    }
}

impl Drop for CallFrameGuard {
    fn drop(&mut self) {
        CALL_FRAMES.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::parser::parse_oql;
    use crate::storage::RustStore;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_executor() -> (QueryExecutor<RustStore>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.owl");
        let store = Arc::new(RustStore::new(path.to_str().unwrap(), 10 * 1024 * 1024).unwrap());
        let executor = QueryExecutor::new_with_real_clock(store);
        (executor, temp_dir)
    }

    fn create_test_documents() -> Vec<Document> {
        vec![
            Document::with_id("user_1".to_string(), "users".to_string(), {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), Value::String("Alice".to_string()));
                fields.insert("age".to_string(), Value::Integer(30));
                fields.insert("active".to_string(), Value::Boolean(true));
                fields
            }),
            Document::with_id("user_2".to_string(), "users".to_string(), {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), Value::String("Bob".to_string()));
                fields.insert("age".to_string(), Value::Integer(25));
                fields.insert("active".to_string(), Value::Boolean(false));
                fields
            }),
            Document::with_id("user_3".to_string(), "users".to_string(), {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), Value::String("Charlie".to_string()));
                fields.insert("age".to_string(), Value::Integer(35));
                fields.insert("active".to_string(), Value::Boolean(true));
                fields
            }),
        ]
    }

    #[test]
    fn test_scan_collection() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Scan collection
        let scanned = executor.scan_collection("users").unwrap();
        assert_eq!(scanned.len(), 3);

        // Verify all documents are present
        let ids: Vec<&String> = scanned.iter().map(|d| &d.id).collect();
        assert!(ids.contains(&&"user_1".to_string()));
        assert!(ids.contains(&&"user_2".to_string()));
        assert!(ids.contains(&&"user_3".to_string()));
    }

    #[test]
    fn test_where_filter_equality() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test query: users.where { .name == "Alice" }
        let query = parse_oql(r#"users.where { .name == "Alice" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "user_1");
        assert_eq!(
            results[0].get_field("name"),
            Some(&Value::String("Alice".to_string()))
        );
    }

    #[test]
    fn indexed_read_pipeline_executes_where_sort_select_and_limit() {
        use crate::schema::FieldType;
        use crate::storage::ScalarIndexDefinition;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([
                    ("level".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                    ("ts".to_string(), FieldType::Integer),
                ]),
            ))
            .unwrap();
        executor
            .store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "events_by_level".to_string(),
                schema: "events".to_string(),
                fields: vec!["level".to_string()],
                unique: false,
            })
            .unwrap();

        for (id, level, message, ts) in [
            ("e1", "error", "older", 10),
            ("e2", "info", "ignored", 30),
            ("e3", "error", "newer", 20),
        ] {
            executor
                .store
                .put_document(&Document::with_id(
                    id.to_string(),
                    "events".to_string(),
                    HashMap::from([
                        ("level".to_string(), Value::String(level.to_string())),
                        ("message".to_string(), Value::String(message.to_string())),
                        ("ts".to_string(), Value::Integer(ts)),
                    ]),
                ))
                .unwrap();
        }

        let query = parse_oql(
            r#"events.where { .level == "error" }.sort { .ts.desc }.select { level, message, ts }.limit 1"#,
        )
        .unwrap();
        let explain = executor
            .explain_indexed_read(&query)
            .unwrap()
            .expect("native indexed plan");
        assert!(explain.contains("index=events_by_level"), "{explain}");
        assert!(explain.contains("access=NativeTyped"), "{explain}");
        assert!(explain.contains("pushed_limit=None"), "{explain}");
        assert!(
            explain.contains("pushed_projection=[\"level\", \"message\", \"ts\"]"),
            "{explain}"
        );

        let results = executor.execute_query(&query).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "e3");
        assert_eq!(
            results[0].get_field("message"),
            Some(&Value::String("newer".to_string()))
        );
        assert_eq!(results[0].fields.len(), 3);

        let fallback_query = parse_oql(
            r#"events.where { .level == "error" or .message == "ignored" }.sort { .ts }"#,
        )
        .unwrap();
        assert_eq!(
            executor.explain_indexed_read(&fallback_query).unwrap(),
            None
        );
        let fallback_results = executor.execute_query(&fallback_query).unwrap();
        assert_eq!(fallback_results.len(), 3);

        let pushed_query =
            parse_oql(r#"events.where { .level == "error" }.select { message }.limit 1"#).unwrap();
        let pushed_explain = executor
            .explain_indexed_read(&pushed_query)
            .unwrap()
            .expect("pushdown plan");
        assert!(
            pushed_explain.contains("pushed_limit=Some(1)"),
            "{pushed_explain}"
        );
        assert!(
            pushed_explain.contains("pushed_projection=[\"message\"]"),
            "{pushed_explain}"
        );
        let pushed_results = executor.execute_query(&pushed_query).unwrap();
        assert_eq!(pushed_results.len(), 1);
        assert_eq!(pushed_results[0].fields.len(), 1);
        assert!(pushed_results[0].fields.contains_key("message"));

        let profiled = executor.execute_query_with_plan(&pushed_query).unwrap();
        let profile = profiled
            .operator_profile
            .expect("indexed execution profile");
        assert_eq!(profile.operator, "IndexScan");
        assert_eq!(profile.actual_rows, 1);
        fn find_operator<'a>(
            profile: &'a crate::execution::OperatorExecutionStats,
            name: &str,
        ) -> Option<&'a crate::execution::OperatorExecutionStats> {
            (profile.operator == name).then_some(profile).or_else(|| {
                profile
                    .children
                    .iter()
                    .find_map(|child| find_operator(child, name))
            })
        }
        let index = find_operator(&profile, "IndexScan").expect("index profile");
        assert_eq!(index.actual_rows, 1);
        assert_eq!(index.metrics["records_decoded"], 1);
        assert!(index.metrics["index_entries_examined"] >= 1);
    }

    #[test]
    fn sequential_read_pipeline_projects_before_materializing_rows() {
        use crate::schema::FieldType;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "logs".to_string(),
                HashMap::from([
                    ("level".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                    ("payload".to_string(), FieldType::String),
                    ("ts".to_string(), FieldType::Integer),
                ]),
            ))
            .unwrap();

        for (id, level, message, ts) in [
            ("l1", "error", "older", 10),
            ("l2", "info", "ignored", 30),
            ("l3", "error", "newer", 20),
        ] {
            executor
                .store
                .put_document(&Document::with_id(
                    id.to_string(),
                    "logs".to_string(),
                    HashMap::from([
                        ("level".to_string(), Value::String(level.to_string())),
                        ("message".to_string(), Value::String(message.to_string())),
                        ("payload".to_string(), Value::String("x".repeat(64 * 1024))),
                        ("ts".to_string(), Value::Integer(ts)),
                    ]),
                ))
                .unwrap();
        }

        let query = parse_oql(
            r#"logs.where { .level == "error" }.sort { .ts.desc }.select { id, message }.limit 1"#,
        )
        .unwrap();
        assert_eq!(executor.explain_indexed_read(&query).unwrap(), None);

        let execution = executor.execute_query_with_plan(&query).unwrap();
        let plan = execution.physical_plan.expect("sequential physical plan");
        assert!(plan.contains("StorageScan collection=logs"), "{plan}");
        assert!(plan.contains("filters=1"), "{plan}");
        assert!(
            plan.contains("projection=Some([\"id\", \"message\", \"ts\"])"),
            "{plan}"
        );
        assert_eq!(execution.documents.len(), 1);
        assert_eq!(execution.documents[0].id, "l3");
        assert_eq!(
            execution.documents[0].get_field("message"),
            Some(&Value::String("newer".to_string()))
        );
        assert!(!execution.documents[0].fields.contains_key("payload"));

        let profile = execution.operator_profile.expect("sequential profile");
        fn find_operator<'a>(
            profile: &'a crate::execution::OperatorExecutionStats,
            name: &str,
        ) -> Option<&'a crate::execution::OperatorExecutionStats> {
            (profile.operator == name).then_some(profile).or_else(|| {
                profile
                    .children
                    .iter()
                    .find_map(|child| find_operator(child, name))
            })
        }
        assert_eq!(
            find_operator(&profile, "StorageScan")
                .expect("storage scan profile")
                .actual_rows,
            2
        );

        let fused = parse_oql(r#"logs.where { .level == "error" }.select { message }"#).unwrap();
        let fused_execution = executor.execute_query_with_plan(&fused).unwrap();
        let fused_plan = fused_execution.physical_plan.expect("fused scan plan");
        assert!(
            fused_plan.starts_with("StorageScan collection=logs"),
            "{fused_plan}"
        );
        assert!(!fused_plan.contains("Filter "), "{fused_plan}");
        assert!(!fused_plan.contains("Project "), "{fused_plan}");
        assert!(fused_plan.contains("projection=Some([\"message\"])"));
        assert_eq!(fused_execution.documents.len(), 2);
        assert!(fused_execution
            .documents
            .iter()
            .all(|document| document.fields.len() == 1));
        let fused_profile = fused_execution
            .operator_profile
            .as_ref()
            .expect("fused columnar profile");
        let storage_scan =
            find_operator(fused_profile, "StorageScan").expect("fused storage scan profile");
        assert_eq!(storage_scan.metrics["columnar_rows"], 2);
        assert_eq!(storage_scan.metrics["columnar_batches"], 1);

        let batched = executor.execute_query_batches(&fused).unwrap();
        assert_eq!(batched.len(), 2);
        assert_eq!(batched.projected_chunk_count(), 1);
        let batched_documents = batched.into_documents();
        assert!(batched_documents
            .iter()
            .all(|document| document.fields.len() == 1));
        assert!(batched_documents
            .iter()
            .all(|document| document.fields.contains_key("message")));

        let system_projection =
            parse_oql(r#"logs.where { .level == "error" }.select { id, message }"#).unwrap();
        let system_plan = executor
            .execute_query_with_plan(&system_projection)
            .unwrap()
            .physical_plan
            .expect("system projection plan");
        assert!(system_plan.starts_with("Project fields=2"), "{system_plan}");
        assert!(
            system_plan.contains("StorageScan collection=logs"),
            "{system_plan}"
        );
    }

    #[test]
    fn index_union_executes_or_branches_and_deduplicates_overlaps() {
        use crate::schema::FieldType;
        use crate::storage::ScalarIndexDefinition;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([
                    ("level".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                    ("ts".to_string(), FieldType::Integer),
                ]),
            ))
            .unwrap();
        for (name, field) in [
            ("events_by_level", "level"),
            ("events_by_message", "message"),
        ] {
            executor
                .store
                .upsert_scalar_index(ScalarIndexDefinition {
                    name: name.to_string(),
                    schema: "events".to_string(),
                    fields: vec![field.to_string()],
                    unique: false,
                })
                .unwrap();
        }
        for (id, level, message, ts) in [
            ("e1", "error", "older", 10),
            ("e2", "info", "ignored", 30),
            ("e3", "error", "newer", 20),
        ] {
            executor
                .store
                .put_document(&Document::with_id(
                    id.to_string(),
                    "events".to_string(),
                    HashMap::from([
                        ("level".to_string(), Value::String(level.to_string())),
                        ("message".to_string(), Value::String(message.to_string())),
                        ("ts".to_string(), Value::Integer(ts)),
                    ]),
                ))
                .unwrap();
        }

        let query = parse_oql(
            r#"events.where { .level == "error" or .message == "newer" }.sort { .ts }.select { message, ts }"#,
        )
        .unwrap();
        let explain = executor
            .explain_indexed_read(&query)
            .unwrap()
            .expect("index union plan");
        assert!(explain.contains("IndexUnion branches=2"), "{explain}");
        assert!(explain.contains("index=events_by_level"), "{explain}");
        assert!(explain.contains("index=events_by_message"), "{explain}");
        assert_eq!(
            explain
                .matches("pushed_projection=[\"message\", \"ts\"]")
                .count(),
            2
        );

        let execution = executor.execute_query_with_plan(&query).unwrap();
        let messages = execution
            .documents
            .iter()
            .map(|document| document.get_field("message").cloned().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            vec![
                Value::String("older".to_string()),
                Value::String("newer".to_string())
            ]
        );
        let profile = execution.operator_profile.expect("union profile");
        fn find_operator<'a>(
            profile: &'a crate::execution::OperatorExecutionStats,
            name: &str,
        ) -> Option<&'a crate::execution::OperatorExecutionStats> {
            (profile.operator == name).then_some(profile).or_else(|| {
                profile
                    .children
                    .iter()
                    .find_map(|child| find_operator(child, name))
            })
        }
        assert_eq!(
            find_operator(&profile, "IndexUnion")
                .expect("index union profile")
                .actual_rows,
            2
        );

        let transaction = executor.store.begin_local_transaction().unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "e4".to_string(),
                "events".to_string(),
                HashMap::from([
                    ("level".to_string(), Value::String("error".to_string())),
                    (
                        "message".to_string(),
                        Value::String("transaction-local".to_string()),
                    ),
                    ("ts".to_string(), Value::Integer(40)),
                ]),
            ))
            .unwrap();
        assert_eq!(executor.explain_indexed_read(&query).unwrap(), None);
        let transactional = executor.execute_query_with_plan(&query).unwrap();
        assert!(transactional.physical_plan.is_none());
        assert_eq!(transactional.documents.len(), 3);
        assert_eq!(
            transactional.documents[2].get_field("message"),
            Some(&Value::String("transaction-local".to_string()))
        );
        executor
            .store
            .rollback_local_transaction(transaction)
            .unwrap();
    }

    #[test]
    fn ordered_index_union_pushes_a_bounded_global_top_k() {
        use crate::schema::FieldType;
        use crate::storage::ScalarIndexDefinition;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([
                    ("level".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                    ("ts".to_string(), FieldType::Integer),
                ]),
            ))
            .unwrap();
        executor
            .store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "events_by_level_ts".to_string(),
                schema: "events".to_string(),
                fields: vec!["level".to_string(), "ts".to_string()],
                unique: false,
            })
            .unwrap();
        for (id, level, ts) in [
            ("e1", "error", 10),
            ("e2", "info", 30),
            ("e3", "error", 20),
            ("e4", "debug", 40),
        ] {
            executor
                .store
                .put_document(&Document::with_id(
                    id.to_string(),
                    "events".to_string(),
                    HashMap::from([
                        ("level".to_string(), Value::String(level.to_string())),
                        ("message".to_string(), Value::String(id.to_string())),
                        ("ts".to_string(), Value::Integer(ts)),
                    ]),
                ))
                .unwrap();
        }

        let query = parse_oql(
            r#"events.where { .level == "error" or .level == "info" }.sort { .ts.desc }.select { message }.limit 2"#,
        )
        .unwrap();
        let explain = executor
            .explain_indexed_read(&query)
            .unwrap()
            .expect("ordered index union");
        assert!(explain.contains("IndexUnion branches=2"), "{explain}");
        assert!(explain.contains("order_fields=[\"ts\"]"), "{explain}");
        assert!(explain.contains("direction=Descending"), "{explain}");
        assert!(explain.contains("pushed_limit=Some(2)"), "{explain}");
        assert!(!explain.contains("Sort keys="), "{explain}");
        assert_eq!(explain.matches("pushed_limit=Some(2)").count(), 3);

        let documents = executor.execute_query(&query).unwrap();
        let messages = documents
            .iter()
            .map(|document| document.get_field("message").cloned().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            vec![
                Value::String("e2".to_string()),
                Value::String("e3".to_string())
            ]
        );
    }

    #[test]
    fn composite_index_order_eliminates_sort_and_pushes_top_k_limit() {
        use crate::schema::FieldType;
        use crate::storage::ScalarIndexDefinition;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([
                    ("app".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                    ("timestamp".to_string(), FieldType::Integer),
                ]),
            ))
            .unwrap();
        executor
            .store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "events_by_app_time".to_string(),
                schema: "events".to_string(),
                fields: vec!["app".to_string(), "timestamp".to_string()],
                unique: false,
            })
            .unwrap();
        for (id, app, timestamp) in [
            ("old", "api", 10),
            ("new", "api", 30),
            ("middle", "api", 20),
            ("other-app", "web", 100),
        ] {
            executor
                .store
                .put_document(&Document::with_id(
                    id.to_string(),
                    "events".to_string(),
                    HashMap::from([
                        ("app".to_string(), Value::String(app.to_string())),
                        ("message".to_string(), Value::String(id.to_string())),
                        ("timestamp".to_string(), Value::Integer(timestamp)),
                    ]),
                ))
                .unwrap();
        }

        let query = parse_oql(
            r#"events.where { .app == "api" and .timestamp >= 15 }.sort { .timestamp.desc }.select { message }.limit 2"#,
        )
        .unwrap();
        let explain = executor
            .explain_indexed_read(&query)
            .unwrap()
            .expect("ordered index plan");
        assert!(!explain.contains("Sort keys="), "{explain}");
        assert!(explain.contains("direction=Descending"), "{explain}");
        assert!(explain.contains("pushed_limit=Some(2)"), "{explain}");
        assert!(
            explain.contains("pushed_projection=[\"message\"]"),
            "{explain}"
        );

        let results = executor.execute_query(&query).unwrap();
        let messages = results
            .iter()
            .map(|document| document.get_field("message").cloned().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            vec![Value::String("new".into()), Value::String("middle".into())]
        );

        let ascending = parse_oql(
            r#"events.where { .app == "api" }.sort { .timestamp.asc }.select { message }.limit 2"#,
        )
        .unwrap();
        let ascending_explain = executor
            .explain_indexed_read(&ascending)
            .unwrap()
            .expect("ascending ordered plan");
        assert!(
            !ascending_explain.contains("Sort keys="),
            "{ascending_explain}"
        );
        assert!(
            ascending_explain.contains("direction=Ascending"),
            "{ascending_explain}"
        );
        let ascending_messages = executor
            .execute_query(&ascending)
            .unwrap()
            .into_iter()
            .map(|document| document.get_field("message").cloned().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            ascending_messages,
            vec![Value::String("old".into()), Value::String("middle".into())]
        );

        let unrelated_sort =
            parse_oql(r#"events.where { .app == "api" }.sort { .message.desc }.limit 2"#).unwrap();
        let unrelated_explain = executor
            .explain_indexed_read(&unrelated_sort)
            .unwrap()
            .expect("indexed plan with explicit sort");
        assert!(
            unrelated_explain.contains("Sort keys=1"),
            "{unrelated_explain}"
        );
        assert!(
            unrelated_explain.contains("pushed_limit=None"),
            "{unrelated_explain}"
        );
    }

    #[test]
    fn indexed_projection_supports_all_document_system_fields() {
        use crate::schema::FieldType;
        use crate::storage::ScalarIndexDefinition;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([
                    ("app".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                    ("timestamp".to_string(), FieldType::Integer),
                ]),
            ))
            .unwrap();
        executor
            .store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "events_by_app_time".to_string(),
                schema: "events".to_string(),
                fields: vec!["app".to_string(), "timestamp".to_string()],
                unique: false,
            })
            .unwrap();

        let mut source = Document::with_id(
            "event-42".to_string(),
            "events".to_string(),
            HashMap::from([
                ("app".to_string(), Value::String("api".to_string())),
                ("message".to_string(), Value::String("failed".to_string())),
                ("timestamp".to_string(), Value::Integer(42)),
            ]),
        );
        source.version = 7;
        source.updated_at = source.created_at + chrono::Duration::seconds(5);
        executor.store.put_document(&source).unwrap();

        let query = parse_oql(
            r#"events.where { .app == "api" }.sort { .timestamp desc }.select { id, schema, created_at, updated_at, version, message }.limit 1"#,
        )
        .unwrap();
        let explain = executor
            .explain_indexed_read(&query)
            .unwrap()
            .expect("document system fields must retain the indexed path");
        assert!(explain.contains("index=events_by_app_time"), "{explain}");
        assert!(explain.contains("access=NativeTyped"), "{explain}");

        let result = executor.execute_query(&query).unwrap();
        assert_eq!(result.len(), 1);
        let projected = &result[0];
        assert_eq!(projected.id, source.id);
        assert_eq!(projected.schema, source.schema);
        assert_eq!(projected.created_at, source.created_at);
        assert_eq!(projected.updated_at, source.updated_at);
        assert_eq!(projected.version, source.version);
        assert_eq!(projected.fields.get("id"), Some(&Value::String(source.id)));
        assert_eq!(
            projected.fields.get("schema"),
            Some(&Value::String("events".to_string()))
        );
        assert_eq!(
            projected.fields.get("created_at"),
            Some(&Value::DateTime(source.created_at))
        );
        assert_eq!(
            projected.fields.get("updated_at"),
            Some(&Value::DateTime(source.updated_at))
        );
        assert_eq!(projected.fields.get("version"), Some(&Value::Integer(7)));
    }

    #[test]
    fn ordered_index_intersection_stops_at_qualifying_row_goal() {
        use crate::schema::FieldType;
        use crate::storage::ScalarIndexDefinition;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([
                    ("application".to_string(), FieldType::String),
                    ("environment".to_string(), FieldType::String),
                    ("level_rank".to_string(), FieldType::Integer),
                    ("occurred_at".to_string(), FieldType::Integer),
                    ("message".to_string(), FieldType::String),
                ]),
            ))
            .unwrap();
        for (name, fields) in [
            (
                "events_by_application_time",
                vec!["application", "environment", "occurred_at", "id"],
            ),
            (
                "events_by_level_time",
                vec![
                    "application",
                    "environment",
                    "level_rank",
                    "occurred_at",
                    "id",
                ],
            ),
        ] {
            executor
                .store
                .upsert_scalar_index(ScalarIndexDefinition {
                    name: name.to_string(),
                    schema: "events".to_string(),
                    fields: fields.into_iter().map(str::to_string).collect(),
                    unique: false,
                })
                .unwrap();
        }
        for (id, rank, occurred_at) in [
            ("new-info", 30, 60),
            ("new-error", 50, 50),
            ("middle-info", 30, 40),
            ("middle-error", 50, 30),
            ("old-error", 50, 20),
        ] {
            executor
                .store
                .put_document(&Document::with_id(
                    id.to_string(),
                    "events".to_string(),
                    HashMap::from([
                        ("application".to_string(), Value::String("api".into())),
                        ("environment".to_string(), Value::String("prod".into())),
                        ("level_rank".to_string(), Value::Integer(rank)),
                        ("occurred_at".to_string(), Value::Integer(occurred_at)),
                        ("message".to_string(), Value::String(id.to_string())),
                    ]),
                ))
                .unwrap();
        }

        let query = parse_oql(
            r#"events.where { .application == "api" && .environment == "prod" && .level_rank >= 50 }.sort { .occurred_at desc }.select { id, message }.limit 2"#,
        )
        .unwrap();
        let explain = executor
            .explain_indexed_read(&query)
            .unwrap()
            .expect("native indexed plan");
        assert!(
            explain.contains("index=events_by_application_time"),
            "{explain}"
        );
        assert!(
            explain.contains("prefilter_index=Some(\"events_by_level_time\")"),
            "{explain}"
        );
        assert!(explain.contains("pushed_limit=Some(2)"), "{explain}");
        assert!(!explain.contains("Sort keys="), "{explain}");

        let results = executor.execute_query(&query).unwrap();
        let ids = results
            .into_iter()
            .map(|document| document.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["new-error", "middle-error"]);

        executor
            .store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "events_by_application_time".to_string(),
                schema: "events".to_string(),
                fields: [
                    "application",
                    "environment",
                    "occurred_at",
                    "level_rank",
                    "id",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
                unique: false,
            })
            .unwrap();
        let covering_explain = executor
            .explain_indexed_read(&query)
            .unwrap()
            .expect("covering ordered plan");
        assert!(
            covering_explain.contains("index=events_by_application_time"),
            "{covering_explain}"
        );
        assert!(
            covering_explain.contains("prefilter_index=Some(\"events_by_level_time\")"),
            "{covering_explain}"
        );
        let covering_ids = executor
            .execute_query(&query)
            .unwrap()
            .into_iter()
            .map(|document| document.id)
            .collect::<Vec<_>>();
        assert_eq!(covering_ids, vec!["new-error", "middle-error"]);
    }

    #[test]
    fn test_where_filter_comparison() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test query: users.where { .age >= 30 }
        let query = parse_oql(r#"users.where { .age >= 30 }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 2);
        let ids: Vec<&String> = results.iter().map(|d| &d.id).collect();
        assert!(ids.contains(&&"user_1".to_string())); // Alice, age 30
        assert!(ids.contains(&&"user_3".to_string())); // Charlie, age 35
    }

    #[test]
    fn test_where_filter_boolean() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test query: users.where { .active == true }
        let query = parse_oql(r#"users.where { .active == true }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 2);
        let ids: Vec<&String> = results.iter().map(|d| &d.id).collect();
        assert!(ids.contains(&&"user_1".to_string())); // Alice, active
        assert!(ids.contains(&&"user_3".to_string())); // Charlie, active
    }

    #[test]
    fn test_where_filter_no_matches() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test query: users.where { .name == "David" }
        let query = parse_oql(r#"users.where { .name == "David" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_where_filter_missing_field() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test query: users.where { .email == "test@example.com" }
        let query = parse_oql(r#"users.where { .email == "test@example.com" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        // Should return no results since no documents have an email field
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_value_comparison_type_coercion() {
        let (executor, _temp_dir) = create_test_executor();

        // Test integer to float comparison
        assert!(executor.values_equal(&Value::Integer(42), &Value::Float(42.0)));
        assert!(executor.values_equal(&Value::Float(42.0), &Value::Integer(42)));

        // Test comparison operators with type coercion
        assert!(executor
            .compare_ordered_values(&Value::Integer(10), &Value::Float(20.5), |ord| ord.is_lt())
            .unwrap());

        assert!(executor
            .compare_ordered_values(&Value::Float(30.5), &Value::Integer(20), |ord| ord.is_gt())
            .unwrap());
    }

    #[test]
    fn test_empty_collection() {
        let (executor, _temp_dir) = create_test_executor();

        // Test scanning empty collection
        let results = executor.scan_collection("empty_collection").unwrap();
        assert_eq!(results.len(), 0);

        // Test query on empty collection
        let query = parse_oql(r#"empty_collection.where { .name == "Alice" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_insert_operation() {
        let (executor, _temp_dir) = create_test_executor();

        // Test INSERT operation
        let query =
            parse_oql(r#"users.insert { id: "user_4", name: "David", age: 28, active: true }"#)
                .unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 1);
        let inserted_doc = &results[0];
        assert_eq!(inserted_doc.schema, "users");
        assert_eq!(
            inserted_doc.get_field("name"),
            Some(&Value::String("David".to_string()))
        );
        assert_eq!(inserted_doc.get_field("age"), Some(&Value::Integer(28)));
        assert_eq!(
            inserted_doc.get_field("active"),
            Some(&Value::Boolean(true))
        );

        // Verify document was stored
        let stored_doc = executor
            .store
            .get_document("users", &inserted_doc.id)
            .unwrap();
        assert!(stored_doc.is_some());
        assert_eq!(stored_doc.unwrap(), *inserted_doc);
    }

    #[test]
    fn insert_many_commits_one_atomic_wal_batch() {
        use crate::schema::FieldType;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new(
                "events".to_string(),
                HashMap::from([
                    ("id".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                ]),
            ))
            .unwrap();
        let before = executor.store.committed_lsn();
        let query = parse_oql(
            r#"events.insert [
                { id: "e1", message: "one" },
                { id: "e2", message: "two" },
                { id: "e3", message: "three" }
            ]"#,
        )
        .unwrap();

        let inserted = executor.execute_query(&query).unwrap();
        assert_eq!(inserted.len(), 3);
        assert_eq!(executor.store.committed_lsn(), before + 1);
        assert_eq!(executor.store.scan_documents("events").unwrap().len(), 3);
    }

    #[test]
    fn insert_many_rejects_same_batch_unique_duplicates_without_partial_commit() {
        use crate::schema::FieldType;

        let (executor, _temp_dir) = create_test_executor();
        executor
            .store
            .put_schema(&Schema::new_with_constraints(
                "events".to_string(),
                HashMap::from([
                    ("fingerprint".to_string(), FieldType::String),
                    ("message".to_string(), FieldType::String),
                ]),
                vec![SchemaConstraint::Unique {
                    fields: vec!["fingerprint".to_string()],
                    name: Some("unique_fingerprint".to_string()),
                }],
            ))
            .unwrap();
        let before = executor.store.committed_lsn();
        let query = parse_oql(
            r#"events.insert [
                { fingerprint: "same", message: "one" },
                { fingerprint: "same", message: "two" }
            ]"#,
        )
        .unwrap();

        let error = executor.execute_query(&query).unwrap_err();
        assert!(error.to_string().contains("unique_fingerprint"));
        assert_eq!(executor.store.committed_lsn(), before);
        assert!(executor.store.scan_documents("events").unwrap().is_empty());
    }

    #[test]
    fn test_update_operation() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test UPDATE operation: users.where { .name == "Alice" }.update { age: 31 }
        let query = parse_oql(r#"users.where { .name == "Alice" }.update { age: 31 }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 1);
        let updated_doc = &results[0];
        assert_eq!(updated_doc.id, "user_1");
        assert_eq!(updated_doc.get_field("age"), Some(&Value::Integer(31)));
        assert_eq!(
            updated_doc.get_field("name"),
            Some(&Value::String("Alice".to_string()))
        );

        // Verify document was updated in storage
        let stored_doc = executor
            .store
            .get_document("users", &updated_doc.id)
            .unwrap()
            .unwrap();
        assert_eq!(stored_doc.get_field("age"), Some(&Value::Integer(31)));
    }

    #[test]
    fn test_delete_operation() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test DELETE operation: users.where { .name == "Bob" }.delete
        let query = parse_oql(r#"users.where { .name == "Bob" }.delete"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 1);
        let deleted_doc = &results[0];
        assert_eq!(deleted_doc.id, "user_2");
        assert_eq!(
            deleted_doc.get_field("name"),
            Some(&Value::String("Bob".to_string()))
        );

        // Verify document was deleted from storage
        let stored_doc = executor
            .store
            .get_document("users", &deleted_doc.id)
            .unwrap();
        assert!(stored_doc.is_none());

        // Verify other documents still exist
        let alice_doc = executor
            .store
            .get_document("users", &"user_1".to_string())
            .unwrap();
        assert!(alice_doc.is_some());
    }

    #[test]
    fn test_select_projection() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test SELECT with field projection: users.select { name, age }
        let query = parse_oql(r#"users.select { name, age }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 3);

        for doc in &results {
            // Should only have name and age fields
            assert!(doc.get_field("name").is_some());
            assert!(doc.get_field("age").is_some());
            // Should not have active field (not selected)
            assert!(doc.get_field("active").is_none());
        }
    }

    #[test]
    fn test_select_with_alias() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test SELECT with alias: users.select { full_name: .name, years: .age }
        let query = parse_oql(r#"users.select { full_name: .name, years: .age }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 3);

        for doc in &results {
            // Should have aliased fields
            assert!(doc.get_field("full_name").is_some());
            assert!(doc.get_field("years").is_some());
            // Should not have original field names
            assert!(doc.get_field("name").is_none());
            assert!(doc.get_field("age").is_none());
        }

        // Check specific values
        let alice_doc = results
            .iter()
            .find(|d| d.get_field("full_name") == Some(&Value::String("Alice".to_string())))
            .unwrap();
        assert_eq!(alice_doc.get_field("years"), Some(&Value::Integer(30)));
    }

    #[test]
    fn test_combined_operations() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test combined WHERE and SELECT: users.where { .age >= 30 }.select { name }
        let query = parse_oql(r#"users.where { .age >= 30 }.select { name }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 2);

        for doc in &results {
            // Should only have name field
            assert!(doc.get_field("name").is_some());
            assert!(doc.get_field("age").is_none());
            assert!(doc.get_field("active").is_none());
        }

        // Check that only Alice and Charlie are returned (age >= 30)
        let names: Vec<String> = results
            .iter()
            .filter_map(|d| match d.get_field("name") {
                Some(Value::String(name)) => Some(name.clone()),
                _ => None,
            })
            .collect();

        assert!(names.contains(&"Alice".to_string()));
        assert!(names.contains(&"Charlie".to_string()));
        assert!(!names.contains(&"Bob".to_string()));
    }

    #[test]
    fn test_update_multiple_documents() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test UPDATE multiple: users.where { .active == true }.update { age: 99 }
        let query = parse_oql(r#"users.where { .active == true }.update { age: 99 }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 2); // Alice and Charlie are active

        for doc in &results {
            assert_eq!(doc.get_field("age"), Some(&Value::Integer(99)));
            assert_eq!(doc.get_field("active"), Some(&Value::Boolean(true)));
        }

        // Verify Bob's age wasn't changed (he's not active)
        let bob_doc = executor
            .store
            .get_document("users", &"user_2".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(bob_doc.get_field("age"), Some(&Value::Integer(25)));
    }

    #[test]
    fn test_update_converts_reference_typed_fields() {
        let (executor, _temp_dir) = create_test_executor();

        let mut org_fields = HashMap::new();
        org_fields.insert("id".to_string(), crate::schema::FieldType::String);
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "organizations".to_string(),
                org_fields,
            ))
            .unwrap();

        let mut company_fields = HashMap::new();
        company_fields.insert("id".to_string(), crate::schema::FieldType::String);
        company_fields.insert(
            "org".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "organizations".to_string(),
            )),
        );
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "companies".to_string(),
                company_fields,
            ))
            .unwrap();

        executor
            .execute_query(&parse_oql(r#"organizations.insert { id: "org_1" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"organizations.insert { id: "org_2" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"companies.insert { id: "c1", org: "org_1" }"#).unwrap())
            .unwrap();

        executor
            .execute_query(
                &parse_oql(r#"companies.where { .id == "c1" }.update { org: "org_2" }"#).unwrap(),
            )
            .unwrap();

        let results = executor
            .execute_query(&parse_oql(r#"companies.where { .id == "c1" }"#).unwrap())
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].get_field("org"),
            Some(Value::Reference(_)) | Some(Value::GlobalReference(_))
        ));
        assert_eq!(
            executor
                .execute_query(
                    &parse_oql(r#"companies.where { .id == "c1" && .org.id == "org_2" }"#).unwrap()
                )
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn test_delete_multiple_documents() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();

        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }

        // Test DELETE multiple: users.where { .active == true }.delete
        let query = parse_oql(r#"users.where { .active == true }.delete"#).unwrap();
        let results = executor.execute_query(&query).unwrap();

        assert_eq!(results.len(), 2); // Alice and Charlie are active

        // Verify active users were deleted
        let alice_doc = executor
            .store
            .get_document("users", &"user_1".to_string())
            .unwrap();
        assert!(alice_doc.is_none());
        let charlie_doc = executor
            .store
            .get_document("users", &"user_3".to_string())
            .unwrap();
        assert!(charlie_doc.is_none());

        // Verify Bob still exists (he's not active)
        let bob_doc = executor
            .store
            .get_document("users", &"user_2".to_string())
            .unwrap();
        assert!(bob_doc.is_some());
    }

    #[test]
    fn test_group_by_aggregations_use_group_schema() {
        let (executor, _temp_dir) = create_test_executor();

        let mut o1 = HashMap::new();
        o1.insert("id".to_string(), Value::String("o1".to_string()));
        o1.insert("user_id".to_string(), Value::String("u1".to_string()));
        o1.insert("amount".to_string(), Value::Float(320.0));
        o1.insert("status".to_string(), Value::String("paid".to_string()));
        let mut o2 = HashMap::new();
        o2.insert("id".to_string(), Value::String("o2".to_string()));
        o2.insert("user_id".to_string(), Value::String("u1".to_string()));
        o2.insert("amount".to_string(), Value::Float(120.0));
        o2.insert("status".to_string(), Value::String("pending".to_string()));
        let mut o3 = HashMap::new();
        o3.insert("id".to_string(), Value::String("o3".to_string()));
        o3.insert("user_id".to_string(), Value::String("u2".to_string()));
        o3.insert("amount".to_string(), Value::Float(80.0));
        o3.insert("status".to_string(), Value::String("paid".to_string()));

        executor
            .store
            .put_document(&Document::with_id(
                "o1".to_string(),
                "orders".to_string(),
                o1,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "o2".to_string(),
                "orders".to_string(),
                o2,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "o3".to_string(),
                "orders".to_string(),
                o3,
            ))
            .unwrap();

        let query = parse_oql(
            r#"orders.group_by { .user_id }.select {
                user_id,
                total_orders: count(),
                total_amount: sum(.amount),
                avg_amount: avg(.amount)
            }"#,
        )
        .unwrap();

        let results = executor.execute_query(&query).unwrap();
        assert_eq!(results.len(), 2);

        let u1 = results
            .iter()
            .find(|d| d.get_field("user_id") == Some(&Value::String("u1".to_string())))
            .unwrap();
        assert_eq!(u1.get_field("total_orders"), Some(&Value::Integer(2)));
        assert_eq!(u1.get_field("total_amount"), Some(&Value::Float(440.0)));
        assert_eq!(u1.get_field("avg_amount"), Some(&Value::Float(220.0)));

        let u2 = results
            .iter()
            .find(|d| d.get_field("user_id") == Some(&Value::String("u2".to_string())))
            .unwrap();
        assert_eq!(u2.get_field("total_orders"), Some(&Value::Integer(1)));
        assert_eq!(u2.get_field("total_amount"), Some(&Value::Float(80.0)));
        assert_eq!(u2.get_field("avg_amount"), Some(&Value::Float(80.0)));
    }

    #[test]
    fn test_group_by_preserves_ref_derived_key_values() {
        let (executor, _temp_dir) = create_test_executor();

        let mut org_fields = HashMap::new();
        org_fields.insert("id".to_string(), crate::schema::FieldType::String);
        org_fields.insert("name".to_string(), crate::schema::FieldType::String);
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "organizations".to_string(),
                org_fields,
            ))
            .unwrap();

        let mut team_fields = HashMap::new();
        team_fields.insert("id".to_string(), crate::schema::FieldType::String);
        team_fields.insert("name".to_string(), crate::schema::FieldType::String);
        team_fields.insert(
            "org".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "organizations".to_string(),
            )),
        );
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "teams".to_string(),
                team_fields,
            ))
            .unwrap();

        let mut chunk_fields = HashMap::new();
        chunk_fields.insert("id".to_string(), crate::schema::FieldType::String);
        chunk_fields.insert("status".to_string(), crate::schema::FieldType::String);
        chunk_fields.insert(
            "team".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "teams".to_string(),
            )),
        );
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "kb_chunks".to_string(),
                chunk_fields,
            ))
            .unwrap();

        executor
            .execute_query(
                &parse_oql(r#"organizations.insert { id: "org_1", name: "Northwind" }"#).unwrap(),
            )
            .unwrap();
        executor
            .execute_query(
                &parse_oql(r#"teams.insert { id: "team_infra", name: "Infra", org: "org_1" }"#)
                    .unwrap(),
            )
            .unwrap();
        executor
            .execute_query(
                &parse_oql(
                    r#"teams.insert { id: "team_payments", name: "Payments", org: "org_1" }"#,
                )
                .unwrap(),
            )
            .unwrap();

        executor
            .execute_query(
                &parse_oql(
                    r#"kb_chunks.insert [
                        { id: "c1", status: "published", team: "team_infra" },
                        { id: "c2", status: "published", team: "team_infra" },
                        { id: "c3", status: "published", team: "team_payments" }
                    ]"#,
                )
                .unwrap(),
            )
            .unwrap();

        let query = parse_oql(
            r#"kb_chunks.where { .status == "published" }.group_by { .team.id, .team.name }.select {
                team_id,
                team_name,
                chunk_count: count()
            }"#,
        )
        .unwrap();

        let results = executor.execute_query(&query).unwrap();
        assert_eq!(results.len(), 2);

        let infra = results
            .iter()
            .find(|d| d.get_field("team_id") == Some(&Value::String("team_infra".to_string())))
            .expect("infra group");
        assert_eq!(
            infra.get_field("team_name"),
            Some(&Value::String("Infra".to_string()))
        );
        assert_eq!(infra.get_field("chunk_count"), Some(&Value::Integer(2)));

        let payments = results
            .iter()
            .find(|d| d.get_field("team_id") == Some(&Value::String("team_payments".to_string())))
            .expect("payments group");
        assert_eq!(
            payments.get_field("team_name"),
            Some(&Value::String("Payments".to_string()))
        );
        assert_eq!(payments.get_field("chunk_count"), Some(&Value::Integer(1)));
    }

    #[test]
    fn test_select_with_query_expression_count_executes() {
        let (executor, _temp_dir) = create_test_executor();

        let mut u1 = HashMap::new();
        u1.insert("id".to_string(), Value::String("u1".to_string()));
        let mut u2 = HashMap::new();
        u2.insert("id".to_string(), Value::String("u2".to_string()));
        executor
            .store
            .put_document(&Document::with_id(
                "u1".to_string(),
                "users".to_string(),
                u1,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "u2".to_string(),
                "users".to_string(),
                u2,
            ))
            .unwrap();

        let mut f1 = HashMap::new();
        f1.insert("status".to_string(), Value::String("open".to_string()));
        let mut f2 = HashMap::new();
        f2.insert("status".to_string(), Value::String("open".to_string()));
        let mut f3 = HashMap::new();
        f3.insert("status".to_string(), Value::String("closed".to_string()));
        executor
            .store
            .put_document(&Document::with_id(
                "f1".to_string(),
                "findings".to_string(),
                f1,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "f2".to_string(),
                "findings".to_string(),
                f2,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "f3".to_string(),
                "findings".to_string(),
                f3,
            ))
            .unwrap();

        let query = parse_oql(
            r#"users.select { id, open_count: findings.where { .status == "open" }.count() }"#,
        )
        .unwrap();
        let results = executor.execute_query(&query).unwrap();
        assert_eq!(results.len(), 2);
        for row in results {
            assert_eq!(row.get_field("open_count"), Some(&Value::Integer(2)));
        }
    }

    #[test]
    fn test_select_with_query_expression_count_correlates_with_collection_scope_name() {
        let (executor, _temp_dir) = create_test_executor();

        let mut c1 = HashMap::new();
        c1.insert("id".to_string(), Value::String("c1".to_string()));
        let mut c2 = HashMap::new();
        c2.insert("id".to_string(), Value::String("c2".to_string()));
        executor
            .store
            .put_document(&Document::with_id(
                "c1".to_string(),
                "companies".to_string(),
                c1,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "c2".to_string(),
                "companies".to_string(),
                c2,
            ))
            .unwrap();

        let mut f1 = HashMap::new();
        f1.insert("company_id".to_string(), Value::String("c1".to_string()));
        let mut f2 = HashMap::new();
        f2.insert("company_id".to_string(), Value::String("c1".to_string()));
        let mut f3 = HashMap::new();
        f3.insert("company_id".to_string(), Value::String("c2".to_string()));
        executor
            .store
            .put_document(&Document::with_id(
                "f1".to_string(),
                "findings".to_string(),
                f1,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "f2".to_string(),
                "findings".to_string(),
                f2,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::with_id(
                "f3".to_string(),
                "findings".to_string(),
                f3,
            ))
            .unwrap();

        let query = parse_oql(
            r#"companies.select { id, finding_count: findings.where { .company_id == companies.id }.count() }"#,
        )
        .unwrap();
        let results = executor.execute_query(&query).unwrap();
        assert_eq!(results.len(), 2);

        let r_c1 = results
            .iter()
            .find(|d| d.get_field("id") == Some(&Value::String("c1".to_string())))
            .expect("row c1");
        let r_c2 = results
            .iter()
            .find(|d| d.get_field("id") == Some(&Value::String("c2".to_string())))
            .expect("row c2");

        assert_eq!(r_c1.get_field("finding_count"), Some(&Value::Integer(2)));
        assert_eq!(r_c2.get_field("finding_count"), Some(&Value::Integer(1)));
    }

    #[test]
    fn test_guard_pattern_is_not_silently_accepted() {
        let (executor, _temp_dir) = create_test_executor();
        let pattern =
            crate::parser::MatchPattern::Guard(Box::new(crate::parser::Condition::Comparison {
                left: Expression::Literal(crate::parser::Literal::Integer(1)),
                operator: crate::parser::ComparisonOperator::Equal,
                right: Expression::Literal(crate::parser::Literal::Integer(1)),
            }));
        let res = executor.pattern_matches(&pattern, &Value::Integer(1));
        assert!(
            res.is_err(),
            "guard pattern should not be silently accepted"
        );
    }

    #[test]
    fn test_insert_enforces_unique_constraint() {
        let (executor, _temp_dir) = create_test_executor();
        let mut fields = HashMap::new();
        fields.insert("email".to_string(), crate::schema::FieldType::String);
        let schema = crate::schema::Schema::new_with_constraints(
            "customers".to_string(),
            fields,
            vec![crate::schema::SchemaConstraint::Unique {
                fields: vec!["email".to_string()],
                name: Some("uq_customers_email".to_string()),
            }],
        );
        executor.store.put_schema(&schema).unwrap();

        let q1 = parse_oql(r#"customers.insert { email: "a@x.com" }"#).unwrap();
        executor.execute_query(&q1).unwrap();

        let q2 = parse_oql(r#"customers.insert { email: "a@x.com" }"#).unwrap();
        let err = executor
            .execute_query(&q2)
            .expect_err("expected unique violation");
        assert!(err.to_string().contains("Unique constraint"));
    }

    #[test]
    fn test_insert_enforces_check_constraint() {
        let (executor, _temp_dir) = create_test_executor();
        let mut fields = HashMap::new();
        fields.insert("age".to_string(), crate::schema::FieldType::Integer);
        let schema = crate::schema::Schema::new_with_constraints(
            "people".to_string(),
            fields,
            vec![crate::schema::SchemaConstraint::Check {
                field: "age".to_string(),
                op: crate::schema::CheckOperator::Gte,
                value: Value::Integer(18),
                name: Some("age_adult".to_string()),
            }],
        );
        executor.store.put_schema(&schema).unwrap();

        let q = parse_oql(r#"people.insert { age: 12 }"#).unwrap();
        let err = executor
            .execute_query(&q)
            .expect_err("expected check violation");
        assert!(err.to_string().contains("Check constraint"));
    }

    #[test]
    fn test_insert_enforces_ref_integrity_constraint() {
        let (executor, _temp_dir) = create_test_executor();

        let mut company_fields = HashMap::new();
        company_fields.insert("id".to_string(), crate::schema::FieldType::String);
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "companies".to_string(),
                company_fields,
            ))
            .unwrap();
        executor
            .store
            .put_document(&Document::new(
                "companies".to_string(),
                HashMap::from([(String::from("id"), Value::String("c1".to_string()))]),
            ))
            .unwrap();

        let mut finding_fields = HashMap::new();
        finding_fields.insert("company_id".to_string(), crate::schema::FieldType::String);
        let finding_schema = crate::schema::Schema::new_with_constraints(
            "findings".to_string(),
            finding_fields,
            vec![crate::schema::SchemaConstraint::RefIntegrity {
                field: "company_id".to_string(),
                target_schema: "companies".to_string(),
                target_field: Some("id".to_string()),
                required: true,
                name: Some("fk_findings_company".to_string()),
            }],
        );
        executor.store.put_schema(&finding_schema).unwrap();

        let ok_q = parse_oql(r#"findings.insert { company_id: "c1" }"#).unwrap();
        executor.execute_query(&ok_q).unwrap();

        let bad_q = parse_oql(r#"findings.insert { company_id: "missing" }"#).unwrap();
        let err = executor
            .execute_query(&bad_q)
            .expect_err("expected ref integrity violation");
        assert!(err.to_string().contains("Ref constraint"));
    }

    #[test]
    fn test_nearest_returns_ranked_documents_with_score() {
        let (executor, _temp_dir) = create_test_executor();

        let mut fields = HashMap::new();
        fields.insert("id".to_string(), crate::schema::FieldType::String);
        fields.insert(
            "embedding".to_string(),
            crate::schema::FieldType::Vector { dim: 3 },
        );
        let schema = crate::schema::Schema::new("products".to_string(), fields);
        executor.store.put_schema(&schema).unwrap();

        let q = parse_oql(
            r#"products.insert [
                { id: "p1", embedding: [1.0, 0.0, 0.0] },
                { id: "p2", embedding: [0.0, 1.0, 0.0] },
                { id: "p3", embedding: [0.9, 0.1, 0.0] }
            ]"#,
        )
        .unwrap();
        executor.execute_query(&q).unwrap();

        let nearest_q = parse_oql(
            r#"products.nearest { field: .embedding, query: [1.0, 0.0, 0.0], metric: "cosine", k: 2 }"#,
        )
        .unwrap();
        let results = executor.execute_query(&nearest_q).unwrap();

        assert_eq!(results.len(), 2);
        let top_id = results[0].get_field("id").cloned();
        assert_eq!(top_id, Some(Value::String("p1".to_string())));
        assert!(results[0].get_field("_score").is_some());
    }

    #[test]
    fn test_ref_scoped_filter_uses_logical_id_field() {
        let (executor, _temp_dir) = create_test_executor();

        let mut org_fields = HashMap::new();
        org_fields.insert("id".to_string(), crate::schema::FieldType::String);
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "organizations".to_string(),
                org_fields,
            ))
            .unwrap();

        let mut team_fields = HashMap::new();
        team_fields.insert("id".to_string(), crate::schema::FieldType::String);
        team_fields.insert(
            "org".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "organizations".to_string(),
            )),
        );
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "teams".to_string(),
                team_fields,
            ))
            .unwrap();

        let mut chunk_fields = HashMap::new();
        chunk_fields.insert("id".to_string(), crate::schema::FieldType::String);
        chunk_fields.insert(
            "org".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "organizations".to_string(),
            )),
        );
        chunk_fields.insert(
            "team".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "teams".to_string(),
            )),
        );
        chunk_fields.insert("status".to_string(), crate::schema::FieldType::String);
        chunk_fields.insert(
            "embedding".to_string(),
            crate::schema::FieldType::Vector { dim: 3 },
        );
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "kb_chunks".to_string(),
                chunk_fields,
            ))
            .unwrap();

        executor
            .execute_query(&parse_oql(r#"organizations.insert { id: "org_1" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(
                &parse_oql(r#"teams.insert { id: "team_payments", org: "org_1" }"#).unwrap(),
            )
            .unwrap();
        executor
            .execute_query(
                &parse_oql(
                    r#"kb_chunks.insert {
                        id: "chunk_1",
                        org: "org_1",
                        team: "team_payments",
                        status: "published",
                        embedding: [1.0, 0.0, 0.0]
                    }"#,
                )
                .unwrap(),
            )
            .unwrap();

        let q = parse_oql(
            r#"kb_chunks.where { .org.id == "org_1" && .team.id == "team_payments" }.select { id }"#,
        )
        .unwrap();
        let results = executor.execute_query(&q).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].get_field("id"),
            Some(&Value::String("chunk_1".to_string()))
        );
    }

    #[test]
    fn test_prefilter_documents_for_query_uses_reverse_reference_index() {
        let (executor, _temp_dir) = create_test_executor();

        let mut org_fields = HashMap::new();
        org_fields.insert("id".to_string(), crate::schema::FieldType::String);
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "organizations".to_string(),
                org_fields,
            ))
            .unwrap();

        let mut team_fields = HashMap::new();
        team_fields.insert("id".to_string(), crate::schema::FieldType::String);
        team_fields.insert(
            "org".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "organizations".to_string(),
            )),
        );
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "teams".to_string(),
                team_fields,
            ))
            .unwrap();

        let mut chunk_fields = HashMap::new();
        chunk_fields.insert("id".to_string(), crate::schema::FieldType::String);
        chunk_fields.insert(
            "org".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "organizations".to_string(),
            )),
        );
        chunk_fields.insert(
            "team".to_string(),
            crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                "teams".to_string(),
            )),
        );
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "kb_chunks".to_string(),
                chunk_fields,
            ))
            .unwrap();

        executor
            .execute_query(&parse_oql(r#"organizations.insert { id: "org_1" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"organizations.insert { id: "org_2" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(
                &parse_oql(r#"teams.insert { id: "team_payments", org: "org_1" }"#).unwrap(),
            )
            .unwrap();
        executor
            .execute_query(
                &parse_oql(r#"teams.insert { id: "team_infra", org: "org_2" }"#).unwrap(),
            )
            .unwrap();

        executor
            .execute_query(
                &parse_oql(
                    r#"kb_chunks.insert { id: "chunk_1", org: "org_1", team: "team_payments" }"#,
                )
                .unwrap(),
            )
            .unwrap();
        executor
            .execute_query(
                &parse_oql(
                    r#"kb_chunks.insert { id: "chunk_2", org: "org_2", team: "team_infra" }"#,
                )
                .unwrap(),
            )
            .unwrap();

        let q = parse_oql(
            r#"kb_chunks.where { .org.id == "org_1" && .team.id == "team_payments" }.select { id }"#,
        )
        .unwrap();

        let prefiltered = executor
            .prefilter_documents_for_query(&q)
            .unwrap()
            .expect("expected reverse-ref prefilter candidates");
        assert_eq!(prefiltered.len(), 1);
        assert_eq!(
            prefiltered[0].get_field("id"),
            Some(&Value::String("chunk_1".to_string()))
        );
    }

    #[test]
    fn test_prefilter_documents_for_query_uses_nested_reverse_reference_chain() {
        let (executor, _temp_dir) = create_test_executor();

        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "organizations".to_string(),
                HashMap::from([("id".to_string(), crate::schema::FieldType::String)]),
            ))
            .unwrap();
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "companies".to_string(),
                HashMap::from([
                    ("id".to_string(), crate::schema::FieldType::String),
                    (
                        "org".to_string(),
                        crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                            "organizations".to_string(),
                        )),
                    ),
                ]),
            ))
            .unwrap();
        executor
            .store
            .put_schema(&crate::schema::Schema::new(
                "findings".to_string(),
                HashMap::from([
                    ("id".to_string(), crate::schema::FieldType::String),
                    (
                        "company".to_string(),
                        crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                            "companies".to_string(),
                        )),
                    ),
                ]),
            ))
            .unwrap();

        executor
            .execute_query(&parse_oql(r#"organizations.insert { id: "org_1" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"organizations.insert { id: "org_2" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"companies.insert { id: "c1", org: "org_1" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"companies.insert { id: "c2", org: "org_2" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"findings.insert { id: "f1", company: "c1" }"#).unwrap())
            .unwrap();
        executor
            .execute_query(&parse_oql(r#"findings.insert { id: "f2", company: "c2" }"#).unwrap())
            .unwrap();

        let q =
            parse_oql(r#"findings.where { .company.org.id == "org_1" }.select { id }"#).unwrap();
        let prefiltered = executor
            .prefilter_documents_for_query(&q)
            .unwrap()
            .expect("expected nested reverse-ref prefilter candidates");

        assert_eq!(prefiltered.len(), 1);
        assert_eq!(
            prefiltered[0].get_field("id"),
            Some(&Value::String("f1".to_string()))
        );
    }

    #[test]
    fn test_nearest_rejects_dimension_mismatch() {
        let (executor, _temp_dir) = create_test_executor();

        let mut fields = HashMap::new();
        fields.insert(
            "embedding".to_string(),
            crate::schema::FieldType::Vector { dim: 2 },
        );
        let schema = crate::schema::Schema::new("items".to_string(), fields);
        executor.store.put_schema(&schema).unwrap();
        executor
            .store
            .put_document(&Document::new(
                "items".to_string(),
                HashMap::from([(
                    "embedding".to_string(),
                    Value::List(vec![Value::Float(1.0), Value::Float(2.0)]),
                )]),
            ))
            .unwrap();

        let q = parse_oql(r#"items.nearest { field: .embedding, query: [1.0, 2.0, 3.0], k: 1 }"#)
            .unwrap();
        let err = executor
            .execute_query(&q)
            .expect_err("dimension mismatch expected");
        assert!(err.to_string().contains("Type error"));
    }

    #[test]
    fn test_nearest_hnsw_mode_uses_ann_candidates_and_rerank() {
        let (executor, _temp_dir) = create_test_executor();

        let mut fields = HashMap::new();
        fields.insert("id".to_string(), crate::schema::FieldType::String);
        fields.insert(
            "embedding".to_string(),
            crate::schema::FieldType::Vector { dim: 4 },
        );
        let schema = crate::schema::Schema::new("products".to_string(), fields);
        executor.store.put_schema(&schema).unwrap();

        executor
            .store
            .upsert_vector_index(crate::storage::VectorIndexDefinition {
                name: "idx_products_embedding".to_string(),
                schema: "products".to_string(),
                field: "embedding".to_string(),
                dimension: 4,
                metric: crate::storage::VectorMetricType::Cosine,
                index_type: crate::storage::VectorIndexType::Hnsw,
                quantization: Some("scalar".to_string()),
                state: crate::storage::VectorIndexState::Ready,
            })
            .unwrap();

        let q = parse_oql(
            r#"products.insert [
                { id: "p1", embedding: [1.0, 0.0, 0.0, 0.0] },
                { id: "p2", embedding: [0.0, 1.0, 0.0, 0.0] },
                { id: "p3", embedding: [0.9, 0.1, 0.0, 0.0] },
                { id: "p4", embedding: [0.0, 0.0, 1.0, 0.0] }
            ]"#,
        )
        .unwrap();
        executor.execute_query(&q).unwrap();

        let nearest_q = parse_oql(
            r#"products.nearest {
                field: .embedding,
                query: [1.0, 0.0, 0.0, 0.0],
                metric: "cosine",
                k: 2,
                num_candidates: 2,
                exact: false
            }"#,
        )
        .unwrap();
        let results = executor.execute_query(&nearest_q).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|d| d.get_field("_score").is_some()));
    }
}
