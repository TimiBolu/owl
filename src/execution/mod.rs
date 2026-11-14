// Physical Execution Module
// Iterator-based execution of physical operators

pub mod aggregate;
pub mod index_scan;
pub mod join;
pub mod operator;
pub mod parallel_aggregate;
pub mod parallel_scan;
pub mod profile;
pub mod scan;
pub mod thread_pool;

pub use aggregate::{AggregateExec, GroupByExec, HavingExec};
pub use index_scan::{IndexScanExec, IndexUnionExec};
pub use join::{HashJoinExec, NestedLoopJoinExec};
pub use operator::{DataChunk, PhysicalOperatorCollect, PhysicalOperatorExec, DEFAULT_CHUNK_ROWS};
pub use parallel_aggregate::ParallelAggregateExec;
pub use parallel_scan::{ParallelFilterExec, ParallelSeqScanExec};
pub use profile::{ExecutionProfile, OperatorExecutionStats};
pub use scan::{FilterExec, LimitExec, OffsetExec, ProjectExec, SeqScanExec, SortExec};
pub use thread_pool::ThreadPool;

use crate::error::{OQLError, OQLResult};
use crate::planner::operators::PhysicalOperator;
use crate::schema::DocumentSystemField;

/// Materialize the executable iterator tree for the read-only operators that
/// have production execution implementations. Mutation, aggregation, vector,
/// and join plans deliberately remain outside this boundary for now.
pub fn build_read_executor(
    operator: &PhysicalOperator,
) -> OQLResult<Box<dyn PhysicalOperatorExec>> {
    let projection = required_seq_scan_fields(operator);
    let filters = seq_scan_filters(operator);
    build_read_executor_node(operator, projection.as_deref(), &filters)
}

fn build_read_executor_node(
    operator: &PhysicalOperator,
    seq_scan_projection: Option<&[String]>,
    seq_scan_filters: &[crate::storage::ScanPredicate],
) -> OQLResult<Box<dyn PhysicalOperatorExec>> {
    match operator {
        PhysicalOperator::SeqScan(scan) => {
            let mut executor = SeqScanExec::new(scan.schema.clone());
            if let Some(fields) = seq_scan_projection {
                executor = executor.with_projection(fields.to_vec());
            }
            executor = executor.with_filters(seq_scan_filters.to_vec());
            Ok(Box::new(executor))
        }
        PhysicalOperator::StorageScan(scan) => {
            let filters = scan
                .filters
                .iter()
                .map(lower_scan_condition)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| OQLError::generic("storage scan contains an unsupported filter"))?;
            let mut executor = SeqScanExec::new(scan.schema.clone()).with_filters(filters);
            if let Some(projection) = &scan.projection {
                executor = executor.with_projection(projection.clone());
            }
            Ok(Box::new(executor))
        }
        PhysicalOperator::IndexScan(scan) => {
            let mut executor = IndexScanExec::new(
                scan.schema.clone(),
                scan.index_name.clone(),
                scan.key_conditions.clone(),
            );
            if let Some(limit) = scan.pushed_limit {
                executor = executor.with_limit(limit as usize);
            }
            if let Some(prefilter_index) = &scan.prefilter_index {
                executor = executor.with_prefilter_index(prefilter_index.clone());
            }
            if !scan.pushed_projection.is_empty() {
                executor = executor.with_projection(scan.pushed_projection.clone());
            }
            executor = executor.with_direction(scan.direction);
            Ok(Box::new(executor))
        }
        PhysicalOperator::IndexUnion(union) => {
            let inputs = union
                .inputs
                .iter()
                .map(|input| build_read_executor_node(input, seq_scan_projection, seq_scan_filters))
                .collect::<OQLResult<Vec<_>>>()?;
            Ok(Box::new(IndexUnionExec::new(
                inputs,
                union.order_fields.clone(),
                union.direction,
                union.pushed_limit.map(|limit| limit as usize),
            )))
        }
        PhysicalOperator::Filter(filter) => Ok(Box::new(FilterExec::new(
            filter.condition.clone(),
            build_read_executor_node(&filter.input, seq_scan_projection, seq_scan_filters)?,
        ))),
        PhysicalOperator::Project(project) => Ok(Box::new(ProjectExec::new(
            project.projections.clone(),
            build_read_executor_node(&project.input, seq_scan_projection, seq_scan_filters)?,
        ))),
        PhysicalOperator::Sort(sort) => Ok(Box::new(SortExec::new(
            sort.sort_specs.clone(),
            build_read_executor_node(&sort.input, seq_scan_projection, seq_scan_filters)?,
        ))),
        PhysicalOperator::Limit(limit) => Ok(Box::new(LimitExec::new(
            limit.limit,
            build_read_executor_node(&limit.input, seq_scan_projection, seq_scan_filters)?,
        ))),
        PhysicalOperator::Offset(offset) => Ok(Box::new(OffsetExec::new(
            offset.offset,
            build_read_executor_node(&offset.input, seq_scan_projection, seq_scan_filters)?,
        ))),
        other => Err(OQLError::generic(&format!(
            "physical operator is not supported by the read pipeline: {other:?}"
        ))),
    }
}

