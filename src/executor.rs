// OQL Query Executor - Document scanning and filtering

use crate::distributed::GlobalObjRef;
use crate::document::{Document, ObjRef, Value};
use crate::error::{OQLError, OQLResult};
use crate::execution::{
    build_profiled_read_executor, build_read_executor, DataChunk, OperatorExecutionStats,
    PhysicalOperatorCollect,
};
use crate::functions::FunctionRegistry;
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
use crate::storage::{DocumentStore, ProjectedRowRef, ProjectedValueRef, ScanPredicate};
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
            &Document::with_id("$script".to_string(), "$script".to_string(), crate::document::Fields::default()),
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

    /// See and define the same OQL functions as `other` from now on. A
    /// script runs on an executor of its own, and this is what makes a
    /// function it defines outlast it.
    pub fn share_functions_with(&mut self, other: &Self) {
        self.functions.share_user_functions(&other.functions);
    }

    /// Remove a user-defined function, for example when the transaction
    /// that defined it is rolled back.
    pub fn unregister_user_function(&mut self, name: &str) {
        self.functions.unregister_user_function(name);
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
        self.atomically(query, || self.execute_statement(query))
    }

    /// Run a statement so that, if it writes, it is one indivisible step.
    ///
    /// A statement that writes reads first: the rows to update, the id that
    /// must be free, the value to add one to. Run on its own it takes
    /// effect only if nobody else changed what it wrote in the meantime,
    /// and is otherwise run again, so `update { stock: .stock - 1 }` from
    /// two callers takes two off and `where { .version == 3 }.update { .. }`
    /// is a compare-and-set. Inside a transaction, or inside another
    /// statement, it is part of that instead.
    fn atomically<T>(
        &self,
        query: &OQLQuery,
        mut statement: impl FnMut() -> OQLResult<T>,
    ) -> OQLResult<T> {
        let writes = query.operations.iter().any(|operation| {
            matches!(
                operation,
                Operation::Insert(_)
                    | Operation::InsertMany(_)
                    | Operation::Upsert(_)
                    | Operation::Update(_)
                    | Operation::Delete
            )
        });
        if !writes || StatementGuard::active() {
            return statement();
        }
        let mut outcome = None;
        self.store.run_atomically(&mut || {
            outcome = Some(statement()?);
            Ok(())
        })?;
        outcome.ok_or_else(|| OQLError::Generic {
            message: "the statement did not run".to_string(),
        })
    }

    fn execute_statement(&self, query: &OQLQuery) -> OQLResult<Vec<Document>> {
        let _statement = StatementGuard::enter();
        for operation in &query.operations {
            if let Operation::Sort(sort) = operation {
                self.check_sort_functions(sort)?;
            }
        }
        if let Some(Operation::Call(arguments)) = query.operations.first() {
            return self.execute_call_query(query, arguments);
        }
        profile_call!("execute_query", {
            if let Some(documents) = self.tie_broken_top_k(query)? {
                self.validate_declared_paths(query)?;
                return Ok(documents);
            }
            if let Some(plan) = self.try_plan_safe_read(query)? {
                self.validate_declared_paths(query)?;
                let mut rows = self.execute_read_plan(&plan)?;
                Self::name_absent_selected_fields(query, &mut rows);
                return Ok(rows);
            }

            self.execute_general_query(query)
        })
    }

    /// A `select` names its fields in every row: one a row has no value for
    /// is `null` there, not missing. Rows fetched through an index carry
    /// only the values their records hold, so the rest are named here, as
    /// they already are in rows read by a scan.
    fn name_absent_selected_fields(query: &OQLQuery, rows: &mut [Document]) {
        // Only when the `select` is what shapes the result: nothing after it
        // but filters, a sort or a limit.
        let Some(at) = query
            .operations
            .iter()
            .rposition(|operation| matches!(operation, Operation::Select(_)))
        else {
            return;
        };
        let shaped_later = query.operations[at + 1..].iter().any(|operation| {
            !matches!(operation, Operation::Where(_) | Operation::Sort(_) | Operation::Limit(_))
        });
        let Operation::Select(select) = &query.operations[at] else {
            return;
        };
        if shaped_later {
            return;
        }
        let names: Vec<&String> = select
            .fields
            .iter()
            .filter_map(|field| match &field.expression {
                Expression::FieldAccess(access) if access.path.len() == 1 && access.path[0] != "*" => {
                    Some(field.alias.as_ref().unwrap_or(&access.path[0]))
                }
                _ => None,
            })
            .collect();
        for row in rows {
            for name in &names {
                if !row.fields.contains_key(*name) {
                    row.fields.insert((*name).clone(), Value::Null);
                }
            }
        }
    }

    /// Execute a read while retaining Owl's batch-native result representation.
    ///
    /// This is the preferred boundary for broad projections and streaming
    /// consumers. Queries outside the safe physical-read subset remain fully
    /// supported by wrapping their established document output in chunks.
    pub fn execute_query_batches(&self, query: &OQLQuery) -> OQLResult<BatchQueryExecution> {
        self.atomically(query, || self.execute_statement_batches(query))
    }

    fn execute_statement_batches(&self, query: &OQLQuery) -> OQLResult<BatchQueryExecution> {
        let _statement = StatementGuard::enter();
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
        self.atomically(query, || self.execute_statement_with_plan(query))
    }

    fn execute_statement_with_plan(&self, query: &OQLQuery) -> OQLResult<QueryExecution> {
        let _statement = StatementGuard::enter();
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
        if let Some((Operation::Explain, described)) = query.operations.split_last() {
            return self.explain_query(&OQLQuery {
                collection: query.collection.clone(),
                operations: described.to_vec(),
            });
        }
        self.validate_declared_paths(query)?;
        // Everything before the first operation that only this route knows
        // runs as a query of its own, with every shortcut open to it; the
        // rest is applied to its rows one operation at a time.
        let limited = Self::limit_before_select(query);
        let query = limited.as_ref().unwrap_or(query);
        let named = self.reduction_named(query)?;
        let query = named.as_ref().unwrap_or(query);
        let filtered = Self::combined_as_one_filter(query);
        let query = filtered.as_ref().unwrap_or(query);
        let origin = Self::step_keeping_origin(query);
        let plain = query.operations.iter().position(Self::runs_plainly);
        let split = match (plain, &origin) {
            (Some(plain), Some((step, _))) => Some(plain.min(*step)),
            (plain, origin) => plain.or(origin.as_ref().map(|(step, _)| *step)),
        };
        if let Some(at) = split {
            let (before, after) = query.operations.split_at(at);
            // A function called for each row of a whole collection reads
            // the rows where they are; it may not need them all.
            let mut whole = match after.first() {
                Some(Operation::AggregateExpr { function, .. })
                    if before.is_empty() && !Self::is_aggregate_name(function) =>
                {
                    self.store.scan_documents_shared(&query.collection)?
                }
                _ => None,
            };
            // A walk that does not keep the way reads only two things of
            // the rows it starts from: which rows they are, and the field
            // it leaves them by when that is one they hold.
            let mut before = before.to_vec();
            if let Some(Operation::Walk { path, paths: false, .. }) = after.first() {
                let narrowed = before.iter().all(|operation| {
                    matches!(
                        operation,
                        Operation::Where(_) | Operation::Sort(_) | Operation::Limit(_) | Operation::Offset(_)
                    )
                });
                if let (true, Some(schema)) = (narrowed, self.schema_of(&query.collection)?) {
                    let mut names = vec!["id".to_string()];
                    names.extend(path.path.first().filter(|name| schema.fields.contains_key(*name)).cloned());
                    before.push(Operation::Select(SelectBlock {
                        fields: names
                            .into_iter()
                            .map(|name| crate::parser::SelectField {
                                alias: None,
                                expression: Expression::FieldAccess(FieldAccess { path: vec![name] }),
                            })
                            .collect(),
                    }));
                }
            }
            let mut documents = match whole {
                Some(_) => Vec::new(),
                None => self.execute_general_query(&OQLQuery {
                    collection: query.collection.clone(),
                    operations: before.clone(),
                })?,
            };
            let mut grouped = before
                .iter()
                .any(|operation| matches!(operation, Operation::GroupBy(_)));
            for (position, operation) in after.iter().enumerate() {
                if grouped
                    && documents.is_empty()
                    && matches!(operation, Operation::Select(_) | Operation::Having(_))
                {
                    continue;
                }
                documents = match operation {
                    Operation::Sort(sort) => self.sort_keeping(
                        documents,
                        sort,
                        Self::rows_kept_after_sort(after, position),
                    )?,
                    Operation::Combine { how, other } => {
                        let whole = |operations: &[Operation]| {
                            operations.iter().all(|operation| {
                                matches!(
                                    operation,
                                    Operation::Where(_)
                                        | Operation::Sort(_)
                                        | Operation::Limit(_)
                                        | Operation::Offset(_)
                                        | Operation::Distinct
                                        | Operation::Combine { .. }
                                )
                            })
                        };
                        let same_rows = whole(&query.operations[..at + position])
                            && match other {
                                Expression::QueryExpression { query: theirs } => {
                                    theirs.collection == query.collection
                                        && whole(&theirs.operations)
                                        && !theirs.operations.iter().any(|operation| {
                                            matches!(operation, Operation::Combine { .. })
                                        })
                                }
                                Expression::Identifier(name) => *name == query.collection,
                                _ => false,
                            };
                        // An earlier combination with shaped rows would have
                        // left rows that are not this collection's.
                        let same_rows = same_rows
                            && query.operations[..at + position].iter().all(|operation| {
                                match operation {
                                    Operation::Combine { other, .. } => match other {
                                        Expression::QueryExpression { query: theirs } => {
                                            theirs.collection == query.collection
                                                && whole(&theirs.operations)
                                        }
                                        Expression::Identifier(name) => *name == query.collection,
                                        _ => false,
                                    },
                                    _ => true,
                                }
                            });
                        self.apply_combine(documents, how, other, &query.collection, same_rows)?
                    }
                    Operation::AggregateExpr { function, input }
                        if !Self::is_aggregate_name(function) =>
                    {
                        self.apply_function_to(
                            match whole.take() {
                                Some(rows) => RowsIn::Shared(rows),
                                None => RowsIn::Owned(documents),
                            },
                            function,
                            input,
                            &query.collection,
                            &after[position + 1..],
                        )?
                    }
                    Operation::Step(path) if origin.is_some() => {
                        // What follows may name the rows in hand, whose
                        // collection is the one the query starts from or
                        // one an earlier step reached; or a collection
                        // this step's own path passes through.
                        let following = &after[position + 1..];
                        let in_hand = documents.first().map(|row| row.schema.clone());
                        let wanted = in_hand
                            .as_deref()
                            .map(|collection| Self::anchored_paths(following, collection))
                            .filter(|paths| !paths.is_empty());
                        match (in_hand, wanted) {
                            (Some(collection), Some(wanted)) => {
                                self.step_from_origin(documents, path, &collection, Some(&wanted))?
                            }
                            _ => {
                                let mut reached = Vec::new();
                                for document in documents {
                                    match self.path_keeping_origin(&document, &path.path, following)? {
                                        Some(rows) => reached.extend(rows),
                                        None => reached.extend(self.step_from_origin(
                                            vec![document],
                                            path,
                                            &query.collection,
                                            None,
                                        )?),
                                    }
                                }
                                reached
                            }
                        }
                    }
                    _ => self.apply_operation(documents, operation, &query.collection)?,
                };
                grouped |= matches!(operation, Operation::GroupBy(_));
            }
            return Ok(documents);
        }
        let ranged = self.pattern_ranges(query)?;
        let query = ranged.as_ref().unwrap_or(query);
        let reordered = Self::storage_filter_first(query);
        let query = reordered.as_ref().unwrap_or(query);
        if let Some(documents) = self.aggregate_from_columns(query, false)? {
            return Ok(documents);
        }
        let mut documents = Vec::new();
        let mut is_insert_only = false;
        let mut filters_applied = 0;

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
                if let Some((rows, applied)) = self.first_rows_by_id(query)? {
                    filters_applied = applied;
                    rows
                } else if let Some(prefiltered) = self.prefilter_documents_for_query(query)? {
                    prefiltered
                } else if let Some(rows) = self.rows_through_reference(query)? {
                    // Only rows that can pass the filter; it still runs.
                    rows
                } else if let Some(rows) = self.count_from_columns(query)? {
                    // Whole columns answer a filter on plain fields without
                    // reaching into a single row.
                    return Ok(rows);
                } else if let Some(rows) = self.count_of_held_rows(query)? {
                    // Rows the store holds decoded are judged where they
                    // are, which costs less than reading their columns.
                    return Ok(rows);
                } else if let Some(documents) = self.aggregate_from_columns(query, true)? {
                    // Nothing narrows the read, so every row is visited
                    // once and none is kept.
                    return Ok(documents);
                } else if let Some((plan, applied)) = self.pipeline_prefix(query)? {
                    // An index that narrows the read beats visiting every
                    // row; without one, shaping rows as they are read does.
                    let mut lent = None;
                    if !Self::contains_native_index_scan(&plan.root) {
                        if let Some(documents) = self.select_from_columns(query)? {
                            return Ok(documents);
                        }
                        // Every row is visited either way. Filtering the
                        // rows the store holds copies only those that pass.
                        lent = self.rows_by_reference(query)?;
                    }
                    match lent {
                        Some((rows, done)) => {
                            filters_applied = done;
                            rows
                        }
                        None => {
                            // Storage already applied the leading filters.
                            filters_applied = applied;
                            self.execute_read_plan(&plan)?
                        }
                    }
                } else if let Some(documents) = self.select_from_columns(query)? {
                    return Ok(documents);
                } else if let Some((rows, applied)) = self.rows_by_reference(query)? {
                    filters_applied = applied;
                    rows
                } else if let Some(fields) = self.fields_read_by(query) {
                    // The query shapes its own output and names every field
                    // it reads, so the rest need not be decoded.
                    match self.store.scan_documents_filtered_projection(
                        &query.collection,
                        &[],
                        Some(&fields),
                    )? {
                        Some(documents) => documents,
                        None => self
                            .store
                            .scan_documents_projection(&query.collection, &fields)?,
                    }
                } else {
                    self.scan_collection(&query.collection)?
                };
            documents.extend(scanned_docs);
        }

        // Apply operations in sequence
        let mut grouped = false;
        for (position, operation) in query.operations.iter().enumerate().skip(filters_applied) {
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
            documents = match operation {
                // Only the rows the following `limit` keeps are put in order.
                Operation::Sort(sort) => self.sort_keeping(
                    temp_docs,
                    sort,
                    Self::rows_kept_after_sort(&query.operations, position),
                )?,
                _ => self.apply_operation(temp_docs, operation, &query.collection)?,
            };
            grouped |= matches!(operation, Operation::GroupBy(_));
        }

        Ok(documents)
    }

    /// The first path step of a query whose later operations name the
    /// collection to read the row an element came from, as in
    /// `tasks.worklog.group_by { .state, task: tasks.state }`, with the
    /// paths they read.
    fn step_keeping_origin(query: &OQLQuery) -> Option<(usize, Vec<Vec<String>>)> {
        let step = query
            .operations
            .iter()
            .position(|operation| matches!(operation, Operation::Step(_)))?;
        // Any `name.field` after the step may name the collection the
        // query starts from, or one the steps pass through.
        let after = &query.operations[step + 1..];
        let named = serde_json::to_value(after).map_or(true, |tree| Self::reads_by_name(&tree));
        named.then(|| (step, Self::anchored_paths(after, &query.collection)))
    }

    /// Whether anything in a query's operations is read as `name.field`.
    fn reads_by_name(node: &serde_json::Value) -> bool {
        match node {
            serde_json::Value::Object(fields) => {
                fields.contains_key("ChainedAccess") || fields.values().any(Self::reads_by_name)
            }
            serde_json::Value::Array(items) => items.iter().any(Self::reads_by_name),
            _ => false,
        }
    }

    /// Whole rows of one collection combined as sets are the rows that pass
    /// one filter: `a.where { x }.intersect(a.where { y })` is
    /// `a.where { x && y }`, `except` is `x && !y`, and `union` is `x || y`.
    /// One filter is read once and every way of reading less is open to it.
    ///
    /// `intersect` and `except` keep the order of the rows before them, as
    /// the filter does. `union` puts the other query's rows after, so it is
    /// rewritten only when what follows makes the order immaterial: a
    /// count, or a sort by `id`. `None` when the query is not of this kind.
    fn combined_as_one_filter(query: &OQLQuery) -> Option<OQLQuery> {
        fn all(conditions: Vec<Condition>) -> Option<Condition> {
            conditions
                .into_iter()
                .reduce(|left, right| Condition::And(Box::new(left), Box::new(right)))
        }
        let filters_of = |operations: &[Operation]| -> Option<Vec<Condition>> {
            operations
                .iter()
                .map(|operation| match operation {
                    Operation::Where(block) => Some(block.condition.clone()),
                    _ => None,
                })
                .collect()
        };
        let first = query
            .operations
            .iter()
            .position(|operation| matches!(operation, Operation::Combine { .. }))?;
        let combined = query.operations[first..]
            .iter()
            .take_while(|operation| matches!(operation, Operation::Combine { .. }))
            .count();
        let rest = &query.operations[first + combined..];
        let mut condition = all(filters_of(&query.operations[..first])?)?;
        for operation in &query.operations[first..first + combined] {
            let Operation::Combine { how, other } = operation else {
                return None;
            };
            let Expression::QueryExpression { query: theirs } = other else {
                return None;
            };
            if theirs.collection != query.collection {
                return None;
            }
            let other = all(filters_of(&theirs.operations)?)?;
            condition = match how.as_str() {
                "intersect" => Condition::And(Box::new(condition), Box::new(other)),
                "except" => Condition::And(
                    Box::new(condition),
                    Box::new(Condition::Not(Box::new(other))),
                ),
                "union" => {
                    let unordered = match rest.first() {
                        Some(Operation::Count) => rest.len() == 1,
                        Some(Operation::Sort(sort)) => sort.fields.iter().any(|field| {
                            matches!(&field.expression, Expression::FieldAccess(access) if access.path == ["id"])
                        }),
                        _ => false,
                    };
                    if !unordered {
                        return None;
                    }
                    Condition::Or(Box::new(condition), Box::new(other))
                }
                _ => return None,
            };
        }
        let mut operations = vec![Operation::Where(WhereBlock { condition })];
        operations.extend(rest.iter().cloned());
        Some(OQLQuery {
            collection: query.collection.clone(),
            operations,
        })
    }

    /// What reducing the values of a field means when written the other
    /// way round: `.salary.median()` is `median(.salary)`, and
    /// `.customer.distinct().count()` is a count of the distinct values of
    /// `customer`. The second form is the one every shortcut knows. `None` when the
    /// operations are not such a reduction.
    fn reduction_naming(field: &str, operations: &[Operation]) -> Option<Vec<Operation>> {
        let named = FieldAccess { path: vec![field.to_string()] };
        let itself = |expression: &Expression| {
            matches!(expression, Expression::FieldAccess(access) if access.path.is_empty())
        };
        Some(vec![match operations {
            [Operation::Distinct, Operation::Count] => Operation::AggregateExpr {
                function: DISTINCT_COUNT.to_string(),
                input: Expression::FieldAccess(named),
            },
            [Operation::AggregateExpr { function, input }]
                if itself(input) && Self::is_aggregate_name(function) =>
            {
                match function.as_str() {
                    "sum" => Operation::Sum(named),
                    "avg" => Operation::Avg(named),
                    "min" => Operation::Min(named),
                    "max" => Operation::Max(named),
                    _ => Operation::AggregateExpr {
                        function: function.clone(),
                        input: Expression::FieldAccess(named),
                    },
                }
            }
            [Operation::AggregateExpr { function, input: Expression::ArrayLiteral(pair) }]
                if function == "percentile" && pair.len() == 2 && itself(&pair[0]) =>
            {
                Operation::AggregateExpr {
                    function: function.clone(),
                    input: Expression::ArrayLiteral(vec![
                        Expression::FieldAccess(named),
                        pair[1].clone(),
                    ]),
                }
            }
            _ => return None,
        }])
    }

    /// `people.where { .. }.salary.median()` as `people.where { .. }.median(.salary)`:
    /// a step into one plain field, then a reduction of its values, is the
    /// aggregate of that field over the rows. `None` when the query does
    /// not end that way.
    fn reduction_named(&self, query: &OQLQuery) -> OQLResult<Option<OQLQuery>> {
        let Some(step) = query
            .operations
            .iter()
            .position(|operation| matches!(operation, Operation::Step(_)))
        else {
            return Ok(None);
        };
        let Operation::Step(FieldAccess { path }) = &query.operations[step] else {
            return Ok(None);
        };
        let [field] = path.as_slice() else {
            return Ok(None);
        };
        // The rows stepped from must still be rows of the collection.
        let whole = query.operations[..step].iter().all(|operation| {
            matches!(
                operation,
                Operation::Where(_) | Operation::Sort(_) | Operation::Limit(_) | Operation::Offset(_)
            )
        });
        let Some(named) = Self::reduction_naming(field, &query.operations[step + 1..]) else {
            return Ok(None);
        };
        // A query's result is named after its last operation, and
        // `.distinct().count()` is a `count`, so it is left as written.
        if matches!(query.operations.last(), Some(Operation::Count)) {
            return Ok(None);
        }
        if !whole || !self.holds_one_value(&query.collection, field)? {
            return Ok(None);
        }
        let mut operations = query.operations[..step].to_vec();
        operations.extend(named);
        Ok(Some(OQLQuery {
            collection: query.collection.clone(),
            operations,
        }))
    }

    /// Whether `field` of a row of `collection` holds one plain value, so
    /// that reducing "the values of the field" and reducing the rows by
    /// that field are the same thing. A list or an embedded object is not:
    /// a path through one has a value for each element.
    fn holds_one_value(&self, collection: &str, field: &str) -> OQLResult<bool> {
        let Some(schema) = self.schema_of(collection)? else {
            return Ok(false);
        };
        let mut declared = schema.fields.get(field);
        while let Some(FieldType::Optional(inner)) = declared {
            declared = Some(inner);
        }
        Ok(matches!(
            declared,
            Some(
                FieldType::String
                    | FieldType::Integer
                    | FieldType::Float
                    | FieldType::Boolean
                    | FieldType::DateTime
                    | FieldType::Reference(_)
            )
        ))
    }

    /// The rows a path reaches when the operations applied to them name a
    /// collection the path passes through on the way. `None` when they name
    /// none, which is nearly always.
    fn path_keeping_origin(
        &self,
        scope_document: &Document,
        path: &[String],
        operations: &[Operation],
    ) -> OQLResult<Option<Vec<Document>>> {
        // Almost no operations read anything as `name.field`, and only
        // those can name a collection on the way. Without one there is
        // nothing to keep and the rows on the way are not looked at.
        let place = (operations.as_ptr() as usize, operations.len());
        let known = STATEMENT.with(|scope| {
            scope
                .borrow()
                .names_rows
                .get(&place)
                .filter(|(remembered, _)| remembered.as_slice() == operations)
                .map(|(_, names)| *names)
        });
        let names = match known {
            Some(names) => names,
            None => {
                let names = serde_json::to_value(operations).map_or(true, |tree| Self::reads_by_name(&tree));
                if StatementGuard::active() {
                    STATEMENT.with(|scope| {
                        scope
                            .borrow_mut()
                            .names_rows
                            .insert(place, (operations.to_vec(), names))
                    });
                }
                names
            }
        };
        if !names {
            return Ok(None);
        }
        let mut rows = vec![scope_document.clone()];
        for (position, segment) in path[..path.len() - 1].iter().enumerate() {
            let mut next = Vec::new();
            for row in &rows {
                next.extend(self.path_source_rows(row, std::slice::from_ref(segment))?.0);
            }
            rows = next;
            let Some(collection) = rows.first().map(|row| row.schema.clone()) else {
                return Ok(None);
            };
            // The enclosing row's own collection means the enclosing row.
            if collection == scope_document.schema {
                continue;
            }
            let wanted = Self::anchored_paths(operations, &collection);
            if wanted.is_empty() {
                continue;
            }
            let mut reached = rows;
            let rest = &path[position + 1..];
            for (hop, segment) in rest.iter().enumerate() {
                reached = self.step_from_origin(
                    reached,
                    &FieldAccess { path: vec![segment.clone()] },
                    &collection,
                    (hop == 0).then_some(wanted.as_slice()),
                )?;
            }
            return Ok(Some(reached));
        }
        Ok(None)
    }

    /// The paths that `operations` read from a row of `collection` by naming
    /// the collection, as `tasks.state` does.
    fn anchored_paths(operations: &[Operation], collection: &str) -> Vec<Vec<String>> {
        // `collection.a.b` is a chain of accesses that bottoms out in the
        // collection's name.
        fn collect(node: &serde_json::Value, collection: &str, paths: &mut Vec<Vec<String>>) {
            let mut path = Vec::new();
            let mut current = node;
            while let Some(access) = current.get("ChainedAccess") {
                path.extend(access.get("field").and_then(|field| field.as_str()).map(str::to_string));
                current = access.get("base").unwrap_or(&serde_json::Value::Null);
            }
            if !path.is_empty()
                && current.get("Identifier").and_then(|name| name.as_str()) == Some(collection)
            {
                path.reverse();
                if !paths.contains(&path) {
                    paths.push(path);
                }
                return;
            }
            match node {
                serde_json::Value::Object(fields) => {
                    fields.values().for_each(|value| collect(value, collection, paths))
                }
                serde_json::Value::Array(items) => {
                    items.iter().for_each(|value| collect(value, collection, paths))
                }
                _ => {}
            }
        }
        let mut paths = Vec::new();
        if let Ok(tree) = serde_json::to_value(operations) {
            collect(&tree, collection, &mut paths);
        }
        paths
    }

    /// A path step that lets the rows it reaches still answer for the row
    /// they came from. `wanted` are the paths to keep from each row of
    /// `collection`; without it, what an earlier step kept is passed on.
    fn step_from_origin(
        &self,
        documents: Vec<Document>,
        path: &FieldAccess,
        collection: &str,
        wanted: Option<&[Vec<String>]>,
    ) -> OQLResult<Vec<Document>> {
        let mut reached = Vec::new();
        for document in &documents {
            let origin = match wanted {
                Some(paths) => {
                    let mut kept = crate::document::Fields::with_capacity_and_hasher(paths.len() + 1, Default::default());
                    kept.insert(ORIGIN_NAME.to_string(), Value::String(collection.to_string()));
                    for path in paths {
                        kept.insert(
                            path.join("."),
                            self.get_field_value(document, &FieldAccess { path: path.clone() })?,
                        );
                    }
                    Some(Value::Object(kept))
                }
                None => document.fields.get(ORIGIN_FIELD).cloned(),
            };
            for mut row in self.path_source_rows(document, &path.path)?.0 {
                if let Some(origin) = &origin {
                    row.fields.insert(ORIGIN_FIELD.to_string(), origin.clone());
                }
                reached.push(row);
            }
        }
        Ok(reached)
    }

    /// The columns of `fields` for a fold, with the rows `filters` leave
    /// when there are filters. `None` when the store has no such columns or
    /// a filter is not one the columns can decide.
    fn fold_columns(
        &self,
        collection: &str,
        fields: &[String],
        filters: &[ScanPredicate],
    ) -> OQLResult<Option<(crate::storage::ColumnBatch, Option<Vec<u32>>)>> {
        if fields.is_empty() || !COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            return Ok(None);
        }
        // The filters as parts that must all hold.
        let mut parts: Vec<(&String, &ComparisonOperator, &Value)> = Vec::new();
        let mut pending: Vec<&ScanPredicate> = filters.iter().collect();
        while let Some(next) = pending.pop() {
            match next {
                ScanPredicate::And(left, right) => {
                    pending.push(right);
                    pending.push(left);
                }
                ScanPredicate::Comparison { field, operator, value } => parts.push((field, operator, value)),
                ScanPredicate::Or(..) => return Ok(None),
            }
        }
        let mut names = fields.to_vec();
        for (field, _, _) in &parts {
            if DocumentSystemField::from_name(field).is_some() {
                return Ok(None);
            }
            if !names.contains(*field) {
                names.push((*field).clone());
            }
        }
        let Some(mut batches) = self.store.scan_columns(collection, &names)? else {
            return Ok(None);
        };
        let (Some(batch), true) = (batches.pop(), batches.is_empty()) else {
            return Ok(None);
        };
        if parts.is_empty() {
            return Ok(Some((batch, None)));
        }
        let mut chosen = Vec::new();
        let mut whole = true;
        for (field, operator, wanted) in parts {
            let at = names.iter().position(|name| name == field).unwrap_or(0);
            if !Self::narrow_by_column(batch.column(at), operator, wanted, batch.rows(), whole, &mut chosen) {
                return Ok(None);
            }
            whole = false;
        }
        Ok(Some((batch, Some(chosen))))
    }

    /// `collection.where { .a == 1 && .b != null }.count()` over the store's
    /// columns. Each part of the filter is a field against a constant, and
    /// each is one pass over one column that narrows the rows still in the
    /// running. `None` when the filter has any other part, or the store
    /// does not hold the fields as columns.
    ///
    /// The answer is what [`Self::count_of_held_rows`] gives; it is reached
    /// without reading a row.
    fn count_from_columns(&self, query: &OQLQuery) -> OQLResult<Option<Vec<Document>>> {
        if !COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            return Ok(None);
        }
        let Some((Operation::Count, filters)) = query.operations.split_last() else {
            return Ok(None);
        };
        if filters.is_empty() {
            return Ok(None);
        }
        let mut pending: Vec<&Condition> = Vec::new();
        for operation in filters.iter().rev() {
            match operation {
                Operation::Where(block) => pending.push(&block.condition),
                _ => return Ok(None),
            }
        }
        // Each part: which field asked for, the comparison, the constant.
        let mut names: Vec<String> = Vec::new();
        let mut parts: Vec<(usize, &ComparisonOperator, Value)> = Vec::new();
        while let Some(next) = pending.pop() {
            match next {
                Condition::And(left, right) => {
                    pending.push(right);
                    pending.push(left);
                }
                Condition::Comparison {
                    left: Expression::FieldAccess(access),
                    operator,
                    right: Expression::Literal(literal),
                } => {
                    let [name] = access.path.as_slice() else {
                        return Ok(None);
                    };
                    if DocumentSystemField::from_name(name).is_some() {
                        return Ok(None);
                    }
                    let field = match names.iter().position(|known| known == name) {
                        Some(field) => field,
                        None => {
                            names.push(name.clone());
                            names.len() - 1
                        }
                    };
                    parts.push((field, operator, self.literal_to_value(literal)));
                }
                _ => return Ok(None),
            }
        }
        let Some(batches) = self.store.scan_columns(&query.collection, &names)? else {
            return Ok(None);
        };
        let mut count = 0usize;
        let mut chosen: Vec<u32> = Vec::new();
        for batch in &batches {
            let mut whole = true;
            for (field, operator, wanted) in &parts {
                if !Self::narrow_by_column(batch.column(*field), operator, wanted, batch.rows(), whole, &mut chosen)
                {
                    return Ok(None);
                }
                whole = false;
                if chosen.is_empty() {
                    break;
                }
            }
            count += chosen.len();
        }
        COLUMN_SCANS_RUN.with(|runs| runs.set(runs.get() + 1));
        Ok(Some(self.apply_count_operation(Vec::new()).map(|mut rows| {
            rows[0]
                .fields
                .insert("count".to_string(), Value::Integer(count as i64));
            rows
        })?))
    }

    /// Keep in `chosen` the rows whose value in `column` is `operator`
    /// `wanted`: of all `rows` when `whole`, and of those already chosen
    /// otherwise. Decided exactly as a comparison of the row's field is.
    /// `false` when this cannot decide it, and the rows must be asked.
    fn narrow_by_column(
        column: crate::storage::Column<'_>,
        operator: &ComparisonOperator,
        wanted: &Value,
        rows: usize,
        whole: bool,
        chosen: &mut Vec<u32>,
    ) -> bool {
        use crate::storage::{Column, ColumnData};
        use crate::value_semantics::matches;
        // A row with no value there is null.
        let without = matches(&Value::Null, operator, wanted);
        // Against null, any value answers alike, whatever its kind.
        let against_null = wanted.is_null().then(|| matches(&Value::Boolean(true), operator, wanted));
        let mut keep = |passes: &dyn Fn(usize) -> bool| {
            let judge = |row: usize| match column.has_value(row) {
                false => without,
                true => against_null.unwrap_or_else(|| passes(row)),
            };
            if whole {
                chosen.clear();
                chosen.extend((0..rows as u32).filter(|row| judge(*row as usize)));
            } else {
                chosen.retain(|row| judge(*row as usize));
            }
        };
        match column.data() {
            ColumnData::Integers(values) => match wanted {
                // The commonest case gets a loop with nothing in it but
                // the comparison.
                Value::Integer(wanted) => {
                    let wanted = *wanted;
                    match operator {
                        ComparisonOperator::Equal => keep(&|row| values[row] == wanted),
                        ComparisonOperator::NotEqual => keep(&|row| values[row] != wanted),
                        ComparisonOperator::LessThan => keep(&|row| values[row] < wanted),
                        ComparisonOperator::LessEqual => keep(&|row| values[row] <= wanted),
                        ComparisonOperator::GreaterThan => keep(&|row| values[row] > wanted),
                        ComparisonOperator::GreaterEqual => keep(&|row| values[row] >= wanted),
                        _ => keep(&|row| matches(&Value::Integer(values[row]), operator, &Value::Integer(wanted))),
                    }
                }
                _ => keep(&|row| matches(&Value::Integer(values[row]), operator, wanted)),
            },
            ColumnData::Floats(values) => keep(&|row| matches(&Value::Float(values[row]), operator, wanted)),
            ColumnData::Booleans(values) => {
                keep(&|row| matches(&Value::Boolean(values[row] != 0), operator, wanted))
            }
            ColumnData::DateTimes(values) => {
                keep(&|row| matches(&Value::DateTime(values[row]), operator, wanted))
            }
            ColumnData::Strings { offsets, bytes } => match wanted {
                Value::String(text) if Self::text_matches("", operator, text).is_some() => keep(&|row| {
                    Self::text_matches(Column::text(offsets, bytes, row), operator, text).unwrap_or(false)
                }),
                _ => keep(&|row| {
                    matches(&Value::String(Column::text(offsets, bytes, row).to_string()), operator, wanted)
                }),
            },
            // Which row a reference leads to is not in the column; only
            // whether there is one.
            ColumnData::References(_) => {
                if against_null.is_none() {
                    return false;
                }
                keep(&|_| false)
            }
        }
        true
    }

    /// `collection.where { .. }.count()` over rows the store holds decoded:
    /// each is judged where it is and none is copied. `None` for any other
    /// query, or when the store does not hold every row.
    fn count_of_held_rows(&self, query: &OQLQuery) -> OQLResult<Option<Vec<Document>>> {
        let Some((Operation::Count, filters)) = query.operations.split_last() else {
            return Ok(None);
        };
        if filters.is_empty() || !filters.iter().all(|operation| matches!(operation, Operation::Where(_))) {
            return Ok(None);
        }
        Ok(self
            .rows_by_reference(query)?
            .filter(|(_, done)| *done == query.operations.len())
            .map(|(rows, _)| rows))
    }

    /// Run a query's leading filters, and the operation after them when it
    /// is a count, a path step or a sort with a limit, against the stored
    /// rows themselves. Only rows that survive are copied, and for a count
    /// none are. Returns the rows and how many operations they account for.
    fn rows_by_reference(&self, query: &OQLQuery) -> OQLResult<Option<(Vec<Document>, usize)>> {
        let operations = &query.operations;
        let conditions: Vec<&Condition> = operations
            .iter()
            .map_while(|operation| match operation {
                Operation::Where(block) => Some(&block.condition),
                _ => None,
            })
            .collect();
        let filters = conditions.len();
        let next = operations.get(filters);
        // A sort whose result is cut to a few rows: with or without an
        // offset before the limit.
        let cut = match (next, operations.get(filters + 1), operations.get(filters + 2)) {
            (Some(Operation::Sort(sort)), Some(Operation::Limit(limit)), _) => {
                Some((sort, 0usize, *limit as usize, 2))
            }
            (Some(Operation::Sort(sort)), Some(Operation::Offset(offset)), Some(Operation::Limit(limit))) => {
                Some((sort, *offset as usize, *limit as usize, 3))
            }
            _ => None,
        };
        if filters == 0 && cut.is_none() && !matches!(next, Some(Operation::Step(_))) {
            return Ok(None);
        }
        // A row that is kept is copied, down to the fields the query reads
        // when it names them all.
        let wanted = self.fields_read_by(query);
        let keep = |row: &Document| match &wanted {
            Some(fields) => row.with_projected_fields(
                fields
                    .iter()
                    .filter_map(|field| Some((field.clone(), row.fields.get(field)?.clone())))
                    .collect(),
            ),
            None => row.clone(),
        };
        // The filters are the same for every row, so what can be settled
        // about them is settled once: the parts joined by `&&`, and for a
        // field against a constant, the field's name and the constant.
        let mut parts: Vec<&Condition> = Vec::new();
        for condition in &conditions {
            let mut pending = vec![*condition];
            while let Some(next) = pending.pop() {
                match next {
                    Condition::And(left, right) => {
                        pending.push(right);
                        pending.push(left);
                    }
                    other => parts.push(other),
                }
            }
        }
        // A field the schema declares, when a row does not hold it, is null.
        let declared = self.schema_of(&query.collection)?;
        let prepared: Vec<(
            &Condition,
            Option<(crate::fields::FieldReader, &ComparisonOperator, Value)>,
        )> = parts
            .into_iter()
            .map(|part| {
                let plain = match part {
                    Condition::Comparison {
                        left: Expression::FieldAccess(access),
                        operator,
                        right: Expression::Literal(literal),
                    } => match access.path.as_slice() {
                        [name] if DocumentSystemField::from_name(name).is_none() => Some((
                            crate::fields::FieldReader::new(name),
                            operator,
                            self.literal_to_value(literal),
                        )),
                        _ => None,
                    },
                    _ => None,
                };
                (part, plain)
            })
            .collect();
        // The parts already decided for the rows in hand, by their columns.
        let decided = std::cell::RefCell::new(vec![false; prepared.len()]);
        let passes = |row: &Document| -> OQLResult<bool> {
            let decided = decided.borrow();
            for ((condition, plain), _) in prepared.iter().zip(decided.iter()).filter(|(_, done)| !**done) {
                // The same answers as `evaluate_condition` gives for a
                // field compared where it lies.
                let settled = plain.as_ref().and_then(|(reader, operator, wanted)| {
                    let stored = match reader.read(&row.fields) {
                        Some(stored) => stored,
                        None if declared
                            .as_ref()
                            .is_some_and(|schema| schema.fields.contains_key(reader.name())) =>
                        {
                            &Value::Null
                        }
                        None => return None,
                    };
                    match (stored, wanted) {
                        (Value::String(text), Value::String(wanted)) => {
                            Self::text_matches(text, operator, wanted)
                        }
                        (
                            stored @ (Value::String(_)
                            | Value::Integer(_)
                            | Value::Float(_)
                            | Value::Boolean(_)
                            | Value::DateTime(_)
                            | Value::Null),
                            wanted,
                        ) => Some(crate::value_semantics::matches(stored, operator, wanted)),
                        (stored @ Value::Reference(_), Value::Null) => {
                            Some(crate::value_semantics::matches(stored, operator, wanted))
                        }
                        _ => None,
                    }
                });
                let passed = match settled {
                    Some(passed) => passed,
                    None => self.evaluate_condition(row, condition)?,
                };
                if !passed {
                    return Ok(false);
                }
            }
            Ok(true)
        };
        // The first rows of a sort through a reference are found without
        // reading the collection.
        if let Some((sort, offset, limit, taken)) = cut {
            if let Some(total) = self.store.collection_len(&query.collection)? {
                if let Some(first) = self.first_through_reference(
                    &query.collection,
                    sort,
                    offset.saturating_add(limit),
                    total,
                    &passes,
                )? {
                    let kept = first.iter().skip(offset).map(|row| keep(row)).collect();
                    return Ok(Some((kept, filters + taken)));
                }
            }
        }
        // The parts of the filter that are a field against a constant are
        // decided from the fields' columns, a pass over each, and only the
        // rows they leave are read for anything else.
        let names: Vec<String> = {
            let mut names: Vec<String> = Vec::new();
            for (_, plain) in &prepared {
                if let Some((reader, _, _)) = plain {
                    if !names.iter().any(|known| known == reader.name()) {
                        names.push(reader.name().to_string());
                    }
                }
            }
            names
        };
        // With no such part, a sort that is cut short still wants the rows
        // listed as its columns are, so that it can sort by them.
        let asked: Vec<String> = match (names.is_empty(), cut) {
            (true, Some((sort, ..))) => Self::plain_sort_fields(sort).unwrap_or_default(),
            _ => names.clone(),
        };
        // The collection's rows as the columns list them; which of them
        // are still in the running; and what the columns were stamped.
        let mut listed: Option<(Arc<Vec<Arc<Document>>>, u64)> = None;
        let mut origin: Option<Vec<u32>> = None;
        if !asked.is_empty() && COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            if let Some((rows, batch)) = self.store.scan_rows_and_columns(&query.collection, &asked)? {
                let mut chosen: Vec<u32> = Vec::new();
                let mut whole = true;
                for (part, (_, plain)) in prepared.iter().enumerate() {
                    let Some((reader, operator, wanted)) = plain else {
                        continue;
                    };
                    let Some(field) = names.iter().position(|known| known == reader.name()) else {
                        continue;
                    };
                    if Self::narrow_by_column(batch.column(field), operator, wanted, batch.rows(), whole, &mut chosen)
                    {
                        decided.borrow_mut()[part] = true;
                        whole = false;
                    }
                }
                COLUMN_SCANS_RUN.with(|runs| runs.set(runs.get() + 1));
                if !whole {
                    origin = Some(chosen);
                }
                listed = Some((rows, batch.stamp()));
            }
        }
        // The rows to go on with: those the columns left, or every row.
        let narrowed: Vec<Arc<Document>>;
        let stored: &[Arc<Document>] = match (&listed, &origin) {
            (Some((rows, _)), Some(origin)) => {
                narrowed = origin.iter().map(|row| rows[*row as usize].clone()).collect();
                &narrowed
            }
            (Some((rows, _)), None) => rows.as_slice(),
            (None, _) => match self.store.scan_documents_shared(&query.collection)? {
                Some(rows) => {
                    narrowed = rows;
                    &narrowed
                }
                None => return Ok(None),
            },
        };
        match (next, operations.get(filters + 1)) {
            (Some(Operation::Count), None) => {
                let mut count = 0usize;
                for row in stored.iter() {
                    count += usize::from(passes(row)?);
                }
                Ok(Some((
                    self.apply_count_operation(Vec::new()).map(|mut rows| {
                        rows[0]
                            .fields
                            .insert("count".to_string(), Value::Integer(count as i64));
                        rows
                    })?,
                    operations.len(),
                )))
            }
            (Some(Operation::Step(path)), _) if Self::step_keeping_origin(query).is_none() => {
                // What follows the step says which fields of the rows it
                // reaches are read.
                let read = self.fields_read_by_operations(&query.collection, &operations[filters + 1..]);
                let mut reached = Vec::new();
                for row in stored.iter() {
                    if passes(row)? {
                        reached.extend(
                            self.path_source_rows_reading(row, &path.path, read.as_deref())?.0,
                        );
                    }
                }
                Ok(Some((reached, filters + 1)))
            }
            // A costly filter ahead of a sort on plain fields and a limit:
            // putting the rows in order first means the filter runs only
            // until enough rows have passed it.
            (Some(Operation::Sort(sort)), Some(Operation::Limit(limit)))
                if sort.fields.iter().all(|field| {
                    matches!(&field.expression, Expression::FieldAccess(access) if access.path.len() == 1)
                }) && conditions.iter().any(|condition| Self::condition_is_nested(condition)) =>
            {
                // The keys are read where the rows hold them; a copy of
                // every row's keys costs more than the sort does.
                let lent: Vec<&Arc<Document>> = stored.iter().collect();
                let mut kept = Vec::new();
                for position in self.sorted_positions_of_stored(&lent, sort, None) {
                    if kept.len() as u64 >= *limit {
                        break;
                    }
                    if passes(&stored[position])? {
                        kept.push(keep(&stored[position]));
                    }
                }
                Ok(Some((kept, filters + 2)))
            }
            // Any other sort that is cut short: the rows are put in order
            // by their keys alone, and only those kept are copied.
            _ if cut.is_some() => {
                let (sort, offset, limit, taken) = cut.expect("checked just above");
                let wanted_rows = offset.saturating_add(limit);
                let mut passing = Vec::new();
                // Where each passing row is among the rows as listed.
                let mut places: Vec<u32> = Vec::new();
                for (at, row) in stored.iter().enumerate() {
                    if passes(row)? {
                        passing.push(row);
                        places.push(origin.as_ref().map_or(at as u32, |origin| origin[at]));
                    }
                }
                // By the sort fields' columns when they can be had for
                // these same rows; by the rows otherwise.
                let by_columns = match &listed {
                    Some((_, stamp)) => self.sorted_positions_by_columns(
                        &query.collection,
                        sort,
                        *stamp,
                        &passing,
                        &places,
                        Some(wanted_rows),
                    )?,
                    None => None,
                };
                let order = match by_columns {
                    Some(order) => order,
                    None => self.sorted_positions_of_stored(&passing, sort, Some(wanted_rows)),
                };
                let kept = order
                    .into_iter()
                    .skip(offset)
                    .map(|position| keep(passing[position]))
                    .collect();
                Ok(Some((kept, filters + taken)))
            }
            _ => {
                let mut kept = Vec::new();
                for row in stored.iter() {
                    if passes(row)? {
                        kept.push(keep(row));
                    }
                }
                Ok(Some((kept, filters)))
            }
        }
    }

    /// Whether a condition runs a query or walks a collection for each row
    /// it is asked about, which makes it the costly part of a query.
    fn condition_is_nested(condition: &Condition) -> bool {
        match condition {
            Condition::Comparison { left, right, .. } => {
                Self::expression_is_nested(left) || Self::expression_is_nested(right)
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                Self::condition_is_nested(left) || Self::condition_is_nested(right)
            }
            Condition::Not(inner) => Self::condition_is_nested(inner),
        }
    }

    /// [`Self::condition_is_nested`] for an expression.
    fn expression_is_nested(expression: &Expression) -> bool {
        match expression {
            Expression::QueryExpression { .. } | Expression::CollectionExpression { .. } => true,
            Expression::FunctionCall { args, .. } => args.iter().any(Self::expression_is_nested),
            Expression::IfThenElse { condition, then_expr, else_expr } => {
                Self::condition_is_nested(condition)
                    || Self::expression_is_nested(then_expr)
                    || Self::expression_is_nested(else_expr)
            }
            Expression::LogicalAnd(left, right)
            | Expression::LogicalOr(left, right)
            | Expression::BinaryOp { left, right, .. } => {
                Self::expression_is_nested(left) || Self::expression_is_nested(right)
            }
            Expression::LogicalNot(inner) => Self::expression_is_nested(inner),
            Expression::ConditionExpr(condition) => Self::condition_is_nested(condition),
            Expression::ArrayLiteral(items) | Expression::Block(items) => {
                items.iter().any(Self::expression_is_nested)
            }
            Expression::ObjectLiteral(fields) => fields
                .iter()
                .any(|field| Self::expression_is_nested(&field.value)),
            _ => false,
        }
    }

    /// Operations no shortcut understands: they are applied to rows in
    /// hand, in the order written.
    fn runs_plainly(operation: &Operation) -> bool {
        match operation {
            Operation::Combine { .. } | Operation::Walk { .. } | Operation::PartitionBy(_) => true,
            // A function written in OQL, anywhere in the chain.
            Operation::AggregateExpr { function, .. } => !Self::is_aggregate_name(function),
            Operation::Select(block) => block
                .fields
                .iter()
                .any(|field| Self::expression_uses_position(&field.expression)),
            _ => false,
        }
    }

    /// How many nested queries this thread has actually executed, as
    /// opposed to answered from an identical one earlier in the same
    /// statement. Intended for tests and benchmarks.
    pub fn nested_queries_run() -> u64 {
        NESTED_QUERIES_RUN.with(std::cell::Cell::get)
    }

    /// A collection's schema, read once per statement.
    fn schema_of(&self, name: &str) -> OQLResult<Option<Arc<Schema>>> {
        if !StatementGuard::active() {
            return Ok(self.store.get_schema(name)?.map(Arc::new));
        }
        if let Some(known) = STATEMENT.with(|scope| scope.borrow().schemas.get(name).cloned()) {
            return Ok(known);
        }
        let schema = self.store.get_schema(name)?.map(Arc::new);
        STATEMENT.with(|scope| {
            scope
                .borrow_mut()
                .schemas
                .insert(name.to_string(), schema.clone())
        });
        Ok(schema)
    }

    /// The names of every collection with a schema, read once per statement.
    fn schema_names(&self) -> OQLResult<Arc<Vec<String>>> {
        if !StatementGuard::active() {
            return Ok(Arc::new(self.store.list_schemas()?));
        }
        if let Some(known) = STATEMENT.with(|scope| scope.borrow().schema_names.clone()) {
            return Ok(known);
        }
        let names = Arc::new(self.store.list_schemas()?);
        STATEMENT.with(|scope| scope.borrow_mut().schema_names = Some(names.clone()));
        Ok(names)
    }

    /// `query.explain()`: one row saying how the query would run, without
    /// running it.
    fn explain_query(&self, query: &OQLQuery) -> OQLResult<Vec<Document>> {
        self.validate_declared_paths(query)?;
        let plan = self.try_plan_safe_read(query)?;
        let mut fields = crate::document::Fields::default();
        match &plan {
            Some(plan) => {
                fields.insert(
                    "path".to_string(),
                    Value::String("physical pipeline".to_string()),
                );
                fields.insert(
                    "uses_index".to_string(),
                    Value::Boolean(Self::contains_native_index_scan(&plan.root)),
                );
                fields.insert("plan".to_string(), Value::String(plan.root.explain()));
            }
            None => {
                fields.insert(
                    "path".to_string(),
                    Value::String("general executor".to_string()),
                );
                // The general executor still reads its rows through an
                // index when its leading filters allow one.
                let ranged = self.pattern_ranges(query)?;
                let sourced = ranged.as_ref().unwrap_or(query);
                let reordered = Self::storage_filter_first(sourced);
                let sourced = reordered.as_ref().unwrap_or(sourced);
                let index_plan = self
                    .pipeline_prefix(sourced)?
                    .filter(|(plan, _)| Self::contains_native_index_scan(&plan.root));
                fields.insert(
                    "uses_index".to_string(),
                    Value::Boolean(index_plan.is_some()),
                );
                fields.insert(
                    "plan".to_string(),
                    Value::String(match index_plan {
                        Some((plan, applied)) => format!(
                            "read {} through an index for its first {} filter(s), then apply the \
                             remaining operations in order\n{}",
                            query.collection,
                            applied,
                            plan.root.explain()
                        ),
                        None => format!(
                            "scan {} and apply {} operation(s) in order",
                            query.collection,
                            query.operations.len()
                        ),
                    }),
                );
            }
        }
        Ok(vec![Document::with_id(
            "explain".to_string(),
            "explain".to_string(),
            fields,
        )])
    }

    /// Reject a path whose first name the collection's schema does not
    /// know. With a declared shape, `.nmae` cannot be a missing value; it is
    /// a mistake, and reading it as null would hide that.
    fn validate_declared_paths(&self, query: &OQLQuery) -> OQLResult<()> {
        // A function's body run for each of many rows was checked once,
        // before the first.
        if PATHS_CHECKED.with(|checked| checked.get()) {
            return Ok(());
        }
        let Some(schema) = self.schema_of(&query.collection)? else {
            return Ok(());
        };
        // Paths are checked up to the first operation that reshapes rows;
        // after it, names refer to what that operation produced.
        let mut checked = Vec::new();
        let mut direct: Vec<String> = Vec::new();
        for operation in &query.operations {
            match operation {
                Operation::Where(_) | Operation::Sort(_) => checked.push(operation),
                // These carry their path directly rather than as an
                // expression.
                Operation::Sum(field)
                | Operation::Avg(field)
                | Operation::Min(field)
                | Operation::Max(field) => {
                    direct.extend(field.path.first().cloned());
                    break;
                }
                Operation::GroupBy(block) => {
                    direct.extend(block.fields.iter().filter_map(|field| field.path.first().cloned()));
                    checked.push(operation);
                    break;
                }
                Operation::Select(_) | Operation::AggregateExpr { .. } => {
                    checked.push(operation);
                    break;
                }
                Operation::Step(path) => {
                    direct.extend(path.path.first().cloned());
                    break;
                }
                Operation::Limit(_)
                | Operation::Offset(_)
                | Operation::LimitExpr(_)
                | Operation::OffsetExpr(_)
                | Operation::First
                | Operation::Distinct => {}
                _ => break,
            }
        }

        // Paths inside a nested collection or query belong to its rows, so
        // only a nested expression's source is in this scope.
        fn first_names(node: &serde_json::Value, names: &mut Vec<String>) {
            match node {
                serde_json::Value::Object(fields) => {
                    if let Some(path) = fields
                        .get("FieldAccess")
                        .and_then(|access| access.get("path"))
                        .and_then(|path| path.as_array())
                    {
                        if let Some(first) = path.first().and_then(|name| name.as_str()) {
                            names.push(first.to_string());
                        }
                    }
                    for (key, value) in fields {
                        match key.as_str() {
                            "QueryExpression" => {}
                            "CollectionExpression" => {
                                if let Some(source) = value.get("source") {
                                    first_names(source, names);
                                }
                            }
                            _ => first_names(value, names),
                        }
                    }
                }
                serde_json::Value::Array(items) => {
                    items.iter().for_each(|item| first_names(item, names))
                }
                _ => {}
            }
        }
        let mut names = direct;
        for operation in checked {
            if let Ok(tree) = serde_json::to_value(operation) {
                first_names(&tree, &mut names);
            }
        }

        for name in names {
            if name == "*"
                || name.starts_with('_')
                || schema.fields.contains_key(&name)
                || DocumentSystemField::from_name(&name).is_some()
            {
                continue;
            }
            if matches!(
                self.reverse_relationship(&schema.name, &name)?,
                ReverseRelationship::Unknown
            ) {
                let closest = schema
                    .fields
                    .keys()
                    .map(|field| (Self::edit_distance(field, &name), field))
                    .filter(|(distance, _)| *distance <= 2)
                    .min()
                    .map(|(_, field)| format!(". Did you mean '{field}'?"))
                    .unwrap_or_default();
                return Err(OQLError::SchemaValidation {
                    message: format!(
                        "'{}' has no field or relationship '{}'{}",
                        schema.name, name, closest
                    ),
                });
            }
        }
        // The names above are the first of each path in the query's own
        // scope. Every later name, and every name inside a nested filter or
        // projection, is checked by following the schema to where it is
        // read. Once for the statement: a nested query is part of it.
        if STATEMENT.with(|scope| scope.borrow().depth) <= 1 {
            self.check_operations(Place::Rows(schema), &query.operations)?;
        }
        Ok(())
    }

    /// Where one more name of a path leads from `place`, or the error for a
    /// name that the declared shape there does not have.
    fn step_place(&self, place: &Place, name: &str) -> OQLResult<Place> {
        let unknown = |owner: &str, names: Vec<&String>, kind: &str| {
            let closest = names
                .into_iter()
                .map(|field| (Self::edit_distance(field, name), field))
                .filter(|(distance, _)| *distance <= 2)
                .min()
                .map(|(_, field)| format!(". Did you mean '{field}'?"))
                .unwrap_or_default();
            OQLError::SchemaValidation {
                message: format!("'{owner}' has no {kind} '{name}'{closest}"),
            }
        };
        // What a declared type holds: rows of a collection, an object of a
        // declared shape, or something whose inside is not declared.
        let held = |declared: &FieldType, schema: &Arc<Schema>, path: String| -> OQLResult<Place> {
            let mut declared = declared;
            let mut path = path;
            loop {
                match declared {
                    FieldType::Optional(inner) => declared = inner,
                    FieldType::List(inner) => {
                        declared = inner;
                        path.push_str("[]");
                    }
                    FieldType::Reference(reference) => {
                        return Ok(match self.schema_of(&reference.target_schema)? {
                            Some(target) => Place::Rows(target),
                            None => Place::Any,
                        })
                    }
                    FieldType::Object => {
                        return Ok(match schema.shape(&path) {
                            Some(fields) => Place::Shape {
                                schema: schema.clone(),
                                path,
                                fields: fields.to_vec(),
                            },
                            None => Place::Any,
                        })
                    }
                    _ => return Ok(Place::Any),
                }
            }
        };
        if name == "*" || name.starts_with('_') || name.starts_with('$') {
            return Ok(Place::Any);
        }
        match place {
            Place::Any => Ok(Place::Any),
            Place::Rows(schema) => {
                if DocumentSystemField::from_name(name).is_some() {
                    return Ok(Place::Any);
                }
                if let Some(declared) = schema.fields.get(name) {
                    return held(declared, schema, name.to_string());
                }
                match self.reverse_relationship(&schema.name, name)? {
                    ReverseRelationship::Declared { source, .. } => Ok(match self.schema_of(&source)? {
                        Some(source) => Place::Rows(source),
                        None => Place::Any,
                    }),
                    ReverseRelationship::Unknown => Err(unknown(
                        &schema.name,
                        schema.fields.keys().collect(),
                        "field or relationship",
                    )),
                    _ => Ok(Place::Any),
                }
            }
            Place::Shaped { names, rows } => {
                if names.iter().any(|field| field == name)
                    || DocumentSystemField::from_name(name).is_some()
                {
                    return Ok(Place::Any);
                }
                if let Some(schema) = rows {
                    if let ReverseRelationship::Declared { source, .. } =
                        self.reverse_relationship(&schema.name, name)?
                    {
                        return Ok(match self.schema_of(&source)? {
                            Some(source) => Place::Rows(source),
                            None => Place::Any,
                        });
                    }
                }
                Err(unknown("the select before it", names.iter().collect(), "field"))
            }
            Place::Shape { schema, path, fields } => {
                match fields.iter().find(|(field, _)| field == name) {
                    Some((_, declared)) => held(declared, schema, format!("{path}.{name}")),
                    None => Err(unknown(
                        &format!("{}.{}", schema.name, path.trim_end_matches("[]")),
                        fields.iter().map(|(field, _)| field).collect(),
                        "field",
                    )),
                }
            }
        }
    }

    fn path_place(&self, place: &Place, path: &[String]) -> OQLResult<Place> {
        let mut place = place.clone();
        for name in path {
            if matches!(place, Place::Any) {
                break;
            }
            place = self.step_place(&place, name)?;
        }
        Ok(place)
    }

    /// Check every path of a chain of operations against the declared shape
    /// of the rows each operation is given.
    fn check_operations(&self, place: Place, operations: &[Operation]) -> OQLResult<()> {
        let mut place = place;
        for operation in operations {
            match operation {
                Operation::Where(block) => self.check_condition(&place, &block.condition)?,
                Operation::Having(block) => self.check_condition(&place, &block.condition)?,
                Operation::Sort(block) => {
                    for field in &block.fields {
                        self.check_expression(&place, &field.expression)?;
                    }
                }
                Operation::AggregateExpr { input, .. } => self.check_expression(&place, input)?,
                Operation::Sum(field)
                | Operation::Avg(field)
                | Operation::Min(field)
                | Operation::Max(field) => {
                    self.path_place(&place, &field.path)?;
                }
                // What a select or a grouping produces is named by the
                // query, not by a schema.
                Operation::Select(block) => {
                    for field in &block.fields {
                        self.check_expression(&place, &field.expression)?;
                    }
                    // What follows reads the fields this select named.
                    // With `*`, or a name that cannot be read off the
                    // text, nothing can be ruled out.
                    let names: Option<Vec<String>> = block
                        .fields
                        .iter()
                        .map(|field| match (&field.alias, &field.expression) {
                            (_, Expression::Identifier(marker))
                                if marker == crate::parser::OMITTED_FIELD =>
                            {
                                None
                            }
                            (Some(alias), _) => Some(alias.clone()),
                            (None, Expression::FieldAccess(access))
                                if !Self::is_all_fields(&field.expression) =>
                            {
                                access.path.last().cloned()
                            }
                            (None, Expression::Identifier(name)) => Some(name.clone()),
                            (None, Expression::FunctionCall { name, .. }) => Some(name.clone()),
                            _ => None,
                        })
                        .collect();
                    let aggregates = block
                        .fields
                        .iter()
                        .any(|field| Self::expression_contains_aggregate(&field.expression));
                    place = match (names, aggregates, &place) {
                        (Some(names), false, Place::Rows(schema)) => Place::Shaped {
                            names,
                            rows: Some(schema.clone()),
                        },
                        (Some(names), _, _) => Place::Shaped { names, rows: None },
                        _ => Place::Any,
                    };
                }
                Operation::GroupBy(block) | Operation::PartitionBy(block) => {
                    for field in &block.fields {
                        self.path_place(&place, &field.path)?;
                    }
                    for key in &block.computed {
                        self.check_expression(&place, &key.expression)?;
                    }
                    if matches!(operation, Operation::GroupBy(_)) {
                        place = Place::Any;
                    }
                }
                Operation::Step(path) | Operation::Walk { path, .. } => {
                    place = self.path_place(&place, &path.path)?;
                }
                Operation::Combine { other, .. } => self.check_expression(&Place::Any, other)?,
                Operation::Limit(_)
                | Operation::Offset(_)
                | Operation::LimitExpr(_)
                | Operation::OffsetExpr(_)
                | Operation::Distinct
                | Operation::First
                | Operation::Exists
                | Operation::Count
                | Operation::Explain => {}
                // A write, or anything else, is checked where it is done.
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    fn check_condition(&self, place: &Place, condition: &Condition) -> OQLResult<()> {
        match condition {
            Condition::Comparison { left, operator, right } => {
                self.check_expression(place, left)?;
                self.check_expression(place, right)?;
                // A field declared as a list, compared with one value. It
                // would be false for every row as a mismatch of types, and
                // what was almost certainly meant is `contains`.
                let one_value = |expression: &Expression| {
                    matches!(
                        expression,
                        Expression::Literal(
                            Literal::String(_)
                                | Literal::Integer(_)
                                | Literal::Float(_)
                                | Literal::Boolean(_)
                                | Literal::Timestamp(_)
                        )
                    )
                };
                let compares = matches!(
                    operator,
                    ComparisonOperator::Equal
                        | ComparisonOperator::NotEqual
                        | ComparisonOperator::LessThan
                        | ComparisonOperator::LessEqual
                        | ComparisonOperator::GreaterThan
                        | ComparisonOperator::GreaterEqual
                );
                for (list, value) in [(left, right), (right, left)] {
                    let Expression::FieldAccess(access) = list else {
                        continue;
                    };
                    if compares && one_value(value) && self.declared_a_list(place, &access.path)? {
                        return Err(Self::list_compared_with_one(access, value, operator));
                    }
                }
                Ok(())
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                self.check_condition(place, left)?;
                self.check_condition(place, right)
            }
            Condition::Not(inner) => self.check_condition(place, inner),
        }
    }

    /// Whether a path ends at a field the schema declares as a list.
    fn declared_a_list(&self, place: &Place, path: &[String]) -> OQLResult<bool> {
        let Some((last, before)) = path.split_last() else {
            return Ok(false);
        };
        let is_list = |declared: Option<&FieldType>| {
            let mut declared = declared;
            while let Some(FieldType::Optional(inner)) = declared {
                declared = Some(inner);
            }
            matches!(declared, Some(FieldType::List(_)))
        };
        Ok(match self.path_place(place, before)? {
            Place::Rows(schema) => is_list(schema.fields.get(last)),
            Place::Shape { fields, .. } => {
                is_list(fields.iter().find(|(name, _)| name == last).map(|(_, declared)| declared))
            }
            Place::Shaped { .. } | Place::Any => false,
        })
    }

    /// The error for comparing a field that is a list with one value.
    fn list_compared_with_one(
        list: &FieldAccess,
        value: &Expression,
        operator: &ComparisonOperator,
    ) -> OQLError {
        let written = format!(".{}", list.path.join("."));
        let symbol = match operator {
            ComparisonOperator::Equal => "==",
            ComparisonOperator::NotEqual => "!=",
            ComparisonOperator::LessThan => "<",
            ComparisonOperator::LessEqual => "<=",
            ComparisonOperator::GreaterThan => ">",
            _ => ">=",
        };
        let value = match value {
            Expression::Literal(Literal::String(text)) => format!("\"{text}\""),
            Expression::Literal(Literal::Integer(number)) => number.to_string(),
            Expression::Literal(Literal::Float(number)) => number.to_string(),
            Expression::Literal(Literal::Boolean(flag)) => flag.to_string(),
            _ => "value".to_string(),
        };
        let advice = match operator {
            ComparisonOperator::Equal => format!(
                "To ask whether it holds {value}, write {written} contains {value}. To ask \
                 whether it is exactly that one value, write {written} == [{value}]."
            ),
            ComparisonOperator::NotEqual => format!(
                "To ask whether it lacks {value}, write !({written} contains {value})."
            ),
            _ => format!(
                "To ask whether any element does, write {written}.where {{ . {symbol} {value} }}.exists()."
            ),
        };
        OQLError::FunctionError {
            message: format!(
                "{written} is a list, so {symbol} cannot compare it with one value. {advice}"
            ),
        }
    }

    fn check_expression(&self, place: &Place, expression: &Expression) -> OQLResult<()> {
        let check = |node: &Expression| self.check_expression(place, node);
        match expression {
            Expression::FieldAccess(access) => self.path_place(place, &access.path).map(|_| ()),
            // The operations of a nested collection are given its rows.
            Expression::CollectionExpression { source, operations } => {
                let rows = match source.as_ref() {
                    Expression::FieldAccess(access) => self.path_place(place, &access.path)?,
                    Expression::Identifier(name) if name == crate::parser::ROWS_SO_FAR => {
                        place.clone()
                    }
                    other => {
                        check(other)?;
                        Place::Any
                    }
                };
                self.check_operations(rows, operations)
            }
            Expression::QueryExpression { query } => {
                let rows = match self.schema_of(&query.collection)? {
                    Some(schema) => Place::Rows(schema),
                    None => Place::Any,
                };
                self.check_operations(rows, &query.operations)
            }
            // The existence tests ask about a path that may not be there.
            Expression::FunctionCall { name, .. }
                if matches!(name.as_str(), "exists" | "is_null" | "is_missing" | "is_broken_ref") =>
            {
                Ok(())
            }
            Expression::FunctionCall { args, .. } => args.iter().try_for_each(check),
            Expression::IfThenElse { condition, then_expr, else_expr } => {
                self.check_condition(place, condition)?;
                check(then_expr)?;
                check(else_expr)
            }
            Expression::Match { expr, arms } => {
                check(expr)?;
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.check_condition(place, guard)?;
                    }
                    check(&arm.body)?;
                }
                Ok(())
            }
            Expression::LogicalAnd(left, right)
            | Expression::LogicalOr(left, right)
            | Expression::BinaryOp { left, right, .. } => {
                check(left)?;
                check(right)
            }
            Expression::LogicalNot(inner) => check(inner),
            Expression::ArrayAccess { array, index } => {
                check(array)?;
                check(index)
            }
            Expression::ChainedAccess { base, .. } => check(base),
            Expression::Block(items) | Expression::ArrayLiteral(items) => {
                items.iter().try_for_each(check)
            }
            Expression::ObjectLiteral(fields) => {
                fields.iter().try_for_each(|field| check(&field.value))
            }
            Expression::ConditionExpr(condition) => self.check_condition(place, condition),
            Expression::Identifier(_) | Expression::Literal(_) => Ok(()),
        }
    }

    fn edit_distance(left: &str, right: &str) -> usize {
        let right: Vec<char> = right.chars().collect();
        let mut row: Vec<usize> = (0..=right.len()).collect();
        for (i, a) in left.chars().enumerate() {
            let mut previous = row[0];
            row[0] = i + 1;
            for (j, b) in right.iter().enumerate() {
                let cost = previous + usize::from(a != *b);
                previous = row[j + 1];
                row[j + 1] = cost.min(row[j] + 1).min(previous + 1);
            }
        }
        row[right.len()]
    }

    /// Answer a filter-then-aggregate query by folding storage's column
    /// batches, without building a row object for each input row.
    ///
    /// Covers `count()`, a terminal aggregate, an aggregate-only `select`,
    /// and `group_by` followed by an optional `having` and a `select` of
    /// keys and aggregates. Filters storage can evaluate are pushed into the
    /// scan. Everything else (any other filter, a computed or referenced
    /// group key, an aggregate of an expression, an aggregate over a
    /// filtered group) is evaluated by the general evaluator against one
    /// reused row, and only when `with_row_evaluation` allows it: the caller
    /// first tries the ways of reading fewer rows.
    ///
    /// Anything else returns `None` and takes the general path, which
    /// defines the semantics; this path must produce exactly the same rows.
    fn aggregate_from_columns(
        &self,
        query: &OQLQuery,
        with_row_evaluation: bool,
    ) -> OQLResult<Option<Vec<Document>>> {
        if !COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            return Ok(None);
        }
        let leading = query
            .operations
            .iter()
            .take_while(|operation| matches!(operation, Operation::Where(_)))
            .count();
        let rest = &query.operations[leading..];
        if !matches!(
            rest,
            [Operation::Count
                | Operation::Sum(_)
                | Operation::Avg(_)
                | Operation::Min(_)
                | Operation::Max(_)
                | Operation::AggregateExpr { .. }
                | Operation::Select(_), ..]
                | [Operation::GroupBy(_), Operation::Select(_) | Operation::Having(_), ..]
        ) {
            return Ok(None);
        }
        let Some(schema) = self.schema_of(&query.collection)? else {
            return Ok(None);
        };

        enum Shape {
            Terminal(String),
            Select,
            Grouped,
        }
        let mut plan = FoldPlan::default();
        let (shape, consumed) = match rest {
            [Operation::Count, ..] => {
                plan.aggregates.push(FoldAggregate {
                    kind: AggregateKind::Rows,
                    input: RowInput::Rows,
                    filters: Vec::new(),
                });
                plan.outputs.push(("count".into(), FoldOutput::Aggregate(0)));
                (Shape::Terminal("count".into()), 1)
            }
            [Operation::Sum(field) | Operation::Avg(field) | Operation::Min(field) | Operation::Max(field), ..] =>
            {
                let (name, kind) = match &rest[0] {
                    Operation::Sum(_) => ("sum", AggregateKind::Sum),
                    Operation::Avg(_) => ("avg", AggregateKind::Avg),
                    Operation::Min(_) => ("min", AggregateKind::Min),
                    _ => ("max", AggregateKind::Max),
                };
                let Some(input) = plan.input(&schema, &field.path) else {
                    return Ok(None);
                };
                plan.aggregates.push(FoldAggregate {
                    kind,
                    input,
                    filters: Vec::new(),
                });
                plan.outputs.push((name.into(), FoldOutput::Aggregate(0)));
                (Shape::Terminal(name.into()), 1)
            }
            [Operation::AggregateExpr { function, input }, ..] => {
                let Some(kind) = AggregateKind::named(function, false) else {
                    return Ok(None);
                };
                if Self::expression_contains_aggregate(input) {
                    return Ok(None);
                }
                plan.aggregates.push(FoldAggregate {
                    kind,
                    input: RowInput::Expression(input.clone()),
                    filters: Vec::new(),
                });
                plan.outputs.push((function.clone(), FoldOutput::Aggregate(0)));
                (Shape::Terminal(function.clone()), 1)
            }
            [Operation::GroupBy(group), after @ ..] => {
                // The names a group row answers to, as `group_by` assigns
                // them: later assignments of a name win, except that a
                // path's last segment never displaces an earlier name.
                for field in &group.fields {
                    let Some(input) = plan.input(&schema, &field.path) else {
                        return Ok(None);
                    };
                    let position = plan.keys.len();
                    plan.keys.push(input);
                    plan.name_key(field.path.join("_"), position, true);
                    if let Some(last) = field.path.last() {
                        plan.name_key(last.clone(), position, false);
                    }
                }
                for key in &group.computed {
                    if Self::expression_contains_aggregate(&key.expression) {
                        return Ok(None);
                    }
                    let position = plan.keys.len();
                    plan.keys.push(RowInput::Expression(key.expression.clone()));
                    plan.name_key(key.name.clone(), position, true);
                }
                plan.key_names.retain(|(name, _)| {
                    !matches!(name.as_str(), "group_count" | "_group_schema" | "_group_rows")
                        && DocumentSystemField::from_name(name).is_none()
                });
                let (select, consumed) = match after {
                    [Operation::Select(select), ..] => (select, 2),
                    [Operation::Having(having), Operation::Select(select), ..] => {
                        let Some(condition) =
                            self.fold_condition(&mut plan, &schema, &having.condition)
                        else {
                            return Ok(None);
                        };
                        plan.having = Some(condition);
                        (select, 3)
                    }
                    _ => return Ok(None),
                };
                for entry in &select.fields {
                    let Some(output) = Self::fold_output(&mut plan, &schema, entry, true) else {
                        return Ok(None);
                    };
                    plan.outputs.push(output);
                }
                (Shape::Grouped, consumed)
            }
            [Operation::Select(select), ..] => {
                for entry in &select.fields {
                    let Some(output) = Self::fold_output(&mut plan, &schema, entry, false) else {
                        return Ok(None);
                    };
                    plan.outputs.push(output);
                }
                (Shape::Select, 1)
            }
            _ => return Ok(None),
        };

        let (filters, residual) =
            Self::split_storage_filters(&query.collection, &schema, &query.operations[..leading]);
        plan.residual = residual;

        let needs_row = plan.needs_row();
        if needs_row {
            if !with_row_evaluation {
                return Ok(None);
            }
            // The row the evaluator sees carries every field the query
            // reads, exactly as a projected scan would give it.
            let Some(read) = self.fields_read_by(query) else {
                return Ok(None);
            };
            for name in read {
                plan.slot(&schema, &name);
            }
        }
        if self.store.has_pending_writes()? {
            return Ok(None);
        }
        // When nothing asks for the row itself, the fields come from whole
        // columns, and the filters storage would have run are run on the
        // columns first.
        let columns = match needs_row {
            false => self.fold_columns(&query.collection, &plan.fields, &filters)?,
            true => None,
        };
        let batches = match &columns {
            Some(_) => Vec::new(),
            None => match self
                .store
                .scan_projected_batches(&query.collection, &filters, &plan.fields, 4096)?
            {
                Some(batches) => batches,
                None => return Ok(None),
            },
        };

        // One state per group; ungrouped shapes have the single empty key.
        let mut groups: Vec<ColumnGroup> = Vec::new();
        let mut by_hash: HashMap<u64, Vec<usize>> = HashMap::new();
        let new_group = |key: Vec<Value>, atoms: Vec<OwnedKeyAtom>| ColumnGroup {
            key,
            atoms,
            accumulators: plan
                .aggregates
                .iter()
                .map(|aggregate| Accumulator::new(aggregate.kind))
                .collect(),
        };
        if plan.keys.is_empty() {
            groups.push(new_group(Vec::new(), Vec::new()));
        }
        let mut current: Vec<Option<ProjectedValueRef<'_>>> = vec![None; plan.fields.len()];
        let mut scratch =
            Document::with_id(String::new(), query.collection.clone(), crate::document::Fields::default());
        let mut key_values: Vec<Held> = Vec::with_capacity(plan.keys.len());
        let mut memos: Vec<HashMap<ObjRef, usize>> = vec![HashMap::new(); plan.memos];
        let mut remembered: Vec<Value> = Vec::new();
        // What is done with each row once its fields are in hand. `false`
        // when this way of answering cannot go on and the general way must.
        let mut each = |current: &[Option<ProjectedValueRef<'_>>], document: &Document| -> OQLResult<bool> {
            for condition in &plan.residual {
                if !self.evaluate_condition(document, condition)? {
                    return Ok(true);
                }
            }
            key_values.clear();
            for key in &plan.keys {
                key_values.push(match key {
                    RowInput::Rows | RowInput::Slot(_) => Held::Owned(Value::Null),
                    evaluated => {
                        self.fold_input(evaluated, document, &current, &mut memos, &mut remembered)?
                    }
                });
            }
            let group = if plan.keys.is_empty() {
                0
            } else {
                let atom = |position: usize| match (&plan.keys[position], &key_values[position]) {
                    (RowInput::Slot(slot), _) => KeyAtom::of(current[*slot]),
                    (_, Held::Owned(value)) => KeyAtom::of(Some(ProjectedValueRef::Value(value))),
                    (_, Held::Remembered(index)) => {
                        KeyAtom::of(Some(ProjectedValueRef::Value(&remembered[*index])))
                    }
                };
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                for position in 0..plan.keys.len() {
                    match atom(position) {
                        Some(atom) => std::hash::Hash::hash(&atom, &mut hasher),
                        // A list or object as a key: leave it to the
                        // general path.
                        None => return Ok(false),
                    }
                }
                let hash = std::hash::Hasher::finish(&hasher);
                let candidates = by_hash.entry(hash).or_default();
                let found = candidates.iter().copied().find(|index| {
                    groups[*index]
                        .atoms
                        .iter()
                        .enumerate()
                        .all(|(position, owned)| {
                            atom(position).is_some_and(|atom| atom.same(owned))
                        })
                });
                match found {
                    Some(index) => index,
                    None => {
                        let index = groups.len();
                        let key = plan
                            .keys
                            .iter()
                            .zip(&key_values)
                            .map(|(key, held)| match (key, held) {
                                (RowInput::Slot(slot), _) => current[*slot]
                                    .map_or(Value::Null, ProjectedValueRef::to_owned),
                                (_, Held::Owned(value)) => value.clone(),
                                (_, Held::Remembered(index)) => remembered[*index].clone(),
                            })
                            .collect();
                        let atoms = (0..plan.keys.len())
                            .filter_map(|position| atom(position).map(|atom| atom.owned()))
                            .collect();
                        groups.push(new_group(key, atoms));
                        candidates.push(index);
                        index
                    }
                }
            };
            let group = &mut groups[group];
            'aggregates: for (aggregate, accumulator) in
                plan.aggregates.iter().zip(&mut group.accumulators)
            {
                for condition in &aggregate.filters {
                    if !self.evaluate_condition(document, condition)? {
                        continue 'aggregates;
                    }
                }
                match &aggregate.input {
                    RowInput::Rows => accumulator.count += 1,
                    RowInput::Slot(slot) => accumulator.push(current[*slot])?,
                    evaluated => {
                        match self.fold_input(evaluated, document, &current, &mut memos, &mut remembered)? {
                            Held::Owned(value) => {
                                accumulator.push(Some(ProjectedValueRef::Value(&value)))?
                            }
                            Held::Remembered(index) => accumulator
                                .push(Some(ProjectedValueRef::Value(&remembered[index])))?,
                        }
                    }
                }
            }
            Ok(true)
        };
        match &columns {
            // From whole columns: no row is read.
            Some((batch, chosen)) => {
                let cell = |field: usize, row: usize| -> Option<ProjectedValueRef<'_>> {
                    use crate::storage::{Column, ColumnData};
                    let column = batch.column(field);
                    if !column.is_present(row) {
                        return None;
                    }
                    if !column.has_value(row) {
                        return Some(ProjectedValueRef::Null);
                    }
                    Some(match column.data() {
                        ColumnData::Integers(values) => ProjectedValueRef::Integer(values[row]),
                        ColumnData::Floats(values) => ProjectedValueRef::Float(values[row]),
                        ColumnData::Booleans(values) => ProjectedValueRef::Boolean(values[row] != 0),
                        ColumnData::DateTimes(values) => ProjectedValueRef::DateTime(values[row]),
                        ColumnData::References(values) => ProjectedValueRef::Reference(values[row]),
                        ColumnData::Strings { offsets, bytes } => {
                            ProjectedValueRef::String(Column::text(offsets, bytes, row))
                        }
                    })
                };
                let mut one = |row: usize| -> OQLResult<bool> {
                    for (field, value) in current.iter_mut().enumerate() {
                        *value = cell(field, row);
                    }
                    each(&current, &scratch)
                };
                match chosen {
                    Some(chosen) => {
                        for row in chosen {
                            if !one(*row as usize)? {
                                return Ok(None);
                            }
                        }
                    }
                    None => {
                        for row in 0..batch.rows() {
                            if !one(row)? {
                                return Ok(None);
                            }
                        }
                    }
                }
                COLUMN_SCANS_RUN.with(|runs| runs.set(runs.get() + 1));
            }
            None => {
                for batch in &batches {
                    for row in batch.rows() {
                        current.iter_mut().for_each(|value| *value = None);
                        for (name, value) in row.fields() {
                            if let Some(index) = plan.fields.iter().position(|field| field == name) {
                                current[index] = Some(value);
                            }
                        }
                        // The row the evaluator reads: the stored document
                        // when it is in memory, which holds at least the
                        // fields read, and otherwise one filled from the
                        // columns.
                        let document: &Document = match row.document() {
                            Some(document) => document,
                            None => {
                                if needs_row {
                                    Self::fill_scratch_row(&mut scratch, &row, &plan.fields, &current);
                                }
                                &scratch
                            }
                        };
                        if !each(&current, document)? {
                            return Ok(None);
                        }
                    }
                }
            }
        }
        drop(each);

        // Groups come out in the order the general path produces them.
        let mut ordered: Vec<(Vec<u8>, ColumnGroup)> = groups
            .into_iter()
            .map(|group| {
                let canonical: Vec<Value> =
                    group.key.iter().map(Self::canonical_key_value).collect();
                bincode::serialize(&canonical)
                    .map(|encoded| (encoded, group))
                    .map_err(|error| OQLError::Generic {
                        message: format!("Failed to encode group key: {}", error),
                    })
            })
            .collect::<OQLResult<_>>()?;
        ordered.sort_by(|left, right| left.0.cmp(&right.0));

        let mut documents = Vec::with_capacity(ordered.len());
        for (index, (_, group)) in ordered.into_iter().enumerate() {
            let values: Vec<Value> = group
                .accumulators
                .into_iter()
                .map(Accumulator::finish)
                .collect();
            if let Some(having) = &plan.having {
                if !self.fold_condition_holds(having, &group.key, &values)? {
                    continue;
                }
            }
            let mut row = crate::document::Fields::with_capacity_and_hasher(plan.outputs.len(), Default::default());
            for (name, output) in &plan.outputs {
                let value = match output {
                    FoldOutput::Key(position) => group.key[*position].clone(),
                    FoldOutput::Aggregate(position) => values[*position].clone(),
                };
                row.insert(name.clone(), value);
            }
            documents.push(match &shape {
                Shape::Terminal(function) => Document::with_id(
                    format!("{function}_result"),
                    "aggregate".to_string(),
                    row,
                ),
                Shape::Select => Document::with_id(
                    "aggregation_result".to_string(),
                    "aggregation".to_string(),
                    row,
                ),
                Shape::Grouped => Document::with_id(format!("group_{index}"), "group".to_string(), row),
            });
        }
        COLUMN_FOLDS_RUN.with(|count| count.set(count.get() + 1));
        for operation in &rest[consumed..] {
            // No groups means no group rows to shape or filter; see the
            // same rule in `execute_general_query`.
            if matches!(shape, Shape::Grouped)
                && documents.is_empty()
                && matches!(operation, Operation::Select(_) | Operation::Having(_))
            {
                continue;
            }
            documents = self.apply_operation(documents, operation, &query.collection)?;
        }
        Ok(Some(documents))
    }

    /// Split leading `where` operations into the part storage evaluates and
    /// the part left to the evaluator. Each conjunct goes to storage when
    /// storage gives the same answer as the evaluator, and stays behind
    /// otherwise.
    fn split_storage_filters(
        collection: &str,
        schema: &Schema,
        leading: &[Operation],
    ) -> (Vec<ScanPredicate>, Vec<Condition>) {
        fn conjuncts<'a>(condition: &'a Condition, parts: &mut Vec<&'a Condition>) {
            match condition {
                Condition::And(left, right) => {
                    conjuncts(left, parts);
                    conjuncts(right, parts);
                }
                other => parts.push(other),
            }
        }
        fn predicate(condition: &Condition) -> Option<ScanPredicate> {
            Some(match condition {
                Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess { path }),
                    operator,
                    right: Expression::Literal(literal),
                } if path.len() == 1 => ScanPredicate::Comparison {
                    field: path[0].clone(),
                    operator: operator.clone(),
                    value: match literal {
                        Literal::String(value) => Value::String(value.clone()),
                        Literal::Integer(value) => Value::Integer(*value),
                        Literal::Float(value) => Value::Float(*value),
                        Literal::Boolean(value) => Value::Boolean(*value),
                        Literal::Null => Value::Null,
                        Literal::Timestamp(value) => Value::DateTime(*value),
                    },
                },
                Condition::And(left, right) => {
                    ScanPredicate::And(Box::new(predicate(left)?), Box::new(predicate(right)?))
                }
                Condition::Or(left, right) => {
                    ScanPredicate::Or(Box::new(predicate(left)?), Box::new(predicate(right)?))
                }
                _ => return None,
            })
        }
        // Storage compares stored values with the literal as given, so the
        // two must be of one plain type. A reference compared with an id, or
        // anything compared with null, has meaning only the evaluator knows.
        fn plainly_typed(predicate: &ScanPredicate, schema: &Schema) -> bool {
            match predicate {
                ScanPredicate::Comparison { field, value, .. } => {
                    if DocumentSystemField::from_name(field).is_some() {
                        return false;
                    }
                    let mut declared = schema.fields.get(field);
                    while let Some(FieldType::Optional(inner)) = declared {
                        declared = Some(inner);
                    }
                    matches!(
                        (declared, value),
                        (Some(FieldType::String), Value::String(_))
                            | (Some(FieldType::Integer), Value::Integer(_))
                            | (Some(FieldType::Float), Value::Float(_))
                            | (Some(FieldType::Boolean), Value::Boolean(_))
                            | (Some(FieldType::DateTime), Value::DateTime(_))
                    )
                }
                ScanPredicate::And(left, right) | ScanPredicate::Or(left, right) => {
                    plainly_typed(left, schema) && plainly_typed(right, schema)
                }
            }
        }
        let mut filters = Vec::new();
        let mut residual = Vec::new();
        for operation in leading {
            let Operation::Where(block) = operation else {
                continue;
            };
            let mut parts = Vec::new();
            conjuncts(&block.condition, &mut parts);
            for part in parts {
                let pushed = Self::is_pipeline_safe_condition(part)
                    .then(|| {
                        Self::normalize_pipeline_literals(
                            &OQLQuery {
                                collection: collection.to_string(),
                                operations: vec![Operation::Where(WhereBlock {
                                    condition: part.clone(),
                                })],
                            },
                            schema,
                        )
                    })
                    .flatten()
                    .and_then(|typed| match typed.operations.first() {
                        Some(Operation::Where(block)) => predicate(&block.condition),
                        _ => None,
                    })
                    .filter(|filter| plainly_typed(filter, schema));
                match pushed {
                    Some(filter) => filters.push(filter),
                    None => residual.push(part.clone()),
                }
            }
        }
        (filters, residual)
    }

    /// Answer `where { .. }.select { .. }` by shaping each stored row
    /// straight into its result row, without first building a row of the
    /// fields the query reads.
    ///
    /// The filters are split as in [`Self::aggregate_from_columns`] and the
    /// `select` is evaluated by the general evaluator, so the rows are the
    /// ones the general path returns. `None` leaves the query to it.
    fn select_from_columns(&self, query: &OQLQuery) -> OQLResult<Option<Vec<Document>>> {
        if !COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            return Ok(None);
        }
        let leading = query
            .operations
            .iter()
            .take_while(|operation| matches!(operation, Operation::Where(_)))
            .count();
        let [Operation::Select(select), after @ ..] = &query.operations[leading..] else {
            return Ok(None);
        };
        // A `select` of aggregates summarises its rows; that is a fold.
        if select
            .fields
            .iter()
            .any(|field| Self::expression_contains_aggregate(&field.expression))
        {
            return Ok(None);
        }
        let Some(schema) = self.schema_of(&query.collection)? else {
            return Ok(None);
        };
        // Every field read is named, so a stored document, which holds more
        // than a projected row would, answers alike.
        let Some(read) = self.fields_read_by(query) else {
            return Ok(None);
        };
        if self.store.has_pending_writes()? {
            return Ok(None);
        }
        let (filters, residual) =
            Self::split_storage_filters(&query.collection, &schema, &query.operations[..leading]);
        let fields: Vec<String> = read
            .into_iter()
            .filter(|name| {
                schema.fields.contains_key(name) && DocumentSystemField::from_name(name).is_none()
            })
            .collect();
        let Some(batches) =
            self.store
                .scan_projected_batches(&query.collection, &filters, &fields, 4096)?
        else {
            return Ok(None);
        };

        let mut documents = Vec::new();
        let mut current: Vec<Option<ProjectedValueRef<'_>>> = vec![None; fields.len()];
        let mut scratch =
            Document::with_id(String::new(), query.collection.clone(), crate::document::Fields::default());
        for batch in &batches {
            'rows: for row in batch.rows() {
                let document: &Document = match row.document() {
                    Some(document) => document,
                    None => {
                        current.iter_mut().for_each(|value| *value = None);
                        for (name, value) in row.fields() {
                            if let Some(index) = fields.iter().position(|field| field == name) {
                                current[index] = Some(value);
                            }
                        }
                        Self::fill_scratch_row(&mut scratch, &row, &fields, &current);
                        &scratch
                    }
                };
                for condition in &residual {
                    if !self.evaluate_condition(document, condition)? {
                        continue 'rows;
                    }
                }
                let shaped = self.select_row(document, select)?;
                documents.push(document.with_projected_fields(shaped));
            }
        }
        COLUMN_FOLDS_RUN.with(|count| count.set(count.get() + 1));
        for operation in after {
            documents = self.apply_operation(documents, operation, &query.collection)?;
        }
        Ok(Some(documents))
    }

    /// Compute a group key or an aggregate's input for one row.
    fn fold_input(
        &self,
        input: &RowInput,
        document: &Document,
        current: &[Option<ProjectedValueRef<'_>>],
        memos: &mut [HashMap<ObjRef, usize>],
        remembered: &mut Vec<Value>,
    ) -> OQLResult<Held> {
        match input {
            RowInput::Rows | RowInput::Slot(_) => Ok(Held::Owned(Value::Null)),
            RowInput::Path(path) => self.get_field_value(document, path).map(Held::Owned),
            RowInput::Expression(expression) => {
                self.evaluate_expression(document, expression).map(Held::Owned)
            }
            RowInput::Through { slot, memo, path } => {
                let reference = match current[*slot] {
                    Some(ProjectedValueRef::Reference(reference)) => reference,
                    Some(ProjectedValueRef::Value(Value::Reference(reference))) => *reference,
                    _ => return self.get_field_value(document, path).map(Held::Owned),
                };
                if let Some(index) = memos[*memo].get(&reference) {
                    return Ok(Held::Remembered(*index));
                }
                remembered.push(self.get_field_value(document, path)?);
                memos[*memo].insert(reference, remembered.len() - 1);
                Ok(Held::Remembered(remembered.len() - 1))
            }
        }
    }

    /// Make `scratch` the row a projected scan would have built for `row`,
    /// reusing its map, its keys and its string buffers.
    fn fill_scratch_row(
        scratch: &mut Document,
        row: &ProjectedRowRef<'_>,
        names: &[String],
        current: &[Option<ProjectedValueRef<'_>>],
    ) {
        scratch.id.clear();
        scratch.id.push_str(row.id());
        scratch.created_at = row.created_at();
        scratch.updated_at = row.updated_at();
        scratch.version = row.version();
        for (name, value) in names.iter().zip(current) {
            match value {
                Some(value) => match scratch.fields.get_mut(name) {
                    Some(held) => match (held, *value) {
                        (Value::String(text), ProjectedValueRef::String(new)) => {
                            text.clear();
                            text.push_str(new);
                        }
                        (held, value) => *held = value.to_owned(),
                    },
                    None => {
                        scratch.fields.insert(name.clone(), ProjectedValueRef::to_owned(*value));
                    }
                },
                None => {
                    scratch.fields.remove(name);
                }
            }
        }
    }

    /// Classify one `select` entry as a group key or an aggregate, with the
    /// name it takes in the result.
    fn fold_output(
        plan: &mut FoldPlan,
        schema: &Schema,
        entry: &crate::parser::SelectField,
        grouped: bool,
    ) -> Option<(String, FoldOutput)> {
        match &entry.expression {
            Expression::FieldAccess(FieldAccess { path }) if path.len() == 1 => {
                if !grouped {
                    return None;
                }
                let position = plan.key_named(&path[0])?;
                Some((
                    entry.alias.clone().unwrap_or_else(|| path[0].clone()),
                    FoldOutput::Key(position),
                ))
            }
            expression => {
                let position = Self::fold_aggregate(plan, schema, expression)?;
                let name = match (&entry.alias, expression) {
                    (Some(alias), _) => alias.clone(),
                    (None, Expression::FunctionCall { name, .. }) => name.clone(),
                    (None, _) => "computed".to_string(),
                };
                Some((name, FoldOutput::Aggregate(position)))
            }
        }
    }

    /// Add `expression` to the plan when it is one aggregate over the rows
    /// in hand: `sum(.total)`, `count()`, `avg(.qty * .price)`, or
    /// `where { .tier == "gold" }.count()`.
    fn fold_aggregate(
        plan: &mut FoldPlan,
        schema: &Schema,
        expression: &Expression,
    ) -> Option<usize> {
        let aggregate = match expression {
            Expression::FunctionCall { name, args } => {
                let input = match Self::aggregate_input(name, args).ok()?? {
                    AggregateInput::AllRows => RowInput::Rows,
                    AggregateInput::Path(path) => plan.input(schema, &path)?,
                    AggregateInput::Expression(inner) => {
                        if Self::expression_contains_aggregate(&inner) {
                            return None;
                        }
                        RowInput::Expression(inner)
                    }
                };
                FoldAggregate {
                    kind: AggregateKind::named(name, matches!(input, RowInput::Rows))?,
                    input,
                    filters: Vec::new(),
                }
            }
            Expression::CollectionExpression { source, operations }
                if matches!(source.as_ref(), Expression::Identifier(name) if name == crate::parser::CURRENT_ROWS) =>
            {
                let (terminal, before) = operations.split_last()?;
                let mut filters = Vec::with_capacity(before.len());
                for operation in before {
                    match operation {
                        Operation::Where(block)
                            if Self::is_pipeline_safe_condition(&block.condition) =>
                        {
                            filters.push(block.condition.clone())
                        }
                        _ => return None,
                    }
                }
                let (kind, input) = match terminal {
                    Operation::Count => (AggregateKind::Rows, RowInput::Rows),
                    Operation::Sum(field) => (AggregateKind::Sum, plan.input(schema, &field.path)?),
                    Operation::Avg(field) => (AggregateKind::Avg, plan.input(schema, &field.path)?),
                    Operation::Min(field) => (AggregateKind::Min, plan.input(schema, &field.path)?),
                    Operation::Max(field) => (AggregateKind::Max, plan.input(schema, &field.path)?),
                    _ => return None,
                };
                FoldAggregate {
                    kind,
                    input,
                    filters,
                }
            }
            _ => return None,
        };
        plan.aggregates.push(aggregate);
        Some(plan.aggregates.len() - 1)
    }

    /// A `having` condition over keys, aggregates and literals.
    fn fold_condition(
        &self,
        plan: &mut FoldPlan,
        schema: &Schema,
        condition: &Condition,
    ) -> Option<FoldCondition> {
        let operand = |expression: &Expression, plan: &mut FoldPlan| match expression {
            Expression::Literal(literal) => Some(FoldOperand::Literal(self.literal_to_value(literal))),
            Expression::FieldAccess(FieldAccess { path }) if path.len() == 1 => {
                plan.key_named(&path[0]).map(FoldOperand::Key)
            }
            other => Self::fold_aggregate(plan, schema, other).map(FoldOperand::Aggregate),
        };
        Some(match condition {
            Condition::Comparison {
                left,
                operator,
                right,
            } => {
                let left = operand(left, plan)?;
                let right = operand(right, plan)?;
                FoldCondition::Compare(left, operator.clone(), right)
            }
            Condition::And(left, right) => FoldCondition::And(
                Box::new(self.fold_condition(plan, schema, left)?),
                Box::new(self.fold_condition(plan, schema, right)?),
            ),
            Condition::Or(left, right) => FoldCondition::Or(
                Box::new(self.fold_condition(plan, schema, left)?),
                Box::new(self.fold_condition(plan, schema, right)?),
            ),
            Condition::Not(inner) => {
                FoldCondition::Not(Box::new(self.fold_condition(plan, schema, inner)?))
            }
        })
    }

    fn fold_condition_holds(
        &self,
        condition: &FoldCondition,
        key: &[Value],
        aggregates: &[Value],
    ) -> OQLResult<bool> {
        let value = |operand: &FoldOperand| match operand {
            FoldOperand::Key(position) => key[*position].clone(),
            FoldOperand::Aggregate(position) => aggregates[*position].clone(),
            FoldOperand::Literal(value) => value.clone(),
        };
        match condition {
            FoldCondition::Compare(left, operator, right) => {
                self.compare_values(&value(left), operator, &value(right))
            }
            FoldCondition::And(left, right) => Ok(self.fold_condition_holds(left, key, aggregates)?
                && self.fold_condition_holds(right, key, aggregates)?),
            FoldCondition::Or(left, right) => Ok(self.fold_condition_holds(left, key, aggregates)?
                || self.fold_condition_holds(right, key, aggregates)?),
            FoldCondition::Not(inner) => Ok(!self.fold_condition_holds(inner, key, aggregates)?),
        }
    }

    /// Source the rows of a query whose leading filter tests a field of a
    /// referenced document, by starting from the documents that pass the
    /// test and following the relationship back.
    ///
    /// `orders.where { .customer.country == "NG" }` reads the customers in
    /// that country and then their orders, instead of every order. The rows
    /// returned are a superset of the filter's answer, in the order a scan
    /// yields them (by id), so the query then runs on them unchanged. `None`
    /// when the shape does not apply or too many documents pass the test for
    /// this to be cheaper than a scan.
    fn rows_through_reference(&self, query: &OQLQuery) -> OQLResult<Option<Vec<Document>>> {
        if query.operations.iter().any(|operation| {
            matches!(
                operation,
                Operation::Insert(_)
                    | Operation::InsertMany(_)
                    | Operation::Upsert(_)
                    | Operation::Update(_)
                    | Operation::Delete
                    | Operation::FunctionDef(_)
                    | Operation::Explain
            )
        }) {
            return Ok(None);
        }
        // Every conjunct of the leading filters must hold for a row to
        // survive, so any one of them may choose where to start.
        fn conjuncts<'a>(condition: &'a Condition, parts: &mut Vec<&'a Condition>) {
            match condition {
                Condition::And(left, right) => {
                    conjuncts(left, parts);
                    conjuncts(right, parts);
                }
                other => parts.push(other),
            }
        }
        let mut parts = Vec::new();
        for operation in &query.operations {
            let Operation::Where(block) = operation else {
                break;
            };
            conjuncts(&block.condition, &mut parts);
        }
        let Some(schema) = self.schema_of(&query.collection)? else {
            return Ok(None);
        };
        let mut chosen = None;
        for part in parts {
            let Condition::Comparison {
                left: Expression::FieldAccess(FieldAccess { path }),
                operator: ComparisonOperator::Equal,
                right: Expression::Literal(literal),
            } = part
            else {
                continue;
            };
            if path.len() != 2 || matches!(literal, Literal::Null) {
                continue;
            }
            let mut declared = schema.fields.get(&path[0]);
            while let Some(FieldType::Optional(inner)) = declared {
                declared = Some(inner);
            }
            let Some(FieldType::Reference(reference)) = declared else {
                continue;
            };
            let Some(target) = self.schema_of(&reference.target_schema)? else {
                continue;
            };
            let mut tested = target.fields.get(&path[1]);
            while let Some(FieldType::Optional(inner)) = tested {
                tested = Some(inner);
            }
            if matches!(
                tested,
                Some(
                    FieldType::String
                        | FieldType::Integer
                        | FieldType::Float
                        | FieldType::Boolean
                        | FieldType::DateTime
                )
            ) && DocumentSystemField::from_name(&path[1]).is_none()
            {
                chosen = Some((path, part, reference.target_schema.clone()));
                break;
            }
        }
        let Some((path, test, target)) = chosen else {
            return Ok(None);
        };
        if self.store.has_pending_writes()? {
            return Ok(None);
        }

        // The documents that pass the test, asked as an ordinary query so the
        // comparison means exactly what it means in the filter.
        let Condition::Comparison { operator, right, .. } = test else {
            return Ok(None);
        };
        let matching = self.execute_query(&OQLQuery {
            collection: target.clone(),
            operations: vec![
                Operation::Where(crate::parser::WhereBlock {
                    condition: Condition::Comparison {
                        left: Expression::FieldAccess(FieldAccess {
                            path: vec![path[1].clone()],
                        }),
                        operator: operator.clone(),
                        right: right.clone(),
                    },
                }),
                Operation::Select(SelectBlock {
                    fields: vec![crate::parser::SelectField {
                        alias: None,
                        expression: Expression::FieldAccess(FieldAccess {
                            path: vec!["id".to_string()],
                        }),
                    }],
                }),
            ],
        })?;
        // Following the relationship costs more per row than scanning, so it
        // pays only when the test keeps a minority of the documents.
        let total = match self.store.collection_len(&target)? {
            Some(total) => total,
            None => {
                let total = self.execute_query(&OQLQuery {
                    collection: target.clone(),
                    operations: vec![Operation::Count],
                })?;
                match total.first().and_then(|row| row.get_field("count")) {
                    Some(Value::Integer(count)) => *count as usize,
                    _ => return Ok(None),
                }
            }
        };
        if matching.len().saturating_mul(4) > total {
            return Ok(None);
        }

        let fields = self.fields_read_by(query);
        let mut related = Vec::new();
        for document in &matching {
            let Some(Value::String(id)) = document.get_field("id") else {
                return Ok(None);
            };
            related.extend(self.store.scan_referencing_documents_shared(
                &target,
                &id.clone().into(),
                Some(&query.collection),
                Some(&path[0]),
            )?);
        }
        // A scan yields rows by id, each once.
        related.sort_by(|left, right| left.id.cmp(&right.id));
        related.dedup_by(|left, right| left.id == right.id);
        // The leading filters run again on what is returned. Running them
        // here first, against the stored documents, means only rows that
        // pass are copied.
        let leading: Vec<&Condition> = query
            .operations
            .iter()
            .map_while(|operation| match operation {
                Operation::Where(block) => Some(&block.condition),
                _ => None,
            })
            .collect();
        // The chosen test holds for every row reached through a document
        // that passed it, so it is not asked again here.
        let mut remaining = Vec::new();
        for condition in &leading {
            conjuncts(condition, &mut remaining);
        }
        remaining.retain(|part| !std::ptr::eq(*part, test));
        let mut passing = Vec::with_capacity(related.len());
        'documents: for document in &related {
            for condition in &remaining {
                if !self.evaluate_condition(document, condition)? {
                    continue 'documents;
                }
            }
            passing.push(document);
        }
        // `sort { .. }.limit(n)` straight after the filters keeps `n` of
        // these rows. Choosing them here, by the fields the sort names,
        // means only they are copied. They stay in order of id; the sort
        // itself still runs on them.
        if let (Some(Operation::Sort(sort)), Some(keep)) = (
            query.operations.get(leading.len()),
            Self::rows_kept_after_sort(&query.operations, leading.len()),
        ) {
            let plain = sort.fields.iter().all(|field| {
                matches!(&field.expression, Expression::FieldAccess(access) if access.path.len() == 1)
            });
            if plain && keep < passing.len() && COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
                // Rows are compared where they lie; most pairs are settled
                // by the first key, so nothing is worked out ahead of need.
                let mut kept: Vec<usize> = (0..passing.len()).collect();
                if keep == 0 {
                    kept.clear();
                } else {
                    kept.select_nth_unstable_by(keep - 1, |left, right| {
                        self.compare_stored_rows(sort, passing[*left], passing[*right])
                            .then(left.cmp(right))
                    });
                    kept.truncate(keep);
                }
                kept.sort_unstable();
                passing = kept.into_iter().map(|position| passing[position]).collect();
            }
        }
        let mut rows = Vec::with_capacity(passing.len());
        for document in passing {
            rows.push(match &fields {
                None => document.as_ref().clone(),
                Some(fields) => document.with_projected_fields(
                    fields
                        .iter()
                        .filter_map(|name| {
                            document
                                .fields
                                .get_key_value(name)
                                .map(|(name, value)| (name.clone(), value.clone()))
                        })
                        .collect(),
                ),
            });
        }
        Ok(Some(rows))
    }

    /// Give a leading `matches(.field, "^prefix..")` the range of text it
    /// can match.
    ///
    /// A pattern anchored at the start matches only text beginning with a
    /// known prefix, which is a range of the field: from the prefix up to
    /// the next one. Stating that range beside the pattern changes no
    /// answer, and lets storage or an index on the field leave out every
    /// row outside it. `None` when no leading pattern gives one.
    fn pattern_ranges(&self, query: &OQLQuery) -> OQLResult<Option<OQLQuery>> {
        let Some(Operation::Where(block)) = query.operations.first() else {
            return Ok(None);
        };
        fn conjuncts<'a>(condition: &'a Condition, parts: &mut Vec<&'a Condition>) {
            match condition {
                Condition::And(left, right) => {
                    conjuncts(left, parts);
                    conjuncts(right, parts);
                }
                other => parts.push(other),
            }
        }
        let mut parts = Vec::new();
        conjuncts(&block.condition, &mut parts);
        let mut ranges = Vec::new();
        let mut schema = None;
        for part in &parts {
            let Condition::Comparison {
                left: Expression::FunctionCall { name, args },
                operator: ComparisonOperator::Equal,
                right: Expression::Literal(Literal::Boolean(true)),
            } = part
            else {
                continue;
            };
            let (
                "matches",
                [Expression::FieldAccess(field), Expression::Literal(Literal::String(pattern))],
            ) = (name.as_str(), args.as_slice())
            else {
                continue;
            };
            if field.path.len() != 1 {
                continue;
            }
            if schema.is_none() {
                schema = self.schema_of(&query.collection)?;
            }
            // Only text has a prefix; anything else matches nothing anyway.
            let mut declared = schema
                .as_ref()
                .and_then(|schema| schema.fields.get(&field.path[0]));
            while let Some(FieldType::Optional(inner)) = declared {
                declared = Some(inner);
            }
            if !matches!(declared, Some(FieldType::String))
                || DocumentSystemField::from_name(&field.path[0]).is_some()
            {
                continue;
            }
            let Some(prefix) = crate::functions::required_prefix(pattern) else {
                continue;
            };
            let bound = |operator: ComparisonOperator, text: String| Condition::Comparison {
                left: Expression::FieldAccess(field.clone()),
                operator,
                right: Expression::Literal(Literal::String(text)),
            };
            if let Some(after) = crate::functions::after_prefix(&prefix) {
                ranges.push(bound(ComparisonOperator::LessThan, after));
            }
            ranges.push(bound(ComparisonOperator::GreaterEqual, prefix));
        }
        if ranges.is_empty() {
            return Ok(None);
        }
        let condition = ranges
            .into_iter()
            .fold(block.condition.clone(), |rest, range| {
                Condition::And(Box::new(range), Box::new(rest))
            });
        let mut operations = query.operations.clone();
        operations[0] = Operation::Where(WhereBlock { condition });
        Ok(Some(OQLQuery {
            collection: query.collection.clone(),
            operations,
        }))
    }

    /// Move a `limit`, an `offset`, or a `sort` on fields the `select`
    /// passes through unchanged, from after a `select` to before it.
    ///
    /// `projects.select { id, team: .assignments.count() }.sort { .id.asc }.limit(10)`
    /// is the same ten rows as sorting and limiting first, and then only
    /// ten rows are shaped instead of all of them. `None` when nothing moves.
    fn limit_before_select(query: &OQLQuery) -> Option<OQLQuery> {
        if CallFrame::any_active() {
            return None;
        }
        let mut operations = query.operations.clone();
        let mut moved = false;
        // A select that gives one row per row, in the same order.
        let row_for_row = |operations: &[Operation], at: usize| match &operations[at] {
            Operation::Select(block) => {
                !operations[..at].iter().any(|before| {
                    matches!(before, Operation::GroupBy(_) | Operation::PartitionBy(_))
                }) && !block.fields.iter().any(|field| {
                    Self::expression_contains_aggregate(&field.expression)
                        || Self::expression_uses_position(&field.expression)
                })
                // A select of plain fields is shaped as rows are read, which
                // is cheaper still; only one that asks a question per row
                // is worth shaping late.
                && block
                    .fields
                    .iter()
                    .any(|field| Self::expression_is_nested(&field.expression))
            }
            _ => false,
        };
        // The select hands this field on under its own name, untouched.
        let passes_through = |block: &SelectBlock, name: &str| {
            block.fields.iter().filter(|field| {
                field.alias.as_deref() == Some(name)
                    || matches!(&field.expression,
                        Expression::FieldAccess(access) if access.path.last().map(String::as_str) == Some(name))
                    || matches!(&field.expression, Expression::Identifier(bare) if bare == name)
                    || Self::is_all_fields(&field.expression)
            }).count() == 1
                && block.fields.iter().any(|field| {
                    field.alias.is_none()
                        && match &field.expression {
                            Expression::FieldAccess(access) => access.path.len() == 1 && access.path[0] == name,
                            Expression::Identifier(bare) => bare == name && !bare.starts_with('$'),
                            _ => false,
                        }
                })
        };
        // Position functions that only look back give a row the same
        // values whether or not later rows exist, so a limit can come
        // before the select that uses them, and before the `partition_by`
        // that serves it. That leaves the limit next to the sort.
        let looks_back_only = |operation: &Operation| match operation {
            Operation::PartitionBy(_) => true,
            Operation::Select(block) => {
                // An aggregate or a bare operation is over the whole
                // partition, later rows included.
                block.fields.iter().all(|field| {
                    // The rows so far are earlier rows by definition.
                    let so_far = matches!(
                        &field.expression,
                        Expression::CollectionExpression { source, .. }
                            if matches!(source.as_ref(), Expression::Identifier(name) if name == crate::parser::ROWS_SO_FAR)
                    ) && !Self::expression_calls(&field.expression, crate::parser::CURRENT_ROWS);
                    !Self::expression_contains_aggregate(&field.expression)
                        && (so_far || !Self::expression_is_nested(&field.expression))
                        && !Self::expression_calls(&field.expression, "next")
                }) && block
                    .fields
                    .iter()
                    .any(|field| Self::expression_uses_position(&field.expression))
            }
            _ => false,
        };
        let mut hop = operations.len();
        while hop > 1 {
            hop -= 1;
            let mut limit = hop;
            while limit > 0
                && matches!(operations[limit], Operation::Limit(_))
                && looks_back_only(&operations[limit - 1])
                && !operations[..limit - 1]
                    .iter()
                    .any(|before| matches!(before, Operation::GroupBy(_)))
            {
                operations.swap(limit - 1, limit);
                moved = true;
                limit -= 1;
            }
        }
        let mut at = 0;
        while at + 1 < operations.len() {
            if !row_for_row(&operations, at) {
                at += 1;
                continue;
            }
            let Operation::Select(block) = &operations[at] else {
                unreachable!("row_for_row matched a select");
            };
            let next_moves = match &operations[at + 1] {
                Operation::Limit(_) | Operation::Offset(_) => true,
                // A sort moves only together with the limit that makes
                // moving it worthwhile.
                Operation::Sort(sort) => {
                    matches!(
                        operations.get(at + 2),
                        Some(Operation::Limit(_) | Operation::Offset(_))
                    ) && sort.fields.iter().all(|field| match &field.expression {
                        Expression::FieldAccess(access) if access.path.len() == 1 => {
                            passes_through(block, &access.path[0])
                        }
                        _ => false,
                    })
                }
                _ => false,
            };
            if !next_moves {
                at += 1;
                continue;
            }
            operations.swap(at, at + 1);
            moved = true;
            at += 1;
        }
        moved.then(|| OQLQuery {
            collection: query.collection.clone(),
            operations,
        })
    }

    /// Split a leading filter so the part storage can evaluate comes first.
    ///
    /// `where { .customer.country == "NG" && .total > 900 }` becomes
    /// `where { .total > 900 }.where { .customer.country == "NG" }`: the
    /// same rows, but the reference is followed only for rows that passed
    /// the cheap test. `None` when there is nothing to split.
    fn storage_filter_first(query: &OQLQuery) -> Option<OQLQuery> {
        let Some(Operation::Where(block)) = query.operations.first() else {
            return None;
        };
        fn conjuncts<'a>(condition: &'a Condition, parts: &mut Vec<&'a Condition>) {
            match condition {
                Condition::And(left, right) => {
                    conjuncts(left, parts);
                    conjuncts(right, parts);
                }
                other => parts.push(other),
            }
        }
        let mut parts = Vec::new();
        conjuncts(&block.condition, &mut parts);
        let (pushable, rest): (Vec<&Condition>, Vec<&Condition>) = parts
            .into_iter()
            .partition(|part| Self::is_pipeline_safe_condition(part));
        if pushable.is_empty() || rest.is_empty() {
            return None;
        }
        let join = |parts: Vec<&Condition>| {
            parts
                .into_iter()
                .cloned()
                .reduce(|left, right| Condition::And(Box::new(left), Box::new(right)))
                .map(|condition| Operation::Where(crate::parser::WhereBlock { condition }))
        };
        let mut operations = Vec::with_capacity(query.operations.len() + 1);
        operations.extend(join(pushable));
        operations.extend(join(rest));
        operations.extend(query.operations[1..].iter().cloned());
        Some(OQLQuery {
            collection: query.collection.clone(),
            operations,
        })
    }

    /// `where { .. }.sort { .id.asc }.limit(n)`: a collection is read in
    /// order of id, so the first `n` rows that pass the filter are the
    /// answer and the read can stop there. Returns those rows and how many
    /// leading filters they already satisfy; the sort and limit still run.
    /// `None` when the shape does not apply.
    fn first_rows_by_id(&self, query: &OQLQuery) -> OQLResult<Option<(Vec<Document>, usize)>> {
        if !COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            return Ok(None);
        }
        let leading = query
            .operations
            .iter()
            .take_while(|operation| matches!(operation, Operation::Where(_)))
            .count();
        let (Some(Operation::Sort(sort)), Some(Operation::Limit(limit))) = (
            query.operations.get(leading),
            query.operations.get(leading + 1),
        ) else {
            return Ok(None);
        };
        let [key] = sort.fields.as_slice() else {
            return Ok(None);
        };
        let by_id = matches!(key.direction, crate::parser::SortDirection::Asc)
            && matches!(&key.expression, Expression::FieldAccess(field) if field.path == ["id"]);
        if !by_id || *limit > 10_000 {
            return Ok(None);
        }
        let Some(schema) = self.schema_of(&query.collection)? else {
            return Ok(None);
        };
        let (filters, residual) =
            Self::split_storage_filters(&query.collection, &schema, &query.operations[..leading]);
        if !residual.is_empty() {
            return Ok(None);
        }
        let fields: Option<Vec<String>> = self.fields_read_by(query).map(|fields| {
            fields
                .into_iter()
                .filter(|name| schema.fields.contains_key(name))
                .collect()
        });
        Ok(self
            .store
            .scan_first_documents(&query.collection, &filters, fields.as_deref(), *limit as usize)?
            .map(|rows| (rows, leading)))
    }

    /// Fetch the root rows of a general query through the physical pipeline.
    ///
    /// The pipeline evaluates simple filters inside storage, picks an index
    /// when one fits, and decodes only requested fields. A general query can
    /// use all of that for its first step even though the rest of it (a
    /// grouping, a nested projection) runs here. Returns the rows and how
    /// many leading operations they already satisfy.
    fn pipeline_prefix(&self, query: &OQLQuery) -> OQLResult<Option<(PhysicalPlan, usize)>> {
        let leading: Vec<Operation> = query
            .operations
            .iter()
            .take_while(|operation| {
                matches!(operation, Operation::Where(block) if Self::is_pipeline_safe_condition(&block.condition))
            })
            .cloned()
            .collect();
        // With no filter to push down, the direct projected scan is already
        // the cheapest way to read the rows.
        if leading.is_empty() {
            return Ok(None);
        }
        let fields = self.fields_read_by(query);
        if self.store.has_pending_writes()? {
            return Ok(None);
        }
        let Some(schema) = self.schema_of(&query.collection)? else {
            return Ok(None);
        };

        let applied = leading.len();
        let mut operations = leading;
        if let Some(fields) = fields {
            // Names that are not stored fields (a relationship, a name from
            // a nested scope) have nothing to decode.
            let mut stored: Vec<String> = fields
                .into_iter()
                .filter(|name| schema.fields.contains_key(name))
                .collect();
            if stored.is_empty() {
                stored.push("id".to_string());
            }
            stored.sort();
            operations.push(Operation::Select(SelectBlock {
                fields: stored
                    .into_iter()
                    .map(|name| crate::parser::SelectField {
                        alias: None,
                        expression: Expression::FieldAccess(FieldAccess { path: vec![name] }),
                    })
                    .collect(),
            }));
        }
        let prefix = OQLQuery {
            collection: query.collection.clone(),
            operations,
        };
        // Every row that passes is fetched here; nothing bounds the read.
        // The planner has no statistics, so judge selectivity by shape: a
        // range usually narrows to a small part of the collection, while an
        // equality alone (a status, a flag) often keeps most of it, and
        // fetching most of a collection through an index is slower than
        // scanning it.
        fn has_range(condition: &Condition) -> bool {
            match condition {
                Condition::Comparison { operator, .. } => matches!(
                    operator,
                    ComparisonOperator::LessThan
                        | ComparisonOperator::LessEqual
                        | ComparisonOperator::GreaterThan
                        | ComparisonOperator::GreaterEqual
                ),
                Condition::And(left, right) | Condition::Or(left, right) => {
                    has_range(left) || has_range(right)
                }
                Condition::Not(inner) => has_range(inner),
            }
        }
        let narrowing = prefix.operations.iter().any(|operation| {
            matches!(operation, Operation::Where(block) if has_range(&block.condition))
        });
        let Some(plan) = self.plan_read_with(&prefix, &schema, narrowing)? else {
            return Ok(None);
        };
        Ok(Some((plan, applied)))
    }

    /// Plan a read made only of pipeline-safe operations.
    fn plan_read(&self, query: &OQLQuery, schema: &Schema) -> OQLResult<Option<PhysicalPlan>> {
        self.plan_read_with(query, schema, true)
    }

    fn plan_read_with(
        &self,
        query: &OQLQuery,
        schema: &Schema,
        offer_indexes: bool,
    ) -> OQLResult<Option<PhysicalPlan>> {
        let Some(query) = Self::normalize_pipeline_literals(query, schema) else {
            return Ok(None);
        };
        let registry = SchemaRegistry::new();
        registry.register(schema.clone())?;
        let Ok(logical) = LogicalPlanner::with_schema_registry(registry).plan(&query) else {
            return Ok(None);
        };
        let planner = if offer_indexes {
            PhysicalPlanner::new().with_storage_catalog(self.store.as_ref())?
        } else {
            PhysicalPlanner::new().with_storage_scan_only(self.store.as_ref())
        };
        Ok(Some(planner.plan(&logical)?))
    }

    /// `sort { .a.desc, .b.asc }.limit(n)` when an index supplies `.a` in
    /// order: take the first `n` rows in index order, then settle the rows
    /// that tie on the last `.a` value by the remaining keys. This reads
    /// about `n` rows instead of every row that passes the filter.
    fn tie_broken_top_k(&self, query: &OQLQuery) -> OQLResult<Option<Vec<Document>>> {
        let Some(sort_at) = query
            .operations
            .iter()
            .position(|operation| matches!(operation, Operation::Sort(_)))
        else {
            return Ok(None);
        };
        let (Some(Operation::Sort(sort)), Some(Operation::Limit(limit))) = (
            query.operations.get(sort_at),
            query.operations.get(sort_at + 1),
        ) else {
            return Ok(None);
        };
        let filters = &query.operations[..sort_at];
        if sort.fields.len() < 2
            || query
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::Explain))
            || *limit == 0
            || *limit > 10_000
            || !filters.iter().all(|operation| {
                matches!(operation, Operation::Where(block) if Self::is_pipeline_safe_condition(&block.condition))
            })
        {
            return Ok(None);
        }
        let Expression::FieldAccess(leading_key) = &sort.fields[0].expression else {
            return Ok(None);
        };
        if leading_key.path.len() != 1 || self.store.has_pending_writes()? {
            return Ok(None);
        }
        let Some(schema) = self.schema_of(&query.collection)? else {
            return Ok(None);
        };

        // Only worth doing when the index really supplies the order.
        let mut first_rows = filters.to_vec();
        first_rows.push(Operation::Sort(SortBlock {
            fields: vec![sort.fields[0].clone()],
        }));
        // One row more than asked for shows whether the last place is
        // contested.
        first_rows.push(Operation::Limit(*limit + 1));
        let Some(plan) = self.plan_read(
            &OQLQuery {
                collection: query.collection.clone(),
                operations: first_rows,
            },
            &schema,
        )?
        else {
            return Ok(None);
        };
        let sorts_itself = plan
            .root
            .explain()
            .lines()
            .any(|line| line.trim_start().starts_with("Sort"));
        if sorts_itself || !Self::contains_native_index_scan(&plan.root) {
            return Ok(None);
        }

        let mut candidates = self.execute_read_plan(&plan)?;
        // The row past the page, when there is one. The index yields equal
        // values together, so if it differs from the last row of the page
        // then every row sharing that last value is already in hand.
        let beyond = if candidates.len() as u64 > *limit {
            candidates.pop()
        } else {
            None
        };
        let contested = match (&beyond, candidates.last()) {
            (Some(beyond), Some(last)) => {
                let boundary = self.get_field_value(last, leading_key)?;
                boundary.is_null()
                    || self.values_equal(&self.get_field_value(beyond, leading_key)?, &boundary)
            }
            _ => false,
        };
        if contested {
            let boundary = self.get_field_value(
                candidates.last().expect("a full page has a last row"),
                leading_key,
            )?;
            // Every row tied with the boundary competes for the last places.
            let Ok(boundary_literal) = Self::value_as_expression(boundary.clone()) else {
                return Ok(None);
            };
            if boundary.is_null() {
                return Ok(None);
            }
            let mut tied = filters.to_vec();
            tied.push(Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(leading_key.clone()),
                    operator: ComparisonOperator::Equal,
                    right: boundary_literal,
                },
            }));
            let tied = self.execute_query(&OQLQuery {
                collection: query.collection.clone(),
                operations: tied,
            })?;
            let mut settled = Vec::with_capacity(candidates.len() + tied.len());
            for row in candidates {
                if !self.values_equal(&self.get_field_value(&row, leading_key)?, &boundary) {
                    settled.push(row);
                }
            }
            settled.extend(tied);
            candidates = settled;
        }

        let mut documents = self.apply_sort_operation(candidates, sort)?;
        documents.truncate(*limit as usize);
        for operation in &query.operations[sort_at + 2..] {
            documents = self.apply_operation(documents, operation, &query.collection)?;
        }
        Ok(Some(documents))
    }

    /// [`Self::fields_read_by`] for the operations of a nested expression,
    /// worked out once per statement. A `select` evaluates the same nested
    /// expression for every row, and working out what it reads means
    /// walking it whole.
    fn fields_read_by_operations(
        &self,
        collection: &str,
        operations: &[Operation],
    ) -> Option<Vec<String>> {
        let compute = || {
            self.fields_read_by(&OQLQuery {
                collection: collection.to_string(),
                operations: operations.to_vec(),
            })
        };
        if !StatementGuard::active() {
            return compute();
        }
        // Found by where the operations are, confirmed by what they are:
        // an address can be reused within a statement.
        let place = (operations.as_ptr() as usize, operations.len());
        let known = STATEMENT.with(|scope| {
            scope
                .borrow()
                .fields_read
                .get(&place)
                .filter(|(remembered, _)| remembered.as_slice() == operations)
                .map(|(_, fields)| fields.clone())
        });
        if let Some(fields) = known {
            return fields;
        }
        let fields = compute();
        STATEMENT.with(|scope| {
            scope
                .borrow_mut()
                .fields_read
                .insert(place, (operations.to_vec(), fields.clone()))
        });
        fields
    }

    /// The top-level fields of the root collection that `query` can read, or
    /// `None` when it may need whole rows.
    ///
    /// The answer is a superset: names from nested scopes are included,
    /// which costs a little decoding and never an answer. `None` is returned
    /// whenever something could observe a field that is not named: a write,
    /// a result that returns rows unshaped, `*`, the row itself (`.`), a
    /// bare name, or a user function, whose body may read anything.
    fn fields_read_by(&self, query: &OQLQuery) -> Option<Vec<String>> {
        let shapes_rows = |operation: &Operation| {
            matches!(
                operation,
                Operation::Select(_)
                    | Operation::GroupBy(_)
                    | Operation::Count
                    | Operation::Exists
                    | Operation::Sum(_)
                    | Operation::Avg(_)
                    | Operation::Min(_)
                    | Operation::Max(_)
                    | Operation::AggregateExpr { .. }
                    | Operation::Step(_)
            )
        };
        let mut shaped = false;
        for operation in &query.operations {
            match operation {
                Operation::Insert(_)
                | Operation::InsertMany(_)
                | Operation::Upsert(_)
                | Operation::Update(_)
                | Operation::Delete
                | Operation::FunctionDef(_)
                | Operation::Explain => return None,
                // A function written in OQL is given the rows, and its body
                // may read anything of them.
                Operation::AggregateExpr { function, .. } if !Self::is_aggregate_name(function) => {
                    return None
                }
                // Comparing whole rows needs whole rows.
                Operation::Distinct if !shaped => return None,
                // So does combining them with another query's, and a walk
                // returns rows that nothing has shaped.
                Operation::Combine { .. } | Operation::Walk { .. } => return None,
                operation => shaped |= shapes_rows(operation),
            }
        }
        if !shaped {
            return None;
        }

        fn collect(
            node: &serde_json::Value,
            user_function: &dyn Fn(&str) -> bool,
            fields: &mut HashSet<String>,
        ) -> bool {
            match node {
                serde_json::Value::Object(object) => {
                    // Any `{ path: [..] }`: a field access, an aggregate's
                    // path, a group key, a step.
                    if let Some(path) = object.get("path").and_then(|path| path.as_array()) {
                        match path.first().and_then(|name| name.as_str()) {
                            None | Some("*") => return false,
                            Some(first) => {
                                fields.insert(first.to_string());
                            }
                        }
                    }
                    if let Some(name) = object.get("Identifier").and_then(|name| name.as_str()) {
                        // A bare name may be a field of the row. The marker
                        // for "the rows in hand" is not; it only matters
                        // when what it returns is left unshaped.
                        if name != crate::parser::CURRENT_ROWS
                            && name != crate::parser::OMITTED_FIELD
                            && !name.starts_with('$')
                        {
                            fields.insert(name.to_string());
                        }
                    }
                    if let Some(call) = object.get("FunctionCall") {
                        if call
                            .get("name")
                            .and_then(|name| name.as_str())
                            .is_some_and(user_function)
                        {
                            return false;
                        }
                    }
                    if let Some(access) = object.get("ChainedAccess") {
                        // `collection.field` inside a nested query reads
                        // that field of the enclosing row.
                        if let Some(field) = access.get("field").and_then(|field| field.as_str()) {
                            fields.insert(field.to_string());
                        }
                    }
                    if let Some(nested) = object.get("CollectionExpression") {
                        let over_rows_in_hand = nested
                            .get("source")
                            .and_then(|source| source.get("Identifier"))
                            .and_then(|name| name.as_str())
                            == Some(crate::parser::CURRENT_ROWS);
                        let shaped = nested
                            .get("operations")
                            .and_then(|operations| operations.as_array())
                            .is_some_and(|operations| {
                                operations.iter().any(|operation| {
                                    [
                                        "Select", "Count", "Exists", "Sum", "Avg", "Min", "Max",
                                        "AggregateExpr", "Step", "GroupBy",
                                    ]
                                    .iter()
                                    .any(|kind| {
                                        operation.get(kind).is_some()
                                            || operation.as_str() == Some(kind)
                                    })
                                })
                            });
                        if over_rows_in_hand && !shaped {
                            return false;
                        }
                    }
                    object
                        .values()
                        .all(|value| collect(value, user_function, fields))
                }
                serde_json::Value::Array(items) => items
                    .iter()
                    .all(|item| collect(item, user_function, fields)),
                _ => true,
            }
        }

        let tree = serde_json::to_value(&query.operations).ok()?;
        let mut fields = HashSet::new();
        // A function written by a user may be given a row and read anything
        // of it. A library function is given values, which are named.
        let user_function = |name: &str| {
            self.functions
                .get_user_function(name)
                .is_some_and(|function| !function.library)
        };
        collect(&tree, &user_function, &mut fields).then(|| fields.into_iter().collect())
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
        let Some(schema) = self.schema_of(&query.collection)? else {
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
        registry.register(schema.as_ref().clone())?;
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
                        // Timestamp text becomes the instant it denotes, so
                        // an index on the field can serve the comparison.
                        (Some(FieldType::DateTime), Literal::String(text)) => {
                            Literal::Timestamp(crate::value_semantics::parse_timestamp(text)?)
                        }
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
                Operation::Limit(_)
                | Operation::Offset(_)
                | Operation::LimitExpr(_)
                | Operation::OffsetExpr(_)
                | Operation::First => bounded = true,
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
            Expression::FieldAccess(FieldAccess { path }) => path.len() == 1 && path[0] != "*",
            _ => false,
        }
    }

    /// The `*` entry of a `select` block.
    fn is_all_fields(expression: &Expression) -> bool {
        matches!(expression, Expression::FieldAccess(FieldAccess { path }) if path.len() == 1 && path[0] == "*")
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

        // `.id == "x"` anywhere in the leading conjunction identifies at
        // most one document, which is stored under that id.
        fn identified<'a>(condition: &'a Condition) -> Option<&'a Literal> {
            match condition {
                Condition::And(left, right) => identified(left).or_else(|| identified(right)),
                Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess { path }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(literal),
                }
                | Condition::Comparison {
                    left: Expression::Literal(literal),
                    operator: ComparisonOperator::Equal,
                    right: Expression::FieldAccess(FieldAccess { path }),
                } if path.len() == 1 && path[0] == "id" => Some(literal),
                _ => None,
            }
        }
        if let Some(id) = identified(&where_block.condition) {
            let id = match id {
                Literal::String(id) => id.clone(),
                Literal::Integer(id) => id.to_string(),
                _ => return Ok(Some(Vec::new())),
            };
            return Ok(Some(
                self.store
                    .get_document(&query.collection, &id)?
                    .into_iter()
                    .collect(),
            ));
        }

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
            let docs = self.resolve_reverse_reference_clause_documents(
                &clause,
                &target_id,
                Some(&where_block.condition),
            )?;

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
        whole_filter: Option<&Condition>,
    ) -> OQLResult<Vec<Document>> {
        let mut current_targets = vec![(
            clause.target_schema.clone(),
            canonical_target_id.to_string(),
        )];
        let mut current_documents = Vec::new();
        let last = clause.steps.len().saturating_sub(1);

        for (taken, step) in clause.steps.iter().rev().enumerate() {
            let mut next_documents = Vec::new();
            let mut seen = HashSet::new();

            for (target_schema, target_id) in &current_targets {
                // The last step reaches the rows of the query itself. They
                // must pass its whole filter, of which this clause is one
                // part, so the filter is asked of them where the store
                // holds them and only those that pass are copied.
                if let (true, Some(filter)) = (taken == last, whole_filter) {
                    for doc in self.store.scan_referencing_documents_shared(
                        target_schema,
                        target_id,
                        Some(&step.source_schema),
                        Some(&step.field_path),
                    )? {
                        if self.evaluate_condition(&doc, filter)?
                            && seen.insert((doc.schema.clone(), doc.id.clone()))
                        {
                            next_documents.push(doc.as_ref().clone());
                        }
                    }
                    continue;
                }
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
        // Either `.user.id == "u1"` or `.user == "u1"`: a reference equals
        // the id of its target, so both ask for the rows that refer to that
        // document, which the reverse-reference index answers directly.
        let reference_path = match field_access.path.split_last() {
            Some((last, rest)) if last == "id" && !rest.is_empty() => rest,
            _ => &field_access.path[..],
        };
        if reference_path.is_empty() {
            return Ok(None);
        }
        let mut current_schema = source_collection.to_string();
        let mut steps = Vec::new();

        for segment in reference_path {
            let Some(schema) = self.schema_of(&current_schema)? else {
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

        // A document is stored under its id, so there is nowhere else to
        // look.
        Ok(None)
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
            Operation::LimitExpr(count) => {
                let count = self.evaluate_row_count("limit", count)?;
                self.apply_limit_operation(documents, count)
            }
            Operation::Distinct => self.apply_distinct_operation(documents),
            Operation::Last(count) => {
                let count = self.evaluate_row_count("last", count)? as usize;
                let skipped = documents.len().saturating_sub(count);
                Ok(documents.into_iter().skip(skipped).collect())
            }
            Operation::Combine { how, other } => {
                self.apply_combine(documents, how, other, collection, false)
            }
            Operation::Walk { path, depth, every, paths } => {
                self.apply_walk(documents, path, depth.as_ref(), None, *every, *paths)
            }
            Operation::PartitionBy(block) => self.apply_partition_by(documents, block),
            Operation::Call(_) => Err(OQLError::Generic {
                message: "a function call gives a query its rows, so it must come first".to_string(),
            }),
            Operation::Step(path) => {
                let mut reached = Vec::new();
                for document in &documents {
                    reached.extend(self.path_source_rows(document, &path.path)?.0);
                }
                Ok(reached)
            }
            Operation::Explain => Err(OQLError::Generic {
                message: "explain() describes a whole query, so it must be its last operation"
                    .to_string(),
            }),
            // A function written in OQL at the end of a chain is given the
            // rows in hand.
            Operation::AggregateExpr { function, input }
                if !Self::is_aggregate_name(function) =>
            {
                self.apply_function_to_rows(documents, function, input, collection, &[])
            }
            Operation::AggregateExpr { function, input } => self.aggregate_result_row(
                &documents,
                function,
                &AggregateInput::Expression(input.clone()),
            ),
            Operation::OffsetExpr(count) => {
                let count = self.evaluate_row_count("offset", count)?;
                Ok(documents.into_iter().skip(count as usize).collect())
            }
            Operation::First => Ok(documents.into_iter().take(1).collect()),
            Operation::Exists => {
                let exists = !documents.is_empty();
                Ok(vec![Document::with_id(
                    "exists_result".to_string(),
                    "aggregate".to_string(),
                    crate::document::Fields::from_iter([("exists".to_string(), Value::Boolean(exists))]),
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

    /// Evaluate the count of a `limit(expr)` or `offset(expr)`. The
    /// expression has no current row; it is a constant, a parameter, or a
    /// value computed from other collections.
    fn evaluate_row_count(&self, operation: &str, count: &Expression) -> OQLResult<u64> {
        let scope = Document::with_id(String::new(), String::new(), crate::document::Fields::default());
        match self.evaluate_expression(&scope, count)? {
            Value::Integer(count) if count >= 0 => Ok(count as u64),
            other => Err(OQLError::TypeError {
                expected: format!("a non-negative whole number for {operation}"),
                actual: match other {
                    Value::Integer(count) => count.to_string(),
                    other => other.type_name().to_string(),
                },
            }),
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
        let temp_doc = Document::new("__vector_query__".to_string(), crate::document::Fields::default());
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
                // A plain field of the row against a constant, the commonest
                // comparison there is: the field is compared where it lies
                // instead of being copied out first.
                if let (Expression::FieldAccess(access), Expression::Literal(literal)) = (left, right) {
                    if let [name] = access.path.as_slice() {
                        if DocumentSystemField::from_name(name).is_none() {
                            match (document.fields.get(name), literal) {
                                // Text against text needs no copy of either.
                                (Some(Value::String(text)), Literal::String(wanted)) => {
                                    if let Some(answer) = Self::text_matches(text, operator, wanted) {
                                        return Ok(answer);
                                    }
                                }
                                (
                                    Some(
                                        stored @ (Value::String(_)
                                        | Value::Integer(_)
                                        | Value::Float(_)
                                        | Value::Boolean(_)
                                        | Value::DateTime(_)
                                        | Value::Null),
                                    ),
                                    _,
                                ) => {
                                    return Ok(crate::value_semantics::matches(
                                        stored,
                                        operator,
                                        &self.literal_to_value(literal),
                                    ));
                                }
                                // A reference is there or it is not; which
                                // row it leads to does not matter to `null`.
                                (Some(stored @ Value::Reference(_)), Literal::Null) => {
                                    return Ok(crate::value_semantics::matches(
                                        stored,
                                        operator,
                                        &Value::Null,
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                MANY_VALUED.with(|flag| flag.set(false));
                let left_value = self.evaluate_expression(document, left)?;
                let left_many = MANY_VALUED.with(|flag| flag.replace(false))
                    && matches!(left, Expression::FieldAccess(_));
                let right_value = self.evaluate_expression(document, right)?;
                let right_many = MANY_VALUED.with(|flag| flag.replace(false))
                    && matches!(right, Expression::FieldAccess(_));
                // A path through a collection has a value for each row it
                // reaches. Whether "is it x" means any of them or all of
                // them is not for the engine to guess, and answering
                // "false" would hide the question.
                if matches!(
                    operator,
                    ComparisonOperator::Equal
                        | ComparisonOperator::NotEqual
                        | ComparisonOperator::LessThan
                        | ComparisonOperator::LessEqual
                        | ComparisonOperator::GreaterThan
                        | ComparisonOperator::GreaterEqual
                ) && ((left_many && !matches!(right_value, Value::List(_)))
                    || (right_many && !matches!(left_value, Value::List(_))))
                {
                    let (many, one) = if left_many { (left, right) } else { (right, left) };
                    return Err(Self::many_compared_with_one(many, one, operator));
                }
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

    /// Evaluate `left operator right`. Kept out of `evaluate_expression`,
    /// which calls itself for every level of nesting and of recursion, so
    /// that what this needs is not held on the stack at each of them.
    #[inline(never)]
    fn evaluate_arithmetic(
        &self,
        document: &Document,
        left: &Expression,
        operator: &crate::parser::BinaryOperator,
        right: &Expression,
    ) -> OQLResult<Value> {
        MANY_VALUED.with(|flag| flag.set(false));
        let left_value = self.evaluate_expression(document, left)?;
        let left_many = MANY_VALUED.with(|flag| flag.replace(false))
            && matches!(left, Expression::FieldAccess(_));
        let right_value = self.evaluate_expression(document, right)?;
        let right_many = MANY_VALUED.with(|flag| flag.replace(false))
            && matches!(right, Expression::FieldAccess(_));
        // Arithmetic needs one value on each side. A list joined to
        // a list with `+` is the exception, and stays.
        let joins_lists = matches!(operator, crate::parser::BinaryOperator::Add)
            && matches!((&left_value, &right_value), (Value::List(_), Value::List(_)));
        if (left_many || right_many) && !joins_lists {
            return Err(Self::arithmetic_on_many(left, operator, right, left_many));
        }
        self.evaluate_binary_operation(&left_value, operator, &right_value)
    }

    /// A query written inside an expression. Inside a function, a name
    /// that is one of its parameters is that parameter's rows or values,
    /// not a collection of the same name.
    #[inline(never)]
    fn evaluate_nested_query(&self, document: &Document, query: &OQLQuery) -> OQLResult<Value> {
        if let Some(rows) = CallFrame::collection(&query.collection) {
            return self.run_collection_operations(
                rows.as_ref().clone(),
                false,
                &query.operations,
                document,
                &query.collection,
            );
        }
        if matches!(CallFrame::value(&query.collection), Some(Value::List(_))) {
            return self.evaluate_collection_expression(
                document,
                &Expression::Identifier(query.collection.clone()),
                &query.operations,
            );
        }
        self.evaluate_query_expression(document, query)
    }

    /// A query that starts with a function call, `seniors(50).count()`.
    ///
    /// When the function's body is a query, that query runs as it would if
    /// written out, with every way of reading less open to it. Otherwise the
    /// value the function gives is the collection. What follows the call is
    /// then applied to the rows.
    fn execute_call_query(&self, query: &OQLQuery, args: &[Expression]) -> OQLResult<Vec<Document>> {
        let scope = Document::with_id(String::new(), String::new(), crate::document::Fields::default());
        let mut documents = if query.collection == crate::parser::ROWS_OF {
            match args {
                [rows] => self.rows_of_expression(&scope, rows)?,
                _ => Vec::new(),
            }
        } else {
            self.rows_of_call(&scope, query, args)?
        };
        let after = &query.operations[1..];
        let mut grouped = false;
        for (position, operation) in after.iter().enumerate() {
            if grouped
                && documents.is_empty()
                && matches!(operation, Operation::Select(_) | Operation::Having(_))
            {
                continue;
            }
            documents = match operation {
                Operation::Sort(sort) => self.sort_keeping(
                    documents,
                    sort,
                    Self::rows_kept_after_sort(after, position),
                )?,
                Operation::AggregateExpr { function, input }
                    if !Self::is_aggregate_name(function) =>
                {
                    self.apply_function_to_rows(
                        documents,
                        function,
                        input,
                        &query.collection,
                        &after[position + 1..],
                    )?
                }
                _ => self.apply_operation(documents, operation, &query.collection)?,
            };
            grouped |= matches!(operation, Operation::GroupBy(_));
        }
        Ok(documents)
    }

    /// The rows an expression stands for at the start of a query. Two
    /// collections joined with `+` are the rows of one followed by the rows
    /// of the other, every row kept, each still a row of its collection.
    fn rows_of_expression(&self, scope: &Document, expression: &Expression) -> OQLResult<Vec<Document>> {
        match expression {
            Expression::BinaryOp {
                left,
                operator: crate::parser::BinaryOperator::Add,
                right,
            } => {
                let mut rows = self.rows_of_expression(scope, left)?;
                rows.extend(self.rows_of_expression(scope, right)?);
                Ok(rows)
            }
            Expression::QueryExpression { query } => self.execute_query(query),
            // A bare name is a whole collection, unless a script gave the
            // name to something else.
            Expression::Identifier(name)
                if CallFrame::value(name).is_none() && self.schema_of(name)?.is_some() =>
            {
                self.execute_query(&OQLQuery {
                    collection: name.clone(),
                    operations: Vec::new(),
                })
            }
            other => {
                let mut rows = Vec::new();
                self.push_source_rows(self.evaluate_expression(scope, other)?, "", &mut rows)?;
                Ok(rows.into_iter().map(SourceRow::into_document).collect())
            }
        }
    }

    /// The rows a function gives at the start of a query.
    fn rows_of_call(
        &self,
        scope: &Document,
        query: &OQLQuery,
        args: &[Expression],
    ) -> OQLResult<Vec<Document>> {
        let Some(function) = self.functions.get_user_function(&query.collection) else {
            return Err(OQLError::Generic {
                message: format!("Unknown function: {}", query.collection),
            });
        };
        Ok(match &function.definition.body {
            Expression::QueryExpression { query: body } => {
                let frame = self.bind_arguments(scope, &function, args)?;
                let _guard = CallFrameGuard::enter(frame, &function.definition.name)?;
                self.execute_general_query(body)?
            }
            _ => {
                let value = self.evaluate_user_function(scope, &function, args)?;
                let mut rows = Vec::new();
                self.push_source_rows(value, &query.collection, &mut rows)?;
                rows.into_iter().map(SourceRow::into_document).collect()
            }
        })
    }

    /// The error for arithmetic on a path through a collection.
    fn arithmetic_on_many(
        left: &Expression,
        operator: &crate::parser::BinaryOperator,
        right: &Expression,
        left_many: bool,
    ) -> OQLError {
        use crate::parser::BinaryOperator;
        let written = |expression: &Expression| match expression {
            Expression::FieldAccess(access) => format!(".{}", access.path.join(".")),
            _ => "a value".to_string(),
        };
        let symbol = match operator {
            BinaryOperator::Add => "+",
            BinaryOperator::Subtract => "-",
            BinaryOperator::Multiply => "*",
            BinaryOperator::Divide => "/",
            BinaryOperator::Modulo => "%",
        };
        let many = written(if left_many { left } else { right });
        // `.children*.id` is the commonest way to arrive here: `*` written
        // to mean "any number of steps", as in a path language.
        let advice = match (operator, left, right) {
            (BinaryOperator::Multiply, Expression::FieldAccess(_), Expression::FieldAccess(_)) if left_many => {
                format!(
                    " `*` multiplies; it does not repeat a step. To follow {many} to any depth, \
                     write .walk({many})."
                )
            }
            _ => format!(
                " To combine them into one value first, use an aggregate such as \
                 {many_rows}.sum({last}), or shape each with {many_rows}.select {{ .. }}.",
                many_rows = match if left_many { left } else { right } {
                    Expression::FieldAccess(access) if access.path.len() > 1 => {
                        format!(".{}", access.path[..access.path.len() - 1].join("."))
                    }
                    _ => many.clone(),
                },
                last = match if left_many { left } else { right } {
                    Expression::FieldAccess(access) if access.path.len() > 1 => {
                        format!(".{}", access.path[access.path.len() - 1])
                    }
                    _ => "..".to_string(),
                },
            ),
        };
        OQLError::FunctionError {
            message: format!(
                "{many} passes through a collection, so it has many values and {symbol} needs one.{advice}"
            ),
        }
    }

    /// The error for comparing a path through a collection with one value.
    fn many_compared_with_one(
        many: &Expression,
        one: &Expression,
        operator: &ComparisonOperator,
    ) -> OQLError {
        let path = match many {
            Expression::FieldAccess(access) => access.path.clone(),
            _ => Vec::new(),
        };
        let written = format!(".{}", path.join("."));
        let symbol = match operator {
            ComparisonOperator::Equal => "==",
            ComparisonOperator::NotEqual => "!=",
            ComparisonOperator::LessThan => "<",
            ComparisonOperator::LessEqual => "<=",
            ComparisonOperator::GreaterThan => ">",
            _ => ">=",
        };
        let value = match one {
            Expression::Literal(Literal::String(text)) => format!("\"{text}\""),
            Expression::Literal(Literal::Integer(number)) => number.to_string(),
            Expression::Literal(Literal::Float(number)) => number.to_string(),
            Expression::Literal(Literal::Boolean(flag)) => flag.to_string(),
            Expression::Literal(Literal::Null) => "null".to_string(),
            _ => "value".to_string(),
        };
        let advice = if matches!(one, Expression::Literal(Literal::Null)) {
            format!("To ask whether there are any, write {written}.exists() or !{written}.exists().")
        } else {
            match path.split_last() {
                Some((last, before)) if !before.is_empty() => format!(
                    "To ask whether any of them does, write .{}.where {{ .{last} {symbol} {value} }}.exists(){}.",
                    before.join("."),
                    if matches!(operator, ComparisonOperator::Equal) {
                        format!(", or {written} contains {value}")
                    } else {
                        String::new()
                    }
                ),
                _ => format!(
                    "To ask whether any of them does, filter them and write {written}.where {{ .. }}.exists()."
                ),
            }
        };
        OQLError::FunctionError {
            message: format!(
                "{written} passes through a collection, so it has many values and {symbol} cannot \
                 compare it with one. {advice}"
            ),
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
                if let Some(rows) = CallFrame::collection(name) {
                    return Ok(Value::List(rows.iter().map(Self::row_as_value).collect()));
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
                self.evaluate_arithmetic(document, left, operator, right)
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
                // `rows.pay` for a collection parameter: the value for
                // each of its rows.
                if let Some(values) = self.parameter_values(expression)? {
                    return Ok(values);
                }
                // `tasks.state` on a row that a path step reached from a
                // task is that task's state.
                if let Some(Value::Object(origin)) = document.fields.get(ORIGIN_FIELD) {
                    if let Some(Value::String(collection)) = origin.get(ORIGIN_NAME) {
                        if let Some(kept) = Self::extract_scoped_path(expression, collection)
                            .and_then(|path| origin.get(&path.join(".")))
                        {
                            return Ok(kept.clone());
                        }
                    }
                }
                self.evaluate_named_path(document, expression, base, field)
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
                let mut object_fields = crate::document::Fields::default();
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
                self.evaluate_nested_query(document, query)
            }
            Expression::CollectionExpression { source, operations } => {
                self.evaluate_collection_expression(document, source, operations)
            }
        }
    }

    /// `name.a.b` where nothing above has claimed `name`: not a function's
    /// parameter, and not a row some step was taken from.
    ///
    /// A field of the row called `name` is read as it always was. Failing
    /// that, a collection's name is a row of that collection when the row
    /// in hand is one, as in `staff.select { staff.pay }`; and it is the
    /// collection itself when no such row is in reach, as `units.budget`
    /// is in a query on staff: the value for every row.
    #[inline(never)]
    fn evaluate_named_path(
        &self,
        document: &Document,
        expression: &Expression,
        base: &Expression,
        field: &str,
    ) -> OQLResult<Value> {
        if let Some((name, path)) = Self::named_path(expression) {
            if !document.fields.contains_key(name) && DocumentSystemField::from_name(name).is_none() {
                if document.schema == name {
                    return self.get_field_value(document, &FieldAccess { path });
                }
                if self.schema_of(name)?.is_some() {
                    let rows = self.execute_general_query(&OQLQuery {
                        collection: name.to_string(),
                        operations: vec![Operation::Step(FieldAccess { path })],
                    })?;
                    MANY_VALUED.with(|flag| flag.set(true));
                    return Ok(Value::List(
                        rows.into_iter()
                            .map(|row| match row.fields.get(SCALAR_ROW_FIELD) {
                                Some(value) => value.clone(),
                                None => Self::row_as_value(&row),
                            })
                            .collect(),
                    ));
                }
            }
        }
        let base_value = self.evaluate_expression(document, base)?;
        self.get_field_from_value(&base_value, field)
    }

    /// `name.a.b` as its name and its path.
    fn named_path(expression: &Expression) -> Option<(&str, Vec<String>)> {
        let mut path = Vec::new();
        let mut current = expression;
        while let Expression::ChainedAccess { base, field } = current {
            path.push(field.clone());
            current = base;
        }
        let Expression::Identifier(name) = current else {
            return None;
        };
        path.reverse();
        (!path.is_empty()).then_some((name.as_str(), path))
    }

    fn evaluate_collection_expression(
        &self,
        scope_document: &Document,
        source: &Expression,
        operations: &[Operation],
    ) -> OQLResult<Value> {
        // `staff.pay.sum()` in a query on staff: `staff.pay` is this row's
        // pay, one value, and there is nothing to add up. The writer most
        // likely meant every row's.
        if let (Some((name, path)), false) = (Self::named_path(source), operations.is_empty()) {
            if scope_document.schema == name
                && !scope_document.fields.contains_key(name)
                && CallFrame::row(name).is_none()
                && CallFrame::collection(name).is_none()
                && !scope_document.fields.contains_key(ORIGIN_FIELD)
            {
                return Err(OQLError::FunctionError {
                    message: format!(
                        "{name}.{path} here is one value, the {path} of the row being read, so \
                         there is nothing for what follows it to work on. For every row of \
                         {name}, name the field inside the operation, as in {name}.sum(.{last})",
                        path = path.join("."),
                        last = path.join("."),
                    ),
                });
            }
        }
        let collection_scope = match source {
            Expression::FieldAccess(field) => field
                .path
                .last()
                .cloned()
                .unwrap_or_else(|| "$collection".to_string()),
            Expression::Identifier(name) => name.clone(),
            _ => "$collection".to_string(),
        };
        // `.project.assignments.where { .. }.exists()` asks about the
        // project, not about the row holding the reference. Every row that
        // points at the same project gets the same answer, so it is worked
        // out once per project in a statement.
        if let Some(key) = Self::through_reference_key(scope_document, source, operations) {
            if let Some(known) =
                STATEMENT.with(|scope| scope.borrow().through_reference.get(&key).cloned())
            {
                return Ok(known);
            }
            let answer =
                self.evaluate_collection_from(scope_document, source, operations, collection_scope)?;
            STATEMENT.with(|scope| {
                scope
                    .borrow_mut()
                    .through_reference
                    .insert(key, answer.clone())
            });
            return Ok(answer);
        }
        self.evaluate_collection_from(scope_document, source, operations, collection_scope)
    }

    /// Count the objects of an embedded list that pass a filter, or say
    /// whether any does, without making a row of each.
    ///
    /// One row is reused: for each element, the fields the filter reads are
    /// put into it in place of the last element's. `None` when the question
    /// is not of this shape, and the general route answers it.
    /// `.notes.mins.sum()` or `.notes.sum(.mins)` where `notes` is a list
    /// of objects the row holds: one field of each element, reduced, read
    /// straight from the list. `None` when it is anything else, when an
    /// element's value is not a plain one, or when the reduction would
    /// fail: the general way then answers, or says why not.
    fn reduce_list_elements(
        scope_document: &Document,
        path: &[String],
        operations: &[Operation],
    ) -> Option<Value> {
        if !COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            return None;
        }
        let named;
        let (name, reduction) = match path {
            [name] => (name, operations),
            [name, leaf] => {
                named = Self::reduction_naming(leaf, operations)?;
                (name, named.as_slice())
            }
            _ => return None,
        };
        let (kind, field) = match reduction {
            [Operation::Sum(field)] => (AggregateKind::Sum, field),
            [Operation::Avg(field)] => (AggregateKind::Avg, field),
            [Operation::Min(field)] => (AggregateKind::Min, field),
            [Operation::Max(field)] => (AggregateKind::Max, field),
            _ => return None,
        };
        let [leaf] = field.path.as_slice() else {
            return None;
        };
        let Some(Value::List(items)) = scope_document.fields.get(name) else {
            return None;
        };
        let mut accumulator = Accumulator::new(kind);
        for item in items {
            let Value::Object(fields) = item else {
                return None;
            };
            match fields.get(leaf) {
                None | Some(Value::Null) => {}
                Some(Value::List(_) | Value::Object(_) | Value::Reference(_) | Value::GlobalReference(_)) => {
                    return None
                }
                Some(value) => accumulator.push(Some(ProjectedValueRef::Value(value))).ok()?,
            }
        }
        Some(accumulator.finish())
    }

    fn count_list_elements(
        &self,
        scope_document: &Document,
        path: &[String],
        operations: &[Operation],
        collection_scope: &str,
    ) -> OQLResult<Option<Value>> {
        let [name] = path else {
            return Ok(None);
        };
        let Some((last, filters)) = operations.split_last() else {
            return Ok(None);
        };
        if !matches!(last, Operation::Count | Operation::Exists) {
            return Ok(None);
        }
        let Some(Value::List(items)) = scope_document.fields.get(name) else {
            return Ok(None);
        };
        let mut conditions = Vec::with_capacity(filters.len());
        for operation in filters {
            match operation {
                Operation::Where(block) if !Self::operation_names(operation, &scope_document.schema) => {
                    conditions.push(&block.condition)
                }
                _ => return Ok(None),
            }
        }
        if !items.iter().all(|item| matches!(item, Value::Object(_))) {
            return Ok(None);
        }
        let Some(read) = self.fields_read_by_operations(collection_scope, operations) else {
            return Ok(None);
        };
        let reads_id = read.iter().any(|field| field == "id");
        let mut row = Document::with_id(String::new(), name.clone(), crate::document::Fields::with_capacity_and_hasher(read.len(), Default::default()));
        let mut count = 0i64;
        'items: for (position, item) in items.iter().enumerate() {
            let Value::Object(fields) = item else {
                continue;
            };
            for field in &read {
                match (fields.get(field), row.fields.get_mut(field)) {
                    (Some(value), Some(slot)) => slot.clone_from(value),
                    (Some(value), None) => {
                        row.fields.insert(field.clone(), value.clone());
                    }
                    (None, Some(_)) => {
                        row.fields.remove(field);
                    }
                    (None, None) => {}
                }
            }
            // An element with no id of its own is known by its place.
            if reads_id {
                row.id = position.to_string();
            }
            for condition in &conditions {
                if !self.evaluate_condition(&row, condition)? {
                    continue 'items;
                }
            }
            count += 1;
            if matches!(last, Operation::Exists) {
                break;
            }
        }
        Ok(Some(match last {
            Operation::Exists => Value::Boolean(count > 0),
            _ => Value::Integer(count),
        }))
    }

    /// The rows of a relationship that also refer to one named document,
    /// found from that document's side.
    ///
    /// `.assignments.where { .person == "p1" }` on a project asks which of
    /// the project's assignments are p1's. A project may have hundreds of
    /// assignments and p1 a handful, and the same person is usually asked
    /// about for many projects. So p1's assignments are read once in a
    /// statement and arranged by project, and each project is then one
    /// lookup. `None` when the question does not have this shape. The rows
    /// returned still have the filter applied to them.
    fn related_also_referring(
        &self,
        scope_document: &Document,
        path: &[String],
        operations: &[Operation],
    ) -> OQLResult<Option<Vec<Document>>> {
        let Some(Operation::Where(block)) = operations.first() else {
            return Ok(None);
        };
        if !StatementGuard::active() || CallFrame::any_active() {
            return Ok(None);
        }
        // `.field == "id"` among the conditions that must all hold.
        fn named<'a>(condition: &'a Condition) -> Option<(&'a str, &'a str)> {
            match condition {
                Condition::Comparison {
                    left: Expression::FieldAccess(access),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::String(id)),
                } if access.path.len() == 1 => Some((access.path[0].as_str(), id.as_str())),
                Condition::And(left, right) => named(left).or_else(|| named(right)),
                _ => None,
            }
        }
        let Some((other_field, other_id)) = named(&block.condition) else {
            return Ok(None);
        };
        // The row the relationship is asked of: this one, or the one a
        // single reference leads to.
        let lent;
        let (holder, relationship): (&Document, &String) = match path {
            [relationship] => (scope_document, relationship),
            [reference, relationship] => {
                let Some(Value::Reference(target)) = scope_document.fields.get(reference) else {
                    return Ok(None);
                };
                let Some(target) = self.referenced_document(target)? else {
                    return Ok(None);
                };
                lent = target;
                (&lent, relationship)
            }
            _ => return Ok(None),
        };
        if holder.fields.contains_key(relationship) {
            return Ok(None);
        }
        let ReverseRelationship::Declared { source, field } =
            self.reverse_relationship(&holder.schema, relationship)?
        else {
            return Ok(None);
        };
        let Some(source_schema) = self.schema_of(&source)? else {
            return Ok(None);
        };
        let target_of = |name: &str| {
            let mut declared = source_schema.fields.get(name);
            while let Some(FieldType::Optional(inner)) = declared {
                declared = Some(inner);
            }
            match declared {
                Some(FieldType::Reference(reference)) => Some(reference.target_schema.clone()),
                _ => None,
            }
        };
        let Some(other_target) = target_of(other_field) else {
            return Ok(None);
        };
        // The one field through which the relationship's rows refer to the
        // holder.
        let through = match field {
            Some(field) => field,
            None => {
                let mut referring = source_schema
                    .fields
                    .keys()
                    .filter(|name| target_of(name).as_deref() == Some(holder.schema.as_str()));
                match (referring.next(), referring.next()) {
                    (Some(only), None) => only.clone(),
                    _ => return Ok(None),
                }
            }
        };
        if through == other_field || target_of(&through).as_deref() != Some(holder.schema.as_str()) {
            return Ok(None);
        }

        let key = format!("{source}\u{0}{through}\u{0}{other_field}\u{0}{other_id}");
        let known = STATEMENT.with(|scope| scope.borrow().also_referring.get(&key).cloned());
        let by_holder = match known {
            Some(by_holder) => by_holder,
            None => {
                let rows = self.store.scan_referencing_documents_shared(
                    &other_target,
                    &other_id.to_string(),
                    Some(&source),
                    Some(other_field),
                )?;
                let mut by_holder: HashMap<String, Vec<Arc<Document>>> = HashMap::new();
                for row in rows {
                    let Some(reference @ Value::Reference(_)) = row.fields.get(&through) else {
                        continue;
                    };
                    if let Value::String(holder_id) = self.reference_as_id(reference)? {
                        by_holder.entry(holder_id).or_default().push(row);
                    }
                }
                let by_holder = Arc::new(by_holder);
                STATEMENT.with(|scope| {
                    scope
                        .borrow_mut()
                        .also_referring
                        .insert(key, by_holder.clone())
                });
                by_holder
            }
        };
        Ok(Some(
            by_holder
                .get(&holder.id)
                .map(|rows| rows.iter().map(|row| row.as_ref().clone()).collect())
                .unwrap_or_default(),
        ))
    }

    /// Whether an operation reads the enclosing row by naming its
    /// collection, as `.salary > people.salary` does inside a `people`
    /// query. One that does not is the same for every enclosing row. An
    /// operation this does not know how to look through counts as naming it.
    fn operation_names(operation: &Operation, collection: &str) -> bool {
        let expression = |node: &Expression| Self::expression_names(node, collection);
        match operation {
            Operation::Where(block) => Self::condition_names(&block.condition, collection),
            Operation::Having(block) => Self::condition_names(&block.condition, collection),
            Operation::Select(block) => block.fields.iter().any(|field| expression(&field.expression)),
            Operation::Sort(block) => block.fields.iter().any(|field| expression(&field.expression)),
            Operation::GroupBy(block) | Operation::PartitionBy(block) => {
                block.computed.iter().any(|key| expression(&key.expression))
            }
            Operation::AggregateExpr { input, .. } => expression(input),
            Operation::LimitExpr(count) | Operation::OffsetExpr(count) => expression(count),
            Operation::Limit(_)
            | Operation::Offset(_)
            | Operation::Distinct
            | Operation::Step(_)
            | Operation::First
            | Operation::Exists
            | Operation::Count
            | Operation::Sum(_)
            | Operation::Avg(_)
            | Operation::Min(_)
            | Operation::Max(_) => false,
            _ => true,
        }
    }

    fn condition_names(condition: &Condition, collection: &str) -> bool {
        match condition {
            Condition::Comparison { left, right, .. } => {
                Self::expression_names(left, collection) || Self::expression_names(right, collection)
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                Self::condition_names(left, collection) || Self::condition_names(right, collection)
            }
            Condition::Not(inner) => Self::condition_names(inner, collection),
        }
    }

    fn expression_names(expression: &Expression, collection: &str) -> bool {
        let names = |node: &Expression| Self::expression_names(node, collection);
        match expression {
            Expression::Identifier(name) => name == collection,
            Expression::FieldAccess(access) => access.path.len() == 1 && access.path[0] == collection,
            // A nested query is given the enclosing row's values when it
            // runs, not when the operation holding it is prepared.
            Expression::Literal(_) | Expression::QueryExpression { .. } => false,
            Expression::CollectionExpression { source, operations } => {
                names(source)
                    || operations
                        .iter()
                        .any(|operation| Self::operation_names(operation, collection))
            }
            Expression::FunctionCall { args, .. } => args.iter().any(names),
            Expression::IfThenElse { condition, then_expr, else_expr } => {
                Self::condition_names(condition, collection) || names(then_expr) || names(else_expr)
            }
            Expression::LogicalAnd(left, right)
            | Expression::LogicalOr(left, right)
            | Expression::BinaryOp { left, right, .. } => names(left) || names(right),
            Expression::LogicalNot(inner) => names(inner),
            Expression::ArrayAccess { array, index } => names(array) || names(index),
            Expression::ChainedAccess { base, .. } => names(base),
            Expression::Block(items) | Expression::ArrayLiteral(items) => items.iter().any(names),
            Expression::ObjectLiteral(fields) => fields.iter().any(|field| names(&field.value)),
            Expression::ConditionExpr(condition) => Self::condition_names(condition, collection),
            Expression::Match { .. } => true,
        }
    }

    /// What identifies the answer of a collection expression that starts by
    /// following a single reference: the document referred to, the rest of
    /// the path and the operations. `None` when the answer also depends on
    /// the row asking, because the operations name its collection.
    fn through_reference_key(
        scope_document: &Document,
        source: &Expression,
        operations: &[Operation],
    ) -> Option<(u64, Vec<u8>)> {
        let Expression::FieldAccess(field) = source else {
            return None;
        };
        let (first, rest) = field.path.split_first()?;
        if rest.is_empty() || !StatementGuard::active() || CallFrame::any_active() {
            return None;
        }
        let Some(Value::Reference(target)) = scope_document.fields.get(first) else {
            return None;
        };
        let asked = bincode::serialize(&(rest, operations, &scope_document.schema)).ok()?;
        let names_asker = operations
            .iter()
            .any(|operation| Self::operation_names(operation, &scope_document.schema));
        (!names_asker).then(|| (target.packed(), asked))
    }

    /// [`Self::evaluate_collection_expression`] without looking for an
    /// answer already given.
    fn evaluate_collection_from(
        &self,
        scope_document: &Document,
        source: &Expression,
        operations: &[Operation],
        collection_scope: String,
    ) -> OQLResult<Value> {
        if matches!(source, Expression::Identifier(name) if name == crate::parser::ROWS_SO_FAR) {
            return self.rows_so_far(scope_document, operations, &collection_scope);
        }
        // `.people.salary.median()` is `.people.median(.salary)`, which
        // every way of reading less already understands.
        if let Expression::FieldAccess(field) = source {
            if let Some((last, before)) = field.path.split_last().filter(|(_, before)| !before.is_empty()) {
                if let Some(named) = Self::reduction_naming(last, operations) {
                    let reached = match self.schema_of(&scope_document.schema)? {
                        Some(schema) => self.path_place(&Place::Rows(schema), before).ok(),
                        None => None,
                    };
                    if let Some(Place::Rows(rows)) = reached {
                        if self.holds_one_value(&rows.name, last)? {
                            return self.evaluate_collection_from(
                                scope_document,
                                &Expression::FieldAccess(FieldAccess { path: before.to_vec() }),
                                &named,
                                before.last().cloned().unwrap_or(collection_scope),
                            );
                        }
                    }
                }
            }
        }
        let from_origin = match source {
            Expression::FieldAccess(field) if field.path.len() >= 2 => {
                self.path_keeping_origin(scope_document, &field.path, operations)?
            }
            _ => None,
        };
        if let (Expression::FieldAccess(field), None) = (source, &from_origin) {
            if let Some(answer) = self.related_count(scope_document, &field.path, operations)? {
                return Ok(answer);
            }
            if let Some(answer) = self.related_directly(scope_document, &field.path, operations)? {
                return Ok(answer);
            }
        }
        if let (Expression::FieldAccess(field), None) = (source, &from_origin) {
            if let Some(answer) =
                self.count_list_elements(scope_document, &field.path, operations, &collection_scope)?
            {
                return Ok(answer);
            }
            if let Some(answer) = Self::reduce_list_elements(scope_document, &field.path, operations) {
                return Ok(answer);
            }
        }
        let from_other_side = match source {
            Expression::FieldAccess(field) if field.path != ["*"] => {
                self.related_also_referring(scope_document, &field.path, operations)?
            }
            _ => None,
        };
        let (documents, single) = match source {
            // An operation written with no receiver, evaluated for a group
            // row, applies to that group's rows.
            Expression::Identifier(name) if name == crate::parser::CURRENT_ROWS => {
                // After `partition_by`, the rows in hand are the partition
                // of the row being shaped.
                let partition = WindowFrame::partition_of(scope_document);
                if scope_document.schema != "group" && partition.is_none() {
                    return Err(OQLError::FunctionError {
                        message: "an operation with nothing before it needs a collection to \
                                  apply to; here it has only a single row. Write the collection \
                                  it applies to, as in .items.where { .. }"
                            .to_string(),
                    });
                }
                // The group's rows are shared by everything asked of the
                // group. Leading filters run against them where they are,
                // so only rows that pass are copied, and none for a count.
                let rows = match partition {
                    Some((frame, partition)) => frame.rows_of(partition),
                    None => Self::group_rows(scope_document)?,
                };
                let filters = operations
                    .iter()
                    .take_while(|operation| {
                        matches!(operation, Operation::Where(_))
                            && !Self::operation_names(operation, &scope_document.schema)
                    })
                    .count();
                if filters == 0 {
                    (rows.as_ref().clone(), false)
                } else {
                    let mut passing = Vec::new();
                    'rows: for row in rows.iter() {
                        for operation in &operations[..filters] {
                            if let Operation::Where(block) = operation {
                                if !self.evaluate_condition(row, &block.condition)? {
                                    continue 'rows;
                                }
                            }
                        }
                        passing.push(row);
                    }
                    if matches!(&operations[filters..], [Operation::Count]) {
                        return Ok(Value::Integer(passing.len() as i64));
                    }
                    return self.run_collection_operations(
                        passing.into_iter().cloned().collect(),
                        false,
                        &operations[filters..],
                        scope_document,
                        &collection_scope,
                    );
                }
            }
            // The values of one field across a group, reduced: the same
            // aggregate the group would compute if the field were named.
            Expression::FieldAccess(field)
                if scope_document.schema == "group"
                    && field.path.len() == 1
                    && !scope_document.fields.contains_key(&field.path[0])
                    && scope_document.fields.contains_key("_group_rows")
                    && Self::reduction_naming(&field.path[0], operations).is_some()
                    && Self::group_rows(scope_document)?
                        .first()
                        .map(|row| self.holds_one_value(&row.schema, &field.path[0]))
                        .transpose()?
                        .unwrap_or(false) =>
            {
                let rows = Self::group_rows(scope_document)?;
                let path = field.path.clone();
                return match Self::reduction_naming(&field.path[0], operations).as_deref() {
                    Some([Operation::Sum(_)]) => self.aggregate_rows(&rows, "sum", &AggregateInput::Path(path)),
                    Some([Operation::Avg(_)]) => self.aggregate_rows(&rows, "avg", &AggregateInput::Path(path)),
                    Some([Operation::Min(_)]) => self.aggregate_rows(&rows, "min", &AggregateInput::Path(path)),
                    Some([Operation::Max(_)]) => self.aggregate_rows(&rows, "max", &AggregateInput::Path(path)),
                    Some([Operation::AggregateExpr { function, input }]) => self.aggregate_rows(
                        &rows,
                        function,
                        &match input {
                            Expression::FieldAccess(_) => AggregateInput::Path(path),
                            other => AggregateInput::Expression(other.clone()),
                        },
                    ),
                    _ => unreachable!("reduction_naming gives one aggregate"),
                };
            }
            // On a group row, a path that is not one of its keys starts
            // from each of the rows the group stands for.
            Expression::FieldAccess(field)
                if scope_document.schema == "group"
                    && !field.path.is_empty()
                    && field.path != ["*"]
                    && !scope_document.fields.contains_key(&field.path[0])
                    && !scope_document.fields.contains_key(&field.path.join("_"))
                    && scope_document.fields.contains_key("_group_rows") =>
            {
                let fields = self.fields_read_by_operations(&collection_scope, operations);
                let mut reached = Vec::new();
                for row in Self::group_rows(scope_document)?.iter() {
                    reached.extend(
                        self.path_source_rows_reading(row, &field.path, fields.as_deref())?.0,
                    );
                }
                (reached, false)
            }
            // `.tasks.worklog.select { minutes, state: tasks.state }`: the
            // operations name a collection the path passes through, so each
            // row reached keeps what they read of the row it came from.
            Expression::FieldAccess(_) if from_origin.is_some() => {
                (from_origin.unwrap_or_default(), false)
            }
            // `rows.reports` inside a function: a path from each row the
            // parameter holds, which are still rows of their collection.
            Expression::ChainedAccess { .. } if Self::parameter_rows(source).is_some() => {
                let (rows, path) = Self::parameter_rows(source).unwrap_or_default();
                let fields = self.fields_read_by_operations(&collection_scope, operations);
                let mut reached = Vec::new();
                for row in rows.iter() {
                    reached.extend(self.path_source_rows_reading(row, &path, fields.as_deref())?.0);
                }
                (reached, false)
            }
            Expression::FieldAccess(_) if from_other_side.is_some() => {
                (from_other_side.unwrap_or_default(), false)
            }
            Expression::FieldAccess(field) if field.path != ["*"] => {
                // What the operations read of the rows they are given.
                let fields = self.fields_read_by_operations(&collection_scope, operations);
                self.path_source_rows_reading(scope_document, &field.path, fields.as_deref())?
            }
            _ => {
                let value = self.evaluate_expression(scope_document, source)?;
                let single = !matches!(value, Value::List(_));
                let mut rows = Vec::new();
                self.push_source_rows(value, &collection_scope, &mut rows)?;
                (rows.into_iter().map(|row| row.document).collect(), single)
            }
        };
        self.run_collection_operations(documents, single, operations, scope_document, &collection_scope)
    }

    /// Answer `.relationship.where { .. }.count()` or `.exists()` for one
    /// row from a tally made once for every row.
    ///
    /// Asked for a few rows, the relationship's index is the cheapest way to
    /// answer. Asked for every row of a collection, it is far cheaper to
    /// read the referring collection once and count per referenced
    /// document, so after enough rows have asked the same question in one
    /// statement the tally is built and the rest are answered from it.
    /// `None` leaves the question to the general path, which defines the
    /// answer.
    fn related_count(
        &self,
        row: &Document,
        path: &[String],
        operations: &[Operation],
    ) -> OQLResult<Option<Value>> {
        /// Rows answered through the index before a tally is worth a scan.
        const ROWS_BEFORE_TALLY: usize = 64;

        let [name] = path else {
            return Ok(None);
        };
        let Some((last, filters)) = operations.split_last() else {
            return Ok(None);
        };
        // What is asked of the related rows: how many there are, or one
        // aggregate of one of their fields.
        if !matches!(
            last,
            Operation::Count | Operation::Exists
        ) && !matches!(
            last,
            Operation::Sum(field) | Operation::Avg(field) | Operation::Min(field) | Operation::Max(field)
                if field.path.len() == 1
        ) {
            return Ok(None);
        }
        // A filter storage can apply serves any of these. One that only
        // the evaluator can apply serves a count, provided it asks nothing
        // of the row whose related rows are being counted.
        let counting = matches!(last, Operation::Count | Operation::Exists);
        if !filters.iter().all(|operation| {
                matches!(operation, Operation::Where(block)
                    if Self::is_pipeline_safe_condition(&block.condition)
                        || (counting && !Self::operation_names(operation, &row.schema)))
            })
            || !StatementGuard::active()
            || row.fields.contains_key(name)
            || DocumentSystemField::from_name(name).is_some()
        {
            return Ok(None);
        }
        // The name the tally goes under says what was asked. Every row of a
        // statement asks through the same expression, so the name is made
        // once for it: found by where the operations are, confirmed by
        // what they are.
        let place = (operations.as_ptr() as usize, operations.len());
        let remembered = STATEMENT.with(|scope| {
            scope
                .borrow()
                .related_keys
                .get(&place)
                .filter(|(asked, schema, relationship, _)| {
                    asked.as_slice() == operations && *schema == row.schema && relationship == name
                })
                .map(|(_, _, _, key)| key.clone())
        });
        let key = match remembered {
            Some(key) => key,
            None => {
                let measure = match last {
                    Operation::Count | Operation::Exists => "count".to_string(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                let key: Arc<str> = format!(
                    "{}\u{0}{}\u{0}{}\u{0}{}",
                    row.schema,
                    name,
                    serde_json::to_string(filters).unwrap_or_default(),
                    measure
                )
                .into();
                STATEMENT.with(|scope| {
                    scope.borrow_mut().related_keys.insert(
                        place,
                        (
                            operations.to_vec(),
                            row.schema.clone(),
                            name.clone(),
                            key.clone(),
                        ),
                    )
                });
                key
            }
        };
        // A tally with a filter only the evaluator can apply evaluates it
        // for every referring row, so it has to be asked for by a fair
        // share of that many rows before it is the cheaper way.
        let evaluated = !filters.iter().all(|operation| {
            matches!(operation, Operation::Where(block) if Self::is_pipeline_safe_condition(&block.condition))
        });
        let worth_it = if !evaluated {
            ROWS_BEFORE_TALLY
        } else {
            match self.reverse_relationship(&row.schema, name)? {
                ReverseRelationship::Declared { source, .. } => self
                    .store
                    .collection_len(&source)?
                    .map_or(usize::MAX, |rows| (rows / 16).max(ROWS_BEFORE_TALLY)),
                _ => usize::MAX,
            }
        };
        let known = STATEMENT.with(|scope| {
            let mut scope = scope.borrow_mut();
            match scope.related_counts.entry(key.clone()).or_insert(RelatedCounts::Asked(0)) {
                RelatedCounts::Asked(times) => {
                    *times += 1;
                    (*times > worth_it).then_some(None)
                }
                RelatedCounts::Unavailable => None,
                RelatedCounts::Ready(tally) => Some(Some(tally.clone())),
            }
        });
        let tally = match known {
            None => return Ok(None),
            Some(Some(tally)) => tally,
            Some(None) => {
                let tally = self
                    .tally_related(&row.schema, name, filters, last)?
                    .map(Arc::new);
                STATEMENT.with(|scope| {
                    scope.borrow_mut().related_counts.insert(
                        key,
                        match &tally {
                            Some(tally) => RelatedCounts::Ready(tally.clone()),
                            None => RelatedCounts::Unavailable,
                        },
                    )
                });
                match tally {
                    Some(tally) => tally,
                    None => return Ok(None),
                }
            }
        };
        let Some(reference) = self.store.object_ref_of(&row.schema, &row.id)? else {
            return Ok(None);
        };
        // A document nothing refers to has no entry: its related rows are
        // the empty collection, and each aggregate has an answer for that.
        let found = tally.get(&reference);
        Ok(Some(match last {
            Operation::Exists => {
                Value::Boolean(matches!(found, Some(Value::Integer(count)) if *count > 0))
            }
            Operation::Count | Operation::Sum(_) => found.cloned().unwrap_or(Value::Integer(0)),
            _ => found.cloned().unwrap_or(Value::Null),
        }))
    }

    /// Answer `.relationship.where { .. }.count()`, or another aggregate of
    /// one field, for one row by reading the related rows where storage
    /// holds them, without copying them into a collection first.
    ///
    /// The path may pass through several relationships, as in
    /// `.children.products.count()`. The rows are the ones the general path
    /// reaches, through the same index, so the answer is the same. `None`
    /// leaves the question to it.
    fn related_directly(
        &self,
        row: &Document,
        path: &[String],
        operations: &[Operation],
    ) -> OQLResult<Option<Value>> {
        let Some((last, filters)) = operations.split_last() else {
            return Ok(None);
        };
        let (function, measured) = match last {
            Operation::Count | Operation::Exists => ("count", None),
            Operation::Sum(field) if field.path.len() == 1 => ("sum", Some(&field.path[0])),
            Operation::Avg(field) if field.path.len() == 1 => ("avg", Some(&field.path[0])),
            Operation::Min(field) if field.path.len() == 1 => ("min", Some(&field.path[0])),
            Operation::Max(field) if field.path.len() == 1 => ("max", Some(&field.path[0])),
            _ => return Ok(None),
        };
        let mut conditions = Vec::with_capacity(filters.len());
        for operation in filters {
            match operation {
                // Any condition will do that asks nothing of the row whose
                // related rows these are: it is evaluated for each related
                // row exactly as it would be once they were copied.
                Operation::Where(block)
                    if Self::is_pipeline_safe_condition(&block.condition)
                        || !Self::operation_names(operation, &row.schema) =>
                {
                    conditions.push(&block.condition)
                }
                _ => return Ok(None),
            }
        }
        if path.is_empty() || !COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            return Ok(None);
        }
        let Some(kind) = AggregateKind::named(function, measured.is_none()) else {
            return Ok(None);
        };

        // Follow each step from every row reached so far. A step must be a
        // relationship: a stored field of that name is read differently.
        let mut reached: Option<Vec<Arc<Document>>> = None;
        let mut source_schema = String::new();
        for segment in path {
            if DocumentSystemField::from_name(segment).is_some() {
                return Ok(None);
            }
            let mut next = Vec::new();
            let mut follow = |document: &Document| -> OQLResult<bool> {
                if document.fields.contains_key(segment) {
                    return Ok(false);
                }
                let ReverseRelationship::Declared { source, field } =
                    self.reverse_relationship(&document.schema, segment)?
                else {
                    return Ok(false);
                };
                next.extend(self.store.scan_referencing_documents_shared(
                    &document.schema,
                    &document.id,
                    Some(&source),
                    field.as_deref(),
                )?);
                if source_schema != source {
                    source_schema = source;
                }
                Ok(true)
            };
            let followed = match &reached {
                None => follow(row)?,
                Some(documents) => {
                    let mut all = true;
                    for document in documents {
                        if !follow(document)? {
                            all = false;
                            break;
                        }
                    }
                    all
                }
            };
            if !followed {
                return Ok(None);
            }
            reached = Some(next);
        }
        let related = reached.unwrap_or_default();

        // An aggregate of a field is taken only where the field's declared
        // type makes it one the accumulator and the evaluator agree on.
        if let Some(name) = measured {
            let Some(schema) = self.schema_of(&source_schema)? else {
                return Ok(None);
            };
            let mut declared = schema.fields.get(name);
            while let Some(FieldType::Optional(inner)) = declared {
                declared = Some(inner);
            }
            let fits = match declared {
                Some(FieldType::Integer | FieldType::Float) => true,
                Some(FieldType::String | FieldType::DateTime) => matches!(function, "min" | "max"),
                _ => false,
            };
            if !fits || related.iter().any(|document| document.schema != source_schema) {
                return Ok(None);
            }
        }

        let mut accumulator = Accumulator::new(kind);
        'related: for document in &related {
            for condition in &conditions {
                if !self.evaluate_condition(document, condition)? {
                    continue 'related;
                }
            }
            match measured {
                None => accumulator.count += 1,
                Some(name) => {
                    accumulator.push(document.fields.get(name).map(ProjectedValueRef::Value))?
                }
            }
            // One row is enough to know that some exist.
            if matches!(last, Operation::Exists) {
                break;
            }
        }
        Ok(Some(match last {
            Operation::Exists => Value::Boolean(accumulator.count > 0),
            _ => accumulator.finish(),
        }))
    }

    /// Count, or aggregate one field of, the documents that refer to each
    /// document of `target` through the relationship `name` and pass
    /// `filters`, in one pass over the referring collection. `None` when one
    /// pass cannot give the answer the relationship's index gives.
    fn tally_related(
        &self,
        target: &str,
        name: &str,
        filters: &[Operation],
        measure: &Operation,
    ) -> OQLResult<Option<HashMap<ObjRef, Value>>> {
        let ReverseRelationship::Declared { source, field } =
            self.reverse_relationship(target, name)?
        else {
            return Ok(None);
        };
        let Some(schema) = self.schema_of(&source)? else {
            return Ok(None);
        };
        // The relationship must run through exactly one plain reference
        // field. Through a list, an embedded object, or several fields, a
        // document can refer to its target in ways one column does not show.
        fn mentions(field_type: &FieldType, target: &str) -> bool {
            match field_type {
                FieldType::Reference(reference) => reference.target_schema == target,
                FieldType::Optional(inner) | FieldType::List(inner) => mentions(inner, target),
                _ => false,
            }
        }
        fn plain(field_type: &FieldType, target: &str) -> bool {
            match field_type {
                FieldType::Reference(reference) => reference.target_schema == target,
                FieldType::Optional(inner) => plain(inner, target),
                _ => false,
            }
        }
        let embedded = schema.constraints.iter().any(|constraint| {
            matches!(constraint, crate::schema::SchemaConstraint::Shape { fields, .. }
                if fields.iter().any(|(_, field_type)| mentions(field_type, target)))
        });
        let referring: Vec<&String> = schema
            .fields
            .iter()
            .filter(|(_, field_type)| mentions(field_type, target))
            .map(|(field, _)| field)
            .collect();
        let through = match (&field, referring.as_slice()) {
            (Some(field), _) if schema.fields.get(field).is_some_and(|declared| plain(declared, target)) => {
                field.clone()
            }
            (None, [only]) if !embedded && plain(&schema.fields[*only], target) => (*only).clone(),
            _ => return Ok(None),
        };
        // An aggregate of a field must be one every row can take part in:
        // one pass meets every row, where the index meets only the rows of
        // the documents asked about, so a value that fails must be
        // impossible rather than merely absent so far.
        let (function, measured) = match measure {
            Operation::Sum(field) => ("sum", Some(field)),
            Operation::Avg(field) => ("avg", Some(field)),
            Operation::Min(field) => ("min", Some(field)),
            Operation::Max(field) => ("max", Some(field)),
            _ => ("count", None),
        };
        if let Some(field) = measured {
            let mut declared = field.path.first().and_then(|name| schema.fields.get(name));
            while let Some(FieldType::Optional(inner)) = declared {
                declared = Some(inner);
            }
            let fits = match declared {
                Some(FieldType::Integer | FieldType::Float) => true,
                Some(FieldType::String | FieldType::DateTime) => {
                    matches!(function, "min" | "max")
                }
                _ => false,
            };
            if !fits {
                return Ok(None);
            }
        }
        // One pass over the referring collection, counting or aggregating
        // per referenced document. The tests switch this off with the other
        // column paths to get the answer the index gives.
        if !COLUMN_AGGREGATION.with(|enabled| enabled.get()) || self.store.has_pending_writes()? {
            return Ok(None);
        }
        let (predicates, residual) = Self::split_storage_filters(&source, &schema, filters);
        if !residual.is_empty() {
            // A filter storage cannot apply: count with the evaluator, over
            // the rows where the store holds them.
            if measured.is_some() {
                return Ok(None);
            }
            let Some(stored) = self.store.scan_documents_shared(&source)? else {
                return Ok(None);
            };
            let mut tally: HashMap<ObjRef, i64> = HashMap::new();
            'rows: for row in stored.iter() {
                let Some(Value::Reference(reference)) = row.fields.get(&through) else {
                    continue;
                };
                for operation in filters {
                    let Operation::Where(block) = operation else {
                        return Ok(None);
                    };
                    // A row that cannot be judged might belong to a
                    // document nobody asks about; leave it to the route
                    // that looks only at the rows asked for.
                    match self.evaluate_condition(row, &block.condition) {
                        Ok(true) => {}
                        Ok(false) => continue 'rows,
                        Err(_) => return Ok(None),
                    }
                }
                *tally.entry(*reference).or_insert(0) += 1;
            }
            return Ok(Some(
                tally
                    .into_iter()
                    .map(|(reference, count)| (reference, Value::Integer(count)))
                    .collect(),
            ));
        }
        let Some(kind) = AggregateKind::named(function, measured.is_none()) else {
            return Ok(None);
        };
        // Counting needs only which document each row refers to. When an
        // index holds that after the filtered fields, it answers without a
        // row being read.
        if measured.is_none() {
            if let Some(tally) = self.tally_from_index(&source, &through, &predicates)? {
                return Ok(Some(tally));
            }
        }
        let mut fields = vec![through.clone()];
        fields.extend(measured.and_then(|field| field.path.first().cloned()));
        let Some(batches) =
            self.store
                .scan_projected_batches(&source, &predicates, &fields, 4096)?
        else {
            return Ok(None);
        };
        let mut tally: HashMap<ObjRef, Accumulator> = HashMap::new();
        for batch in &batches {
            for row in batch.rows() {
                let (mut referenced, mut value) = (None, None);
                for (name, cell) in row.fields() {
                    if name == through {
                        referenced = Some(cell);
                    } else {
                        value = Some(cell);
                    }
                }
                let reference = match referenced {
                    Some(ProjectedValueRef::Reference(reference)) => reference,
                    Some(ProjectedValueRef::Value(Value::Reference(reference))) => *reference,
                    None | Some(ProjectedValueRef::Null | ProjectedValueRef::Value(Value::Null)) => {
                        continue
                    }
                    // A value that is not a reference is not something this
                    // tally understands.
                    Some(_) => return Ok(None),
                };
                let accumulator = tally
                    .entry(reference)
                    .or_insert_with(|| Accumulator::new(kind));
                match measured {
                    None => accumulator.count += 1,
                    Some(_) => accumulator.push(value)?,
                }
            }
        }
        Ok(Some(
            tally
                .into_iter()
                .map(|(reference, accumulator)| (reference, accumulator.finish()))
                .collect(),
        ))
    }

    /// Count referring rows per referenced document from an index alone.
    ///
    /// The filters must all be equalities, and some index of the referring
    /// collection must be on exactly those fields followed by the reference:
    /// `index(.status, .customer)` for `.orders.where { .status == "paid" }`.
    /// Its entries under the filtered values then name, in their keys, the
    /// document each matching row refers to. `None` when no index fits.
    fn tally_from_index(
        &self,
        source: &str,
        through: &str,
        predicates: &[ScanPredicate],
    ) -> OQLResult<Option<HashMap<ObjRef, Value>>> {
        use crate::storage::{ScalarIndexBound, ScalarIndexDirection, ScalarIndexRange};

        fn equalities<'a>(
            predicate: &'a ScanPredicate,
            found: &mut Vec<(&'a str, &'a Value)>,
        ) -> bool {
            match predicate {
                ScanPredicate::Comparison {
                    field,
                    operator: ComparisonOperator::Equal,
                    value,
                } => {
                    found.push((field.as_str(), value));
                    true
                }
                ScanPredicate::And(left, right) => {
                    equalities(left, found) && equalities(right, found)
                }
                _ => false,
            }
        }
        let mut equal = Vec::new();
        if !predicates
            .iter()
            .all(|predicate| equalities(predicate, &mut equal))
        {
            return Ok(None);
        }
        // One value per field: `.a == 1 && .a == 2` is not a prefix.
        let distinct: HashSet<&str> = equal.iter().map(|(field, _)| *field).collect();
        if distinct.len() != equal.len() {
            return Ok(None);
        }
        let plain = |field: &String| field.trim_start_matches('.').to_string();
        let indexes = self.store.list_scalar_indexes(Some(source))?;
        let Some(index) = indexes.iter().find(|index| {
            index.fields.len() == equal.len() + 1
                && index.fields.last().map(plain).as_deref() == Some(through)
                && index.fields[..equal.len()]
                    .iter()
                    .all(|field| distinct.contains(plain(field).as_str()))
        }) else {
            return Ok(None);
        };
        let prefix: Vec<Value> = index.fields[..equal.len()]
            .iter()
            .filter_map(|field| {
                let name = plain(field);
                equal
                    .iter()
                    .find(|(candidate, _)| *candidate == name)
                    .map(|(_, value)| (*value).clone())
            })
            .collect();
        if prefix.len() != equal.len() {
            return Ok(None);
        }
        let bound = |prefix_end: bool| {
            (!prefix.is_empty()).then(|| ScalarIndexBound {
                values: prefix.clone(),
                inclusive: !prefix_end,
                prefix_end,
            })
        };
        let mut range = ScalarIndexRange {
            lower: bound(false),
            upper: bound(true),
            limit: None,
            after: None,
            direction: ScalarIndexDirection::Ascending,
        };
        let mut tally: HashMap<ObjRef, i64> = HashMap::new();
        loop {
            let Some(page) = self.store.scan_scalar_index(source, &index.name, &range)? else {
                return Ok(None);
            };
            for key in &page.encoded_keys {
                match crate::storage_engine::TypedKey::decode_component(key, equal.len())? {
                    Value::Reference(reference) => *tally.entry(reference).or_insert(0) += 1,
                    Value::Null => {}
                    // A reference in another form is not one this counts by.
                    _ => return Ok(None),
                }
            }
            match page.next_cursor {
                Some(cursor) => range.after = Some(cursor),
                None => break,
            }
        }
        INDEX_TALLIES_RUN.with(|count| count.set(count.get() + 1));
        Ok(Some(
            tally
                .into_iter()
                .map(|(reference, count)| (reference, Value::Integer(count)))
                .collect(),
        ))
    }

    /// Apply operations to rows already in hand and shape the result as a
    /// value: a scalar for an aggregate, one object for a single row, and a
    /// list otherwise.
    fn run_collection_operations(
        &self,
        mut documents: Vec<Document>,
        mut single: bool,
        operations: &[Operation],
        scope_document: &Document,
        collection_scope: &str,
    ) -> OQLResult<Value> {
        // `first()` narrows a collection to one row; whatever shapes that
        // row afterwards still yields one value, not a list of one.
        single |= operations
            .iter()
            .any(|operation| matches!(operation, Operation::First));

        for (position, operation) in operations.iter().enumerate() {
            // With no rows there is nothing for the enclosing row's values
            // to be compared with, so the operation is used as written.
            let materialized = if (documents.is_empty()
                && !matches!(operation, Operation::Combine { .. }))
                || !Self::operation_names(operation, &scope_document.schema)
            {
                std::borrow::Cow::Borrowed(operation)
            } else {
                std::borrow::Cow::Owned(
                    self.materialize_operation_with_scope(operation, scope_document)?,
                )
            };
            documents = match materialized.as_ref() {
                Operation::Sort(sort) => self.sort_keeping(
                    documents,
                    sort,
                    Self::rows_kept_after_sort(operations, position),
                )?,
                // What follows a step says which fields of the rows it
                // reaches are read; the rest need not be copied.
                Operation::Step(path) => {
                    let read = self.fields_read_by_operations(collection_scope, &operations[position + 1..]);
                    let mut reached = Vec::new();
                    for document in &documents {
                        reached.extend(
                            self.path_source_rows_reading(document, &path.path, read.as_deref())?.0,
                        );
                    }
                    reached
                }
                Operation::Walk { path, depth, every, paths } => {
                    let read = self.fields_read_by_operations(collection_scope, &operations[position + 1..]);
                    self.apply_walk(documents, path, depth.as_ref(), read.as_deref(), *every, *paths)?
                }
                other => self.apply_operation(documents, other, collection_scope)?,
            };
        }

        let aggregate_field = operations.last().and_then(Self::scalar_result_field);
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
                match document.fields.get(SCALAR_ROW_FIELD) {
                    Some(value) => value.clone(),
                    None => Value::Object(document.fields),
                }
            })
            .collect::<Vec<_>>();
        if single {
            Ok(rows.pop().unwrap_or(Value::Null))
        } else {
            Ok(Value::List(rows))
        }
    }

    /// The field that holds the answer of an operation which reduces a
    /// collection to one value.
    fn scalar_result_field(operation: &Operation) -> Option<&str> {
        match operation {
            Operation::Count => Some("count"),
            Operation::Sum(_) => Some("sum"),
            Operation::Avg(_) => Some("avg"),
            Operation::Min(_) => Some("min"),
            Operation::Max(_) => Some("max"),
            Operation::Exists => Some("exists"),
            Operation::AggregateExpr { function, .. } => Some(function.as_str()),
            _ => None,
        }
    }

    /// Expand one value into the rows it contributes to a collection.
    ///
    /// A reference contributes the document it points at, so the row keeps
    /// its own schema and identity and can be traversed further. An object
    /// contributes itself, a list contributes each element, a scalar
    /// contributes a one-field row, and null contributes nothing.
    fn push_source_rows(
        &self,
        value: Value,
        scope: &str,
        rows: &mut Vec<SourceRow>,
    ) -> OQLResult<()> {
        match value {
            Value::Null => {}
            Value::List(items) => {
                for item in items {
                    self.push_source_rows(item, scope, rows)?;
                }
            }
            Value::Reference(reference) => {
                if let Some(document) = self.resolve_objref_target(&reference)? {
                    rows.push(SourceRow::stored(document));
                }
            }
            Value::GlobalReference(reference) => {
                if let Some(document) = self.store.navigate_global_reference(&reference)? {
                    rows.push(SourceRow::stored(document));
                }
            }
            Value::Object(fields) => rows.push(SourceRow::embedded(Document::with_id(
                rows.len().to_string(),
                scope.to_string(),
                fields,
            ))),
            scalar => rows.push(SourceRow::embedded(Document::with_id(
                rows.len().to_string(),
                scope.to_string(),
                crate::document::Fields::from_iter([(SCALAR_ROW_FIELD.to_string(), scalar)]),
            ))),
        }
        Ok(())
    }

    /// Resolve a path to the rows it denotes, starting from one row.
    ///
    /// Each step applies to every row reached so far and the results are
    /// concatenated, so a step through a collection maps and flattens:
    /// `.teams.users` on an organisation is every user of every team. A step
    /// names a stored field when the row has one, and otherwise the reverse
    /// relationship from the collection of that name. The second result is
    /// true when the path can only ever denote a single row (it never passed
    /// through a list or a reverse relationship).
    fn path_source_rows(
        &self,
        scope_document: &Document,
        path: &[String],
    ) -> OQLResult<(Vec<Document>, bool)> {
        self.path_source_rows_reading(scope_document, path, None)
    }

    /// `path_source_rows`, told which fields of the rows at the end of the
    /// path will be read. Rows reached through a relationship in the last
    /// step then carry only those fields, which spares copying the rest.
    fn path_source_rows_reading(
        &self,
        scope_document: &Document,
        path: &[String],
        fields: Option<&[String]>,
    ) -> OQLResult<(Vec<Document>, bool)> {
        let last = path.len().saturating_sub(1);
        // The first step reads the starting row where it is; only the rows
        // reached are copied.
        let mut rows: Vec<SourceRow> = Vec::new();
        let mut single = true;
        for (position, segment) in path.iter().enumerate() {
            let wanted = fields.filter(|_| position == last);
            let mut next = Vec::new();
            if position == 0 {
                self.step_one_row(scope_document, true, segment, wanted, &mut next, &mut single)?;
            } else {
                for row in &rows {
                    self.step_one_row(row.row(), row.stored, segment, wanted, &mut next, &mut single)?;
                }
            }
            rows = next;
        }
        if path.is_empty() {
            rows.push(SourceRow::stored(scope_document.clone()));
        }
        Ok((rows.into_iter().map(SourceRow::into_document).collect(), single))
    }

    /// The rows one step of a path reaches from one row.
    fn step_one_row(
        &self,
        document: &Document,
        stored: bool,
        segment: &String,
        wanted: Option<&[String]>,
        next: &mut Vec<SourceRow>,
        single: &mut bool,
    ) -> OQLResult<()> {
        // The objects of an embedded list, when only some of their fields
        // will be read, are copied down to those fields.
        if let (Some(wanted), Some(Value::List(items))) = (wanted, document.get_field(segment)) {
            *single = false;
            for item in items {
                match item {
                    Value::Object(fields) => next.push(SourceRow::embedded(Document::with_id(
                        next.len().to_string(),
                        segment.to_string(),
                        wanted
                            .iter()
                            .filter_map(|name| {
                                fields
                                    .get_key_value(name)
                                    .map(|(name, value)| (name.clone(), value.clone()))
                            })
                            .collect(),
                    ))),
                    other => self.push_source_rows(other.clone(), segment, next)?,
                }
            }
            return Ok(());
        }
        let stored_value = match DocumentSystemField::from_name(segment) {
            Some(system_field) if stored => Some(system_field.value(document)),
            _ => document.get_field(segment).cloned(),
        };
        // A referenced row is a stepping stone as often as a destination,
        // so it is borrowed from the store and copied only if it is kept.
        if let Some(Value::Reference(reference)) = &stored_value {
            if let Some(target) = self.referenced_document(reference)? {
                next.push(SourceRow::lent(target));
                return Ok(());
            }
        }
        match stored_value {
            Some(value) => {
                *single &= !matches!(value, Value::List(_));
                self.push_source_rows(value, segment, next)?;
            }
            // An embedded value has no identity for anything to
            // refer to, so an absent field is simply absent.
            None if !stored => {}
            None => match self.reverse_relationship(&document.schema, segment)? {
                relationship @ (ReverseRelationship::Declared { .. }
                | ReverseRelationship::Unknowable) => {
                    let (source, field) = match &relationship {
                        ReverseRelationship::Declared { source, field } => {
                            (source.as_str(), field.as_deref())
                        }
                        _ => (segment.as_str(), None),
                    };
                    *single = false;
                    let related = self.store.scan_referencing_documents_shared(
                        &document.schema,
                        &document.id,
                        Some(source),
                        field,
                    )?;
                    next.extend(related.iter().map(|document| {
                        SourceRow::stored(match wanted {
                            None => document.as_ref().clone(),
                            Some(wanted) => Document {
                                fields: wanted
                                    .iter()
                                    .filter_map(|name| {
                                        document
                                            .fields
                                            .get_key_value(name)
                                            .map(|(name, value)| (name.clone(), value.clone()))
                                    })
                                    .collect(),
                                ..Document::with_id(
                                    document.id.to_string(),
                                    document.schema.clone(),
                                    crate::document::Fields::default(),
                                )
                            },
                        })
                    }));
                }
                // A declared field that this row does not carry.
                ReverseRelationship::AbsentField => {}
                ReverseRelationship::Unknown => {
                    return Err(OQLError::SchemaValidation {
                        message: format!(
                            "'{}' has no field '{}', and no collection named '{}' \
                             references it",
                            document.schema, segment, segment
                        ),
                    });
                }
            },
        }
        Ok(())
    }

    /// Decide what an absent name on a stored row means.
    fn reverse_relationship(&self, schema: &str, name: &str) -> OQLResult<ReverseRelationship> {
        let key = (schema.to_string(), name.to_string());
        if let Some(known) =
            STATEMENT.with(|scope| scope.borrow().reverse_relationships.get(&key).cloned())
        {
            return Ok(known);
        }
        let relationship = self.resolve_reverse_relationship(schema, name)?;
        if StatementGuard::active() {
            STATEMENT.with(|scope| {
                scope
                    .borrow_mut()
                    .reverse_relationships
                    .insert(key, relationship.clone())
            });
        }
        Ok(relationship)
    }

    fn resolve_reverse_relationship(
        &self,
        schema: &str,
        name: &str,
    ) -> OQLResult<ReverseRelationship> {
        let Some(declared) = self.schema_of(schema)? else {
            // Without a schema nothing says which fields exist, so the name
            // may be an absent field or a relationship; reading it is
            // graceful either way.
            return Ok(ReverseRelationship::Unknowable);
        };
        if declared.fields.contains_key(name) {
            return Ok(ReverseRelationship::AbsentField);
        }
        fn references(field_type: &FieldType, target: &str) -> bool {
            match field_type {
                FieldType::Reference(reference) => reference.target_schema == target,
                FieldType::Optional(inner) | FieldType::List(inner) => references(inner, target),
                _ => false,
            }
        }
        // A relationship named with `as` takes precedence over the name of
        // a collection.
        for source in self.schema_names()?.iter().cloned() {
            let Some(source_schema) = self.schema_of(&source)? else {
                continue;
            };
            for (field, field_type) in &source_schema.fields {
                if references(field_type, schema) && source_schema.reverse_name(field) == Some(name)
                {
                    return Ok(ReverseRelationship::Declared {
                        source,
                        field: Some(field.clone()),
                    });
                }
            }
        }
        let refers = self
            .schema_of(name)?
            .is_some_and(|source| source.fields.values().any(|field| references(field, schema)));
        Ok(if refers {
            ReverseRelationship::Declared {
                source: name.to_string(),
                field: None,
            }
        } else {
            ReverseRelationship::Unknown
        })
    }

    fn evaluate_query_expression(
        &self,
        scope_document: &Document,
        query: &crate::parser::OQLQuery,
    ) -> OQLResult<Value> {
        let materialized_query = self.materialize_query_with_scope(query, scope_document)?;
        // Once the enclosing row's values are substituted, the nested query
        // is self-contained. If another row already asked the same question
        // in this statement, the answer is the same: an uncorrelated nested
        // query runs once, and a correlated one runs once per distinct
        // value it is correlated on. A query that reads function parameters
        // is not self-contained, so it is never shared.
        let memo_key = (StatementGuard::active() && !CallFrame::any_active())
            .then(|| serde_json::to_string(&materialized_query).ok())
            .flatten();
        if let Some(key) = &memo_key {
            if let Some(known) =
                STATEMENT.with(|scope| scope.borrow().nested_results.get(key).cloned())
            {
                return Ok(known);
            }
        }
        let value = self.evaluate_materialized_query(&materialized_query)?;
        if let Some(key) = memo_key {
            STATEMENT.with(|scope| {
                scope
                    .borrow_mut()
                    .nested_results
                    .insert(key, value.clone())
            });
        }
        Ok(value)
    }

    fn evaluate_materialized_query(
        &self,
        materialized_query: &crate::parser::OQLQuery,
    ) -> OQLResult<Value> {
        NESTED_QUERIES_RUN.with(|count| count.set(count.get() + 1));
        let docs = self.execute_query(materialized_query)?;
        let aggregate_field = materialized_query
            .operations
            .last()
            .and_then(Self::scalar_result_field);
        if let Some(field) = aggregate_field {
            let value = docs
                .first()
                .and_then(|doc| doc.fields.get(field))
                .cloned()
                .unwrap_or(Value::Null);
            return Ok(value);
        }

        // A row is its fields, exactly as when the same rows are reached
        // through a path. Rows that were not shaped by a `select` also keep
        // their identity, so `orders.first().id` works for generated ids.
        let projected = materialized_query
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::Select(_) | Operation::GroupBy(_)));
        let rows: Vec<Value> = docs
            .into_iter()
            .map(|doc| match doc.fields.get(SCALAR_ROW_FIELD) {
                Some(scalar) => scalar.clone(),
                None if projected => Value::Object(doc.fields),
                None => Self::row_as_value(&doc),
            })
            .collect();
        if materialized_query
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::First))
        {
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
            Operation::AggregateExpr { function, input } => Operation::AggregateExpr {
                function: function.clone(),
                input: self.materialize_expression_with_scope(input, scope_document)?,
            },
            Operation::GroupBy(block) => Operation::GroupBy(GroupByBlock {
                fields: block.fields.clone(),
                computed: block
                    .computed
                    .iter()
                    .map(|key| {
                        Ok(crate::parser::ComputedGroupKey {
                            name: key.name.clone(),
                            expression: self
                                .materialize_expression_with_scope(&key.expression, scope_document)?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
            }),
            Operation::Nearest(block) => Operation::Nearest(crate::parser::NearestBlock {
                field: block.field.clone(),
                query: self.materialize_expression_with_scope(&block.query, scope_document)?,
                metric: block.metric.clone(),
                k: block.k,
                num_candidates: block.num_candidates,
                exact: block.exact,
            }),
            Operation::Combine { how, other } => Operation::Combine {
                how: how.clone(),
                other: self.materialize_expression_with_scope(other, scope_document)?,
            },
            Operation::Walk { path, depth, every, paths } => Operation::Walk {
                path: path.clone(),
                depth: depth
                    .as_ref()
                    .map(|depth| self.materialize_expression_with_scope(depth, scope_document))
                    .transpose()?,
                every: *every,
                paths: *paths,
            },
            Operation::PartitionBy(block) => Operation::PartitionBy(GroupByBlock {
                fields: block.fields.clone(),
                computed: block
                    .computed
                    .iter()
                    .map(|key| {
                        Ok(crate::parser::ComputedGroupKey {
                            name: key.name.clone(),
                            expression: self
                                .materialize_expression_with_scope(&key.expression, scope_document)?,
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?,
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
        // A row parameter of a function is read by the parameter's name
        // where an enclosing row is read by its collection's.
        let named = ROW_READ_AS.with(|name| name.borrow().clone());
        match named {
            Some(parameter) => {
                if let Some(path) = Self::extract_scoped_path(expr, &parameter).filter(|path| !path.is_empty()) {
                    // Only what has one plain value becomes a constant; a
                    // path to many rows is left for the function's own scope.
                    MANY_VALUED.with(|flag| flag.set(false));
                    let value = self.get_field_value(scope_document, &FieldAccess { path: path.clone() })?;
                    let many = MANY_VALUED.with(|flag| flag.replace(false));
                    if !many && !matches!(value, Value::List(_) | Value::Object(_)) {
                        return self.scoped_path_to_literal_expression(scope_document, &path);
                    }
                    return Ok(expr.clone());
                }
            }
            None => {
                if let Some(path) = Self::extract_scoped_path(expr, &scope_document.schema) {
                    return self.scoped_path_to_literal_expression(scope_document, &path);
                }
            }
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

        // The enclosing row's value becomes a constant of the nested query.
        // A reference is carried as the id of its target, which is what a
        // reference compares equal to; an absent value is null, which
        // matches nothing, so a nested query over it is simply empty.
        match value {
            Value::DateTime(instant) => Ok(Expression::Literal(Literal::String(instant.to_rfc3339()))),
            reference @ (Value::Reference(_) | Value::GlobalReference(_)) => {
                Self::value_as_expression(self.reference_as_id(&reference)?)
            }
            other => Self::value_as_expression(other),
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

    /// The value of `param.a.b` for a collection parameter: what the path
    /// gives for each of its rows. Kept out of the evaluator's own frame.
    #[inline(never)]
    fn parameter_values(&self, expression: &Expression) -> OQLResult<Option<Value>> {
        let Some((rows, path)) = Self::parameter_rows(expression) else {
            return Ok(None);
        };
        let access = FieldAccess { path };
        let mut values = Vec::with_capacity(rows.len());
        for row in rows.iter() {
            match self.get_field_value(row, &access)? {
                Value::Null => {}
                Value::List(more) => values.extend(more),
                value => values.push(value),
            }
        }
        MANY_VALUED.with(|flag| flag.set(true));
        Ok(Some(Value::List(values)))
    }

    /// Recognise `param.a.b` where `param` is a parameter of the function
    /// being evaluated that holds rows: a collection, or one row.
    fn parameter_rows(expression: &Expression) -> Option<(Arc<Vec<Document>>, Vec<String>)> {
        let mut path = Vec::new();
        let mut current = expression;
        while let Expression::ChainedAccess { base, field } = current {
            path.push(field.clone());
            current = base;
        }
        let Expression::Identifier(name) = current else {
            return None;
        };
        if path.is_empty() {
            return None;
        }
        path.reverse();
        let rows = match CallFrame::collection(name) {
            Some(rows) => rows,
            None => Arc::new(vec![CallFrame::row(name)?]),
        };
        Some((rows, path))
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
            // Lists concatenate, which is how an update appends:
            // `tags: .tags + ["new"]`. An absent list is empty.
            (Value::List(a), Value::List(b)) if matches!(operator, BinaryOperator::Add) => {
                Ok(Value::List(a.iter().chain(b).cloned().collect()))
            }
            (Value::Null, Value::List(only)) | (Value::List(only), Value::Null)
                if matches!(operator, BinaryOperator::Add) =>
            {
                Ok(Value::List(only.clone()))
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
            // A negative index counts from the end: `[-1]` is the last
            // element. Anything out of range has no element.
            (Value::List(list), Value::Integer(index)) => {
                Ok(Self::resolve_index(*index, list.len())
                    .map(|position| list[position].clone())
                    .unwrap_or(Value::Null))
            }
            (Value::String(s), Value::Integer(index)) => {
                let chars: Vec<char> = s.chars().collect();
                Ok(Self::resolve_index(*index, chars.len())
                    .map(|position| Value::String(chars[position].to_string()))
                    .unwrap_or(Value::Null))
            }
            // Indexing is a read, and reads are null-safe: an absent list
            // or an absent index has no element.
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            _ => Err(OQLError::FunctionError {
                message: format!(
                    "cannot index a {} with a {}",
                    array_value.type_name(),
                    index_value.type_name()
                ),
            }),
        }
    }

    fn resolve_index(index: i64, length: usize) -> Option<usize> {
        let position = if index < 0 {
            index.checked_add(i64::try_from(length).ok()?)?
        } else {
            index
        };
        usize::try_from(position)
            .ok()
            .filter(|position| *position < length)
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
                // The empty path is `.`: the current row or value itself.
                return Ok(match document.fields.get(SCALAR_ROW_FIELD) {
                    Some(value) => value.clone(),
                    None => Self::row_as_value(document),
                });
            }

            // Handle single-level field access directly with optimized lookup
            if field_access.path.len() == 1 {
                let field_name = &field_access.path[0];

                // Handle special fields
                if let Some(system_field) = DocumentSystemField::from_name(field_name) {
                    return Ok(system_field.value(document));
                }
                // Use optimized field access with caching.
                // Read the document itself. A cache keyed by document id
                // and field name cannot tell two versions of a document,
                // or two collections, apart.
                return match document.get_field(field_name) {
                    Some(value) => Ok(value.clone()),
                    // A name the row does not hold may be a relationship:
                    // then its value is the rows that refer to this one.
                    None if document.schema == "group"
                        || self.is_relationship(&document.schema, field_name)? =>
                    {
                        Ok(self
                            .many_valued(document, &field_access.path)?
                            .unwrap_or(Value::Null))
                    }
                    None => Ok(Value::Null),
                };
            }

            // Grouped rows flatten grouped key paths into underscore-separated fields
            // like `team_id` and `team_name`. Allow `.team.id` to resolve against
            // those synthetic fields during the subsequent select phase.
            if document.schema == "group" {
                let flattened_field_name = field_access.path.join("_");
                if let Some(value) = document.get_field(&flattened_field_name).cloned() {
                    return Ok(value);
                }
            }

            // Handle multi-level field access with implicit null handling
            match self.resolve_nested_field_path(document, &field_access.path)? {
                // A path that came to nothing may have met a collection on
                // the way, which the single-valued walk does not go through.
                Value::Null => Ok(self
                    .many_valued(document, &field_access.path)?
                    .unwrap_or(Value::Null)),
                value => Ok(value),
            }
        })
    }

    /// The value of a path that passes through a collection: a list with
    /// what the rest of the path gives for each row reached.
    ///
    /// `.assignments.project.id` on a person is the id of the project of
    /// each of that person's assignments, and `.worklog.minutes` on a task
    /// is the minutes of each entry. Values that are absent are left out.
    /// `None` when the path meets no collection, so it has a single value.
    fn many_valued(&self, document: &Document, path: &[String]) -> OQLResult<Option<Value>> {
        // On a group row, a path that is not one of the group's keys is
        // read from the rows the group stands for.
        if document.schema == "group" {
            if document.fields.contains_key(&path[0])
                || document.fields.contains_key(&path.join("_"))
                || !document.fields.contains_key("_group_rows")
            {
                return Ok(None);
            }
            let rows = Self::group_rows(document)?;
            let access = FieldAccess { path: path.to_vec() };
            let mut values = Vec::with_capacity(rows.len());
            for row in rows.iter() {
                match self.get_field_value(row, &access)? {
                    Value::Null => {}
                    Value::List(more) => values.extend(more),
                    value => values.push(value),
                }
            }
            MANY_VALUED.with(|flag| flag.set(true));
            return Ok(Some(Value::List(values)));
        }
        // Where the path first meets a collection: a relationship, or a
        // list with more of the path to follow.
        let mut held: Option<Arc<Document>> = None;
        let mut fans_at = None;
        for (position, segment) in path.iter().enumerate() {
            let row: &Document = held.as_deref().unwrap_or(document);
            if DocumentSystemField::from_name(segment).is_some() {
                break;
            }
            match row.fields.get(segment) {
                Some(Value::List(_)) if position + 1 < path.len() => {
                    fans_at = Some(position);
                    break;
                }
                Some(Value::Reference(reference)) => match self.referenced_document(reference)? {
                    Some(target) => held = Some(target),
                    None => break,
                },
                Some(_) => break,
                None => {
                    if self.is_relationship(&row.schema, segment)? {
                        fans_at = Some(position);
                    }
                    break;
                }
            }
        }
        let Some(fans_at) = fans_at else {
            return Ok(None);
        };
        let (rows, _) = self.path_source_rows(document, &path[..=fans_at])?;
        let rest = &path[fans_at + 1..];
        let mut values = Vec::with_capacity(rows.len());
        for row in &rows {
            if rest.is_empty() {
                values.push(match row.fields.get(SCALAR_ROW_FIELD) {
                    Some(scalar) => scalar.clone(),
                    None => Self::row_as_value(row),
                });
                continue;
            }
            match self.get_field_value(row, &FieldAccess { path: rest.to_vec() })? {
                Value::Null => {}
                // A list at the end, or a further collection on the way:
                // each step through many rows flattens.
                Value::List(more) => values.extend(more),
                value => values.push(value),
            }
        }
        MANY_VALUED.with(|flag| flag.set(true));
        Ok(Some(Value::List(values)))
    }

    /// Whether `name`, which a row of `schema` does not hold, is a
    /// relationship to it. Asked for every row that lacks a field, so the
    /// answer is kept for the statement and found without copying a name.
    fn is_relationship(&self, schema: &str, name: &str) -> OQLResult<bool> {
        if !StatementGuard::active() || schema.is_empty() {
            return Ok(false);
        }
        if let Some(known) = STATEMENT.with(|scope| {
            scope
                .borrow()
                .relationship_names
                .get(schema)
                .and_then(|names| names.get(name))
                .copied()
        }) {
            return Ok(known);
        }
        let related = matches!(
            self.reverse_relationship(schema, name)?,
            ReverseRelationship::Declared { .. }
        );
        STATEMENT.with(|scope| {
            scope
                .borrow_mut()
                .relationship_names
                .entry(schema.to_string())
                .or_default()
                .insert(name.to_string(), related)
        });
        Ok(related)
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
        object: &crate::document::Fields,
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
                // The row referred to is read where the store holds it.
                if let Some(target) = self.referenced_document(&reference)? {
                    return if remaining.is_empty() {
                        Ok(PathState::Present(Value::Reference(reference)))
                    } else {
                        self.resolve_path_state(&target, remaining)
                    };
                }
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

    /// The document a reference points at, fetched once per statement.
    /// Many rows usually refer to few targets (orders to their customers),
    /// so traversing `.customer.country` for every row need not decode the
    /// same customer again each time.
    fn referenced_document(&self, reference: &ObjRef) -> OQLResult<Option<Arc<Document>>> {
        // A null reference points nowhere, and a reference the store cannot
        // follow is left to the slower lookup by id.
        let follow = || -> Option<Arc<Document>> {
            if reference.is_null() {
                return None;
            }
            self.store.navigate_reference_shared(reference).ok().flatten()
        };
        if !StatementGuard::active() {
            return Ok(follow());
        }
        if let Some(known) =
            STATEMENT.with(|scope| scope.borrow().referenced.get(reference).cloned())
        {
            return Ok(known);
        }
        let document = follow();
        STATEMENT.with(|scope| {
            scope
                .borrow_mut()
                .referenced
                .insert(*reference, document.clone())
        });
        Ok(document)
    }

    fn resolve_objref_target(&self, reference: &ObjRef) -> OQLResult<Option<Document>> {
        if let Some(document) = self.referenced_document(reference)? {
            return Ok(Some(document.as_ref().clone()));
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
        object: &crate::document::Fields,
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
        if let Some(referenced_doc) = self.referenced_document(objref)? {
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

    /// Convert a literal to a Value
    fn literal_to_value(&self, literal: &Literal) -> Value {
        match literal {
            Literal::String(s) => Value::String(s.clone()),
            Literal::Integer(i) => Value::Integer(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::Boolean(b) => Value::Boolean(*b),
            Literal::Null => Value::Null,
            Literal::Timestamp(instant) => Value::DateTime(*instant),
        }
    }

    /// `text <operator> wanted` for two pieces of text, as
    /// [`crate::value_semantics::matches`] decides it. `None` for an
    /// operator this does not cover.
    fn text_matches(text: &str, operator: &ComparisonOperator, wanted: &str) -> Option<bool> {
        Some(match operator {
            ComparisonOperator::Equal => text == wanted,
            ComparisonOperator::NotEqual => text != wanted,
            ComparisonOperator::LessThan => text < wanted,
            ComparisonOperator::GreaterThan => text > wanted,
            ComparisonOperator::LessEqual => text <= wanted,
            ComparisonOperator::GreaterEqual => text >= wanted,
            ComparisonOperator::Contains => text.contains(wanted),
            ComparisonOperator::StartsWith => text.starts_with(wanted),
            ComparisonOperator::EndsWith => text.ends_with(wanted),
            _ => return None,
        })
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
        // A reference is the document it points at, and a document is
        // identified by its id, so `.user == "u1"` asks whether the
        // reference targets that document.
        if matches!(
            (left, right),
            (
                Value::Reference(_) | Value::GlobalReference(_),
                Value::String(_) | Value::List(_)
            ) | (
                Value::String(_),
                Value::Reference(_) | Value::GlobalReference(_) | Value::List(_)
            )
        ) {
            let left = self.reference_as_id(left)?;
            let right = self.reference_as_id(right)?;
            return Ok(crate::value_semantics::matches(&left, operator, &right));
        }
        // A list of references, as a path through a collection gives, is
        // likewise the ids of the documents referred to.
        if let (Value::List(items), Value::String(_)) = (left, right) {
            if items
                .iter()
                .any(|item| matches!(item, Value::Reference(_) | Value::GlobalReference(_)))
            {
                let left = self.reference_as_id(left)?;
                return Ok(crate::value_semantics::matches(&left, operator, right));
            }
        }
        Ok(crate::value_semantics::matches(left, operator, right))
    }

    /// Replace a reference, or references inside a list, with the id of the
    /// document referred to. A broken reference has no id.
    /// `value` as a client outside the process can hold it: every
    /// reference, at any depth, replaced by the id of the document it
    /// points at, which is also how a reference is written when inserting.
    /// A reference to a document that no longer exists becomes null.
    pub fn with_reference_ids(&self, value: &Value) -> OQLResult<Value> {
        Ok(match value {
            Value::Reference(_) | Value::GlobalReference(_) => self.reference_as_id(value)?,
            Value::List(items) => Value::List(
                items
                    .iter()
                    .map(|item| self.with_reference_ids(item))
                    .collect::<OQLResult<_>>()?,
            ),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), self.with_reference_ids(value)?)))
                    .collect::<OQLResult<_>>()?,
            ),
            other => other.clone(),
        })
    }

    fn reference_as_id(&self, value: &Value) -> OQLResult<Value> {
        Ok(match value {
            Value::Reference(reference) => match self.referenced_document(reference)? {
                Some(document) => Value::String(document.id.clone()),
                None => self
                    .resolve_objref_target(reference)?
                    .map(|document| Value::String(document.id))
                    .unwrap_or(Value::Null),
            },
            Value::GlobalReference(reference) => self
                .store
                .navigate_global_reference(reference)?
                .map(|document| Value::String(document.id))
                .unwrap_or(Value::Null),
            Value::List(items) => Value::List(
                items
                    .iter()
                    .map(|item| self.reference_as_id(item))
                    .collect::<OQLResult<_>>()?,
            ),
            other => other.clone(),
        })
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

        // After `partition_by` a select keeps every row, and an aggregate
        // in it is over the row's partition.
        if documents
            .first()
            .is_some_and(|row| row.fields.contains_key(PARTITION_FIELD))
        {
            return self.select_by_position(documents, select_block);
        }

        if has_aggregation && !is_grouped_data {
            // Handle aggregation over all documents - return single result document
            let mut projected_fields = crate::document::Fields::default();

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
            if select_block
                .fields
                .iter()
                .any(|field| Self::expression_uses_position(&field.expression))
            {
                return self.select_by_position(documents, select_block);
            }
            // Handle regular projection - return one document per input document
            let mut projected_documents = Vec::with_capacity(documents.len());
            for document in documents {
                let projected_fields = self.select_row(&document, select_block)?;
                projected_documents.push(document.with_projected_fields(projected_fields));
            }
            Ok(projected_documents)
        }
    }

    /// A `select` that uses position functions: every row is shaped with
    /// the rows around it in reach.
    fn select_by_position(
        &self,
        documents: Vec<Document>,
        select_block: &SelectBlock,
    ) -> OQLResult<Vec<Document>> {
        let frame = std::rc::Rc::new(WindowFrame::new(documents)?);
        WINDOWS.with(|frames| frames.borrow_mut().push(frame.clone()));
        let _guard = WindowGuard;
        let mut projected = Vec::with_capacity(frame.rows.len());
        for (position, row) in frame.rows.iter().enumerate() {
            frame.position.set(position);
            projected.push(row.with_projected_fields(self.select_row(row, select_block)?));
        }
        Ok(projected)
    }

    /// Whether an expression calls a position function on the rows of the
    /// select it is written in. A nested query or collection has its own
    /// rows, so it is not looked into.
    fn expression_uses_position(expression: &Expression) -> bool {
        let any = |expressions: &[Expression]| expressions.iter().any(Self::expression_uses_position);
        match expression {
            Expression::FunctionCall { name, args } => {
                WindowFrame::is_position_function(name) || any(args)
            }
            Expression::IfThenElse { condition, then_expr, else_expr } => {
                Self::condition_uses_position(condition)
                    || Self::expression_uses_position(then_expr)
                    || Self::expression_uses_position(else_expr)
            }
            Expression::Match { expr, arms } => {
                Self::expression_uses_position(expr)
                    || arms.iter().any(|arm| {
                        arm.guard.as_ref().is_some_and(Self::condition_uses_position)
                            || Self::expression_uses_position(&arm.body)
                    })
            }
            Expression::LogicalAnd(left, right) | Expression::LogicalOr(left, right) => {
                Self::expression_uses_position(left) || Self::expression_uses_position(right)
            }
            Expression::BinaryOp { left, right, .. } => {
                Self::expression_uses_position(left) || Self::expression_uses_position(right)
            }
            Expression::LogicalNot(inner) => Self::expression_uses_position(inner),
            Expression::ArrayAccess { array, index } => {
                Self::expression_uses_position(array) || Self::expression_uses_position(index)
            }
            Expression::ChainedAccess { base, .. } => Self::expression_uses_position(base),
            Expression::Block(items) | Expression::ArrayLiteral(items) => any(items),
            Expression::ObjectLiteral(fields) => fields
                .iter()
                .any(|field| Self::expression_uses_position(&field.value)),
            Expression::ConditionExpr(condition) => Self::condition_uses_position(condition),
            // `so_far()` is the rows up to this one.
            Expression::CollectionExpression { source, .. } => {
                matches!(source.as_ref(), Expression::Identifier(name) if name == crate::parser::ROWS_SO_FAR)
            }
            Expression::Identifier(_)
            | Expression::FieldAccess(_)
            | Expression::Literal(_)
            | Expression::QueryExpression { .. } => false,
        }
    }

    /// Whether an expression calls the function `wanted` on the rows of the
    /// select it is written in. Anything this cannot see into counts.
    fn expression_calls(expression: &Expression, wanted: &str) -> bool {
        let calls = |node: &Expression| Self::expression_calls(node, wanted);
        fn condition(node: &Condition, wanted: &str, calls: &dyn Fn(&Expression) -> bool) -> bool {
            match node {
                Condition::Comparison { left, right, .. } => calls(left) || calls(right),
                Condition::And(left, right) | Condition::Or(left, right) => {
                    condition(left, wanted, calls) || condition(right, wanted, calls)
                }
                Condition::Not(inner) => condition(inner, wanted, calls),
            }
        }
        match expression {
            Expression::FunctionCall { name, args } => name == wanted || args.iter().any(calls),
            Expression::IfThenElse { condition: test, then_expr, else_expr } => {
                condition(test, wanted, &calls) || calls(then_expr) || calls(else_expr)
            }
            Expression::LogicalAnd(left, right)
            | Expression::LogicalOr(left, right)
            | Expression::BinaryOp { left, right, .. } => calls(left) || calls(right),
            Expression::LogicalNot(inner) => calls(inner),
            Expression::ArrayAccess { array, index } => calls(array) || calls(index),
            Expression::ChainedAccess { base, .. } => calls(base),
            Expression::Block(items) | Expression::ArrayLiteral(items) => items.iter().any(calls),
            Expression::ObjectLiteral(fields) => fields.iter().any(|field| calls(&field.value)),
            Expression::ConditionExpr(test) => condition(test, wanted, &calls),
            // A chain on the rows so far may call it for those rows.
            Expression::CollectionExpression { source, operations }
                if matches!(source.as_ref(), Expression::Identifier(name) if name == crate::parser::ROWS_SO_FAR) =>
            {
                serde_json::to_string(operations)
                    .map_or(true, |text| text.contains(&format!("\"{wanted}\"")))
            }
            Expression::Identifier(_)
            | Expression::FieldAccess(_)
            | Expression::Literal(_)
            | Expression::QueryExpression { .. }
            | Expression::CollectionExpression { .. } => false,
            Expression::Match { .. } => true,
        }
    }

    fn condition_uses_position(condition: &Condition) -> bool {
        match condition {
            Condition::Comparison { left, right, .. } => {
                Self::expression_uses_position(left) || Self::expression_uses_position(right)
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                Self::condition_uses_position(left) || Self::condition_uses_position(right)
            }
            Condition::Not(inner) => Self::condition_uses_position(inner),
        }
    }

    /// Evaluate a position function for the row its select is shaping.
    fn evaluate_position_function(&self, name: &str, args: &[Expression]) -> OQLResult<Value> {
        let Some(frame) = WINDOWS.with(|frames| frames.borrow().last().cloned()) else {
            return Err(OQLError::FunctionError {
                message: format!(
                    "{name}() depends on a row's position among the rows being selected, so it \
                     belongs in a select"
                ),
            });
        };
        let arity = |expected: std::ops::RangeInclusive<usize>, form: &str| {
            if expected.contains(&args.len()) {
                Ok(())
            } else {
                Err(OQLError::FunctionError {
                    message: format!("{name} is written {form}"),
                })
            }
        };
        let position = frame.position.get();
        let (partition, place) = frame.place[position];
        let members = &frame.members[partition];
        // Evaluate for another row of the frame as if it were being shaped.
        let at = |row: usize, expression: &Expression| {
            frame.position.set(row);
            let value = self.evaluate_expression(&frame.rows[row], expression);
            frame.position.set(position);
            value
        };
        match name {
            "row_number" => {
                arity(0..=0, "row_number()")?;
                Ok(Value::Integer(place as i64 + 1))
            }
            "prev" | "next" => {
                arity(1..=2, "prev(value) or prev(value, rows)")?;
                let distance = match args.get(1) {
                    Some(distance) => self.evaluate_row_count(name, distance)? as usize,
                    None => 1,
                };
                let target = if name == "prev" {
                    place.checked_sub(distance)
                } else {
                    place.checked_add(distance)
                };
                match target.and_then(|target| members.get(target)) {
                    Some(row) => at(*row, &args[0]),
                    None => Ok(Value::Null),
                }
            }
            _ => {
                arity(1..=1, "with the value to rank by, as in rank(.score)")?;
                // One state per call site and partition. The call is known
                // by where its name is held, which the query keeps in place.
                let key = (name.as_ptr() as usize, partition);
                let mut state = frame.running.borrow_mut().remove(&key).unwrap_or_default();
                // An earlier row asked after a later one: start over.
                if state.taken > place + 1 {
                    state = Running::default();
                }
                while state.taken <= place {
                    let value = at(members[state.taken], &args[0])?;
                    // Rows that follow one another with equal values share
                    // a rank.
                    let tied = match &state.last {
                        Some(last) => self.values_equal(last, &value),
                        None => false,
                    };
                    if !tied {
                        state.rank = state.taken as i64 + 1;
                        state.dense_rank += 1;
                    }
                    state.last = Some(value);
                    state.taken += 1;
                }
                let answer = if name == "rank" { state.rank } else { state.dense_rank };
                frame.running.borrow_mut().insert(key, state);
                Ok(Value::Integer(answer))
            }
        }
    }

    /// `so_far()` and what is chained on it: the rows of the partition from
    /// its first row to the one being shaped, in order.
    ///
    /// A reduction of all of them (`so_far().amount.sum()`) is kept as a
    /// running state, so each row adds itself to what the rows before it
    /// gave. One of the last few (`so_far().last(7).amount.avg()`) reads
    /// those few. Only a chain that is neither copies the rows.
    #[inline(never)]
    fn rows_so_far(
        &self,
        scope_document: &Document,
        operations: &[Operation],
        collection_scope: &str,
    ) -> OQLResult<Value> {
        let frame = WINDOWS
            .with(|frames| frames.borrow().last().cloned())
            .filter(|frame| {
                frame
                    .rows
                    .get(frame.position.get())
                    .is_some_and(|row| std::ptr::eq(row, scope_document))
            })
            .ok_or_else(|| OQLError::FunctionError {
                message: "so_far() is the rows up to the one a select is shaping, so it belongs \
                          in a select, as in events.sort { .at.asc }.select { total: \
                          so_far().amount.sum() }"
                    .to_string(),
            })?;
        let position = frame.position.get();
        let (partition, place) = frame.place[position];
        let members = &frame.members[partition][..=place];
        let schema = scope_document.schema.as_str();

        // Filters that ask only about the row they test run against the
        // rows where they are, each as if it were the row being shaped, so
        // `prev` in one is that row's own neighbour.
        let filters_from = |from: usize| {
            operations[from..]
                .iter()
                .take_while(|operation| {
                    matches!(operation, Operation::Where(_)) && !Self::operation_names(operation, schema)
                })
                .count()
        };
        let passes = |row: usize, filters: &[Operation]| -> OQLResult<bool> {
            frame.position.set(row);
            let mut passed = Ok(true);
            for operation in filters {
                if let Operation::Where(block) = operation {
                    match self.evaluate_condition(&frame.rows[row], &block.condition) {
                        Ok(true) => {}
                        other => {
                            passed = other;
                            break;
                        }
                    }
                }
            }
            frame.position.set(position);
            passed
        };
        let lead = filters_from(0);
        let window = match operations.get(lead) {
            Some(Operation::Last(count)) => Some(self.evaluate_row_count("last", count)? as usize),
            _ => None,
        };
        let after = lead + usize::from(window.is_some());
        let trail = filters_from(after);
        let rest = &operations[after + trail..];
        let reduction = self
            .reduction_of_rows(schema, rest)?
            .filter(|(_, input)| !input.as_ref().is_some_and(|input| Self::expression_names(input, schema)));
        // Take one row into a reduction. `false` when its value is not one
        // a running state can hold, and the rows must be read the long way.
        let take = |state: &mut Running, name: &str, input: Option<&Expression>, row: usize| -> OQLResult<bool> {
            let value = match input {
                Some(expression) => {
                    frame.position.set(row);
                    let value = self.evaluate_expression(&frame.rows[row], expression);
                    frame.position.set(position);
                    value?
                }
                None => Value::Boolean(true),
            };
            if matches!(value, Value::List(_) | Value::Object(_)) {
                return Ok(false);
            }
            self.take_into_running(name, state, value)?;
            Ok(true)
        };

        // Every row so far, reduced: carried from row to row.
        if let (None, Some((name, input))) = (window, &reduction) {
            let key = (operations.as_ptr() as usize, partition);
            let mut state = frame.running.borrow_mut().remove(&key).unwrap_or_default();
            if state.taken > place + 1 {
                state = Running::default();
            }
            while !state.unfit && state.taken <= place {
                let row = members[state.taken];
                if passes(row, &operations[..lead])? && !take(&mut state, name, input.as_ref(), row)? {
                    state.unfit = true;
                }
                state.taken += 1;
            }
            let answer = (!state.unfit).then(|| state.answer(name));
            frame.running.borrow_mut().insert(key, state);
            if let Some(answer) = answer {
                return Ok(answer);
            }
        }

        // The rows the chain narrows to, by where they are.
        let mut chosen: Vec<usize> = Vec::new();
        if lead == 0 {
            let from = window.map_or(0, |count| members.len().saturating_sub(count));
            chosen.extend_from_slice(&members[from..]);
        } else {
            for row in members {
                if passes(*row, &operations[..lead])? {
                    chosen.push(*row);
                }
            }
            if let Some(count) = window {
                chosen.drain(..chosen.len().saturating_sub(count));
            }
        }
        if trail > 0 {
            let mut kept = Vec::with_capacity(chosen.len());
            for row in chosen {
                if passes(row, &operations[after..after + trail])? {
                    kept.push(row);
                }
            }
            chosen = kept;
        }
        if let (Some(_), Some((name, input))) = (window, &reduction) {
            let mut state = Running::default();
            let mut fits = true;
            for row in &chosen {
                if !take(&mut state, name, input.as_ref(), *row)? {
                    fits = false;
                    break;
                }
            }
            if fits {
                return Ok(state.answer(name));
            }
        }
        self.run_collection_operations(
            chosen.into_iter().map(|row| frame.rows[row].clone()).collect(),
            false,
            rest,
            scope_document,
            collection_scope,
        )
    }

    /// The reduction that ends a chain over rows, as the aggregate's name
    /// and the value it takes from each row; `None` for a count of rows.
    /// `None` altogether when the chain does not end in one a running state
    /// can keep.
    fn reduction_of_rows(
        &self,
        schema: &str,
        operations: &[Operation],
    ) -> OQLResult<Option<(&'static str, Option<Expression>)>> {
        let named = |name: &str| match name {
            "sum" => Some("sum"),
            "avg" => Some("avg"),
            "min" => Some("min"),
            "max" => Some("max"),
            _ => None,
        };
        let one_field = |field: &FieldAccess| -> OQLResult<Option<Expression>> {
            Ok(match field.path.as_slice() {
                [name] if name != "*" && !self.is_relationship(schema, name)? => {
                    Some(Expression::FieldAccess(field.clone()))
                }
                _ => None,
            })
        };
        Ok(match operations {
            [Operation::Count] => Some(("count", None)),
            [Operation::Sum(field)] => one_field(field)?.map(|input| ("sum", Some(input))),
            [Operation::Avg(field)] => one_field(field)?.map(|input| ("avg", Some(input))),
            [Operation::Min(field)] => one_field(field)?.map(|input| ("min", Some(input))),
            [Operation::Max(field)] => one_field(field)?.map(|input| ("max", Some(input))),
            [Operation::AggregateExpr { function, input }]
                if !Self::expression_is_nested(input)
                    && !Self::expression_contains_aggregate(input) =>
            {
                named(function).map(|name| (name, Some(input.clone())))
            }
            // `.amount.sum()`: the values of one field, reduced.
            [Operation::Step(step), reduction] => match (one_field(step)?, step.path.as_slice()) {
                (Some(_), [field]) => match Self::reduction_naming(field, std::slice::from_ref(reduction)) {
                    Some(named) => self.reduction_of_rows(schema, &named)?,
                    None => None,
                },
                _ => None,
            },
            _ => None,
        })
    }

    /// Take one more row's value into a running reduction.
    fn take_into_running(&self, name: &str, state: &mut Running, value: Value) -> OQLResult<()> {
        if value.is_null() {
            return Ok(());
        }
        state.count += 1;
        match name {
            "sum" | "avg" => match value {
                Value::Integer(integer) => {
                    state.integer_sum = state.integer_sum.checked_add(integer).ok_or_else(|| {
                        OQLError::FunctionError {
                            message: format!("{name} overflowed a whole number"),
                        }
                    })?;
                }
                Value::Float(float) => {
                    state.float_sum += float;
                    state.has_float = true;
                }
                other => {
                    return Err(OQLError::TypeError {
                        expected: "numeric type".to_string(),
                        actual: other.type_name().to_string(),
                    })
                }
            },
            "min" | "max" => {
                let replaces = match &state.extreme {
                    None => true,
                    Some(current) => self.compare_ordered_values(&value, current, |order| {
                        if name == "min" {
                            order.is_lt()
                        } else {
                            order.is_gt()
                        }
                    })?,
                };
                if replaces {
                    state.extreme = Some(value);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The fields one row takes when a `select` without aggregates shapes
    /// it.
    fn select_row(
        &self,
        document: &Document,
        select_block: &SelectBlock,
    ) -> OQLResult<crate::document::Fields> {
        let mut projected_fields = crate::document::Fields::with_capacity_and_hasher(select_block.fields.len(), Default::default());

        for select_field in &select_block.fields {
            // `*` stands for every field of the row. Entries written
            // after it add to, or replace, what it brought in.
            if select_field.alias.is_none() && Self::is_all_fields(&select_field.expression)
            {
                projected_fields.extend(
                    document
                        .fields
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone())),
                );
                continue;
            }
            // `-name` leaves out a field that `*` brought in.
            if matches!(&select_field.expression, Expression::Identifier(marker) if marker == crate::parser::OMITTED_FIELD)
            {
                if let Some(name) = &select_field.alias {
                    projected_fields.remove(name);
                }
                continue;
            }
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

            let value = self.evaluate_expression(document, &select_field.expression)?;

            projected_fields.insert(field_name, value);
        }
        Ok(projected_fields)
    }

    /// Apply INSERT operation
    fn apply_insert_operation(
        &self,
        collection: &str,
        insert_block: &InsertBlock,
    ) -> OQLResult<Vec<Document>> {
        // Convert insert fields to document fields
        let mut fields = crate::document::Fields::default();

        // Create a temporary empty document for expression evaluation context
        let temp_doc = Document::new(collection.to_string(), crate::document::Fields::default());

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

        if let Some(ref schema) = schema {
            self.apply_defaults(schema, &mut fields)?;
            self.coerce_shapes(schema, &mut fields)?;
        }

        // Create new document
        let document = Self::document_for_insert(collection, fields)?;
        self.ensure_new_identity(&document, &mut HashSet::new())?;

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

    /// Declare a collection's schema, or change it.
    ///
    /// Redeclaring a schema is how it is altered. Changes that cannot
    /// invalidate existing documents are applied: adding an optional field,
    /// adding a field with a default (existing documents receive it), making
    /// a field optional, widening `Integer` to `Float`, renaming with
    /// `new: Type from old`, and dropping with `old: removed`. Any other
    /// change is refused with the reason, and nothing is written.
    pub fn define_schema(
        &self,
        definition: &crate::parser::SchemaDefinition,
        schema: Schema,
    ) -> OQLResult<()> {
        let refuse = |reason: String| OQLError::SchemaValidation {
            message: format!("cannot change schema '{}': {reason}", schema.name),
        };
        let existing = self.store.get_schema(&schema.name)?;
        let mut rewritten = Vec::new();
        // Declared again exactly as it stands, as an application does each
        // time it starts, a schema is not written again.
        let unchanged = existing.as_ref() == Some(&schema);

        if let Some(old) = existing.filter(|old| old != &schema) {
            let rows = self.store.scan_documents(&schema.name)?;
            let renames: HashMap<&str, &str> = definition
                .fields
                .iter()
                .filter_map(|field| Some((field.renamed_from.as_deref()?, field.name.as_str())))
                .collect();
            let removed: HashSet<&str> = definition
                .fields
                .iter()
                .filter(|field| field.removed)
                .map(|field| field.name.as_str())
                .collect();

            for name in old.fields.keys() {
                if schema.fields.contains_key(name)
                    || renames.contains_key(name.as_str())
                    || removed.contains(name.as_str())
                {
                    continue;
                }
                let holders = rows
                    .iter()
                    .filter(|row| row.fields.get(name).is_some_and(|value| !value.is_null()))
                    .count();
                if holders > 0 {
                    return Err(refuse(format!(
                        "field '{name}' is no longer declared but {holders} document(s) have a \
                         value for it. Keep the field, rename it with 'new_name: Type from \
                         {name}', or drop it with '{name}: removed'"
                    )));
                }
            }
            for (name, new_type) in &schema.fields {
                let source = renames
                    .iter()
                    .find(|(_, renamed)| **renamed == name.as_str())
                    .map(|(original, _)| *original);
                let Some(old_type) = old.fields.get(source.unwrap_or(name.as_str())) else {
                    let has_default = schema.constraints.iter().any(|constraint| {
                        matches!(constraint, SchemaConstraint::Default { field, .. } if field == name)
                    });
                    if !new_type.is_optional() && !has_default && !rows.is_empty() {
                        return Err(refuse(format!(
                            "new field '{name}' is required, and existing documents have no \
                             value for it. Give it a default ('{name}: Type = value') or make \
                             it optional"
                        )));
                    }
                    continue;
                };
                if !Self::type_change_is_safe(old_type, new_type) && !rows.is_empty() {
                    return Err(refuse(format!(
                        "field '{name}' cannot change from {} to {}",
                        Self::describe_type(old_type),
                        Self::describe_type(new_type)
                    )));
                }
            }

            for mut row in rows {
                let before = row.fields.clone();
                for (original, renamed) in &renames {
                    if let Some(value) = row.fields.remove(*original) {
                        row.fields.insert(renamed.to_string(), value);
                    }
                }
                row.fields
                    .retain(|name, _| schema.fields.contains_key(name));
                self.apply_defaults(&schema, &mut row.fields)?;
                for (name, field_type) in &schema.fields {
                    if let Some(value) = row.fields.get(name).cloned() {
                        let widened = match (value, Self::scalar_field_type(field_type)) {
                            (Value::Integer(integer), FieldType::Float) => {
                                Value::Float(integer as f64)
                            }
                            (value, _) => value,
                        };
                        row.fields.insert(name.clone(), widened);
                    }
                }
                schema
                    .validate_document(&row.fields)
                    .map_err(|error| refuse(format!("document '{}' would not fit: {error}", row.id)))?;
                self.check_expressions(&schema, &row)
                    .map_err(|error| refuse(error.to_string()))?;
                if row.fields != before {
                    rewritten.push(row);
                }
            }
        }

        let (scalar_indexes, vector_indexes) =
            crate::parser::schema_definition_indexes(definition, &schema)?;
        if !unchanged {
            self.store.put_schema(&schema)?;
        }
        if !rewritten.is_empty() {
            self.store.put_documents(&rewritten)?;
        }

        // Indexes declared in the block are the collection's indexes. One
        // that an earlier declaration created and this one omits is dropped.
        let declared: HashSet<&str> = scalar_indexes
            .iter()
            .map(|index| index.name.as_str())
            .collect();
        for index in self.store.list_scalar_indexes(Some(&schema.name))? {
            let ours = index.name.starts_with(&format!("{}_by_", schema.name))
                || index.name.starts_with(&format!("{}_unique_", schema.name));
            if ours && !declared.contains(index.name.as_str()) {
                self.store.delete_scalar_index(&schema.name, &index.name)?;
            }
        }
        for index in scalar_indexes {
            self.store.upsert_scalar_index(index)?;
        }
        for index in vector_indexes {
            self.store.upsert_vector_index(index)?;
        }
        Ok(())
    }

    /// Remove a collection: its documents, its indexes and its schema. A
    /// collection that others still refer to is not dropped.
    pub fn drop_collection(&self, name: &str) -> OQLResult<usize> {
        for other in self.store.list_schemas()? {
            if other == name {
                continue;
            }
            let Some(schema) = self.store.get_schema(&other)? else {
                continue;
            };
            if let Some((field, _)) = schema
                .get_reference_fields()
                .into_iter()
                .find(|(_, reference)| reference.target_schema == name)
            {
                return Err(OQLError::SchemaValidation {
                    message: format!(
                        "cannot drop '{name}': '{other}.{field}' refers to it. Drop '{other}' \
                         first, or remove that field"
                    ),
                });
            }
        }
        let documents = self.store.scan_documents(name)?;
        if documents.is_empty() && self.store.get_schema(name)?.is_none() {
            return Err(OQLError::SchemaValidation {
                message: format!("cannot drop '{name}': there is no such collection"),
            });
        }
        for document in &documents {
            self.store.delete_document(name, &document.id)?;
        }
        for index in self.store.list_scalar_indexes(Some(name))? {
            self.store.delete_scalar_index(name, &index.name)?;
        }
        self.store.delete_schema(name)?;
        Ok(documents.len())
    }

    fn scalar_field_type(field_type: &FieldType) -> &FieldType {
        match field_type {
            FieldType::Optional(inner) => Self::scalar_field_type(inner),
            other => other,
        }
    }

    /// Whether every value valid under `old` is valid, or can be made valid
    /// without loss, under `new`.
    fn type_change_is_safe(old: &FieldType, new: &FieldType) -> bool {
        match (old, new) {
            // Required may become optional, never the reverse.
            (FieldType::Optional(old), FieldType::Optional(new)) => {
                Self::type_change_is_safe(old, new)
            }
            (FieldType::Optional(_), _) => false,
            (old, FieldType::Optional(new)) => Self::type_change_is_safe(old, new),
            (FieldType::Integer, FieldType::Float) => true,
            (FieldType::List(old), FieldType::List(new)) => Self::type_change_is_safe(old, new),
            // Rules attached to a reference may change; its target may not.
            (FieldType::Reference(old), FieldType::Reference(new)) => {
                old.target_schema == new.target_schema
            }
            (old, new) => old == new,
        }
    }

    fn describe_type(field_type: &FieldType) -> String {
        match field_type {
            FieldType::Optional(inner) => format!("Optional({})", Self::describe_type(inner)),
            FieldType::List(inner) => format!("List({})", Self::describe_type(inner)),
            FieldType::Reference(reference) => format!("Ref({})", reference.target_schema),
            FieldType::Vector { dim } => format!("Vector({dim})"),
            other => other.type_name().to_string(),
        }
    }

    /// Evaluate a schema's `check(..)` declarations for one document.
    fn check_expressions(&self, schema: &Schema, document: &Document) -> OQLResult<()> {
        let checks = Schema::new_with_constraints(
            schema.name.clone(),
            HashMap::new(),
            schema
                .constraints
                .iter()
                .filter(|constraint| matches!(constraint, SchemaConstraint::CheckExpression { .. }))
                .cloned()
                .collect(),
        );
        self.validate_document_constraints(&checks, document, None)
    }

    /// Fill in declared defaults for fields an insert left out. A default
    /// is an expression, evaluated against the fields written so far, so it
    /// may be computed: `created: DateTime = now()`.
    fn apply_defaults(&self, schema: &Schema, fields: &mut crate::document::Fields) -> OQLResult<()> {
        for constraint in &schema.constraints {
            let SchemaConstraint::Default { field, expression } = constraint else {
                continue;
            };
            if fields.contains_key(field) {
                continue;
            }
            let parsed = crate::parser::parse_stored_expression(expression).map_err(|error| {
                OQLError::SchemaValidation {
                    message: format!(
                        "default of '{}.{field}' is not valid: {error}",
                        schema.name
                    ),
                }
            })?;
            let scope = Document::with_id(String::new(), schema.name.clone(), fields.clone());
            let mut value = self.evaluate_expression(&scope, &parsed)?;
            if let Some(field_type) = schema.get_field_type(field) {
                value = self.convert_to_schema_type(value, field_type, &schema.name)?;
            }
            fields.insert(field.clone(), value);
        }
        Ok(())
    }

    /// Bring the values inside declared embedded objects to their declared
    /// types, exactly as top-level fields are: timestamp text becomes a
    /// timestamp and an id becomes a reference.
    fn coerce_shapes(&self, schema: &Schema, fields: &mut crate::document::Fields) -> OQLResult<()> {
        for constraint in &schema.constraints {
            let SchemaConstraint::Shape { path, .. } = constraint else {
                continue;
            };
            if path.contains('.') {
                continue;
            }
            if let Some(value) = fields.get_mut(path.trim_end_matches("[]")) {
                self.coerce_shaped_value(schema, path, value)?;
            }
        }
        Ok(())
    }

    fn coerce_shaped_value(&self, schema: &Schema, path: &str, value: &mut Value) -> OQLResult<()> {
        let Some(shape) = schema.shape(path) else {
            return Ok(());
        };
        let objects: Vec<&mut crate::document::Fields> = match value {
            Value::Object(object) => vec![object],
            Value::List(items) => items
                .iter_mut()
                .filter_map(|item| match item {
                    Value::Object(object) => Some(object),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let base = path.trim_end_matches("[]");
        for object in objects {
            for (name, field_type) in shape {
                let Some(nested) = object.get_mut(name) else {
                    continue;
                };
                *nested = self.convert_to_schema_type(nested.clone(), field_type, &schema.name)?;
                self.coerce_shaped_value(schema, &format!("{base}.{name}"), nested)?;
                self.coerce_shaped_value(schema, &format!("{base}.{name}[]"), nested)?;
            }
        }
        Ok(())
    }

    /// Build the document for an insert.
    ///
    /// The `id` a user writes is the document's identity: it is the key the
    /// collection stores the document under and the target that references
    /// resolve to. A document inserted without an `id` is given one.
    fn document_for_insert(
        collection: &str,
        fields: crate::document::Fields,
    ) -> OQLResult<Document> {
        let id = match fields.get("id") {
            None | Some(Value::Null) => {
                return Ok(Document::new(collection.to_string(), fields));
            }
            Some(Value::String(id)) if !id.is_empty() => id.clone(),
            Some(Value::Integer(id)) => id.to_string(),
            Some(other) => {
                return Err(OQLError::SchemaValidation {
                    message: format!(
                        "'{collection}' id must be a non-empty String or an Integer, found {}",
                        other.type_name()
                    ),
                });
            }
        };
        Ok(Document::with_id(id, collection.to_string(), fields))
    }

    /// Reject an insert whose id is already taken, in the collection or
    /// earlier in the same batch. Replacing a document is `upsert`'s job.
    fn ensure_new_identity(
        &self,
        document: &Document,
        batch_ids: &mut HashSet<String>,
    ) -> OQLResult<()> {
        if !document.fields.contains_key("id") {
            return Ok(());
        }
        if !batch_ids.insert(document.id.clone())
            || self
                .store
                .document_exists(&document.schema, &document.id)?
        {
            return Err(OQLError::SchemaValidation {
                message: format!(
                    "a '{}' document with id '{}' already exists; use upsert to replace it",
                    document.schema, document.id
                ),
            });
        }
        Ok(())
    }

    /// Apply INSERT MANY operation (batch insert)
    fn apply_insert_many_operation(
        &self,
        collection: &str,
        insert_blocks: &[InsertBlock],
    ) -> OQLResult<Vec<Document>> {
        // Create a temporary empty document for expression evaluation context
        let temp_doc = Document::new(collection.to_string(), crate::document::Fields::default());
        self.insert_rows(
            collection,
            insert_blocks.len(),
            insert_blocks.iter().map(|block| {
                block.fields.iter().map(|field| {
                    Ok((
                        field.name.clone(),
                        self.evaluate_expression(&temp_doc, &field.value)?,
                    ))
                })
            }),
        )
    }

    /// Insert documents given as field values rather than as OQL text.
    ///
    /// This is `collection.insert [ .. ]` without the parse: the same type
    /// conversion, defaults, validation and constraints, and one durable
    /// write for the whole batch. A reference is given as the id it points
    /// at and a timestamp may be given as text, as in OQL. It is the way to
    /// load data in bulk.
    pub fn insert_documents(
        &self,
        collection: &str,
        rows: Vec<crate::document::Fields>,
    ) -> OQLResult<Vec<Document>> {
        let mut inserted = Vec::new();
        self.store.run_atomically(&mut || {
            let _statement = StatementGuard::enter();
            inserted = self.insert_rows(
                collection,
                rows.len(),
                rows.iter().map(|row| {
                    row.iter()
                        .map(|(name, value)| Ok((name.clone(), value.clone())))
                }),
            )?;
            Ok(())
        })?;
        Ok(inserted)
    }

    /// The one implementation of a batch insert. Each row yields its fields
    /// in turn, so a value is computed, typed and checked before the next
    /// one is looked at.
    fn insert_rows<Row>(
        &self,
        collection: &str,
        expected: usize,
        rows: impl Iterator<Item = Row>,
    ) -> OQLResult<Vec<Document>>
    where
        Row: Iterator<Item = OQLResult<(String, Value)>>,
    {
        let mut documents = Vec::with_capacity(expected);
        let mut batch_ids = HashSet::new();
        // How many of `documents` are already written.
        let mut stored = 0;

        // Get schema once for all documents
        let schema = self.store.get_schema(collection).ok().flatten();

        for row in rows {
            let mut fields = crate::document::Fields::default();

            for field in row {
                let (name, mut value) = field?;

                // Convert value to match schema type if needed
                if let Some(ref schema) = schema {
                    if let Some(field_type) = schema.get_field_type(&name) {
                        // A row may refer to an earlier row of its own
                        // batch, as the rows of a tree do. A reference is
                        // resolved against what is stored, so the earlier
                        // rows are written first; the statement still
                        // commits as a whole.
                        if !documents.is_empty()
                            && Self::refers_into(&value, field_type, collection, &batch_ids)
                        {
                            self.validate_insert_batch_constraints(schema, &documents[stored..])?;
                            self.store.put_documents(&documents[stored..])?;
                            stored = documents.len();
                        }
                        value = self.convert_to_schema_type(value, field_type, collection)?;
                    }
                }

                fields.insert(name, value);
            }

            if let Some(ref schema) = schema {
                self.apply_defaults(schema, &mut fields)?;
                self.coerce_shapes(schema, &mut fields)?;
            }

            // Create new document
            let document = Self::document_for_insert(collection, fields)?;
            self.ensure_new_identity(&document, &mut batch_ids)?;

            // Validate against schema if it exists
            if let Some(ref schema) = schema {
                schema.validate_document(&document.fields)?;
            }

            documents.push(document);
        }

        if let Some(ref schema) = schema {
            self.validate_insert_batch_constraints(schema, &documents[stored..])?;
        }

        // RustStore commits this as one WAL frame and one sync. Compatibility
        // stores retain the DocumentStore default while they migrate.
        self.store.put_documents(&documents[stored..])?;

        // Return all created documents
        Ok(documents)
    }

    /// Whether a value being written holds a reference to one of `ids` in
    /// `collection`: a row of the batch that is being inserted.
    fn refers_into(
        value: &Value,
        field_type: &crate::schema::FieldType,
        collection: &str,
        ids: &HashSet<String>,
    ) -> bool {
        use crate::schema::FieldType;
        match (value, field_type) {
            (_, FieldType::Optional(inner)) => Self::refers_into(value, inner, collection, ids),
            (Value::String(id), FieldType::Reference(reference)) => {
                reference.target_schema == collection && ids.contains(id)
            }
            (Value::List(items), FieldType::List(inner)) => items
                .iter()
                .any(|item| Self::refers_into(item, inner, collection, ids)),
            _ => false,
        }
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
        let temp_doc = Document::new(collection.to_string(), crate::document::Fields::default());
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
            self.coerce_shapes(&schema, &mut document.fields)?;
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
            let violated = || OQLError::SchemaValidation {
                message: format!(
                    "Unique constraint '{}' violated within insert batch on schema '{}' fields {:?}",
                    name.clone().unwrap_or_else(|| "unnamed_unique".to_string()),
                    schema.name,
                    fields
                ),
            };
            // Text, whole numbers, booleans and instants are equal exactly
            // when their encodings are, so one pass finds a repeat. A key
            // holding anything else is compared with every other key.
            let keys: Vec<Vec<Option<Value>>> = documents
                .iter()
                .map(|document| {
                    fields
                        .iter()
                        .map(|field| self.extract_field_path_value(&document.fields, field))
                        .collect()
                })
                .collect();
            let plain = keys.iter().flatten().all(|value| {
                matches!(
                    value,
                    None | Some(
                        Value::Null
                            | Value::String(_)
                            | Value::Integer(_)
                            | Value::Boolean(_)
                            | Value::DateTime(_)
                    )
                )
            });
            if plain {
                let mut seen = HashSet::with_capacity(keys.len());
                for key in &keys {
                    let encoded = bincode::serialize(key).map_err(|error| OQLError::Generic {
                        message: format!("Failed to encode unique key: {}", error),
                    })?;
                    if !seen.insert(encoded) {
                        return Err(violated());
                    }
                }
            } else {
                for left_index in 0..keys.len() {
                    for right_index in left_index + 1..keys.len() {
                        if keys[left_index] == keys[right_index] {
                            return Err(violated());
                        }
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

                // A document is stored under its id, so the target is one
                // lookup away.
                if self.store.document_exists(target_schema, &ref_id)? {
                    if let Some(objref) = self.store.get_global_objref(target_schema, &ref_id)? {
                        return Ok(Value::GlobalReference(objref));
                    }
                    if let Some(objref) = self.store.get_objref(target_schema, &ref_id)? {
                        return Ok(Value::Reference(objref));
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
            // A whole number is a float when a float is what the field
            // holds, as long as the float can hold it exactly. JSON, and so
            // every client that speaks it, writes 40.0 as 40.
            (Value::Integer(integer), FieldType::Float)
                if (integer as f64) as i64 == integer
                    && integer.unsigned_abs() <= 9_007_199_254_740_992 =>
            {
                Ok(Value::Float(integer as f64))
            }
            // A timestamp may be written as text: RFC 3339, or a bare date
            // meaning midnight UTC.
            (Value::String(text), FieldType::DateTime) => {
                crate::value_semantics::parse_timestamp(&text)
                    .map(Value::DateTime)
                    .ok_or_else(|| OQLError::TypeError {
                        expected:
                            "a timestamp such as \"2026-01-05T10:00:00Z\" or \"2026-01-05\""
                                .to_string(),
                        actual: format!("\"{text}\""),
                    })
            }
            (Value::List(items), FieldType::List(inner_type)) => Ok(Value::List(
                items
                    .into_iter()
                    .map(|item| self.convert_to_schema_type(item, inner_type, _current_collection))
                    .collect::<OQLResult<_>>()?,
            )),
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
        let schema = self.schema_of(&document.schema).ok().flatten();

        for update_field in &update_block.fields {
            let mut new_value = self.evaluate_expression(&document, &update_field.value)?;
            let path: Vec<&str> = update_field.name.split('.').collect();
            if path.len() == 1 {
                if update_field.name == "id"
                    && !self.values_equal(&new_value, &Value::String(document.id.clone()))
                {
                    return Err(OQLError::SchemaValidation {
                        message: format!(
                            "cannot change the id of '{}' document '{}': an id is a document's \
                             identity",
                            document.schema, document.id
                        ),
                    });
                }
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
            self.coerce_shapes(&schema, &mut document.fields)?;
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
        object: &mut crate::document::Fields,
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
            object.insert((*head).to_string(), Value::Object(crate::document::Fields::default()));
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
                SchemaConstraint::CheckExpression { expression } => {
                    // The same text always parses to the same condition, so
                    // it is parsed once, not once per document checked.
                    let known = CHECK_CONDITIONS
                        .with(|conditions| conditions.borrow().get(expression).cloned());
                    let condition = match known {
                        Some(condition) => condition,
                        None => {
                            let condition = Arc::new(
                                crate::parser::parse_check_condition(expression).map_err(
                                    |error| OQLError::SchemaValidation {
                                        message: format!(
                                            "check({expression}) on '{}' is not valid: {error}",
                                            schema.name
                                        ),
                                    },
                                )?,
                            );
                            CHECK_CONDITIONS.with(|conditions| {
                                conditions
                                    .borrow_mut()
                                    .insert(expression.clone(), condition.clone())
                            });
                            condition
                        }
                    };
                    if !self.evaluate_condition(document, &condition)? {
                        return Err(OQLError::SchemaValidation {
                            message: format!(
                                "'{}' document '{}' fails check({expression})",
                                schema.name, document.id
                            ),
                        });
                    }
                }
                // Applied when a document is built, not checked afterwards.
                SchemaConstraint::Default { .. }
                | SchemaConstraint::Shape { .. }
                | SchemaConstraint::ReverseName { .. } => {}
                SchemaConstraint::Unique { fields, name } => {
                    // A unique index over the same fields already enforces
                    // this inside the storage engine, without a scan.
                    if self
                        .store
                        .list_scalar_indexes(Some(&schema.name))?
                        .iter()
                        .any(|index| index.unique && &index.fields == fields)
                    {
                        continue;
                    }
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
        fields: &crate::document::Fields,
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
        // Decide everything before changing anything: a `restrict` anywhere
        // in the cascade must leave the database untouched.
        let mut plan = DeletePlan::default();
        for document in &documents {
            self.plan_delete(document, &mut plan)?;
        }

        if !plan.nullified.is_empty() {
            let survivors: Vec<Document> = plan
                .nullified
                .into_values()
                .filter(|document| {
                    !plan
                        .deleted
                        .contains(&(document.schema.clone(), document.id.clone()))
                })
                .collect();
            self.store.put_documents(&survivors)?;
        }
        for document in &plan.cascaded {
            self.store.delete_document(&document.schema, &document.id)?;
        }

        let mut deleted_documents = Vec::new();
        for document in documents {
            if self.store.delete_document(&document.schema, &document.id)? {
                deleted_documents.push(document);
            }
        }
        Ok(deleted_documents)
    }

    /// Work out what deleting `document` entails for the documents that
    /// refer to it, following each reference's `on delete` rule.
    fn plan_delete(&self, document: &Document, plan: &mut DeletePlan) -> OQLResult<()> {
        if !plan
            .deleted
            .insert((document.schema.clone(), document.id.clone()))
        {
            return Ok(());
        }
        for source in self.schema_names()?.iter().cloned() {
            let Some(source_schema) = self.schema_of(&source)? else {
                continue;
            };
            for (field, reference) in source_schema.get_reference_fields() {
                if reference.target_schema != document.schema {
                    continue;
                }
                let referring = self.store.scan_referencing_documents(
                    &document.schema,
                    &document.id,
                    Some(&source),
                    Some(field),
                )?;
                for referrer in referring {
                    let key = (referrer.schema.clone(), referrer.id.clone());
                    if plan.deleted.contains(&key) {
                        continue;
                    }
                    match reference.on_delete {
                        crate::schema::CascadeBehavior::Restrict => {
                            return Err(OQLError::ReferenceError {
                                message: format!(
                                    "cannot delete '{}' document '{}': '{}' document '{}' refers \
                                     to it through '{}'. Delete or repoint that document first, \
                                     or declare the reference 'on delete cascade' or \
                                     'on delete nullify'",
                                    document.schema, document.id, source, referrer.id, field
                                ),
                            });
                        }
                        crate::schema::CascadeBehavior::Cascade => {
                            self.plan_delete(&referrer, plan)?;
                            plan.cascaded.push(referrer);
                        }
                        crate::schema::CascadeBehavior::SetNull => {
                            let entry = plan.nullified.entry(key).or_insert(referrer);
                            let cleared = match entry.fields.get(field) {
                                Some(Value::List(items)) => {
                                    let mut kept = Vec::new();
                                    for item in items {
                                        if self.reference_as_id(item)?
                                            != Value::String(document.id.clone())
                                        {
                                            kept.push(item.clone());
                                        }
                                    }
                                    Value::List(kept)
                                }
                                _ => Value::Null,
                            };
                            entry.fields.insert(field.clone(), cleared);
                        }
                        crate::schema::CascadeBehavior::NoAction => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Evaluate a function call expression
    fn evaluate_function_call(
        &self,
        document: &Document,
        name: &str,
        args: &[Expression],
    ) -> OQLResult<Value> {
        // A function's local name, worked out once for the call.
        if name == crate::parser::LOCAL_VALUE {
            return self.evaluate_local(document, args);
        }
        if document.schema == "group" {
            if let Some(input) = Self::aggregate_input(name, args)? {
                return self.execute_group_aggregation(document, name, &input);
            }
        } else if let Some((frame, partition)) = WindowFrame::partition_of(document) {
            // `sum(.hours)` after `partition_by`: over the row's partition,
            // worked out once for the partition.
            if let Some(input) = Self::aggregate_input(name, args)? {
                let key = (name.as_ptr() as usize, partition);
                if let Some(known) = frame.totals.borrow().get(&key).cloned() {
                    return Ok(known);
                }
                let total = self.aggregate_rows(&frame.rows_of(partition), name, &input)?;
                frame.totals.borrow_mut().insert(key, total.clone());
                return Ok(total);
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
                // `exists(.orders)` on a relationship asks whether there are
                // any related rows, the same question as `.orders.exists()`.
                if name == "exists" && matches!(state, PathState::Missing) {
                    let (rows, _) = self.path_source_rows(document, &path.path)?;
                    return Ok(Value::Boolean(!rows.is_empty()));
                }
                let result = match name {
                    "exists" => matches!(state, PathState::Present(_) | PathState::Null),
                    "is_null" => matches!(state, PathState::Null),
                    "is_missing" => matches!(state, PathState::Missing),
                    "is_broken_ref" => matches!(state, PathState::BrokenReference),
                    _ => unreachable!(),
                };
                Ok(Value::Boolean(result))
            }
            // A function the user defined under one of these names is theirs.
            name if WindowFrame::is_position_function(name)
                && self.functions.get_user_function(name).is_none() =>
            {
                self.evaluate_position_function(name, args)
            }
            // The library's statistics may be computed directly. This is an
            // arm of its own so that every other call's frame is as it was.
            "variance" | "stddev" | "variance_pop" | "stddev_pop" => {
                self.library_or_user_function(document, name, args)
            }
            "median" | "percentile" | "product" => Err(OQLError::FunctionError {
                message: format!(
                    "{name}() has no collection here; use it in select or having, or call it \
                     on a collection as in .orders.{name}(..)"
                ),
            }),
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
                    self.evaluate_user_function(document, &user_func, args)
                } else {
                    Err(OQLError::Generic {
                        message: format!("Unknown function: {}", name),
                    })
                }
            }
        }
    }

    /// What an aggregate call such as `sum(.amount)` aggregates, or `None`
    /// when `name` is not an aggregate.
    fn aggregate_input(name: &str, args: &[Expression]) -> OQLResult<Option<AggregateInput>> {
        if !Self::is_aggregate_name(name) {
            return Ok(None);
        }
        // `min(a, b)` and `max(a, b)` are the lesser and greater of two
        // values, not an aggregate over rows.
        if matches!(name, "min" | "max") && args.len() == 2 {
            return Ok(None);
        }
        if name == "percentile" {
            // The value and the fraction travel together as a pair, so
            // every form of an aggregate carries both.
            return match args {
                [_, _] => Ok(Some(AggregateInput::Expression(Expression::ArrayLiteral(
                    args.to_vec(),
                )))),
                _ => Err(OQLError::FunctionError {
                    message: "percentile expects a value and a fraction, as in \
                              percentile(.salary, 0.9)"
                        .to_string(),
                }),
            };
        }
        match args {
            [] if name == "count" => Ok(Some(AggregateInput::AllRows)),
            [Expression::FieldAccess(field)] if field.path.len() == 1 && field.path[0] == "*" => {
                if name == "count" {
                    Ok(Some(AggregateInput::AllRows))
                } else {
                    Err(OQLError::FunctionError {
                        message: format!("{} does not accept '*'", name),
                    })
                }
            }
            [Expression::FieldAccess(field)] => Ok(Some(AggregateInput::Path(field.path.clone()))),
            // Any other expression is evaluated for each row and the
            // results aggregated: `sum(.price * .qty)`.
            [expression] => Ok(Some(AggregateInput::Expression(expression.clone()))),
            _ => Err(OQLError::FunctionError {
                message: format!(
                    "{} expects {}",
                    name,
                    if name == "count" {
                        "zero or one argument"
                    } else {
                        "exactly one argument"
                    }
                ),
            }),
        }
    }

    /// Functions that reduce a collection of rows to one value.
    fn is_aggregate_name(name: &str) -> bool {
        matches!(
            name,
            "count" | "sum" | "avg" | "min" | "max" | DISTINCT_COUNT | "median" | "percentile" | "product"
        )
    }

    /// The value at `fraction` of the way through the sorted numbers, between
    /// the two nearest when it falls between them. Null for no numbers.
    fn percentile_of(mut numbers: Vec<f64>, fraction: f64) -> OQLResult<Value> {
        if !(0.0..=1.0).contains(&fraction) {
            return Err(OQLError::FunctionError {
                message: format!("a percentile fraction is between 0 and 1, not {fraction}"),
            });
        }
        if numbers.is_empty() {
            return Ok(Value::Null);
        }
        numbers.sort_by(f64::total_cmp);
        let position = fraction * (numbers.len() - 1) as f64;
        let below = position.floor() as usize;
        let above = position.ceil() as usize;
        let share = position - below as f64;
        Ok(Value::Float(
            numbers[below] + (numbers[above] - numbers[below]) * share,
        ))
    }

    fn number_for_aggregate(value: &Value) -> OQLResult<Option<f64>> {
        match value {
            Value::Integer(integer) => Ok(Some(*integer as f64)),
            Value::Float(float) => Ok(Some(*float)),
            Value::Null => Ok(None),
            other => Err(OQLError::TypeError {
                expected: "numeric type".to_string(),
                actual: other.type_name().to_string(),
            }),
        }
    }

    fn aggregate_input_value(&self, row: &Document, input: &AggregateInput) -> OQLResult<Value> {
        match input {
            AggregateInput::AllRows => Ok(Value::Null),
            AggregateInput::Path(path) => {
                self.get_field_value(row, &FieldAccess { path: path.clone() })
            }
            AggregateInput::Expression(expression) => self.evaluate_expression(row, expression),
        }
    }

    fn expression_contains_aggregate(expression: &Expression) -> bool {
        match expression {
            Expression::FunctionCall { name, args } => {
                let of_two_values = matches!(name.as_str(), "min" | "max") && args.len() == 2;
                (Self::is_aggregate_name(name) && !of_two_values)
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
            // An operation with no receiver ranges over the select's input.
            Expression::CollectionExpression { source, .. }
                if matches!(source.as_ref(), Expression::Identifier(name) if name == crate::parser::CURRENT_ROWS) =>
            {
                true
            }
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
            if let Some(input) = Self::aggregate_input(name, args)? {
                return self.aggregate_rows(documents, name, &input);
            }
        }

        match expression {
            Expression::CollectionExpression { source, operations }
                if matches!(source.as_ref(), Expression::Identifier(name) if name == crate::parser::CURRENT_ROWS) =>
            {
                let scope = Document::with_id(String::new(), "$aggregate".to_string(), crate::document::Fields::default());
                self.run_collection_operations(
                    documents.to_vec(),
                    false,
                    operations,
                    &scope,
                    documents.first().map_or("$aggregate", |row| row.schema.as_str()),
                )
            }
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
                .collect::<OQLResult<crate::document::Fields>>()
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
                    .unwrap_or_else(|| Document::new("$aggregate".to_string(), crate::document::Fields::default()));
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
            Value::DateTime(instant) => Ok(Expression::Literal(Literal::Timestamp(instant))),
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
            // Reaching here means `count()` was evaluated for one row with
            // no collection to count: for example inside `where`.
            Err(OQLError::FunctionError {
                message: "count() has no collection to count here; use it in select or having, \
                          or call it on a collection as in .orders.count()"
                    .to_string(),
            })
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
        let input = if field_path.is_empty() {
            AggregateInput::AllRows
        } else {
            AggregateInput::Path(field_path.to_vec())
        };
        self.aggregate_rows(documents, function_name, &input)
    }

    /// The one implementation of every aggregate, shared by the terminal
    /// form (`.sum(.x)`), the function form in a `select` or `having`, and
    /// grouped queries.
    fn aggregate_rows(
        &self,
        documents: &[Document],
        function_name: &str,
        input: &AggregateInput,
    ) -> OQLResult<Value> {
        match function_name {
            "count" => {
                if matches!(input, AggregateInput::AllRows) {
                    // count(*) - count all documents
                    Ok(Value::Integer(documents.len() as i64))
                } else {
                    // count(field) - count non-null values
                    let mut count = 0i64;
                    for doc in documents {
                        let value = self.aggregate_input_value(doc, input)?;
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
                    let value = self.aggregate_input_value(doc, input)?;

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
                    let value = self.aggregate_input_value(doc, input)?;

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
                    let value = self.aggregate_input_value(doc, input)?;

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
                    let value = self.aggregate_input_value(doc, input)?;

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
            DISTINCT_COUNT => {
                if matches!(input, AggregateInput::AllRows) {
                    return Err(OQLError::FunctionError {
                        message: "a count of distinct values needs the values to tell apart".to_string(),
                    });
                }
                let mut seen = HashSet::new();
                for doc in documents {
                    let value = self.aggregate_input_value(doc, input)?;
                    if value.is_null() {
                        continue;
                    }
                    let key = bincode::serialize(&Self::canonical_key_value(&value)).map_err(
                        |error| OQLError::Generic {
                            message: format!("Failed to compare values for a distinct count: {error}"),
                        },
                    )?;
                    seen.insert(key);
                }
                Ok(Value::Integer(seen.len() as i64))
            }
            // Every value multiplied together; 1 when there are none, as
            // a sum of none is 0. Whole numbers stay whole.
            "product" => {
                let mut whole: i64 = 1;
                let mut fraction: Option<f64> = None;
                for doc in documents {
                    match self.aggregate_input_value(doc, input)? {
                        Value::Null => {}
                        Value::Integer(integer) => match &mut fraction {
                            Some(product) => *product *= integer as f64,
                            None => {
                                whole = whole.checked_mul(integer).ok_or_else(|| {
                                    OQLError::FunctionError {
                                        message: "product overflowed a whole number".to_string(),
                                    }
                                })?;
                            }
                        },
                        Value::Float(float) => {
                            fraction = Some(fraction.unwrap_or(whole as f64) * float);
                        }
                        other => {
                            return Err(OQLError::TypeError {
                                expected: "numeric type".to_string(),
                                actual: other.type_name().to_string(),
                            })
                        }
                    }
                }
                Ok(match fraction {
                    Some(product) => Value::Float(product),
                    None => Value::Integer(whole),
                })
            }
            "median" => {
                let mut numbers = Vec::with_capacity(documents.len());
                for doc in documents {
                    let value = self.aggregate_input_value(doc, input)?;
                    numbers.extend(Self::number_for_aggregate(&value)?);
                }
                Self::percentile_of(numbers, 0.5)
            }
            "percentile" => {
                let mut numbers = Vec::with_capacity(documents.len());
                let mut fraction = None;
                for doc in documents {
                    let Value::List(pair) = self.aggregate_input_value(doc, input)? else {
                        return Err(OQLError::FunctionError {
                            message: "percentile expects a value and a fraction".to_string(),
                        });
                    };
                    let [value, wanted] = pair.as_slice() else {
                        return Err(OQLError::FunctionError {
                            message: "percentile expects a value and a fraction".to_string(),
                        });
                    };
                    numbers.extend(Self::number_for_aggregate(value)?);
                    fraction = Self::number_for_aggregate(wanted)?;
                }
                match fraction {
                    Some(fraction) => Self::percentile_of(numbers, fraction),
                    None if documents.is_empty() => Ok(Value::Null),
                    None => Err(OQLError::FunctionError {
                        message: "percentile needs a fraction between 0 and 1".to_string(),
                    }),
                }
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
            for key in &group_by_block.computed {
                group_values.push(self.evaluate_expression(&document, &key.expression)?);
            }

            // A typed composite encoding cannot collide when strings contain a
            // delimiter and keeps Integer(1) distinct from String("1").
            // Keys group by OQL equality, under which `1` and `1.0` are the
            // same value.
            let canonical_key: Vec<Value> = group_values
                .iter()
                .map(Self::canonical_key_value)
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
            let mut fields = crate::document::Fields::default();

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

            for (offset, key) in group_by_block.computed.iter().enumerate() {
                let value = group_values
                    .get(group_by_block.fields.len() + offset)
                    .cloned()
                    .unwrap_or(Value::Null);
                fields.insert(key.name.clone(), value);
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

            // The group's source rows stay as they are, shared by every
            // aggregate that reads them, and are found again by this key.
            let rows_key = StatementScope::keep_group_rows(group_documents);
            fields.insert("_group_rows".to_string(), Value::String(rows_key));

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
        let mut fields = crate::document::Fields::default();
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
        self.aggregate_result_row(
            &documents,
            function,
            &AggregateInput::Path(field_access.path.clone()),
        )
    }

    fn aggregate_result_row(
        &self,
        documents: &[Document],
        function: &str,
        input: &AggregateInput,
    ) -> OQLResult<Vec<Document>> {
        let value = self.aggregate_rows(documents, function, input)?;
        let mut fields = crate::document::Fields::default();
        fields.insert(function.to_string(), value);
        Ok(vec![Document::with_id(
            format!("{function}_result"),
            "aggregate".to_string(),
            fields,
        )])
    }

    /// `distinct()`: rows are the same when all their fields are equal.
    /// The first of each set is kept, so the order of the input survives.
    fn apply_distinct_operation(&self, documents: Vec<Document>) -> OQLResult<Vec<Document>> {
        let mut seen = HashSet::new();
        let mut distinct = Vec::new();
        for document in documents {
            if seen.insert(Self::row_key(&document)?) {
                distinct.push(document);
            }
        }
        Ok(distinct)
    }

    /// Bytes that are the same for two rows exactly when all their fields
    /// are equal.
    fn row_key(document: &Document) -> OQLResult<Vec<u8>> {
        let mut fields: Vec<(&String, Value)> = document
            .fields
            .iter()
            .map(|(name, value)| (name, Self::canonical_key_value(value)))
            .collect();
        fields.sort_by(|left, right| left.0.cmp(right.0));
        bincode::serialize(&fields).map_err(|error| OQLError::Generic {
            message: format!("Failed to compare rows: {error}"),
        })
    }

    /// `union`, `intersect` and `except`: these rows and another query's,
    /// as sets. The result keeps the order of the rows it is called on,
    /// then, for a union, the order of the other query's.
    fn apply_combine(
        &self,
        documents: Vec<Document>,
        how: &str,
        other: &Expression,
        collection: &str,
        by_identity: bool,
    ) -> OQLResult<Vec<Document>> {
        // Whole rows of one collection are the same row exactly when they
        // have the same id, which is far less to compare than every field.
        let row_key = |document: &Document| -> OQLResult<Vec<u8>> {
            if by_identity {
                Ok(document.id.as_bytes().to_vec())
            } else {
                Self::row_key(document)
            }
        };
        let others: Vec<Document> = match other {
            // Rows of a query keep their collection and identity, so what
            // follows can still traverse from them.
            Expression::QueryExpression { query } => self.execute_query(query)?,
            // A bare name is a whole collection.
            Expression::Identifier(name) if CallFrame::value(name).is_none() => {
                self.execute_query(&OQLQuery {
                    collection: name.clone(),
                    operations: Vec::new(),
                })?
            }
            other => {
                let scope = Document::with_id(String::new(), String::new(), crate::document::Fields::default());
                let mut rows = Vec::new();
                self.push_source_rows(self.evaluate_expression(&scope, other)?, collection, &mut rows)?;
                rows.into_iter().map(|row| row.document).collect()
            }
        };
        let mut seen = HashSet::new();
        let mut combined = Vec::new();
        match how {
            "union" => {
                for document in documents.into_iter().chain(others) {
                    if seen.insert(row_key(&document)?) {
                        combined.push(document);
                    }
                }
            }
            "intersect" | "except" => {
                let theirs = others
                    .iter()
                    .map(&row_key)
                    .collect::<OQLResult<HashSet<_>>>()?;
                for document in documents {
                    let key = row_key(&document)?;
                    if theirs.contains(&key) == (how == "intersect") && seen.insert(key) {
                        combined.push(document);
                    }
                }
            }
            other => {
                return Err(OQLError::Generic {
                    message: format!("Unknown set operation: {other}"),
                })
            }
        }
        Ok(combined)
    }

    /// `walk(.path)`: every row reached by following `path` from these rows
    /// one or more times. A row is returned once, at the fewest steps that
    /// reach it, so a cycle ends the walk rather than repeating it.
    ///
    /// With `every` this is `routes(.path)`: a row is returned once for
    /// every route to it. A route does not pass through a row twice, which
    /// is what ends it in a cycle.
    ///
    /// With `paths` each row carries the rows on the way to it, which
    /// includes the rows passed through when one step is through several
    /// relationships.
    fn apply_walk(
        &self,
        documents: Vec<Document>,
        path: &FieldAccess,
        depth: Option<&Expression>,
        read: Option<&[String]>,
        every: bool,
        paths: bool,
    ) -> OQLResult<Vec<Document>> {
        /// More routes than this is a query that will not end.
        const MOST_ROUTES: usize = 1_000_000;
        // The rows reached need the fields that are read of them, the field
        // the walk continues through, and what tells them apart. A row on
        // a path that is kept may be read of anything.
        let wanted: Option<Vec<String>> = read.filter(|_| !paths).map(|fields| {
            let mut wanted = fields.to_vec();
            wanted.extend(path.path.iter().cloned());
            wanted.push("id".to_string());
            wanted
        });
        let limit = match depth {
            Some(depth) => self.evaluate_row_count("walk", depth)?,
            None => u64::MAX,
        };
        // A stored row is known by its collection and id; a row with no id
        // of its own, by everything it holds.
        let identity = |row: &Document| -> OQLResult<Vec<u8>> {
            let key = match row.fields.get("id") {
                Some(id) => (row.schema.as_str(), Self::canonical_key_value(id), Vec::new()),
                None => (row.schema.as_str(), Value::String(row.id.clone()), Self::row_key(row)?),
            };
            bincode::serialize(&key).map_err(|error| OQLError::Generic {
                message: format!("Failed to compare rows for walk: {error}"),
            })
        };
        // A row as one entry of a path: what it holds, less what the walk
        // itself put there.
        let on_path = |row: &Document| {
            let mut fields = row.fields.clone();
            fields.remove(WALK_PATH_FIELD);
            fields.remove(ORIGIN_FIELD);
            Value::Object(fields)
        };
        // The way to a row, given the way to the row it was reached from.
        let extended = |from: &Document, between: Vec<Value>, found: &Document| {
            let mut way = match from.fields.get(WALK_PATH_FIELD) {
                Some(Value::List(way)) => way.clone(),
                _ => vec![on_path(from)],
            };
            way.extend(between);
            way.push(on_path(found));
            Value::List(way)
        };
        // The rows one step leads to. When the step is through several
        // relationships and the way is kept, each comes with the rows
        // passed through between: they are on the way too.
        let step = |row: &Document| -> OQLResult<Vec<(Document, Vec<Value>)>> {
            if !paths || path.path.len() < 2 {
                return Ok(self
                    .path_source_rows_reading(row, &path.path, wanted.as_deref())?
                    .0
                    .into_iter()
                    .map(|found| (found, Vec::new()))
                    .collect());
            }
            let mut at = vec![(row.clone(), Vec::new())];
            for (hop, name) in path.path.iter().enumerate() {
                let mut onward = Vec::new();
                for (from, between) in &at {
                    for found in self
                        .path_source_rows_reading(from, std::slice::from_ref(name), None)?
                        .0
                    {
                        let mut between = between.clone();
                        if hop + 1 < path.path.len() {
                            between.push(on_path(&found));
                        }
                        onward.push((found, between));
                    }
                }
                at = onward;
            }
            Ok(at)
        };
        let mut seen = HashSet::new();
        // For `routes`: the rows each row of the last step came through.
        let mut through: Vec<std::rc::Rc<Vec<Vec<u8>>>> = Vec::new();
        if every {
            for row in &documents {
                through.push(std::rc::Rc::new(vec![identity(row)?]));
            }
        }
        // Every row reached so far; the last step's rows are at the end.
        let mut reached: Vec<Document> = Vec::new();
        let mut frontier = 0;
        let mut steps = 0u64;
        while steps < limit {
            steps += 1;
            let mut next = Vec::new();
            let mut next_through = Vec::new();
            let from = if steps == 1 { &documents[..] } else { &reached[frontier..] };
            for (position, row) in from.iter().enumerate() {
                for (mut found, between) in step(row)? {
                    let key = identity(&found)?;
                    if every {
                        if through[position].contains(&key) {
                            continue;
                        }
                        if reached.len() + next.len() >= MOST_ROUTES {
                            return Err(OQLError::FunctionError {
                                message: format!(
                                    "routes found more than {MOST_ROUTES} routes; give it a \
                                     depth, as in routes(.path, 5), or start from fewer rows"
                                ),
                            });
                        }
                        let mut came = through[position].as_ref().clone();
                        came.push(key);
                        next_through.push(std::rc::Rc::new(came));
                    } else if !seen.insert(key) {
                        continue;
                    }
                    if paths {
                        let way = extended(row, between, &found);
                        found.fields.insert(WALK_PATH_FIELD.to_string(), way);
                    }
                    found
                        .fields
                        .insert(WALK_DEPTH_FIELD.to_string(), Value::Integer(steps as i64));
                    next.push(found);
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = reached.len();
            reached.extend(next);
            through = next_through;
        }
        Ok(reached)
    }

    /// `partition_by { keys }`: note on each row which partition it is in.
    /// The next `select` reads the note; nothing else changes.
    fn apply_partition_by(
        &self,
        mut documents: Vec<Document>,
        block: &GroupByBlock,
    ) -> OQLResult<Vec<Document>> {
        for document in &mut documents {
            let mut key = Vec::with_capacity(block.fields.len() + block.computed.len());
            for field in &block.fields {
                key.push(Self::canonical_key_value(&self.get_field_value(document, field)?));
            }
            for computed in &block.computed {
                key.push(Self::canonical_key_value(
                    &self.evaluate_expression(document, &computed.expression)?,
                ));
            }
            document
                .fields
                .insert(PARTITION_FIELD.to_string(), Value::List(key));
        }
        Ok(documents)
    }

    /// A form of a value in which equal values are identical, for use as a
    /// grouping or deduplication key. OQL equality treats `1` and `1.0` as
    /// one value, so whole floats become integers; objects are ordered by
    /// field name.
    fn canonical_key_value(value: &Value) -> Value {
        match value {
            Value::Float(float)
                if float.fract() == 0.0 && float.abs() < 9_007_199_254_740_992.0 =>
            {
                Value::Integer(*float as i64)
            }
            Value::List(items) => Value::List(items.iter().map(Self::canonical_key_value).collect()),
            Value::Object(fields) => {
                let mut entries: Vec<_> = fields.iter().collect();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                Value::List(
                    entries
                        .into_iter()
                        .flat_map(|(name, value)| {
                            [Value::String(name.clone()), Self::canonical_key_value(value)]
                        })
                        .collect(),
                )
            }
            other => other.clone(),
        }
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
    fn execute_group_aggregation(
        &self,
        group_doc: &Document,
        function_name: &str,
        input: &AggregateInput,
    ) -> OQLResult<Value> {
        let documents = Self::group_rows(group_doc)?;
        self.aggregate_rows(&documents, function_name, input)
    }

    /// The source rows a group row stands for.
    fn group_rows(group_doc: &Document) -> OQLResult<Arc<Vec<Document>>> {
        match group_doc.get_field("_group_rows") {
            Some(Value::String(key)) => {
                StatementScope::group_rows(key).ok_or_else(|| OQLError::Generic {
                    message: "a group row was used outside the statement that produced it"
                        .to_string(),
                })
            }
            _ => Err(OQLError::Generic {
                message: "Group document missing source rows".to_string(),
            }),
        }
    }

    /// Apply SORT operation to documents
    fn apply_sort_operation(
        &self,
        documents: Vec<Document>,
        sort_block: &SortBlock,
    ) -> OQLResult<Vec<Document>> {
        self.sort_keeping(documents, sort_block, None)
    }

    /// Refuse a sort that calls a function that does not exist. A sort key
    /// that cannot be evaluated for a row counts as null, so that absent
    /// data never fails a sort; a misspelt function would otherwise be
    /// null for every row and the sort would quietly do nothing.
    fn check_sort_functions(&self, sort_block: &SortBlock) -> OQLResult<()> {
        fn unknown<'a>(
            expression: &'a Expression,
            known: &dyn Fn(&str) -> bool,
        ) -> Option<&'a str> {
            let look = |node: &'a Expression| unknown(node, known);
            match expression {
                Expression::FunctionCall { name, args } => {
                    if known(name) {
                        args.iter().find_map(look)
                    } else {
                        Some(name.as_str())
                    }
                }
                Expression::BinaryOp { left, right, .. }
                | Expression::LogicalAnd(left, right)
                | Expression::LogicalOr(left, right) => look(left).or_else(|| look(right)),
                Expression::LogicalNot(inner) => look(inner),
                Expression::IfThenElse { then_expr, else_expr, .. } => {
                    look(then_expr).or_else(|| look(else_expr))
                }
                Expression::ArrayAccess { array, index } => look(array).or_else(|| look(index)),
                Expression::ChainedAccess { base, .. } => look(base),
                _ => None,
            }
        }
        let known = |name: &str| {
            matches!(name, "exists" | "is_null" | "is_missing" | "is_broken_ref")
                || name == crate::parser::LOCAL_VALUE
                || Self::is_aggregate_name(name)
                || WindowFrame::is_position_function(name)
                || self.functions.get_function(name).is_some()
                || self.functions.get_user_function(name).is_some()
        };
        for field in &sort_block.fields {
            if let Some(name) = unknown(&field.expression, &known) {
                return Err(OQLError::Generic {
                    message: format!("Unknown function: {name}"),
                });
            }
        }
        Ok(())
    }

    /// What a row is sorted by: the value of each sort expression, with one
    /// that cannot be evaluated counting as null.
    fn sort_keys(&self, document: &Document, sort_block: &SortBlock) -> Vec<Value> {
        sort_block
            .fields
            .iter()
            .map(|field| {
                self.evaluate_expression(document, &field.expression)
                    .unwrap_or(Value::Null)
            })
            .collect()
    }

    /// The sort's fields, when every key is a field of the row or its id.
    /// `None` when a key is anything else.
    fn plain_sort_fields(sort: &SortBlock) -> Option<Vec<String>> {
        let mut names = Vec::new();
        for field in &sort.fields {
            let Expression::FieldAccess(access) = &field.expression else {
                return None;
            };
            let [name] = access.path.as_slice() else {
                return None;
            };
            match DocumentSystemField::from_name(name) {
                Some(DocumentSystemField::Id) => {}
                Some(_) => return None,
                None if !names.contains(name) => names.push(name.clone()),
                None => {}
            }
        }
        Some(names)
    }

    /// The positions of `rows` in sorted order, as
    /// [`Self::sorted_positions_of_stored`] gives them, with the keys read
    /// from the sort fields' columns and not from the rows. `places[n]` is
    /// where `rows[n]` is in those columns. `None` when a key is not a
    /// plain field, a field has no column, or the columns are of other
    /// rows than these (`stamp` differs).
    fn sorted_positions_by_columns(
        &self,
        collection: &str,
        sort: &SortBlock,
        stamp: u64,
        rows: &[&Arc<Document>],
        places: &[u32],
        keep: Option<usize>,
    ) -> OQLResult<Option<Vec<usize>>> {
        use crate::storage::{Column, ColumnData};
        use crate::value_semantics::sort_order;
        use std::cmp::Ordering;
        let Some(names) = Self::plain_sort_fields(sort) else {
            return Ok(None);
        };
        let batches = match names.is_empty() {
            true => Vec::new(),
            false => match self.store.scan_columns(collection, &names)? {
                Some(batches) => batches,
                None => return Ok(None),
            },
        };
        if batches.iter().any(|batch| batch.stamp() != stamp) || batches.len() > 1 {
            return Ok(None);
        }
        // Each key: the row's id, or one of the columns.
        let keys: Vec<Option<Column<'_>>> = sort
            .fields
            .iter()
            .map(|field| match &field.expression {
                Expression::FieldAccess(access) => names
                    .iter()
                    .position(|name| *name == access.path[0])
                    .map(|at| batches[0].column(at)),
                _ => None,
            })
            .collect();
        // A row without a value sorts as null does against any value.
        let null = Value::Null;
        let compare = |left: &usize, right: &usize| {
            for (field, key) in sort.fields.iter().zip(&keys) {
                let order = match key {
                    None => rows[*left].id.cmp(&rows[*right].id),
                    Some(column) => {
                        let (mine, theirs) = (places[*left] as usize, places[*right] as usize);
                        match (column.has_value(mine), column.has_value(theirs)) {
                            (false, false) => Ordering::Equal,
                            (true, true) => match column.data() {
                                ColumnData::Integers(values) => values[mine].cmp(&values[theirs]),
                                ColumnData::DateTimes(values) => values[mine].cmp(&values[theirs]),
                                ColumnData::Strings { offsets, bytes } => Column::text(offsets, bytes, mine)
                                    .cmp(Column::text(offsets, bytes, theirs)),
                                ColumnData::Floats(values) => {
                                    sort_order(&Value::Float(values[mine]), &Value::Float(values[theirs]))
                                }
                                ColumnData::Booleans(values) => values[mine].cmp(&values[theirs]),
                                ColumnData::References(values) => sort_order(
                                    &Value::Reference(values[mine]),
                                    &Value::Reference(values[theirs]),
                                ),
                            },
                            (mine_has, _) => {
                                // Any value of the column's kind stands for
                                // the one that is there.
                                let some = match column.data() {
                                    ColumnData::Integers(_) => Value::Integer(0),
                                    ColumnData::DateTimes(values) => Value::DateTime(values[0]),
                                    ColumnData::Strings { .. } => Value::String(String::new()),
                                    ColumnData::Floats(_) => Value::Float(0.0),
                                    ColumnData::Booleans(_) => Value::Boolean(false),
                                    ColumnData::References(values) => Value::Reference(values[0]),
                                };
                                match mine_has {
                                    true => sort_order(&some, &null),
                                    false => sort_order(&null, &some),
                                }
                            }
                        }
                    }
                };
                let order = match field.direction {
                    crate::parser::SortDirection::Asc => order,
                    crate::parser::SortDirection::Desc => order.reverse(),
                };
                if order.is_ne() {
                    return order;
                }
            }
            left.cmp(right)
        };
        let mut order: Vec<usize> = (0..rows.len()).collect();
        if let Some(keep) = keep.filter(|keep| *keep < order.len()) {
            if keep == 0 {
                return Ok(Some(Vec::new()));
            }
            order.select_nth_unstable_by(keep - 1, compare);
            order.truncate(keep);
        }
        order.sort_unstable_by(compare);
        COLUMN_SCANS_RUN.with(|runs| runs.set(runs.get() + 1));
        Ok(Some(order))
    }

    /// The first `wanted` rows of a collection sorted first by something of
    /// a row each refers to, `.person.id`, found by going through the rows
    /// referred to in that order and reading only the rows that refer to
    /// each, until enough are in hand. It is what an index on the reference
    /// is for: most of the collection is never read.
    ///
    /// Rows that do not pass the query's filters are left out as they are
    /// read.
    ///
    /// `None` when the sort is not of that kind, when a row might refer to
    /// nothing or to a row that is gone, or when most rows are wanted.
    fn first_through_reference(
        &self,
        collection: &str,
        sort: &SortBlock,
        wanted: usize,
        total: usize,
        passes: &dyn Fn(&Document) -> OQLResult<bool>,
    ) -> OQLResult<Option<Vec<Arc<Document>>>> {
        let Some(first) = sort.fields.first() else {
            return Ok(None);
        };
        let Expression::FieldAccess(FieldAccess { path }) = &first.expression else {
            return Ok(None);
        };
        let [through, field] = path.as_slice() else {
            return Ok(None);
        };
        if wanted.saturating_mul(8) > total {
            return Ok(None);
        }
        let Some(schema) = self.schema_of(collection)? else {
            return Ok(None);
        };
        // Every row must refer to a row that is there.
        let target = match schema.fields.get(through) {
            Some(FieldType::Reference(reference))
                if reference.required
                    && matches!(
                        reference.on_delete,
                        crate::schema::CascadeBehavior::Restrict | crate::schema::CascadeBehavior::Cascade
                    ) =>
            {
                reference.target_schema.clone()
            }
            _ => return Ok(None),
        };
        let Some(referred) = self.store.scan_documents_shared(&target)? else {
            return Ok(None);
        };
        let access = FieldAccess { path: vec![field.clone()] };
        let mut keys = Vec::with_capacity(referred.len());
        for row in &referred {
            keys.push(self.get_field_value(row, &access)?);
        }
        let descending = matches!(first.direction, crate::parser::SortDirection::Desc);
        let mut order: Vec<usize> = (0..referred.len()).collect();
        order.sort_by(|left, right| {
            let order = self.compare_values_for_sort(&keys[*left], &keys[*right]);
            if descending { order.reverse() } else { order }
        });
        let mut found: Vec<Arc<Document>> = Vec::new();
        for (position, at) in order.iter().enumerate() {
            // Rows referred to that sort alike are taken together.
            if found.len() >= wanted
                && position > 0
                && self
                    .compare_values_for_sort(&keys[order[position - 1]], &keys[*at])
                    .is_ne()
            {
                break;
            }
            for row in self.store.scan_referencing_documents_shared(
                &target,
                &referred[*at].id,
                Some(collection),
                Some(through),
            )? {
                if passes(&row)? {
                    found.push(row);
                }
            }
        }
        let rows: Vec<&Arc<Document>> = found.iter().collect();
        let kept = self
            .sorted_positions_of_stored(&rows, sort, Some(wanted))
            .into_iter()
            .map(|position| found[position].clone())
            .collect();
        Ok(Some(kept))
    }

    /// The positions of stored rows in sorted order, as
    /// [`Self::sorted_positions`] gives them. A key that is a field of the
    /// row is lent, not copied, and one read through a reference,
    /// `.person.id`, is read once for each row referred to.
    fn sorted_positions_of_stored(
        &self,
        rows: &[&Arc<Document>],
        sort_block: &SortBlock,
        keep: Option<usize>,
    ) -> Vec<usize> {
        use std::borrow::Cow;
        let read = |row: &Document, field: &crate::parser::SortField| {
            self.evaluate_expression(row, &field.expression)
                .unwrap_or(Value::Null)
        };
        // A key read through a reference, when every row has one: each
        // row's reference, and where the row it refers to stands among all
        // those referred to. Rows are then compared by that place, a whole
        // number, and the value is read once for each row referred to.
        let through: Vec<Option<Vec<u32>>> = sort_block
            .fields
            .iter()
            .map(|field| {
                let Expression::FieldAccess(access) = &field.expression else {
                    return None;
                };
                if access.path.len() != 2 {
                    return None;
                }
                let mut referred: HashMap<&ObjRef, u32> = HashMap::new();
                let mut values: Vec<Value> = Vec::new();
                let mut of_row = Vec::with_capacity(rows.len());
                for row in rows {
                    let Some(Value::Reference(reference)) = row.fields.get(&access.path[0]) else {
                        return None;
                    };
                    let next = values.len() as u32;
                    let known = *referred.entry(reference).or_insert(next);
                    if known == next {
                        values.push(read(row, field));
                    }
                    of_row.push(known);
                }
                // The place of each value among them, equal values sharing one.
                let mut order: Vec<u32> = (0..values.len() as u32).collect();
                order.sort_by(|left, right| {
                    self.compare_values_for_sort(&values[*left as usize], &values[*right as usize])
                });
                let mut place = vec![0u32; values.len()];
                let mut at = 0u32;
                for (position, value) in order.iter().enumerate() {
                    if position > 0
                        && self
                            .compare_values_for_sort(
                                &values[order[position - 1] as usize],
                                &values[*value as usize],
                            )
                            .is_ne()
                    {
                        at += 1;
                    }
                    place[*value as usize] = at;
                }
                Some(of_row.into_iter().map(|known| place[known as usize]).collect())
            })
            .collect();
        // A row's own id is text held beside its fields; it is compared
        // where it is.
        enum Key<'a> {
            Value(Cow<'a, Value>),
            Id(&'a str),
            Place(u32),
        }
        // Each key that is a field of the row is read by its place.
        let readers: Vec<Option<crate::fields::FieldReader>> = sort_block
            .fields
            .iter()
            .map(|field| match &field.expression {
                Expression::FieldAccess(access) => match access.path.as_slice() {
                    [name] if DocumentSystemField::from_name(name).is_none() => {
                        Some(crate::fields::FieldReader::new(name))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        // Every row's keys, one row after another in a single run.
        let width = sort_block.fields.len();
        let mut keys: Vec<Key<'_>> = Vec::with_capacity(rows.len() * width);
        for (position, row) in rows.iter().enumerate() {
            keys.extend(
                sort_block
                    .fields
                    .iter()
                    .zip(&through)
                    .zip(&readers)
                    .map(|((field, places), reader)| {
                        if let Some(places) = places {
                            return Key::Place(places[position]);
                        }
                        if let Some(value) = reader
                            .as_ref()
                            .and_then(|reader| reader.read(&row.fields))
                            .filter(|value| !matches!(value, Value::Reference(_) | Value::GlobalReference(_)))
                        {
                            return Key::Value(Cow::Borrowed(value));
                        }
                        // A field the store keeps for every row comes
                        // before a field of that name; the id is lent.
                        if let Expression::FieldAccess(access) = &field.expression {
                            if let [name] = access.path.as_slice() {
                                if let Some(DocumentSystemField::Id) = DocumentSystemField::from_name(name) {
                                    return Key::Id(&row.id);
                                }
                            }
                        }
                        Key::Value(Cow::Owned(read(row, field)))
                    }),
            );
        }
        let compare = |left: &usize, right: &usize| {
            let (mine, theirs) = (
                &keys[*left * width..(*left + 1) * width],
                &keys[*right * width..(*right + 1) * width],
            );
            for ((field, left), right) in sort_block.fields.iter().zip(mine).zip(theirs) {
                let order = match (left, right) {
                    (Key::Place(left), Key::Place(right)) => left.cmp(right),
                    (Key::Place(_), _) | (_, Key::Place(_)) => {
                        unreachable!("a key is a place for every row or for none")
                    }
                    (Key::Id(left), Key::Id(right)) => left.cmp(right),
                    (Key::Value(left), Key::Value(right)) => self.compare_values_for_sort(left, right),
                    (Key::Id(left), Key::Value(right)) => {
                        self.compare_values_for_sort(&Value::String(left.to_string()), right)
                    }
                    (Key::Value(left), Key::Id(right)) => {
                        self.compare_values_for_sort(left, &Value::String(right.to_string()))
                    }
                };
                let order = match field.direction {
                    crate::parser::SortDirection::Asc => order,
                    crate::parser::SortDirection::Desc => order.reverse(),
                };
                if order.is_ne() {
                    return order;
                }
            }
            left.cmp(right)
        };
        let mut order: Vec<usize> = (0..rows.len()).collect();
        if let Some(keep) = keep.filter(|keep| *keep < order.len()) {
            if keep == 0 {
                return Vec::new();
            }
            order.select_nth_unstable_by(keep - 1, compare);
            order.truncate(keep);
        }
        order.sort_unstable_by(compare);
        order
    }

    /// Order two stored rows as `sort` does, reading each key only when
    /// the keys before it tie, and copying only what cannot be lent.
    fn compare_stored_rows(
        &self,
        sort_block: &SortBlock,
        left: &Document,
        right: &Document,
    ) -> std::cmp::Ordering {
        for field in &sort_block.fields {
            let name = match &field.expression {
                Expression::FieldAccess(access) if access.path.len() == 1 => Some(&access.path[0]),
                _ => None,
            };
            let comparison = match name {
                Some(name) if name == "id" => left.id.cmp(&right.id),
                Some(name)
                    if DocumentSystemField::from_name(name).is_none()
                        && left.fields.contains_key(name)
                        && right.fields.contains_key(name) =>
                {
                    self.compare_values_for_sort(&left.fields[name], &right.fields[name])
                }
                _ => self.compare_values_for_sort(
                    &self
                        .evaluate_expression(left, &field.expression)
                        .unwrap_or(Value::Null),
                    &self
                        .evaluate_expression(right, &field.expression)
                        .unwrap_or(Value::Null),
                ),
            };
            let result = match field.direction {
                crate::parser::SortDirection::Asc => comparison,
                crate::parser::SortDirection::Desc => comparison.reverse(),
            };
            if result != std::cmp::Ordering::Equal {
                return result;
            }
        }
        std::cmp::Ordering::Equal
    }

    fn compare_sort_keys<Key: std::borrow::Borrow<Value>>(
        &self,
        sort_block: &SortBlock,
        left: &[Key],
        right: &[Key],
    ) -> std::cmp::Ordering {
        for ((field, left), right) in sort_block.fields.iter().zip(left).zip(right) {
            let comparison = self.compare_values_for_sort(left.borrow(), right.borrow());
            let result = match field.direction {
                crate::parser::SortDirection::Asc => comparison,
                crate::parser::SortDirection::Desc => comparison.reverse(),
            };
            if result != std::cmp::Ordering::Equal {
                return result;
            }
        }
        std::cmp::Ordering::Equal
    }

    /// The positions of the rows with these keys in sorted order, rows with
    /// equal keys staying in the order they came. With `keep`, only the
    /// first that many positions, found without sorting the rest.
    fn sorted_positions<Key: std::borrow::Borrow<Value>>(
        &self,
        keys: &[Vec<Key>],
        sort_block: &SortBlock,
        keep: Option<usize>,
    ) -> Vec<usize> {
        let mut order: Vec<usize> = (0..keys.len()).collect();
        let compare = |left: &usize, right: &usize| {
            self.compare_sort_keys(sort_block, &keys[*left], &keys[*right])
                .then(left.cmp(right))
        };
        if let Some(keep) = keep.filter(|keep| *keep < order.len()) {
            if keep == 0 {
                return Vec::new();
            }
            order.select_nth_unstable_by(keep - 1, compare);
            order.truncate(keep);
        }
        order.sort_unstable_by(compare);
        order
    }

    /// Sort rows, evaluating each row's sort expressions once. With `keep`,
    /// return only the first that many rows of the sorted order: what
    /// `sort { .. }.limit(keep)` asks for.
    fn sort_keeping(
        &self,
        documents: Vec<Document>,
        sort_block: &SortBlock,
        keep: Option<usize>,
    ) -> OQLResult<Vec<Document>> {
        self.check_sort_functions(sort_block)?;
        let keys: Vec<Vec<Value>> = documents
            .iter()
            .map(|document| self.sort_keys(document, sort_block))
            .collect();
        let order = self.sorted_positions(&keys, sort_block, keep);
        let mut rows: Vec<Option<Document>> = documents.into_iter().map(Some).collect();
        Ok(order
            .into_iter()
            .filter_map(|position| rows[position].take())
            .collect())
    }

    /// How many rows the operation after a `sort` keeps, when it is a
    /// `limit`.
    fn rows_kept_after_sort(operations: &[Operation], sort_at: usize) -> Option<usize> {
        match (operations.get(sort_at), operations.get(sort_at + 1)) {
            (Some(Operation::Sort(_)), Some(Operation::Limit(limit))) => {
                usize::try_from(*limit).ok()
            }
            _ => None,
        }
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
        let frame = self.bind_arguments(document, user_func, args)?;
        let _guard = CallFrameGuard::enter(frame, &definition.name)?;
        self.evaluate_expression(document, &definition.body)
    }

    /// Call a function that carries a library statistic's name: directly
    /// when it is the library's own, and as any function otherwise.
    #[inline(never)]
    fn library_or_user_function(
        &self,
        document: &Document,
        name: &str,
        args: &[Expression],
    ) -> OQLResult<Value> {
        let Some(function) = self.functions.get_user_function(name) else {
            return Err(OQLError::Generic {
                message: format!("Unknown function: {}", name),
            });
        };
        if function.library {
            if let Some(answer) = self.library_directly(document, &function, args)? {
                return Ok(answer);
            }
        }
        self.evaluate_user_function(document, &function, args)
    }

    /// A standard library statistic computed without evaluating its
    /// definition, when it is one of those and its values can be read
    /// plainly. Kept out of the evaluator's own frame, which recursion
    /// stacks deeply.
    #[inline(never)]
    fn library_directly(
        &self,
        document: &Document,
        user_func: &crate::functions::UserDefinedFunction,
        args: &[Expression],
    ) -> OQLResult<Option<Value>> {
        let name = &user_func.definition.name;
        if !matches!(name.as_str(), "variance" | "stddev" | "variance_pop" | "stddev_pop") {
            return Ok(None);
        }
        // Of one field across a group: read from the group's rows.
        if let ("group", [Expression::FieldAccess(access)]) = (document.schema.as_str(), args) {
            if access.path.len() == 1 && access.path[0] != "*" {
                let mut values = Vec::new();
                for row in Self::group_rows(document)?.iter() {
                    values.push(self.get_field_value(row, access)?);
                }
                if let Some(answer) = Self::library_statistic(name, values.iter()) {
                    return Ok(Some(answer));
                }
            }
        }
        let frame = self.bind_arguments(document, user_func, args)?;
        Ok(user_func
            .definition
            .parameters
            .first()
            .and_then(|parameter| frame.collections.get(&parameter.name))
            .and_then(|rows| Self::library_statistic_of_rows(name, rows)))
    }

    /// What a standard library statistic gives for rows that each stand for
    /// one value. `None` when the rows are not that.
    fn library_statistic_of_rows(name: &str, rows: &[Document]) -> Option<Value> {
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            values.push(row.fields.get(SCALAR_ROW_FIELD)?);
        }
        Self::library_statistic(name, values.into_iter())
    }

    /// `variance`, `stddev`, `variance_pop` and `stddev_pop` as `std.oql`
    /// defines them, computed directly: the mean, then the squared
    /// distances from it, added in the order given. `None` for any other
    /// function, or for values the definition would have to judge: a list,
    /// or something that is not a number.
    fn library_statistic<'a>(name: &str, values: impl Iterator<Item = &'a Value>) -> Option<Value> {
        let (whole, root) = match name {
            "variance" => (false, false),
            "stddev" => (false, true),
            "variance_pop" => (true, false),
            "stddev_pop" => (true, true),
            _ => return None,
        };
        let mut numbers: Vec<f64> = Vec::new();
        let mut integers: i64 = 0;
        let mut floats = 0.0;
        for value in values {
            match value {
                Value::Null => {}
                Value::Integer(integer) => {
                    integers = integers.checked_add(*integer)?;
                    numbers.push(*integer as f64);
                }
                Value::Float(float) => {
                    floats += float;
                    numbers.push(*float);
                }
                _ => return None,
            }
        }
        let count = numbers.len();
        let least = if whole { 1 } else { 2 };
        if count < least {
            return Some(Value::Null);
        }
        let mean = (floats + integers as f64) / count as f64;
        let mut spread = 0.0;
        for number in &numbers {
            spread += (number - mean) * (number - mean);
        }
        let variance = spread / (if whole { count } else { count - 1 }) as f64;
        Some(Value::Float(if root { variance.sqrt() } else { variance }))
    }

    /// `rows.function(args)` at the end of a chain: call a function written
    /// in OQL with the rows in hand as its first argument.
    ///
    /// A function that gives rows back, gives the chain its rows to carry
    /// on with. Any other value is the answer, in a row named after the
    /// function, as an aggregate's is.
    fn apply_function_to_rows(
        &self,
        rows: Vec<Document>,
        name: &str,
        input: &Expression,
        collection: &str,
        following: &[Operation],
    ) -> OQLResult<Vec<Document>> {
        self.apply_function_to(RowsIn::Owned(rows), name, input, collection, following)
    }

    /// [`Self::apply_function_to_rows`], for rows held or shared.
    fn apply_function_to(
        &self,
        rows: RowsIn,
        name: &str,
        input: &Expression,
        collection: &str,
        following: &[Operation],
    ) -> OQLResult<Vec<Document>> {
        let Some(function) = self.functions.get_user_function(name) else {
            return Err(OQLError::Generic {
                message: format!("Unknown function: {name}"),
            });
        };
        let definition = &function.definition;
        let Some((first, others)) = definition.parameters.split_first() else {
            return Err(OQLError::FunctionError {
                message: format!(
                    "function '{name}' takes no arguments, so it cannot be given the rows before it"
                ),
            });
        };
        // What was written in the parentheses, after the rows themselves.
        let written: &[Expression] = match input {
            Expression::ArrayLiteral(arguments) => &arguments[1..],
            _ => &[],
        };
        if written.len() > others.len() {
            return Err(OQLError::Generic {
                message: format!(
                    "Function '{name}' expects at most {} arguments after the rows, got {}",
                    others.len(),
                    written.len()
                ),
            });
        }
        let scope = Document::with_id(String::new(), String::new(), crate::document::Fields::default());
        let mut frame = CallFrame::default();
        // A first parameter declared as a row of a collection, `s: staff`,
        // makes the function a step: it is called for each row in hand,
        // and the rows it gives for all of them carry on down the chain.
        // Any other first parameter is given the rows together.
        let each_row = self.schema_of(&first.param_type)?.is_some();
        let mut rows = Some(rows);
        if let (false, true, Some(RowsIn::Owned(values))) = (each_row, function.library, &rows) {
            if let Some(answer) = Self::library_statistic_of_rows(name, values) {
                return Ok(vec![Document::with_id(
                    format!("{name}_result"),
                    "aggregate".to_string(),
                    crate::document::Fields::from_iter([(name.to_string(), answer)]),
                )]);
            }
        }
        if !each_row {
            frame
                .collections
                .insert(
                    first.name.clone(),
                    Arc::new(rows.take().map(RowsIn::owned).unwrap_or_default()),
                );
        }
        for (position, parameter) in others.iter().enumerate() {
            let argument = match (written.get(position), &parameter.default) {
                (Some(argument), _) => argument,
                (None, Some(default)) => default,
                (None, None) => {
                    return Err(OQLError::Generic {
                        message: format!(
                            "Function '{name}' needs a value for '{}'",
                            parameter.name
                        ),
                    })
                }
            };
            let value = self.evaluate_expression(&scope, argument)?;
            let value = Self::coerce_argument(value, &parameter.param_type).map_err(|actual| {
                OQLError::TypeError {
                    expected: format!(
                        "{} for parameter '{}' of function '{name}'",
                        parameter.param_type, parameter.name
                    ),
                    actual,
                }
            })?;
            frame.values.insert(parameter.name.clone(), value);
        }
        if let Some(rows) = rows {
            return self.step_through_function(rows.shared(), &function, frame, collection, following);
        }
        let value = {
            let _guard = CallFrameGuard::enter(frame, name)?;
            self.evaluate_expression(&scope, &definition.body)?
        };
        match value {
            Value::List(_) => {
                let mut reached = Vec::new();
                self.push_source_rows(value, collection, &mut reached)?;
                Ok(reached.into_iter().map(SourceRow::into_document).collect())
            }
            value => Ok(vec![Document::with_id(
                format!("{name}_result"),
                "aggregate".to_string(),
                crate::document::Fields::from_iter([(name.to_string(), value)]),
            )]),
        }
    }

    /// `staff.mates()`: call a function for each row and carry on with the
    /// rows it gives for all of them. This is how each row is paired with
    /// others: the function says which.
    ///
    /// When the function's body is a query, its rows are still rows of
    /// their collection. Each keeps what the rest of the chain reads of the
    /// row it was reached from, by that row's collection name, exactly as
    /// a path step does: in `staff.mates().select { a: staff.id, b: .id }`,
    /// `staff.id` is the row `mates` was called for.
    fn step_through_function(
        &self,
        rows: Vec<Arc<Document>>,
        function: &crate::functions::UserDefinedFunction,
        others: CallFrame,
        collection: &str,
        following: &[Operation],
    ) -> OQLResult<Vec<Document>> {
        let definition = &function.definition;
        let parameter = &definition.parameters[0].name;
        let wanted = rows
            .first()
            .map(|row| (row.schema.clone(), Self::anchored_paths(following, &row.schema)))
            .filter(|(_, paths)| !paths.is_empty());
        let scope = Document::with_id(String::new(), String::new(), crate::document::Fields::default());
        // The body is the same query for every row but for the row's own
        // values, so its names are checked once here and not again.
        if let Expression::QueryExpression { query } = &definition.body {
            self.validate_declared_paths(query)?;
        }
        struct Checked(bool);
        impl Drop for Checked {
            fn drop(&mut self) {
                PATHS_CHECKED.with(|checked| checked.set(self.0));
            }
        }
        let _checked = Checked(PATHS_CHECKED.with(|checked| checked.replace(true)));
        // A body that matches rows on equal fields is answered from one
        // table of its collection, made once, when enough rows ask.
        let matched = match &definition.body {
            Expression::QueryExpression { query } if rows.len() >= 8 => {
                self.rows_by_matched_fields(query, &definition.parameters[0])?
            }
            _ => None,
        };
        // `.select { a: staff.id, .. }.sort { .a.asc, .. }.limit(n)` after
        // the step: the result is ordered first by something of the row
        // the function was called for. Calling for the rows in that order
        // means the calls can end once `n` rows are in hand and the next
        // row would sort after all of them.
        let mut ends_early = None;
        let mut rows = rows;
        if let Some((count, field, descending)) =
            rows.first().and_then(|row| Self::ordered_by_calling_row(following, &row.schema))
        {
            let mut keyed = Vec::with_capacity(rows.len());
            for row in rows {
                let key = self.get_field_value(&row, &FieldAccess { path: vec![field.clone()] })?;
                keyed.push((key, row));
            }
            // Where a row without a value sorts is the sort's to decide.
            if keyed.iter().all(|(key, _)| !key.is_null()) {
                keyed.sort_by(|left, right| {
                    let order = self.compare_values_for_sort(&left.0, &right.0);
                    if descending { order.reverse() } else { order }
                });
                ends_early = Some((count, keyed.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>()));
            }
            rows = keyed.into_iter().map(|(_, row)| row).collect();
        }
        let mut reached = Vec::new();
        for (position, row) in rows.into_iter().enumerate() {
            if let Some((count, keys)) = &ends_early {
                if reached.len() >= *count
                    && position > 0
                    && self.compare_values_for_sort(&keys[position - 1], &keys[position]).is_ne()
                {
                    break;
                }
            }
            let origin = match &wanted {
                Some((from, paths)) => {
                    let mut kept = crate::document::Fields::with_capacity_and_hasher(paths.len() + 1, Default::default());
                    kept.insert(ORIGIN_NAME.to_string(), Value::String(from.clone()));
                    for path in paths {
                        kept.insert(
                            path.join("."),
                            self.get_field_value(&row, &FieldAccess { path: path.clone() })?,
                        );
                    }
                    Some(Value::Object(kept))
                }
                None => row.fields.get(ORIGIN_FIELD).cloned(),
            };
            // A body that is a path from the row, `s.reports`, reaches rows
            // that are still rows of their collection.
            if let Some(path) = Self::extract_scoped_path(&definition.body, parameter)
                .filter(|path| !path.is_empty())
            {
                for mut found in self.path_source_rows(&row, &path)?.0 {
                    if let Some(origin) = &origin {
                        found.fields.insert(ORIGIN_FIELD.to_string(), origin.clone());
                    }
                    reached.push(found);
                }
                continue;
            }
            let key = match &matched {
                Some(matched) => matched.key_of(self, &row)?,
                None => None,
            };
            let row = Arc::try_unwrap(row).unwrap_or_else(|shared| shared.as_ref().clone());
            let frame = CallFrame {
                values: others.values.clone(),
                rows: HashMap::from([(parameter.clone(), row)]),
                collections: others.collections.clone(),
                locals: HashMap::new(),
            };
            let given = {
                let _guard = CallFrameGuard::enter(frame, &definition.name)?;
                match &definition.body {
                    _ if key.is_some() => {
                        let matched = matched.as_ref().expect("a key comes from a table");
                        let found = matched.table.get(key.as_ref().expect("checked")).map(Vec::as_slice);
                        let mut given = Vec::new();
                        match &matched.residual {
                            None => given.extend(
                                found
                                    .unwrap_or_default()
                                    .iter()
                                    .map(|at| matched.rows[*at].as_ref().clone()),
                            ),
                            Some(residual) if found.is_some() => {
                                let row = CallFrame::row(parameter).unwrap_or_else(|| scope.clone());
                                ROW_READ_AS.with(|name| *name.borrow_mut() = Some(parameter.clone()));
                                let prepared = self.materialize_condition_with_scope(residual, &row);
                                ROW_READ_AS.with(|name| *name.borrow_mut() = None);
                                let prepared = prepared?;
                                for at in found.unwrap_or_default() {
                                    if self.evaluate_condition(&matched.rows[*at], &prepared)? {
                                        given.push(matched.rows[*at].as_ref().clone());
                                    }
                                }
                            }
                            Some(_) => {}
                        }
                        given
                    }
                    // What the query reads of the row becomes constants in
                    // it, so that an index can serve a filter on them.
                    Expression::QueryExpression { query } => {
                        let row = CallFrame::row(parameter).unwrap_or_else(|| scope.clone());
                        ROW_READ_AS.with(|name| *name.borrow_mut() = Some(parameter.clone()));
                        let prepared = self.materialize_query_with_scope(query, &row);
                        ROW_READ_AS.with(|name| *name.borrow_mut() = None);
                        self.execute_general_query(&prepared?)?
                    }
                    body => {
                        let mut given = Vec::new();
                        self.push_source_rows(
                            self.evaluate_expression(&scope, body)?,
                            collection,
                            &mut given,
                        )?;
                        given.into_iter().map(SourceRow::into_document).collect()
                    }
                }
            };
            for mut found in given {
                if let Some(origin) = &origin {
                    found.fields.insert(ORIGIN_FIELD.to_string(), origin.clone());
                }
                reached.push(found);
            }
        }
        Ok(reached)
    }

    /// Whether what follows a step is `select`, `sort`, `limit(n)` with the
    /// sort's first key a field the select takes straight from the row the
    /// step was taken from, as `a: staff.id`. Gives `n`, that row's field,
    /// and whether the order is descending.
    fn ordered_by_calling_row(following: &[Operation], schema: &str) -> Option<(usize, String, bool)> {
        let [Operation::Select(select), Operation::Sort(sort), Operation::Limit(count), ..] = following
        else {
            return None;
        };
        if schema.is_empty()
            || select.fields.iter().any(|field| {
                Self::expression_contains_aggregate(&field.expression)
                    || Self::expression_uses_position(&field.expression)
            })
        {
            return None;
        }
        let first = sort.fields.first()?;
        let Expression::FieldAccess(FieldAccess { path }) = &first.expression else {
            return None;
        };
        let [alias] = path.as_slice() else {
            return None;
        };
        let mut named = select
            .fields
            .iter()
            .filter(|field| field.alias.as_deref() == Some(alias.as_str()));
        let (Some(field), None) = (named.next(), named.next()) else {
            return None;
        };
        match &field.expression {
            Expression::ChainedAccess { base, field }
                if matches!(base.as_ref(), Expression::Identifier(name) if name == schema) =>
            {
                Some((
                    *count as usize,
                    field.clone(),
                    matches!(first.direction, crate::parser::SortDirection::Desc),
                ))
            }
            _ => None,
        }
    }

    /// For a function whose body is `collection.where { .a == row.x && .. }`:
    /// the collection's rows in a table keyed by the fields compared for
    /// equality with the row's own, so each call looks its rows up instead
    /// of filtering the collection. `None` when the body is not of that
    /// form, and the general way answers.
    fn rows_by_matched_fields(
        &self,
        query: &OQLQuery,
        parameter: &crate::parser::FunctionParameter,
    ) -> OQLResult<Option<MatchedRows>> {
        let [Operation::Where(block)] = query.operations.as_slice() else {
            return Ok(None);
        };
        let (Some(here), Some(there)) =
            (self.schema_of(&query.collection)?, self.schema_of(&parameter.param_type)?)
        else {
            return Ok(None);
        };
        fn declared(mut field: Option<&FieldType>) -> Option<&FieldType> {
            while let Some(FieldType::Optional(inner)) = field {
                field = Some(inner);
            }
            field
        }
        fn parts<'a>(condition: &'a Condition, into: &mut Vec<&'a Condition>) {
            match condition {
                Condition::And(left, right) => {
                    parts(left, into);
                    parts(right, into);
                }
                other => into.push(other),
            }
        }
        let mut conjuncts = Vec::new();
        parts(&block.condition, &mut conjuncts);
        let mut keys = Vec::new();
        let mut residual: Option<Condition> = None;
        for conjunct in conjuncts {
            // `.unit == p.unit`, either way round, between two fields
            // declared alike.
            let pair = match conjunct {
                Condition::Comparison { left, operator: ComparisonOperator::Equal, right } => {
                    let sides = |mine: &Expression, theirs: &Expression| match (mine, theirs) {
                        (
                            Expression::FieldAccess(FieldAccess { path }),
                            Expression::ChainedAccess { base, field },
                        ) if path.len() == 1
                            && matches!(base.as_ref(), Expression::Identifier(name) if *name == parameter.name) =>
                        {
                            Some((path[0].clone(), field.clone()))
                        }
                        _ => None,
                    };
                    sides(left, right).or_else(|| sides(right, left))
                }
                _ => None,
            }
            .filter(|(mine, theirs)| {
                let (mine, theirs) = (declared(here.fields.get(mine)), declared(there.fields.get(theirs)));
                mine.is_some() && mine == theirs
            });
            match pair {
                Some(pair) => keys.push(pair),
                None => {
                    residual = Some(match residual {
                        Some(before) => Condition::And(Box::new(before), Box::new(conjunct.clone())),
                        None => conjunct.clone(),
                    });
                }
            }
        }
        if keys.is_empty() {
            return Ok(None);
        }
        let mut table: HashMap<Vec<MatchAtom>, Vec<usize>> = HashMap::new();
        // From the matched fields' columns when the store has them: the
        // table is then made without reading a row.
        let names: Vec<String> = keys.iter().map(|(mine, _)| mine.clone()).collect();
        let from_columns = match COLUMN_AGGREGATION.with(|enabled| enabled.get()) {
            true => self.store.scan_rows_and_columns(&query.collection, &names)?,
            false => None,
        };
        if let Some((rows, batch)) = from_columns {
            use crate::storage::{Column, ColumnData};
            // The table is good for as long as the columns are, so it is
            // kept with them and made once, not once for each query.
            let kept_as = format!("matched:{}:{}", query.collection, names.join(","));
            if let Some(table) = self
                .store
                .kept_with_columns(&kept_as, batch.stamp())
                .and_then(|kept| kept.downcast::<HashMap<Vec<MatchAtom>, Vec<usize>>>().ok())
            {
                return Ok(Some(MatchedRows { rows, table, keys, residual }));
            }
            'rows: for at in 0..batch.rows() {
                let mut key = Vec::with_capacity(names.len());
                for field in 0..names.len() {
                    let column = batch.column(field);
                    if !column.has_value(at) {
                        continue 'rows;
                    }
                    key.push(match column.data() {
                        ColumnData::Integers(values) => MatchAtom::Integer(values[at]),
                        ColumnData::Floats(values) => MatchAtom::number(values[at]),
                        ColumnData::Booleans(values) => MatchAtom::Boolean(values[at] != 0),
                        ColumnData::DateTimes(values) => MatchAtom::Instant(values[at].timestamp_micros()),
                        ColumnData::References(values) => MatchAtom::Reference(values[at]),
                        ColumnData::Strings { offsets, bytes } => {
                            MatchAtom::Text(Column::text(offsets, bytes, at).to_string())
                        }
                    });
                }
                table.entry(key).or_default().push(at);
            }
            COLUMN_SCANS_RUN.with(|runs| runs.set(runs.get() + 1));
            let table = Arc::new(table);
            self.store.keep_with_columns(&kept_as, batch.stamp(), table.clone());
            return Ok(Some(MatchedRows { rows, table, keys, residual }));
        }
        let rows: Vec<Arc<Document>> = match self.store.scan_documents_shared(&query.collection)? {
            Some(rows) => rows,
            None => self
                .execute_general_query(&OQLQuery {
                    collection: query.collection.clone(),
                    operations: Vec::new(),
                })?
                .into_iter()
                .map(Arc::new)
                .collect(),
        };
        for (at, row) in rows.iter().enumerate() {
            let fields: Vec<&str> = keys.iter().map(|(mine, _)| mine.as_str()).collect();
            if let Some(key) = MatchedRows::key(self, row, &fields)? {
                table.entry(key).or_default().push(at);
            }
        }
        Ok(Some(MatchedRows { rows: Arc::new(rows), table: Arc::new(table), keys, residual }))
    }

    /// The value of a function's local name. It is worked out the first
    /// time the call uses it and kept until the call ends, so a local used
    /// for every row of a nested query is not worked out for every row.
    #[inline(never)]
    fn evaluate_local(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        let [expression, Expression::Literal(Literal::Integer(place))] = args else {
            return Err(OQLError::generic("a function local is malformed"));
        };
        let known = CALL_FRAMES.with(|frames| {
            frames
                .borrow()
                .last()
                .and_then(|frame| frame.locals.get(place).cloned())
        });
        if let Some(known) = known {
            return Ok(known);
        }
        let value = self.evaluate_expression(document, expression)?;
        CALL_FRAMES.with(|frames| {
            if let Some(frame) = frames.borrow_mut().last_mut() {
                frame.locals.insert(*place, value.clone());
            }
        });
        Ok(value)
    }

    /// Bind the arguments of a call to the function's parameter names.
    #[inline(never)]
    fn bind_arguments(
        &self,
        document: &Document,
        user_func: &crate::functions::UserDefinedFunction,
        args: &[Expression],
    ) -> OQLResult<CallFrame> {
        let definition = &user_func.definition;
        // Parameters with a default may be left out, from the end.
        let required = definition
            .parameters
            .iter()
            .rposition(|parameter| parameter.default.is_none())
            .map_or(0, |last| last + 1);
        if args.len() < required || args.len() > definition.parameters.len() {
            return Err(OQLError::Generic {
                message: if required == definition.parameters.len() {
                    format!(
                        "Function '{}' expects {} arguments, got {}",
                        definition.name,
                        required,
                        args.len()
                    )
                } else {
                    format!(
                        "Function '{}' expects {} to {} arguments, got {}",
                        definition.name,
                        required,
                        definition.parameters.len(),
                        args.len()
                    )
                },
            });
        }

        let mut frame = CallFrame::default();
        for (position, parameter) in definition.parameters.iter().enumerate() {
            let arg = match (args.get(position), &parameter.default) {
                (Some(arg), _) => arg,
                (None, Some(default)) => default,
                (None, None) => unreachable!("the argument count was checked"),
            };
            if let Some(row) = self.row_argument(document, arg) {
                frame.rows.insert(parameter.name.clone(), row);
                continue;
            }
            // `.is_boss()` is `is_boss(.)`: the current row itself, for a
            // parameter that takes a row. A scalar parameter, or a row
            // that stands for one value of a list, takes the value.
            let takes_value = matches!(
                parameter.param_type.to_ascii_lowercase().as_str(),
                "string" | "integer" | "int" | "float" | "double" | "boolean" | "bool"
                    | "datetime" | "timestamp" | "object"
            );
            if matches!(arg, Expression::FieldAccess(FieldAccess { path }) if path.is_empty())
                && !takes_value
                && !document.fields.contains_key(SCALAR_ROW_FIELD)
            {
                frame.rows.insert(parameter.name.clone(), document.clone());
                continue;
            }
            MANY_VALUED.with(|flag| flag.set(false));
            let value = self.evaluate_expression(document, arg)?;
            // A relationship, or any path through a collection, given for a
            // parameter that takes more than one value: the rows it reaches,
            // still rows of their collection, so the function can filter,
            // aggregate and follow relationships from them.
            let many = MANY_VALUED.with(|flag| flag.replace(false));
            if let (true, false, Expression::FieldAccess(access)) = (many, takes_value, arg) {
                // In a grouped select the path is read from the group's
                // rows, as it is anywhere else there.
                let rows = if document.schema == "group" {
                    let mut reached = Vec::new();
                    for row in Self::group_rows(document)?.iter() {
                        reached.extend(self.path_source_rows(row, &access.path)?.0);
                    }
                    reached
                } else {
                    self.path_source_rows(document, &access.path)?.0
                };
                frame.collections.insert(parameter.name.clone(), Arc::new(rows));
                continue;
            }
            // `.boss.is_boss()`: a reference given for a row parameter is
            // the row it points at.
            if let (false, Value::Reference(reference)) = (takes_value, &value) {
                if let Some(target) = self.referenced_document(reference)? {
                    frame.rows.insert(parameter.name.clone(), target.as_ref().clone());
                    continue;
                }
            }
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

        Ok(frame)
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

/// The field that carries a scalar when a list of scalars is treated as a
/// collection. It is not a legal field name, so it cannot collide with data.
const SCALAR_ROW_FIELD: &str = "$value";

/// The aggregate that `.field.distinct().count()` is run as. It has no
/// spelling of its own in the language, so the name is one that cannot be
/// written.
const DISTINCT_COUNT: &str = "$distinct_count";

/// Where `partition_by` notes a row's partition for the next `select`. Not
/// a legal field name, so it cannot collide with data.
const PARTITION_FIELD: &str = "$partition";

/// What a path has reached, as far as the schemas say: used to tell a name
/// that is misspelt from one that is merely absent.
#[derive(Clone)]
enum Place {
    /// A row of a collection that has a schema.
    Rows(Arc<Schema>),
    /// An embedded object whose fields the schema declares.
    Shape {
        schema: Arc<Schema>,
        /// The shape's path within the schema, as `items[]` or `address.geo`.
        path: String,
        fields: Vec<(String, FieldType)>,
    },
    /// A row a `select` shaped: its fields are the ones the select named.
    /// It is still a row of `rows` where that is known, so a relationship
    /// can be followed from it.
    Shaped {
        names: Vec<String>,
        rows: Option<Arc<Schema>>,
    },
    /// Anything whose fields are not declared: nothing can be ruled out.
    Any,
}

/// Where a row reached by a path step keeps the values of the row it came
/// from that the rest of the query asks for by the collection's name.
const ORIGIN_FIELD: &str = "$origin";
/// Within [`ORIGIN_FIELD`], the collection the values belong to.
const ORIGIN_NAME: &str = "$name";

/// The steps `walk` took to reach a row, readable as `._depth`.
const WALK_DEPTH_FIELD: &str = "_depth";

/// The rows on the way to a walked row, from the row the walk started at to
/// the row itself, readable as `._path`.
const WALK_PATH_FIELD: &str = "_path";

/// The rows a `select` is shaping, for the functions that depend on a row's
/// position among them: `row_number`, `prev`, `so_far()` and the rest.
struct WindowFrame {
    rows: Vec<Document>,
    /// The row being shaped.
    position: std::cell::Cell<usize>,
    /// Each partition's rows, as positions in `rows`, in order.
    members: Vec<Vec<usize>>,
    /// For each row: its partition, and its place within it.
    place: Vec<(usize, usize)>,
    /// What each running function has accumulated, per call and partition.
    running: std::cell::RefCell<HashMap<(usize, usize), Running>>,
    /// Whether `partition_by` came before the select. If so, an aggregate
    /// or an operation with no receiver in it is over the row's partition.
    partitioned: bool,
    /// Each partition's rows together, made when first asked for.
    together: std::cell::RefCell<HashMap<usize, Arc<Vec<Document>>>>,
    /// What each aggregate gave for each partition, per call.
    totals: std::cell::RefCell<HashMap<(usize, usize), Value>>,
}

/// Rows given to a function: this query's own, or a collection's as the
/// store holds them.
enum RowsIn {
    Owned(Vec<Document>),
    Shared(Vec<Arc<Document>>),
}

impl RowsIn {
    fn owned(self) -> Vec<Document> {
        match self {
            Self::Owned(rows) => rows,
            Self::Shared(rows) => rows.iter().map(|row| row.as_ref().clone()).collect(),
        }
    }

    fn shared(self) -> Vec<Arc<Document>> {
        match self {
            Self::Owned(rows) => rows.into_iter().map(Arc::new).collect(),
            Self::Shared(rows) => rows,
        }
    }
}

/// One value a row is matched by, in a form that is the same whether it
/// was read from a row or from a column.
#[derive(Hash, PartialEq, Eq)]
enum MatchAtom {
    Integer(i64),
    Float(u64),
    Text(String),
    Boolean(bool),
    Instant(i64),
    Reference(ObjRef),
}

impl MatchAtom {
    /// A number that is whole is the same key as that whole number.
    fn number(float: f64) -> Self {
        if float.fract() == 0.0 && float.abs() < 9_007_199_254_740_992.0 {
            Self::Integer(float as i64)
        } else {
            Self::Float(float.to_bits())
        }
    }

    /// `None` for a value rows are not matched by here: none at all, or a
    /// list or an object.
    fn of(value: &Value) -> Option<Self> {
        Some(match value {
            Value::Integer(value) => Self::Integer(*value),
            Value::Float(value) => Self::number(*value),
            Value::String(value) => Self::Text(value.clone()),
            Value::Boolean(value) => Self::Boolean(*value),
            Value::DateTime(value) => Self::Instant(value.timestamp_micros()),
            Value::Reference(value) => Self::Reference(*value),
            _ => return None,
        })
    }
}

/// A collection's rows, findable by the fields a function matches on.
struct MatchedRows {
    rows: Arc<Vec<Arc<Document>>>,
    /// Where the rows with each combination of values are in `rows`.
    table: Arc<HashMap<Vec<MatchAtom>, Vec<usize>>>,
    /// Each matched pair: the collection's field, and the calling row's.
    keys: Vec<(String, String)>,
    /// What else the function asks of a row, if anything.
    residual: Option<Condition>,
}

impl MatchedRows {
    /// The key of `row` by `fields`. `None` when one of them has no value:
    /// what equality means then is for the general way to say.
    fn key<S: DocumentStore>(
        executor: &QueryExecutor<S>,
        row: &Document,
        fields: &[&str],
    ) -> OQLResult<Option<Vec<MatchAtom>>> {
        let mut atoms = Vec::with_capacity(fields.len());
        for field in fields {
            let value = executor.get_field_value(row, &FieldAccess { path: vec![field.to_string()] })?;
            match MatchAtom::of(&value) {
                Some(atom) => atoms.push(atom),
                None => return Ok(None),
            }
        }
        Ok(Some(atoms))
    }

    /// The key a calling row looks its rows up by.
    fn key_of<S: DocumentStore>(
        &self,
        executor: &QueryExecutor<S>,
        row: &Document,
    ) -> OQLResult<Option<Vec<MatchAtom>>> {
        let fields: Vec<&str> = self.keys.iter().map(|(_, theirs)| theirs.as_str()).collect();
        Self::key(executor, row, &fields)
    }
}

/// The state of one running function over one partition.
#[derive(Default)]
struct Running {
    /// How many of the partition's rows have been taken in.
    taken: usize,
    count: i64,
    integer_sum: i64,
    float_sum: f64,
    has_float: bool,
    extreme: Option<Value>,
    /// For `rank` and `dense_rank`: the value of the last row taken in.
    last: Option<Value>,
    rank: i64,
    dense_rank: i64,
    /// A value came that a running state cannot hold, so the rows are read
    /// the long way instead.
    unfit: bool,
}

impl Running {
    /// What the reduction `name` gives for the rows taken in.
    fn answer(&self, name: &str) -> Value {
        match name {
            "count" => Value::Integer(self.count),
            "sum" if self.has_float => Value::Float(self.float_sum + self.integer_sum as f64),
            "sum" => Value::Integer(self.integer_sum),
            "avg" if self.count == 0 => Value::Null,
            "avg" => Value::Float((self.float_sum + self.integer_sum as f64) / self.count as f64),
            _ => self.extreme.clone().unwrap_or(Value::Null),
        }
    }
}

impl WindowFrame {
    fn new(rows: Vec<Document>) -> OQLResult<Self> {
        let mut partitions: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut members: Vec<Vec<usize>> = Vec::new();
        let mut place = Vec::with_capacity(rows.len());
        for (position, row) in rows.iter().enumerate() {
            let key = match row.fields.get(PARTITION_FIELD) {
                Some(key) => bincode::serialize(key).map_err(|error| OQLError::Generic {
                    message: format!("Failed to encode partition key: {error}"),
                })?,
                None => Vec::new(),
            };
            let next = members.len();
            let partition = *partitions.entry(key).or_insert(next);
            if partition == next {
                members.push(Vec::new());
            }
            place.push((partition, members[partition].len()));
            members[partition].push(position);
        }
        Ok(Self {
            partitioned: rows
                .first()
                .is_some_and(|row| row.fields.contains_key(PARTITION_FIELD)),
            rows,
            position: std::cell::Cell::new(0),
            members,
            place,
            running: std::cell::RefCell::new(HashMap::new()),
            together: std::cell::RefCell::new(HashMap::new()),
            totals: std::cell::RefCell::new(HashMap::new()),
        })
    }

    /// The frame and partition of `document`, when it is the row a select
    /// that follows `partition_by` is shaping right now.
    fn partition_of(document: &Document) -> Option<(std::rc::Rc<WindowFrame>, usize)> {
        let frame = WINDOWS.with(|frames| frames.borrow().last().cloned())?;
        if !frame.partitioned {
            return None;
        }
        let position = frame.position.get();
        std::ptr::eq(document, frame.rows.get(position)?).then(|| {
            let partition = frame.place[position].0;
            (frame, partition)
        })
    }

    /// Every row of a partition, in order.
    fn rows_of(&self, partition: usize) -> Arc<Vec<Document>> {
        self.together
            .borrow_mut()
            .entry(partition)
            .or_insert_with(|| {
                Arc::new(
                    self.members[partition]
                        .iter()
                        .map(|position| self.rows[*position].clone())
                        .collect(),
                )
            })
            .clone()
    }

    fn is_position_function(name: &str) -> bool {
        matches!(
            name,
            "row_number"
                | "rank"
                | "dense_rank"
                | "prev"
                | "next"
        )
    }
}

thread_local! {
    /// Set while a function's body, already checked, runs for each row.
    static PATHS_CHECKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// While a function's body is prepared for one row: the name of the
    /// parameter that holds the row.
    static ROW_READ_AS: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// Set when a path was just read that passes through a collection, so
    /// that a comparison can tell such a value from a list that is stored.
    static MANY_VALUED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// The frames of the selects being evaluated, innermost last.
    static WINDOWS: std::cell::RefCell<Vec<std::rc::Rc<WindowFrame>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Removes a select's frame when the select is done, however it ends.
struct WindowGuard;

impl Drop for WindowGuard {
    fn drop(&mut self) {
        WINDOWS.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

thread_local! {
    static NESTED_QUERIES_RUN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

thread_local! {
    static COLUMN_AGGREGATION: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

thread_local! {
    /// `check(..)` constraints by their text, parsed.
    static CHECK_CONDITIONS: std::cell::RefCell<HashMap<String, Arc<Condition>>> =
        std::cell::RefCell::new(HashMap::new());
}

thread_local! {
    static INDEX_TALLIES_RUN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many relationship tallies this thread has answered from an index
/// alone. Intended for tests, which use it to show a query took that path.
#[doc(hidden)]
pub fn index_tallies_run() -> u64 {
    INDEX_TALLIES_RUN.with(std::cell::Cell::get)
}

thread_local! {
    static COLUMN_FOLDS_RUN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// How many queries this thread has answered from whole columns.
    static COLUMN_SCANS_RUN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many queries this thread has answered from whole columns, without
/// reading a row. For tests that must know which way a query was answered.
pub fn column_scans_run() -> u64 {
    COLUMN_SCANS_RUN.with(std::cell::Cell::get)
}

pub fn column_folds_run() -> u64 {
    COLUMN_FOLDS_RUN.with(std::cell::Cell::get)
}

/// Turn the column-batch aggregation path on or off for this thread. It is
/// an optimization only, so tests switch it off to get the reference answer.
#[doc(hidden)]
pub fn set_column_aggregation(enabled: bool) {
    COLUMN_AGGREGATION.with(|flag| flag.set(enabled));
}

/// What one result field of a fold holds.
enum FoldOutput {
    /// The group key at this position.
    Key(usize),
    /// The aggregate at this position.
    Aggregate(usize),
}

/// Where a group key or an aggregate gets its value for one row.
enum RowInput {
    /// The row itself: `count()`.
    Rows,
    /// A stored field, read straight from the column batch.
    Slot(usize),
    /// A path that starts at a reference held in a stored field. What it
    /// reaches depends only on the reference, so within one statement each
    /// distinct reference is resolved once and remembered under `memo`.
    Through {
        slot: usize,
        memo: usize,
        path: FieldAccess,
    },
    /// Any other path the evaluator resolves: into an object, to a
    /// relationship.
    Path(FieldAccess),
    /// An expression the evaluator computes.
    Expression(Expression),
}

struct FoldAggregate {
    kind: AggregateKind,
    input: RowInput,
    /// Conditions a row must pass to count towards this aggregate.
    filters: Vec<Condition>,
}

enum FoldOperand {
    Key(usize),
    Aggregate(usize),
    Literal(Value),
}

enum FoldCondition {
    Compare(FoldOperand, ComparisonOperator, FoldOperand),
    And(Box<FoldCondition>, Box<FoldCondition>),
    Or(Box<FoldCondition>, Box<FoldCondition>),
    Not(Box<FoldCondition>),
}

/// How one query folds storage's column batches into its result.
#[derive(Default)]
struct FoldPlan {
    /// The stored fields the scan decodes.
    fields: Vec<String>,
    /// Filters storage does not evaluate.
    residual: Vec<Condition>,
    keys: Vec<RowInput>,
    /// The names a group row answers to, and the key each one holds.
    key_names: Vec<(String, usize)>,
    aggregates: Vec<FoldAggregate>,
    outputs: Vec<(String, FoldOutput)>,
    having: Option<FoldCondition>,
    /// How many `RowInput::Through` inputs there are.
    memos: usize,
}

/// A value computed for the row in hand: its own, or one remembered for the
/// reference it was reached through.
enum Held {
    Owned(Value),
    Remembered(usize),
}

impl FoldPlan {
    /// The scan column for a stored field, added on first use.
    fn slot(&mut self, schema: &Schema, name: &str) -> Option<usize> {
        if !schema.fields.contains_key(name) || DocumentSystemField::from_name(name).is_some() {
            return None;
        }
        Some(
            self.fields
                .iter()
                .position(|known| known == name)
                .unwrap_or_else(|| {
                    self.fields.push(name.to_string());
                    self.fields.len() - 1
                }),
        )
    }

    fn input(&mut self, schema: &Schema, path: &[String]) -> Option<RowInput> {
        match path {
            [] => None,
            [first, ..] if first == "*" => None,
            [name] => Some(match self.slot(schema, name) {
                Some(slot) => RowInput::Slot(slot),
                None => RowInput::Path(FieldAccess {
                    path: path.to_vec(),
                }),
            }),
            [first, ..] => {
                let mut declared = schema.fields.get(first);
                while let Some(FieldType::Optional(inner)) = declared {
                    declared = Some(inner);
                }
                let path = FieldAccess {
                    path: path.to_vec(),
                };
                Some(match (declared, self.slot(schema, first)) {
                    (Some(FieldType::Reference(_)), Some(slot)) => {
                        self.memos += 1;
                        RowInput::Through {
                            slot,
                            memo: self.memos - 1,
                            path,
                        }
                    }
                    _ => RowInput::Path(path),
                })
            }
        }
    }

    fn name_key(&mut self, name: String, position: usize, replace: bool) {
        match self.key_names.iter_mut().find(|(known, _)| *known == name) {
            Some(entry) if replace => entry.1 = position,
            Some(_) => {}
            None => self.key_names.push((name, position)),
        }
    }

    fn key_named(&self, name: &str) -> Option<usize> {
        self.key_names
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, position)| *position)
    }

    /// Whether any part needs the evaluator, and so a row to give it.
    fn needs_row(&self) -> bool {
        let evaluated = |input: &RowInput| !matches!(input, RowInput::Rows | RowInput::Slot(_));
        !self.residual.is_empty()
            || self.keys.iter().any(evaluated)
            || self
                .aggregates
                .iter()
                .any(|aggregate| evaluated(&aggregate.input) || !aggregate.filters.is_empty())
    }
}

#[derive(Clone, Copy)]
enum AggregateKind {
    /// `count()`
    Rows,
    /// `count(.field)`
    NonNull,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggregateKind {
    fn named(function: &str, counts_rows: bool) -> Option<Self> {
        Some(match function {
            "count" if counts_rows => Self::Rows,
            "count" => Self::NonNull,
            "sum" => Self::Sum,
            "avg" => Self::Avg,
            "min" => Self::Min,
            "max" => Self::Max,
            _ => return None,
        })
    }
}

/// The running state of one aggregate. Its results must equal what
/// `aggregate_rows` computes for the same inputs.
struct Accumulator {
    kind: AggregateKind,
    count: i64,
    integer_sum: i64,
    float_sum: f64,
    has_float: bool,
    extreme: Option<Value>,
}

impl Accumulator {
    fn new(kind: AggregateKind) -> Self {
        Self {
            kind,
            count: 0,
            integer_sum: 0,
            float_sum: 0.0,
            has_float: false,
            extreme: None,
        }
    }

    fn push(&mut self, value: Option<ProjectedValueRef<'_>>) -> OQLResult<()> {
        let value = match value {
            None | Some(ProjectedValueRef::Null) | Some(ProjectedValueRef::Value(Value::Null)) => {
                return Ok(())
            }
            Some(value) => value,
        };
        let number = match value {
            ProjectedValueRef::Integer(integer) => Some(Ok(integer)),
            ProjectedValueRef::Value(Value::Integer(integer)) => Some(Ok(*integer)),
            ProjectedValueRef::Float(float) => Some(Err(float)),
            ProjectedValueRef::Value(Value::Float(float)) => Some(Err(*float)),
            _ => None,
        };
        match self.kind {
            AggregateKind::Rows => self.count += 1,
            AggregateKind::NonNull => self.count += 1,
            AggregateKind::Sum => match number {
                Some(Ok(integer)) => {
                    if self.has_float {
                        self.float_sum += integer as f64;
                    } else {
                        self.integer_sum =
                            self.integer_sum.checked_add(integer).ok_or_else(|| {
                                OQLError::FunctionError {
                                    message: "Integer overflow evaluating sum()".to_string(),
                                }
                            })?;
                    }
                    self.count += 1;
                }
                Some(Err(float)) => {
                    if self.has_float {
                        self.float_sum += float;
                    } else {
                        self.float_sum = self.integer_sum as f64 + float;
                        self.has_float = true;
                    }
                    self.count += 1;
                }
                None => return Err(Self::not_numeric(value)),
            },
            AggregateKind::Avg => match number {
                Some(Ok(integer)) => {
                    self.float_sum += integer as f64;
                    self.count += 1;
                }
                Some(Err(float)) => {
                    self.float_sum += float;
                    self.count += 1;
                }
                None => return Err(Self::not_numeric(value)),
            },
            AggregateKind::Min | AggregateKind::Max => {
                let wanted = if matches!(self.kind, AggregateKind::Min) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
                let candidate = value.to_owned();
                match &self.extreme {
                    None => self.extreme = Some(candidate),
                    Some(current) => {
                        if crate::value_semantics::compare(&candidate, current) == Some(wanted) {
                            self.extreme = Some(candidate);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn not_numeric(value: ProjectedValueRef<'_>) -> OQLError {
        OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: value.to_owned().type_name().to_string(),
        }
    }

    fn finish(self) -> Value {
        match self.kind {
            AggregateKind::Rows | AggregateKind::NonNull => Value::Integer(self.count),
            AggregateKind::Sum => {
                if self.count == 0 {
                    Value::Integer(0)
                } else if self.has_float {
                    Value::Float(self.float_sum)
                } else {
                    Value::Integer(self.integer_sum)
                }
            }
            AggregateKind::Avg => {
                if self.count == 0 {
                    Value::Null
                } else {
                    Value::Float(self.float_sum / self.count as f64)
                }
            }
            AggregateKind::Min | AggregateKind::Max => self.extreme.unwrap_or(Value::Null),
        }
    }
}

/// A group key component, borrowed from a column batch, in the form under
/// which equal OQL values are identical: a whole float is an integer.
#[derive(Hash, PartialEq)]
enum KeyAtom<'a> {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(u64),
    Text(&'a str),
    Instant(i64),
    Reference(ObjRef),
}

#[derive(PartialEq)]
enum OwnedKeyAtom {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(u64),
    Text(String),
    Instant(i64),
    Reference(ObjRef),
}

impl<'a> KeyAtom<'a> {
    /// `None` for a value that is not a scalar.
    fn of(value: Option<ProjectedValueRef<'a>>) -> Option<Self> {
        let number = |float: f64| {
            if float.fract() == 0.0 && float.abs() < 9_007_199_254_740_992.0 {
                KeyAtom::Integer(float as i64)
            } else {
                KeyAtom::Float(float.to_bits())
            }
        };
        Some(match value {
            None | Some(ProjectedValueRef::Null) => KeyAtom::Null,
            Some(ProjectedValueRef::Boolean(value)) => KeyAtom::Boolean(value),
            Some(ProjectedValueRef::Integer(value)) => KeyAtom::Integer(value),
            Some(ProjectedValueRef::Float(value)) => number(value),
            Some(ProjectedValueRef::String(value)) => KeyAtom::Text(value),
            Some(ProjectedValueRef::DateTime(value)) => KeyAtom::Instant(value.timestamp_micros()),
            Some(ProjectedValueRef::Reference(value)) => KeyAtom::Reference(value),
            Some(ProjectedValueRef::Value(value)) => match value {
                Value::Null => KeyAtom::Null,
                Value::Boolean(value) => KeyAtom::Boolean(*value),
                Value::Integer(value) => KeyAtom::Integer(*value),
                Value::Float(value) => number(*value),
                Value::String(value) => KeyAtom::Text(value),
                Value::DateTime(value) => KeyAtom::Instant(value.timestamp_micros()),
                Value::Reference(value) => KeyAtom::Reference(*value),
                _ => return None,
            },
        })
    }

    fn owned(&self) -> OwnedKeyAtom {
        match self {
            KeyAtom::Null => OwnedKeyAtom::Null,
            KeyAtom::Boolean(value) => OwnedKeyAtom::Boolean(*value),
            KeyAtom::Integer(value) => OwnedKeyAtom::Integer(*value),
            KeyAtom::Float(value) => OwnedKeyAtom::Float(*value),
            KeyAtom::Text(value) => OwnedKeyAtom::Text((*value).to_string()),
            KeyAtom::Instant(value) => OwnedKeyAtom::Instant(*value),
            KeyAtom::Reference(value) => OwnedKeyAtom::Reference(*value),
        }
    }

    fn same(&self, owned: &OwnedKeyAtom) -> bool {
        match (self, owned) {
            (KeyAtom::Null, OwnedKeyAtom::Null) => true,
            (KeyAtom::Boolean(a), OwnedKeyAtom::Boolean(b)) => a == b,
            (KeyAtom::Integer(a), OwnedKeyAtom::Integer(b)) => a == b,
            (KeyAtom::Float(a), OwnedKeyAtom::Float(b)) => a == b,
            (KeyAtom::Text(a), OwnedKeyAtom::Text(b)) => *a == b.as_str(),
            (KeyAtom::Instant(a), OwnedKeyAtom::Instant(b)) => a == b,
            (KeyAtom::Reference(a), OwnedKeyAtom::Reference(b)) => a == b,
            _ => false,
        }
    }
}

/// One group of a column-batch aggregation.
struct ColumnGroup {
    /// The key as first seen, which is what the result reports.
    key: Vec<Value>,
    atoms: Vec<OwnedKeyAtom>,
    accumulators: Vec<Accumulator>,
}

/// Work that can be shared by everything one statement evaluates.
///
/// A statement reads a consistent picture of the database: its own writes
/// are published only once it has computed them. So within one statement a
/// schema cannot change and a nested query with the same text has the same
/// answer, however many rows ask for it.
impl StatementScope {
    /// Hold a group's source rows for the rest of the statement and return
    /// the key they can be found under.
    fn keep_group_rows(rows: Vec<Document>) -> String {
        STATEMENT.with(|scope| {
            let mut scope = scope.borrow_mut();
            let key = format!("group-{}", scope.group_rows.len());
            scope.group_rows.insert(key.clone(), Arc::new(rows));
            key
        })
    }

    fn group_rows(key: &str) -> Option<Arc<Vec<Document>>> {
        STATEMENT.with(|scope| scope.borrow().group_rows.get(key).cloned())
    }
}

#[derive(Default)]
struct StatementScope {
    /// Nested statements in progress on this thread; the scope is live
    /// while this is non-zero.
    depth: usize,
    /// Results of nested queries, keyed by the query after the enclosing
    /// row's values were substituted into it.
    nested_results: HashMap<String, Value>,
    schemas: HashMap<String, Option<Arc<Schema>>>,
    schema_names: Option<Arc<Vec<String>>>,
    /// The source rows of each group produced by `group_by`.
    group_rows: HashMap<String, Arc<Vec<Document>>>,
    /// Documents reached through a reference.
    referenced: HashMap<ObjRef, Option<Arc<Document>>>,
    reverse_relationships: HashMap<(String, String), ReverseRelationship>,
    /// Per relationship and filter: how many referring documents each
    /// document has.
    related_counts: HashMap<Arc<str>, RelatedCounts>,
    /// The name each relationship question's tally goes under, by where
    /// the question's operations are.
    related_keys: HashMap<(usize, usize), (Vec<Operation>, String, String, Arc<str>)>,
    /// What a nested expression's operations read, by where they are.
    fields_read: HashMap<(usize, usize), (Vec<Operation>, Option<Vec<String>>)>,
    /// Answers of collection expressions that start by following a
    /// reference, by the document referred to and what was asked of it.
    through_reference: HashMap<(u64, Vec<u8>), Value>,
    /// Whether a nested expression's operations read anything as
    /// `name.field`, by where they are.
    names_rows: HashMap<(usize, usize), (Vec<Operation>, bool)>,
    /// For a collection and a name its rows may lack: whether the name is
    /// a relationship to it.
    relationship_names: HashMap<String, HashMap<String, bool>>,
    /// The rows of one collection that refer to one named document,
    /// arranged by the document they refer to through another field.
    also_referring: HashMap<String, Arc<HashMap<String, Vec<Arc<Document>>>>>,
}

/// The state of one relationship tally within a statement.
enum RelatedCounts {
    /// Rows that have asked so far, each answered through the index.
    Asked(usize),
    Ready(Arc<HashMap<ObjRef, Value>>),
    /// One pass cannot answer this; keep using the index.
    Unavailable,
}

thread_local! {
    static STATEMENT: std::cell::RefCell<StatementScope> =
        std::cell::RefCell::new(StatementScope::default());
}

/// Keeps the statement scope alive for the duration of one statement and
/// discards it afterwards, so nothing cached outlives the data it reflects.
struct StatementGuard;

impl StatementGuard {
    fn enter() -> Self {
        let outermost = STATEMENT.with(|scope| {
            let mut scope = scope.borrow_mut();
            scope.depth += 1;
            scope.depth == 1
        });
        if outermost {
            crate::storage::statement_begins();
        }
        Self
    }

    fn active() -> bool {
        STATEMENT.with(|scope| scope.borrow().depth > 0)
    }
}

impl Drop for StatementGuard {
    fn drop(&mut self) {
        STATEMENT.with(|scope| {
            let mut scope = scope.borrow_mut();
            scope.depth -= 1;
            if scope.depth == 0 {
                *scope = StatementScope::default();
            }
        });
    }
}

/// Everything a delete will do, decided before any of it is done.
#[derive(Default)]
struct DeletePlan {
    /// Documents being deleted, directly or by cascade.
    deleted: HashSet<(String, String)>,
    /// Documents deleted because something they refer to is being deleted.
    cascaded: Vec<Document>,
    /// Referring documents whose reference is cleared instead.
    nullified: HashMap<(String, String), Document>,
}

/// What an aggregate function ranges over.
#[derive(Debug, Clone)]
enum AggregateInput {
    /// `count()`: the rows themselves.
    AllRows,
    /// A stored path, `sum(.amount)`.
    Path(Vec<String>),
    /// A value computed per row, `sum(.price * .qty)`.
    Expression(Expression),
}

/// One row of a collection being assembled from a path or a value.
struct SourceRow {
    document: Document,
    /// True for a document held by a collection, which has a schema and an
    /// identity that other documents can reference. False for an object or
    /// scalar embedded in another document.
    stored: bool,
    /// The row itself when it is the store's own copy, in which case
    /// `document` is empty.
    lent: Option<Arc<Document>>,
}

impl SourceRow {
    fn stored(document: Document) -> Self {
        Self {
            document,
            stored: true,
            lent: None,
        }
    }

    fn embedded(document: Document) -> Self {
        Self {
            document,
            stored: false,
            lent: None,
        }
    }

    /// A stored row that the store still holds.
    fn lent(document: Arc<Document>) -> Self {
        Self {
            document: Document {
                id: String::new(),
                schema: String::new(),
                fields: crate::document::Fields::default(),
                created_at: document.created_at,
                updated_at: document.updated_at,
                version: document.version,
            },
            stored: true,
            lent: Some(document),
        }
    }

    fn row(&self) -> &Document {
        self.lent.as_deref().unwrap_or(&self.document)
    }

    fn into_document(self) -> Document {
        match self.lent {
            Some(lent) => lent.as_ref().clone(),
            None => self.document,
        }
    }
}

/// What a name means on a stored row that has no field of that name.
#[derive(Clone)]
enum ReverseRelationship {
    /// Documents of `source` refer to the row: through `field` when the
    /// relationship was named with `as`, or through any field when it is
    /// reached by the name of the referring collection.
    Declared {
        source: String,
        field: Option<String>,
    },
    /// The row's schema declares the field; this row just has no value.
    AbsentField,
    /// The row's collection has no schema, so nothing can be ruled out.
    Unknowable,
    /// Neither a field nor a referencing collection: a mistake in the query.
    Unknown,
}

/// Parameter bindings of the user function currently being evaluated.
#[derive(Default)]
struct CallFrame {
    /// Parameters bound to ordinary values.
    values: crate::document::Fields,
    /// Parameters bound to a whole row, so `p.team.name` keeps full path
    /// semantics, including reference traversal.
    rows: HashMap<String, Document>,
    /// Parameters bound to a collection of rows.
    collections: HashMap<String, Arc<Vec<Document>>>,
    /// The values of the function's local names that have been worked out.
    locals: HashMap<i64, Value>,
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

    fn collection(name: &str) -> Option<Arc<Vec<Document>>> {
        CALL_FRAMES.with(|frames| {
            frames
                .borrow()
                .last()
                .and_then(|frame| frame.collections.get(name).cloned())
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

    /// Whether a user function is being evaluated on this thread.
    fn any_active() -> bool {
        CALL_FRAMES.with(|frames| !frames.borrow().is_empty())
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
                let mut fields = crate::document::Fields::default();
                fields.insert("name".to_string(), Value::String("Alice".to_string()));
                fields.insert("age".to_string(), Value::Integer(30));
                fields.insert("active".to_string(), Value::Boolean(true));
                fields
            }),
            Document::with_id("user_2".to_string(), "users".to_string(), {
                let mut fields = crate::document::Fields::default();
                fields.insert("name".to_string(), Value::String("Bob".to_string()));
                fields.insert("age".to_string(), Value::Integer(25));
                fields.insert("active".to_string(), Value::Boolean(false));
                fields
            }),
            Document::with_id("user_3".to_string(), "users".to_string(), {
                let mut fields = crate::document::Fields::default();
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
                    crate::document::Fields::from_iter([
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
                    crate::document::Fields::from_iter([
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
                    crate::document::Fields::from_iter([
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
                crate::document::Fields::from_iter([
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
                    crate::document::Fields::from_iter([
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
                    crate::document::Fields::from_iter([
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
            crate::document::Fields::from_iter([
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
                    crate::document::Fields::from_iter([
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

        let mut o1 = crate::document::Fields::default();
        o1.insert("id".to_string(), Value::String("o1".to_string()));
        o1.insert("user_id".to_string(), Value::String("u1".to_string()));
        o1.insert("amount".to_string(), Value::Float(320.0));
        o1.insert("status".to_string(), Value::String("paid".to_string()));
        let mut o2 = crate::document::Fields::default();
        o2.insert("id".to_string(), Value::String("o2".to_string()));
        o2.insert("user_id".to_string(), Value::String("u1".to_string()));
        o2.insert("amount".to_string(), Value::Float(120.0));
        o2.insert("status".to_string(), Value::String("pending".to_string()));
        let mut o3 = crate::document::Fields::default();
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

        let mut u1 = crate::document::Fields::default();
        u1.insert("id".to_string(), Value::String("u1".to_string()));
        let mut u2 = crate::document::Fields::default();
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

        let mut f1 = crate::document::Fields::default();
        f1.insert("status".to_string(), Value::String("open".to_string()));
        let mut f2 = crate::document::Fields::default();
        f2.insert("status".to_string(), Value::String("open".to_string()));
        let mut f3 = crate::document::Fields::default();
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

        let mut c1 = crate::document::Fields::default();
        c1.insert("id".to_string(), Value::String("c1".to_string()));
        let mut c2 = crate::document::Fields::default();
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

        let mut f1 = crate::document::Fields::default();
        f1.insert("company_id".to_string(), Value::String("c1".to_string()));
        let mut f2 = crate::document::Fields::default();
        f2.insert("company_id".to_string(), Value::String("c1".to_string()));
        let mut f3 = crate::document::Fields::default();
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
                crate::document::Fields::from_iter([(String::from("id"), Value::String("c1".to_string()))]),
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
                crate::document::Fields::from_iter([(
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