fn seq_scan_filters(operator: &PhysicalOperator) -> Vec<crate::storage::ScanPredicate> {
    use crate::storage::ScanPredicate;

    let mut filters = Vec::new();
    fn visit(operator: &PhysicalOperator, filters: &mut Vec<ScanPredicate>) {
        match operator {
            PhysicalOperator::Filter(filter) => {
                if let Some(predicate) = lower_scan_condition(&filter.condition) {
                    filters.push(predicate);
                }
                visit(&filter.input, filters);
            }
            PhysicalOperator::Project(project) => visit(&project.input, filters),
            PhysicalOperator::Sort(sort) => visit(&sort.input, filters),
            PhysicalOperator::Limit(limit) => visit(&limit.input, filters),
            PhysicalOperator::Offset(offset) => visit(&offset.input, filters),
            _ => {}
        }
    }
    visit(operator, &mut filters);
    filters
}

fn lower_scan_condition(
    condition: &crate::planner::types::FilterCondition,
) -> Option<crate::storage::ScanPredicate> {
    use crate::document::Value;
    use crate::parser::{Expression, Literal};
    use crate::planner::types::FilterCondition;
    use crate::storage::ScanPredicate;

    fn literal_value(literal: &Literal) -> Value {
        match literal {
            Literal::String(value) => Value::String(value.clone()),
            Literal::Integer(value) => Value::Integer(*value),
            Literal::Float(value) => Value::Float(*value),
            Literal::Boolean(value) => Value::Boolean(*value),
            Literal::Null => Value::Null,
            Literal::Timestamp(instant) => Value::DateTime(*instant),
        }
    }

    match condition {
        FilterCondition::Comparison {
            left: Expression::FieldAccess(field),
            operator,
            right: Expression::Literal(literal),
        } if field.path.len() == 1 => Some(ScanPredicate::Comparison {
            field: field.path[0].clone(),
            operator: operator.clone(),
            value: literal_value(literal),
        }),
        FilterCondition::And(left, right) => Some(ScanPredicate::And(
            Box::new(lower_scan_condition(left)?),
            Box::new(lower_scan_condition(right)?),
        )),
        FilterCondition::Or(left, right) => Some(ScanPredicate::Or(
            Box::new(lower_scan_condition(left)?),
            Box::new(lower_scan_condition(right)?),
        )),
        _ => None,
    }
}

fn required_seq_scan_fields(operator: &PhysicalOperator) -> Option<Vec<String>> {
    use crate::parser::Expression;
    use crate::planner::types::FilterCondition;
    use std::collections::BTreeSet;

    fn expression_field(expression: &Expression, fields: &mut BTreeSet<String>) {
        if let Expression::FieldAccess(field) = expression {
            if field.path.len() == 1 && DocumentSystemField::from_name(&field.path[0]).is_none() {
                fields.insert(field.path[0].clone());
            }
        }
    }

    fn condition_fields(condition: &FilterCondition, fields: &mut BTreeSet<String>) {
        match condition {
            FilterCondition::Comparison { left, right, .. } => {
                expression_field(left, fields);
                expression_field(right, fields);
            }
            FilterCondition::And(left, right) | FilterCondition::Or(left, right) => {
                condition_fields(left, fields);
                condition_fields(right, fields);
            }
            FilterCondition::Not(inner) => condition_fields(inner, fields),
        }
    }

    fn visit(
        operator: &PhysicalOperator,
        has_projection: &mut bool,
        fields: &mut BTreeSet<String>,
    ) {
        match operator {
            PhysicalOperator::Project(project) => {
                *has_projection = true;
                for projection in &project.projections {
                    expression_field(&projection.expression, fields);
                }
                visit(&project.input, has_projection, fields);
            }
            PhysicalOperator::Filter(filter) => {
                condition_fields(&filter.condition, fields);
                visit(&filter.input, has_projection, fields);
            }
            PhysicalOperator::Sort(sort) => {
                for spec in &sort.sort_specs {
                    expression_field(&spec.expression, fields);
                }
                visit(&sort.input, has_projection, fields);
            }
            PhysicalOperator::Limit(limit) => visit(&limit.input, has_projection, fields),
            PhysicalOperator::Offset(offset) => visit(&offset.input, has_projection, fields),
            _ => {}
        }
    }

    let mut has_projection = false;
    let mut fields = BTreeSet::new();
    visit(operator, &mut has_projection, &mut fields);
    has_projection.then(|| fields.into_iter().collect())
}

/// Build the same production read tree with opt-in runtime instrumentation.
/// Normal `execute_query` calls continue to use `build_read_executor` and pay
/// no counter or clock-reading overhead.
pub fn build_profiled_read_executor(
    operator: &PhysicalOperator,
) -> OQLResult<(Box<dyn PhysicalOperatorExec>, ExecutionProfile)> {
    let builder = profile::ProfileBuilder::new();
    let projection = required_seq_scan_fields(operator);
    let filters = seq_scan_filters(operator);
    let (executor, root) =
        build_profiled_node(operator, &builder, None, projection.as_deref(), &filters)?;
    Ok((executor, builder.finish(root)))
}

fn build_profiled_node(
    operator: &PhysicalOperator,
    builder: &profile::ProfileBuilder,
    parent: Option<usize>,
    seq_scan_projection: Option<&[String]>,
    seq_scan_filters: &[crate::storage::ScanPredicate],
) -> OQLResult<(Box<dyn PhysicalOperatorExec>, usize)> {
    let id = builder.register(
        operator.operator_type().to_string(),
        operator.estimate_rows(),
        parent,
    );
    let executor: Box<dyn PhysicalOperatorExec> = match operator {
        PhysicalOperator::SeqScan(scan) => {
            let mut executor = SeqScanExec::new(scan.schema.clone());
            if let Some(fields) = seq_scan_projection {
                executor = executor.with_projection(fields.to_vec());
            }
            executor = executor.with_filters(seq_scan_filters.to_vec());
            Box::new(executor)
        }
        PhysicalOperator::StorageScan(scan) => {
            let filters = scan
                .filters
                .iter()
                .map(lower_scan_condition)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| OQLError::generic("storage scan contains an unsupported filter"))?;
            let mut executor = SeqScanExec::new(scan.schema.clone()).with_filters(filters);
            if let Some(projection) = &scan.projection {
                executor = executor.with_projection(projection.clone());
            }
            Box::new(executor)
        }
        PhysicalOperator::IndexScan(scan) => {
            let mut executor = IndexScanExec::new(
                scan.schema.clone(),
                scan.index_name.clone(),
                scan.key_conditions.clone(),
            );
            if let Some(limit) = scan.pushed_limit {
                executor = executor.with_limit(limit as usize);
            }
            if let Some(prefilter_index) = &scan.prefilter_index {
                executor = executor.with_prefilter_index(prefilter_index.clone());
            }
            if !scan.pushed_projection.is_empty() {
                executor = executor.with_projection(scan.pushed_projection.clone());
            }
            executor = executor.with_direction(scan.direction);
            Box::new(executor)
        }
        PhysicalOperator::IndexUnion(union) => {
            let mut inputs = Vec::with_capacity(union.inputs.len());
            for input in &union.inputs {
                let (executor, _) = build_profiled_node(
                    input,
                    builder,
                    Some(id),
                    seq_scan_projection,
                    seq_scan_filters,
                )?;
                inputs.push(executor);
            }
            Box::new(IndexUnionExec::new(
                inputs,
                union.order_fields.clone(),
                union.direction,
                union.pushed_limit.map(|limit| limit as usize),
            ))
        }
        PhysicalOperator::Filter(filter) => {
            let (input, _) = build_profiled_node(
                &filter.input,
                builder,
                Some(id),
                seq_scan_projection,
                seq_scan_filters,
            )?;
            Box::new(FilterExec::new(filter.condition.clone(), input))
        }
        PhysicalOperator::Project(project) => {
            let (input, _) = build_profiled_node(
                &project.input,
                builder,
                Some(id),
                seq_scan_projection,
                seq_scan_filters,
            )?;
            Box::new(ProjectExec::new(project.projections.clone(), input))
        }
        PhysicalOperator::Sort(sort) => {
            let (input, _) = build_profiled_node(
                &sort.input,
                builder,
                Some(id),
                seq_scan_projection,
                seq_scan_filters,
            )?;
            Box::new(SortExec::new(sort.sort_specs.clone(), input))
        }
        PhysicalOperator::Limit(limit) => {
            let (input, _) = build_profiled_node(
                &limit.input,
                builder,
                Some(id),
                seq_scan_projection,
                seq_scan_filters,
            )?;
            Box::new(LimitExec::new(limit.limit, input))
        }
        PhysicalOperator::Offset(offset) => {
            let (input, _) = build_profiled_node(
                &offset.input,
                builder,
                Some(id),
                seq_scan_projection,
                seq_scan_filters,
            )?;
            Box::new(OffsetExec::new(offset.offset, input))
        }
        other => {
            return Err(OQLError::generic(&format!(
                "physical operator is not supported by the profiled read pipeline: {other:?}"
            )))
        }
    };
    Ok((builder.wrap(id, executor), id))
}
