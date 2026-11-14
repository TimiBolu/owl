// Physical Query Planner
// Converts logical plan to physical execution plan with operator selection

use crate::error::{OQLError, OQLResult};
use crate::planner::logical::LogicalPlan;
use crate::planner::operators::*;
use crate::planner::types::{PlanNode, PlanType};
use crate::schema::SchemaRegistry;
use crate::storage::ScalarIndexDirection;
use crate::storage::{DocumentStore, ScalarIndexDefinition};

/// Physical query plan
#[derive(Debug, Clone)]
pub struct PhysicalPlan {
    /// Root physical operator
    pub root: PhysicalOperator,
    /// Schema registry for metadata
    pub schema_registry: Option<SchemaRegistry>,
}

/// Physical query planner
#[allow(dead_code)]
pub struct PhysicalPlanner {
    /// Schema registry for optimization decisions
    schema_registry: Option<SchemaRegistry>,
    /// Statistics for cost-based decisions
    statistics: Option<Statistics>,
}

/// Simple statistics for cost estimation
#[derive(Debug, Clone)]
pub struct Statistics {
    /// Estimated table sizes
    pub table_sizes: std::collections::HashMap<String, u64>,
    /// Index availability
    pub available_indexes: std::collections::HashMap<String, Vec<String>>,
    /// Authoritative storage-catalog definitions, including real names and
    /// composite field order.
    pub index_definitions: std::collections::HashMap<String, Vec<ScalarIndexDefinition>>,
    pub supports_storage_owned_scan: bool,
}

impl PhysicalPlanner {
    /// Create a new physical planner
    pub fn new() -> Self {
        Self {
            schema_registry: None,
            statistics: None,
        }
    }

    /// Create a planner with schema registry
    pub fn with_schema_registry(schema_registry: SchemaRegistry) -> Self {
        Self {
            schema_registry: Some(schema_registry),
            statistics: None,
        }
    }

    /// Create a planner with statistics
    pub fn with_statistics(mut self, statistics: Statistics) -> Self {
        self.statistics = Some(statistics);
        self
    }

    pub fn with_storage_catalog(mut self, storage: &dyn DocumentStore) -> OQLResult<Self> {
        let statistics = self.statistics.get_or_insert_with(Statistics::new);
        for definition in storage.list_scalar_indexes(None)? {
            statistics.add_index_definition(definition);
        }
        statistics.supports_storage_owned_scan = storage.supports_storage_owned_scan();
        Ok(self)
    }

    /// Plan against storage without offering it any index: filters and
    /// projection are still evaluated inside storage, but every row is
    /// visited. For a filter that keeps most rows this beats fetching them
    /// one by one through an index.
    pub fn with_storage_scan_only(mut self, storage: &dyn DocumentStore) -> Self {
        self.statistics
            .get_or_insert_with(Statistics::new)
            .supports_storage_owned_scan = storage.supports_storage_owned_scan();
        self
    }

    /// Convert a logical plan to a physical plan
    pub fn plan(&self, logical_plan: &LogicalPlan) -> OQLResult<PhysicalPlan> {
        let root = self.plan_node(&logical_plan.root)?;
        let mut root = self.eliminate_index_sorts(root);
        Self::annotate_read_pushdowns(&mut root, None, None, Vec::new());
        root = Self::eliminate_native_filter_rechecks(root);
        // Recompute projection pushdown after removing filters so fields used
        // only by the already-enforced predicate are not decoded.
        Self::annotate_read_pushdowns(&mut root, None, None, Vec::new());
        root = Self::eliminate_native_identity_projects(root);
        root = Self::eliminate_redundant_native_limits(root);
        if self
            .statistics
            .as_ref()
            .is_some_and(|statistics| statistics.supports_storage_owned_scan)
        {
            root = Self::fuse_storage_owned_scans(root, None, Vec::new());
        }

        Ok(PhysicalPlan {
            root,
            schema_registry: logical_plan.schema_registry.clone(),
        })
    }

    fn fuse_storage_owned_scans(
        operator: PhysicalOperator,
        required_fields: Option<std::collections::BTreeSet<String>>,
        filters: Vec<crate::planner::types::FilterCondition>,
    ) -> PhysicalOperator {
        use crate::parser::Expression;

        match operator {
            PhysicalOperator::Project(mut project) => {
                let projection_fields = project
                    .projections
                    .iter()
                    .map(|projection| match &projection.expression {
                        Expression::FieldAccess(field) if field.path.len() == 1 => {
                            Some(field.path[0].clone())
                        }
                        Expression::Literal(_) => None,
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let mut fields = std::collections::BTreeSet::new();
                for field in projection_fields.iter().flatten() {
                    fields.insert(field.clone());
                }
                project.input = Box::new(Self::fuse_storage_owned_scans(
                    *project.input,
                    Some(fields.clone()),
                    filters,
                ));
                let identity = project.projections.iter().all(|projection| {
                    projection.alias.is_none()
                        && matches!(
                            &projection.expression,
                            Expression::FieldAccess(field)
                                if field.path.len() == 1
                                    && crate::schema::DocumentSystemField::from_name(
                                        &field.path[0],
                                    )
                                    .is_none()
                        )
                });
                if identity
                    && matches!(
                        project.input.as_ref(),
                        PhysicalOperator::StorageScan(scan)
                            if scan.projection.as_ref().is_some_and(|projection| {
                                projection.iter().cloned().collect::<std::collections::BTreeSet<_>>()
                                    == fields
                            })
                    )
                {
                    *project.input
                } else {
                    PhysicalOperator::Project(project)
                }
            }
            PhysicalOperator::Sort(mut sort) => {
                let mut fields = required_fields.unwrap_or_default();
                for spec in &sort.sort_specs {
                    if let Expression::FieldAccess(field) = &spec.expression {
                        if field.path.len() == 1 {
                            fields.insert(field.path[0].clone());
                        }
                    }
                }
                sort.input = Box::new(Self::fuse_storage_owned_scans(
                    *sort.input,
                    Some(fields),
                    filters,
                ));
                PhysicalOperator::Sort(sort)
            }
            PhysicalOperator::Filter(mut filter) => {
                let mut pushed = filters;
                pushed.push(filter.condition.clone());
                filter.input = Box::new(Self::fuse_storage_owned_scans(
                    *filter.input,
                    required_fields,
                    pushed,
                ));
                if matches!(filter.input.as_ref(), PhysicalOperator::StorageScan(_)) {
                    *filter.input
                } else {
                    PhysicalOperator::Filter(filter)
                }
            }
            PhysicalOperator::Limit(mut limit) => {
                limit.input = Box::new(Self::fuse_storage_owned_scans(
                    *limit.input,
                    required_fields,
                    filters,
                ));
                PhysicalOperator::Limit(limit)
            }
            PhysicalOperator::Offset(mut offset) => {
                offset.input = Box::new(Self::fuse_storage_owned_scans(
                    *offset.input,
                    required_fields,
                    filters,
                ));
                PhysicalOperator::Offset(offset)
            }
            PhysicalOperator::SeqScan(scan) if !filters.is_empty() => {
                PhysicalOperator::StorageScan(StorageScanOperator {
                    schema: scan.schema,
                    filters,
                    projection: required_fields
                        .map(|fields| fields.into_iter().collect::<Vec<_>>()),
                    estimated_rows: scan.estimated_rows / 10,
                })
            }
            other => other,
        }
    }

    /// Convert a logical plan node to a physical operator
    fn plan_node(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        match node.plan_type {
            PlanType::Scan => self.plan_scan(node),
            PlanType::Filter => self.plan_filter(node),
            PlanType::Project => self.plan_project(node),
            PlanType::Sort => self.plan_sort(node),
            PlanType::Limit => self.plan_limit(node),
            PlanType::Offset => self.plan_offset(node),
            PlanType::GroupBy => self.plan_group_by(node),
            PlanType::Having => self.plan_having(node),
            PlanType::Aggregate => self.plan_aggregate(node),
            PlanType::Insert => self.plan_insert(node),
            PlanType::Update => self.plan_update(node),
            PlanType::Delete => self.plan_delete(node),
            PlanType::VectorNearest => self.plan_vector_nearest(node),
            PlanType::Join => self.plan_join(node),
        }
    }

    /// Plan a scan operation - choose between SeqScan and IndexScan
    fn plan_scan(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let schema = node
            .metadata
            .schema
            .clone()
            .ok_or_else(|| OQLError::Generic {
                message: "Scan node missing schema".to_string(),
            })?;

        // Check if we have an index available and a condition to use it
        if let Some(stats) = &self.statistics {
            if let Some(definitions) = stats.index_definitions.get(&schema) {
                if let Some(condition) = &node.metadata.condition {
                    if let Some(index_field) = self.extract_index_field(condition) {
                        if let Some(definition) = definitions
                            .iter()
                            .find(|definition| definition.fields.first() == Some(&index_field))
                        {
                            return Ok(PhysicalOperator::IndexScan(IndexScanOperator {
                                schema: schema.clone(),
                                index_name: definition.name.clone(),
                                key_conditions: vec![condition.clone()],
                                estimated_rows: self.estimate_table_size(&schema) / 10,
                                access: IndexAccessKind::NativeTyped,
                                key_fields: definition.fields.clone(),
                                prefilter_index: None,
                                pushed_limit: None,
                                pushed_projection: Vec::new(),
                                direction: ScalarIndexDirection::Ascending,
                            }));
                        }
                    }
                }
            }
            if let Some(indexes) = stats.available_indexes.get(&schema) {
                if !indexes.is_empty() {
                    // Check if we have a filter condition that can use an index
                    if let Some(condition) = &node.metadata.condition {
                        // Try to find an index that matches the condition
                        if let Some(index_field) = self.extract_index_field(condition) {
                            if indexes.contains(&index_field) {
                                // Use index scan
                                let index_name = format!("idx_{}_{}", schema, index_field);
                                let estimated_rows = self.estimate_table_size(&schema) / 10; // Assume 10% selectivity

                                return Ok(PhysicalOperator::IndexScan(IndexScanOperator {
                                    schema: schema.clone(),
                                    index_name,
                                    key_conditions: vec![condition.clone()],
                                    estimated_rows,
                                    access: IndexAccessKind::CompatibilityHashed,
                                    key_fields: vec![index_field],
                                    prefilter_index: None,
                                    pushed_limit: None,
                                    pushed_projection: Vec::new(),
                                    direction: ScalarIndexDirection::Ascending,
                                }));
                            }
                        }
                    }
                }
            }
        }

        // Default to sequential scan
        Ok(PhysicalOperator::SeqScan(SeqScanOperator {
            schema: schema.clone(),
            estimated_rows: self.estimate_table_size(&schema),
        }))
    }

    /// Extract the field name from a filter condition for index selection
    fn extract_index_field(
        &self,
        condition: &crate::planner::types::FilterCondition,
    ) -> Option<String> {
        use crate::parser::Expression;
        use crate::planner::types::FilterCondition;

        match condition {
            FilterCondition::Comparison {
                left,
                operator: _,
                right: _,
            } => {
                // Extract field from left side
                if let Expression::FieldAccess(field_access) = left {
                    if !field_access.path.is_empty() {
                        return Some(field_access.path.join("."));
                    }
                }
                None
            }
            FilterCondition::And(left, _right) => {
                // For AND conditions, try to use the first condition
                self.extract_index_field(left)
            }
            _ => None,
        }
    }

    fn select_native_index<'a>(
        &'a self,
        schema: &str,
        condition: &crate::planner::types::FilterCondition,
    ) -> Option<&'a ScalarIndexDefinition> {
        let definitions = self.statistics.as_ref()?.index_definitions.get(schema)?;

        definitions
            .iter()
            .enumerate()
            .filter_map(|(position, definition)| {
                let prefix = Self::usable_index_prefix_len(&definition.fields, condition);
                (prefix > 0).then_some((prefix, std::cmp::Reverse(position), definition))
            })
            .max_by_key(|(prefix, position, _)| (*prefix, *position))
            .map(|(_, _, definition)| definition)
    }

    /// Number of consecutive index components constrained by an equality
    /// prefix followed by at most one range component. This is the portion of
    /// a composite key that the native scalar scan can encode.
    fn usable_index_prefix_len(
        fields: &[String],
        condition: &crate::planner::types::FilterCondition,
    ) -> usize {
        use crate::parser::{ComparisonOperator, Expression};
        use crate::planner::types::FilterCondition;

        fn operators_for_field<'a>(
            condition: &'a FilterCondition,
            field_name: &str,
            output: &mut Vec<&'a ComparisonOperator>,
        ) {
            match condition {
                FilterCondition::Comparison {
                    left: Expression::FieldAccess(field),
                    operator,
                    right: Expression::Literal(_),
                } if field.path.len() == 1 && field.path[0] == field_name => {
                    output.push(operator);
                }
                FilterCondition::And(left, right) => {
                    operators_for_field(left, field_name, output);
                    operators_for_field(right, field_name, output);
                }
                _ => {}
            }
        }

        let mut prefix = 0;
        for field in fields {
            let mut operators = Vec::new();
            operators_for_field(condition, field, &mut operators);
            if operators
                .iter()
                .any(|operator| matches!(operator, ComparisonOperator::Equal))
            {
                prefix += 1;
                continue;
            }
            if operators.iter().any(|operator| {
                matches!(
                    operator,
                    ComparisonOperator::LessThan
                        | ComparisonOperator::LessEqual
                        | ComparisonOperator::GreaterThan
                        | ComparisonOperator::GreaterEqual
                )
            }) {
                prefix += 1;
            }
            break;
        }
        prefix
    }

    /// Plan a filter operation
    fn plan_filter(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let condition = node
            .metadata
            .condition
            .clone()
            .ok_or_else(|| OQLError::Generic {
                message: "Filter node missing condition".to_string(),
            })?;

        let mut input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Filter node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        // Logical OQL plans represent predicates as `Filter -> Scan`. Select
        // the access path here, while retaining this Filter as a residual
        // correctness check for compound predicates.
        if let PhysicalOperator::SeqScan(scan) = input.as_ref() {
            let mut branches = Vec::new();
            Self::collect_or_branches(&condition, &mut branches);
            let union_inputs = (branches.len() > 1)
                .then(|| {
                    branches
                        .iter()
                        .map(|branch| {
                            let definition = self.select_native_index(&scan.schema, branch)?;
                            Some(PhysicalOperator::IndexScan(IndexScanOperator {
                                schema: scan.schema.clone(),
                                index_name: definition.name.clone(),
                                key_conditions: vec![(*branch).clone()],
                                estimated_rows: self.estimate_table_size(&scan.schema) / 10,
                                access: IndexAccessKind::NativeTyped,
                                key_fields: definition.fields.clone(),
                                prefilter_index: None,
                                pushed_limit: None,
                                pushed_projection: Vec::new(),
                                direction: ScalarIndexDirection::Ascending,
                            }))
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .flatten();
            if let Some(inputs) = union_inputs {
                input = Box::new(PhysicalOperator::IndexUnion(IndexUnionOperator {
                    estimated_rows: inputs
                        .iter()
                        .map(PhysicalOperator::estimate_rows)
                        .sum::<u64>()
                        .min(self.estimate_table_size(&scan.schema)),
                    inputs,
                    order_fields: Vec::new(),
                    direction: ScalarIndexDirection::Ascending,
                    pushed_limit: None,
                }));
            } else if let Some(definition) = self.select_native_index(&scan.schema, &condition) {
                input = Box::new(PhysicalOperator::IndexScan(IndexScanOperator {
                    schema: scan.schema.clone(),
                    index_name: definition.name.clone(),
                    key_conditions: vec![condition.clone()],
                    estimated_rows: self.estimate_table_size(&scan.schema) / 10,
                    access: IndexAccessKind::NativeTyped,
                    key_fields: definition.fields.clone(),
                    prefilter_index: None,
                    pushed_limit: None,
                    pushed_projection: Vec::new(),
                    direction: ScalarIndexDirection::Ascending,
                }));
            }
        }

        Ok(PhysicalOperator::Filter(FilterOperator {
            condition,
            input,
            selectivity: 0.1, // Default selectivity estimate
        }))
    }

    fn collect_or_branches<'a>(
        condition: &'a crate::planner::types::FilterCondition,
        branches: &mut Vec<&'a crate::planner::types::FilterCondition>,
    ) {
        use crate::planner::types::FilterCondition;
        match condition {
            FilterCondition::Or(left, right) => {
                Self::collect_or_branches(left, branches);
                Self::collect_or_branches(right, branches);
            }
            branch => branches.push(branch),
        }
    }

    /// Plan a projection operation
    fn plan_project(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let projections = node.metadata.projections.clone();

        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Project node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::Project(ProjectOperator {
            projections,
            input,
        }))
    }

    /// Plan a sort operation
    fn plan_sort(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let sort_specs = node.metadata.sort_specs.clone();

        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Sort node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::Sort(SortOperator { sort_specs, input }))
    }

    /// Plan a limit operation
    fn plan_limit(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let limit = node.metadata.limit.ok_or_else(|| OQLError::Generic {
            message: "Limit node missing limit value".to_string(),
        })?;

        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Limit node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::Limit(LimitOperator { limit, input }))
    }

    fn plan_offset(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let offset = node.metadata.offset.ok_or_else(|| OQLError::Generic {
            message: "Offset node missing offset value".to_string(),
        })?;
        let input = node
            .children
            .first()
            .ok_or_else(|| OQLError::Generic {
                message: "Offset node missing input".to_string(),
            })
            .and_then(|child| self.plan_node(child))
            .map(Box::new)?;
        Ok(PhysicalOperator::Offset(OffsetOperator { offset, input }))
    }

    /// Plan a group by operation
    fn plan_group_by(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let group_by_fields = node.metadata.group_by_fields.clone();

        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "GroupBy node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::GroupBy(GroupByOperator {
            group_by_fields,
            input,
        }))
    }

    /// Plan a having operation
    fn plan_having(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let condition = node
            .metadata
            .condition
            .clone()
            .ok_or_else(|| OQLError::Generic {
                message: "Having node missing condition".to_string(),
            })?;

        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Having node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::Having(HavingOperator {
            condition,
            input,
        }))
    }

    /// Plan an aggregate operation
    fn plan_aggregate(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let aggregates = node.metadata.aggregates.clone();

        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Aggregate node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::Aggregate(AggregateOperator {
            aggregates,
            input,
        }))
    }

    /// Plan an insert operation
    fn plan_insert(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let schema = node
            .metadata
            .schema
            .clone()
            .ok_or_else(|| OQLError::Generic {
                message: "Insert node missing schema".to_string(),
            })?;

        let field_updates = node.metadata.field_updates.clone();

        Ok(PhysicalOperator::Insert(InsertOperator {
            schema,
            field_updates,
        }))
    }

    /// Plan an update operation
    fn plan_update(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let field_updates = node.metadata.field_updates.clone();

        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Update node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::Update(UpdateOperator {
            field_updates,
            input,
        }))
    }

    /// Plan a delete operation
    fn plan_delete(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Delete node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        Ok(PhysicalOperator::Delete(DeleteOperator { input }))
    }

    fn plan_vector_nearest(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "VectorNearest node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };

        let schema = node
            .children
            .first()
            .and_then(|c| c.metadata.schema.clone())
            .or_else(|| node.metadata.schema.clone())
            .ok_or_else(|| OQLError::Generic {
                message: "VectorNearest node missing schema".to_string(),
            })?;

        let field_path = node
            .metadata
            .vector_field
            .clone()
            .ok_or_else(|| OQLError::Generic {
                message: "VectorNearest node missing field path".to_string(),
            })?;
        let query = node
            .metadata
            .vector_query
            .clone()
            .ok_or_else(|| OQLError::Generic {
                message: "VectorNearest node missing query vector expression".to_string(),
            })?;
        let metric = node
            .metadata
            .vector_metric
            .ok_or_else(|| OQLError::Generic {
                message: "VectorNearest node missing metric".to_string(),
            })?;
        let k = node.metadata.vector_k.ok_or_else(|| OQLError::Generic {
            message: "VectorNearest node missing k".to_string(),
        })?;

        Ok(PhysicalOperator::VectorKnn(VectorKnnOperator {
            schema,
            field_path,
            query,
            metric,
            k,
            num_candidates: node.metadata.vector_num_candidates,
            exact: node.metadata.vector_exact,
            input,
        }))
    }

    /// Plan a join operation - choose between NestedLoopJoin and HashJoin
    fn plan_join(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        if node.children.len() < 2 {
            return Err(OQLError::Generic {
                message: "Join node requires two inputs".to_string(),
            });
        }

        let left = Box::new(self.plan_node(&node.children[0])?);
        let right = Box::new(self.plan_node(&node.children[1])?);

        // Estimate sizes for join algorithm selection
        let left_size = self.estimate_operator_size(&left);
        let right_size = self.estimate_operator_size(&right);

        // Use hash join if one side is significantly smaller (good for building hash table)
        // Otherwise use nested loop join
        if left_size < 1000 || right_size < 1000 {
            Ok(PhysicalOperator::HashJoin(HashJoinOperator {
                left,
                right,
                join_condition: node.metadata.condition.clone(),
            }))
        } else {
            Ok(PhysicalOperator::NestedLoopJoin(NestedLoopJoinOperator {
                left,
                right,
                join_condition: node.metadata.condition.clone(),
            }))
        }
    }

    /// Estimate table size from statistics
    fn estimate_table_size(&self, schema: &str) -> u64 {
        if let Some(stats) = &self.statistics {
            stats.table_sizes.get(schema).copied().unwrap_or(1000)
        } else {
            1000 // Default estimate
        }
    }

    /// Estimate the output size of an operator
    fn estimate_operator_size(&self, operator: &PhysicalOperator) -> u64 {
        match operator {
            PhysicalOperator::SeqScan(op) => op.estimated_rows,
            PhysicalOperator::IndexScan(op) => op.estimated_rows,
            PhysicalOperator::IndexUnion(op) => op.estimated_rows,
            PhysicalOperator::Filter(op) => {
                let input_size = self.estimate_operator_size(&op.input);
                (input_size as f64 * op.selectivity) as u64
            }
            PhysicalOperator::Limit(op) => {
                let input_size = self.estimate_operator_size(&op.input);
                input_size.min(op.limit)
            }
            _ => 1000, // Default estimate for other operators
        }
    }

    fn eliminate_index_sorts(&self, operator: PhysicalOperator) -> PhysicalOperator {
        match operator {
            PhysicalOperator::Sort(mut sort) => {
                let mut input = self.eliminate_index_sorts(*sort.input);
                if self.configure_ordered_index(&mut input, &sort.sort_specs, &mut Vec::new()) {
                    input
                } else {
                    sort.input = Box::new(input);
                    PhysicalOperator::Sort(sort)
                }
            }
            PhysicalOperator::Filter(mut filter) => {
                filter.input = Box::new(self.eliminate_index_sorts(*filter.input));
                PhysicalOperator::Filter(filter)
            }
            PhysicalOperator::Project(mut project) => {
                project.input = Box::new(self.eliminate_index_sorts(*project.input));
                PhysicalOperator::Project(project)
            }
            PhysicalOperator::Limit(mut limit) => {
                limit.input = Box::new(self.eliminate_index_sorts(*limit.input));
                PhysicalOperator::Limit(limit)
            }
            PhysicalOperator::IndexUnion(mut union) => {
                union.inputs = union
                    .inputs
                    .into_iter()
                    .map(|input| self.eliminate_index_sorts(input))
                    .collect();
                PhysicalOperator::IndexUnion(union)
            }
            other => other,
        }
    }

    fn eliminate_native_filter_rechecks(operator: PhysicalOperator) -> PhysicalOperator {
        match operator {
            PhysicalOperator::Filter(mut filter) => {
                filter.input = Box::new(Self::eliminate_native_filter_rechecks(*filter.input));
                if matches!(
                    filter.input.as_ref(),
                    PhysicalOperator::IndexScan(scan)
                        if scan.access == IndexAccessKind::NativeTyped
                            && scan.key_conditions.contains(&filter.condition)
                            && Self::condition_covered_by_index_fields(
                                &filter.condition,
                                &scan.key_fields,
                            )
                ) {
                    *filter.input
                } else if matches!(
                    filter.input.as_ref(),
                    PhysicalOperator::IndexUnion(union)
                        if Self::index_union_covers_condition(union, &filter.condition)
                ) {
                    *filter.input
                } else {
                    PhysicalOperator::Filter(filter)
                }
            }
            PhysicalOperator::Project(mut project) => {
                project.input = Box::new(Self::eliminate_native_filter_rechecks(*project.input));
                PhysicalOperator::Project(project)
            }
            PhysicalOperator::Sort(mut sort) => {
                sort.input = Box::new(Self::eliminate_native_filter_rechecks(*sort.input));
                PhysicalOperator::Sort(sort)
            }
            PhysicalOperator::Limit(mut limit) => {
                limit.input = Box::new(Self::eliminate_native_filter_rechecks(*limit.input));
                PhysicalOperator::Limit(limit)
            }
            PhysicalOperator::IndexUnion(mut union) => {
                union.inputs = union
                    .inputs
                    .into_iter()
                    .map(Self::eliminate_native_filter_rechecks)
                    .collect();
                PhysicalOperator::IndexUnion(union)
            }
            other => other,
        }
    }

    fn index_union_covers_condition(
        union: &IndexUnionOperator,
        condition: &crate::planner::types::FilterCondition,
    ) -> bool {
        let mut branches = Vec::new();
        Self::collect_or_branches(condition, &mut branches);
        branches.len() == union.inputs.len()
            && branches
                .iter()
                .zip(&union.inputs)
                .all(|(branch, input)| match input {
                    PhysicalOperator::IndexScan(scan) => {
                        scan.access == IndexAccessKind::NativeTyped
                            && scan.key_conditions.contains(branch)
                            && Self::condition_covered_by_index_fields(branch, &scan.key_fields)
                    }
                    _ => false,
                })
    }

    fn condition_covered_by_index_fields(
        condition: &crate::planner::types::FilterCondition,
        fields: &[String],
    ) -> bool {
        use crate::parser::Expression;
        use crate::planner::types::FilterCondition;

        match condition {
            FilterCondition::Comparison { left, right, .. } => {
                [left, right].iter().all(|expression| match expression {
                    Expression::FieldAccess(field) => {
                        field.path.len() == 1 && fields.contains(&field.path[0])
                    }
                    Expression::Literal(_) => true,
                    _ => false,
                })
            }
            FilterCondition::And(left, right) | FilterCondition::Or(left, right) => {
                Self::condition_covered_by_index_fields(left, fields)
                    && Self::condition_covered_by_index_fields(right, fields)
            }
            FilterCondition::Not(inner) => Self::condition_covered_by_index_fields(inner, fields),
        }
    }

    fn eliminate_native_identity_projects(operator: PhysicalOperator) -> PhysicalOperator {
        use crate::parser::Expression;

        match operator {
            PhysicalOperator::Project(mut project) => {
                project.input = Box::new(Self::eliminate_native_identity_projects(*project.input));
                let projected_fields = project
                    .projections
                    .iter()
                    .map(|projection| {
                        if projection.alias.is_some() {
                            return None;
                        }
                        match &projection.expression {
                            Expression::FieldAccess(field) if field.path.len() == 1 => {
                                Some(field.path[0].clone())
                            }
                            _ => None,
                        }
                    })
                    .collect::<Option<std::collections::BTreeSet<_>>>();
                let covered = match (project.input.as_ref(), projected_fields) {
                    (PhysicalOperator::IndexScan(scan), Some(fields))
                        if scan.access == IndexAccessKind::NativeTyped =>
                    {
                        scan.pushed_projection
                            .iter()
                            .cloned()
                            .collect::<std::collections::BTreeSet<_>>()
                            == fields
                    }
                    _ => false,
                };
                if covered {
                    *project.input
                } else {
                    PhysicalOperator::Project(project)
                }
            }
            PhysicalOperator::Filter(mut filter) => {
                filter.input = Box::new(Self::eliminate_native_identity_projects(*filter.input));
                PhysicalOperator::Filter(filter)
            }
            PhysicalOperator::Sort(mut sort) => {
                sort.input = Box::new(Self::eliminate_native_identity_projects(*sort.input));
                PhysicalOperator::Sort(sort)
            }
            PhysicalOperator::Limit(mut limit) => {
                limit.input = Box::new(Self::eliminate_native_identity_projects(*limit.input));
                PhysicalOperator::Limit(limit)
            }
            PhysicalOperator::IndexUnion(mut union) => {
                union.inputs = union
                    .inputs
                    .into_iter()
                    .map(Self::eliminate_native_identity_projects)
                    .collect();
                PhysicalOperator::IndexUnion(union)
            }
            other => other,
        }
    }

    fn eliminate_redundant_native_limits(operator: PhysicalOperator) -> PhysicalOperator {
        match operator {
            PhysicalOperator::Limit(mut limit) => {
                limit.input = Box::new(Self::eliminate_redundant_native_limits(*limit.input));
                if matches!(
                    limit.input.as_ref(),
                    PhysicalOperator::IndexScan(scan)
                        if scan.access == IndexAccessKind::NativeTyped
                            && scan.pushed_limit.is_some_and(|pushed| pushed <= limit.limit)
                ) || matches!(
                    limit.input.as_ref(),
                    PhysicalOperator::IndexUnion(union)
                        if union.pushed_limit.is_some_and(|pushed| pushed <= limit.limit)
                ) {
                    *limit.input
                } else {
                    PhysicalOperator::Limit(limit)
                }
            }
            PhysicalOperator::Filter(mut filter) => {
                filter.input = Box::new(Self::eliminate_redundant_native_limits(*filter.input));
                PhysicalOperator::Filter(filter)
            }
            PhysicalOperator::Project(mut project) => {
                project.input = Box::new(Self::eliminate_redundant_native_limits(*project.input));
                PhysicalOperator::Project(project)
            }
            PhysicalOperator::Sort(mut sort) => {
                sort.input = Box::new(Self::eliminate_redundant_native_limits(*sort.input));
                PhysicalOperator::Sort(sort)
            }
            PhysicalOperator::IndexUnion(mut union) => {
                union.inputs = union
                    .inputs
                    .into_iter()
                    .map(Self::eliminate_redundant_native_limits)
                    .collect();
                PhysicalOperator::IndexUnion(union)
            }
            other => other,
        }
    }

    fn configure_ordered_index(
        &self,
        operator: &mut PhysicalOperator,
        sort_specs: &[crate::planner::types::SortSpec],
        filters: &mut Vec<crate::planner::types::FilterCondition>,
    ) -> bool {
        match operator {
            PhysicalOperator::Filter(filter) => {
                filters.push(filter.condition.clone());
                self.configure_ordered_index(&mut filter.input, sort_specs, filters)
            }
            PhysicalOperator::IndexScan(scan) if scan.access == IndexAccessKind::NativeTyped => {
                use crate::parser::{Expression, SortDirection};
                if sort_specs.is_empty() {
                    return false;
                }
                let first_direction = &sort_specs[0].direction;
                if sort_specs
                    .iter()
                    .any(|spec| &spec.direction != first_direction)
                {
                    return false;
                }
                let sort_fields = sort_specs
                    .iter()
                    .map(|spec| match &spec.expression {
                        Expression::FieldAccess(field) if field.path.len() == 1 => {
                            Some(field.path[0].as_str())
                        }
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(sort_fields) = sort_fields else {
                    return false;
                };
                let mut conditions = scan.key_conditions.clone();
                for filter in filters.iter() {
                    if !conditions.contains(filter) {
                        conditions.push(filter.clone());
                    }
                }
                if let Some(definition) = self
                    .statistics
                    .as_ref()
                    .and_then(|stats| stats.index_definitions.get(&scan.schema))
                    .and_then(|definitions| {
                        definitions
                            .iter()
                            .filter_map(|definition| {
                                let equality_prefix =
                                    Self::equality_prefix_len(&definition.fields, &conditions);
                                let supports_sort = equality_prefix < definition.fields.len()
                                    && equality_prefix + sort_fields.len()
                                        <= definition.fields.len()
                                    && sort_fields
                                        .iter()
                                        .zip(&definition.fields[equality_prefix..])
                                        .all(|(sort, key)| *sort == key);
                                supports_sort.then_some((equality_prefix, definition))
                            })
                            .max_by_key(|(equality_prefix, _)| *equality_prefix)
                            .map(|(_, definition)| definition)
                    })
                {
                    if scan.index_name != definition.name {
                        let current_prefix = scan
                            .key_conditions
                            .first()
                            .map(|condition| {
                                Self::usable_index_prefix_len(&scan.key_fields, condition)
                            })
                            .unwrap_or_default();
                        let ordered_prefix = scan
                            .key_conditions
                            .first()
                            .map(|condition| {
                                Self::usable_index_prefix_len(&definition.fields, condition)
                            })
                            .unwrap_or_default();
                        if current_prefix > ordered_prefix {
                            scan.prefilter_index = Some(scan.index_name.clone());
                        }
                    }
                    scan.index_name = definition.name.clone();
                    scan.key_fields = definition.fields.clone();
                }
                let equality_prefix = Self::equality_prefix_len(&scan.key_fields, &conditions);
                if equality_prefix >= scan.key_fields.len()
                    || equality_prefix + sort_fields.len() > scan.key_fields.len()
                    || sort_fields
                        .iter()
                        .zip(&scan.key_fields[equality_prefix..])
                        .any(|(sort, key)| *sort != key)
                {
                    return false;
                }
                scan.direction = match first_direction {
                    SortDirection::Asc => ScalarIndexDirection::Ascending,
                    SortDirection::Desc => ScalarIndexDirection::Descending,
                };
                true
            }
            PhysicalOperator::IndexUnion(union) => {
                use crate::parser::Expression;
                let order_fields = sort_specs
                    .iter()
                    .map(|spec| match &spec.expression {
                        Expression::FieldAccess(field) if field.path.len() == 1 => {
                            Some(field.path[0].clone())
                        }
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(order_fields) = order_fields else {
                    return false;
                };
                if union.inputs.is_empty()
                    || !union.inputs.iter_mut().all(|input| {
                        self.configure_ordered_index(input, sort_specs, &mut Vec::new())
                    })
                {
                    return false;
                }
                let direction = match &union.inputs[0] {
                    PhysicalOperator::IndexScan(scan) => scan.direction,
                    _ => return false,
                };
                union.order_fields = order_fields;
                union.direction = direction;
                true
            }
            _ => false,
        }
    }

    fn equality_prefix_len(
        key_fields: &[String],
        conditions: &[crate::planner::types::FilterCondition],
    ) -> usize {
        use crate::parser::{ComparisonOperator, Expression};
        use crate::planner::types::FilterCondition;

        fn has_equality(condition: &FilterCondition, field_name: &str) -> bool {
            match condition {
                FilterCondition::Comparison {
                    left: Expression::FieldAccess(field),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(_),
                } => field.path.len() == 1 && field.path[0] == field_name,
                FilterCondition::And(left, right) => {
                    has_equality(left, field_name) || has_equality(right, field_name)
                }
                _ => false,
            }
        }

        key_fields
            .iter()
            .take_while(|field| {
                conditions
                    .iter()
                    .any(|condition| has_equality(condition, field))
            })
            .count()
    }

    fn annotate_read_pushdowns(
        operator: &mut PhysicalOperator,
        required_fields: Option<std::collections::BTreeSet<String>>,
        inherited_limit: Option<u64>,
        filters: Vec<crate::planner::types::FilterCondition>,
    ) {
        use crate::parser::Expression;

        match operator {
            PhysicalOperator::Limit(limit) => {
                let pushed = Some(
                    inherited_limit
                        .map(|current| current.min(limit.limit))
                        .unwrap_or(limit.limit),
                );
                Self::annotate_read_pushdowns(&mut limit.input, required_fields, pushed, filters);
            }
            PhysicalOperator::Project(project) => {
                let mut fields = std::collections::BTreeSet::new();
                for projection in &project.projections {
                    if let Expression::FieldAccess(field) = &projection.expression {
                        if field.path.len() == 1 {
                            fields.insert(field.path[0].clone());
                        } else {
                            return;
                        }
                    }
                }
                Self::annotate_read_pushdowns(
                    &mut project.input,
                    Some(fields),
                    inherited_limit,
                    filters,
                );
            }
            PhysicalOperator::Sort(sort) => {
                let mut fields = required_fields;
                if let Some(required) = &mut fields {
                    for spec in &sort.sort_specs {
                        if let Expression::FieldAccess(field) = &spec.expression {
                            if field.path.len() == 1 {
                                required.insert(field.path[0].clone());
                            }
                        }
                    }
                }
                // A limit above an explicit sort cannot be applied before the
                // sort unless index ordering is proven equivalent.
                Self::annotate_read_pushdowns(&mut sort.input, fields, None, filters);
            }
            PhysicalOperator::Filter(filter) => {
                let mut fields = required_fields;
                if let Some(required) = &mut fields {
                    Self::collect_condition_fields(&filter.condition, required);
                }
                let mut filters = filters;
                filters.push(filter.condition.clone());
                Self::annotate_read_pushdowns(&mut filter.input, fields, inherited_limit, filters);
            }
            PhysicalOperator::IndexScan(scan) if scan.access == IndexAccessKind::NativeTyped => {
                for filter in &filters {
                    if !scan.key_conditions.contains(filter) {
                        scan.key_conditions.push(filter.clone());
                    }
                }
                scan.pushed_projection = required_fields
                    .map(|fields| fields.into_iter().collect())
                    .unwrap_or_default();
                // Native index execution applies residual predicates while it
                // pages. The inherited limit is therefore an output row goal,
                // even when not every predicate is encoded in the key range.
                scan.pushed_limit = inherited_limit;
            }
            PhysicalOperator::IndexUnion(union) => {
                let mut fields = required_fields.unwrap_or_default();
                fields.extend(union.order_fields.iter().cloned());
                let pushed_limit = (!union.order_fields.is_empty())
                    .then_some(inherited_limit)
                    .flatten();
                union.pushed_limit = pushed_limit;
                for input in &mut union.inputs {
                    Self::annotate_read_pushdowns(
                        input,
                        Some(fields.clone()),
                        pushed_limit,
                        Vec::new(),
                    );
                }
            }
            _ => {}
        }
    }

    fn collect_condition_fields(
        condition: &crate::planner::types::FilterCondition,
        fields: &mut std::collections::BTreeSet<String>,
    ) {
        use crate::parser::Expression;
        use crate::planner::types::FilterCondition;
        match condition {
            FilterCondition::Comparison { left, right, .. } => {
                for expression in [left, right] {
                    if let Expression::FieldAccess(field) = expression {
                        if field.path.len() == 1 {
                            fields.insert(field.path[0].clone());
                        }
                    }
                }
            }
            FilterCondition::And(left, right) | FilterCondition::Or(left, right) => {
                Self::collect_condition_fields(left, fields);
                Self::collect_condition_fields(right, fields);
            }
            FilterCondition::Not(inner) => Self::collect_condition_fields(inner, fields),
        }
    }

    #[cfg(test)]
    fn conditions_fully_enforced(
        key_fields: &[String],
        filters: &[crate::planner::types::FilterCondition],
    ) -> bool {
        use crate::parser::{ComparisonOperator, Expression};
        use crate::planner::types::FilterCondition;

        let mut comparisons = Vec::new();
        fn collect<'a>(
            condition: &'a FilterCondition,
            output: &mut Vec<(&'a str, &'a ComparisonOperator)>,
        ) -> bool {
            match condition {
                FilterCondition::Comparison {
                    left: Expression::FieldAccess(field),
                    operator,
                    right: Expression::Literal(_),
                } if field.path.len() == 1
                    && matches!(
                        operator,
                        ComparisonOperator::Equal
                            | ComparisonOperator::LessThan
                            | ComparisonOperator::LessEqual
                            | ComparisonOperator::GreaterThan
                            | ComparisonOperator::GreaterEqual
                    ) =>
                {
                    output.push((&field.path[0], operator));
                    true
                }
                FilterCondition::And(left, right) => {
                    collect(left, output) && collect(right, output)
                }
                _ => false,
            }
        }
        if filters.is_empty()
            || key_fields.is_empty()
            || !filters
                .iter()
                .all(|condition| collect(condition, &mut comparisons))
        {
            return false;
        }
        if key_fields.len() == 1 {
            return comparisons.iter().all(|(field, _)| *field == key_fields[0]);
        }

        let mut enforceable = std::collections::HashSet::new();
        for (position, field) in key_fields.iter().enumerate() {
            let operators = comparisons
                .iter()
                .filter_map(|(candidate, operator)| (*candidate == field).then_some(*operator))
                .collect::<Vec<_>>();
            if operators.is_empty() {
                break;
            }
            enforceable.insert(field.as_str());
            if operators.len() == 1 && matches!(operators[0], ComparisonOperator::Equal) {
                continue;
            }
            if operators
                .iter()
                .all(|operator| !matches!(operator, ComparisonOperator::Equal))
            {
                // Native composite scans require an equality prefix and may
                // use ordered bounds only on the immediately following field.
                return position > 0
                    && comparisons
                        .iter()
                        .all(|(candidate, _)| enforceable.contains(*candidate));
            }
            // An equality plus another predicate on a composite-prefix field
            // is currently only partially encoded by IndexScanExec.
            return false;
        }
        comparisons
            .iter()
            .all(|(candidate, _)| enforceable.contains(*candidate))
    }
}

impl Default for PhysicalPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PhysicalPlan {
    /// Get the root operator
    pub fn root(&self) -> &PhysicalOperator {
        &self.root
    }

    /// Get the schema registry
    pub fn schema_registry(&self) -> Option<&SchemaRegistry> {
        self.schema_registry.as_ref()
    }

    /// Estimate the cost of executing this plan
    pub fn estimate_cost(&self) -> f64 {
        self.root.estimate_cost()
    }
}

impl Statistics {
    /// Create new statistics
    pub fn new() -> Self {
        Self {
            table_sizes: std::collections::HashMap::new(),
            available_indexes: std::collections::HashMap::new(),
            index_definitions: std::collections::HashMap::new(),
            supports_storage_owned_scan: false,
        }
    }

    /// Set table size
    pub fn set_table_size(&mut self, schema: String, size: u64) {
        self.table_sizes.insert(schema, size);
    }

    /// Add an index
    pub fn add_index(&mut self, schema: String, field: String) {
        self.available_indexes
            .entry(schema)
            .or_insert_with(Vec::new)
            .push(field);
    }

    pub fn add_index_definition(&mut self, definition: ScalarIndexDefinition) {
        let definitions = self
            .index_definitions
            .entry(definition.schema.clone())
            .or_default();
        if let Some(existing) = definitions
            .iter_mut()
            .find(|existing| existing.name == definition.name)
        {
            *existing = definition;
        } else {
            definitions.push(definition);
        }
    }
}

impl Default for Statistics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{ComparisonOperator, Expression, FieldAccess, Literal};
    use crate::planner::types::{FilterCondition, PlanNode, PlanType};

    #[test]
    fn test_plan_scan_uses_index_when_available_for_condition_field() {
        let scan = PlanNode::new(PlanType::Scan)
            .with_schema("users".to_string())
            .with_condition(FilterCondition::Comparison {
                left: Expression::FieldAccess(FieldAccess {
                    path: vec!["age".to_string()],
                }),
                operator: ComparisonOperator::GreaterEqual,
                right: Expression::Literal(Literal::Integer(18)),
            });
        let logical = LogicalPlan {
            root: scan,
            schema_registry: None,
        };

        let mut stats = Statistics::new();
        stats.set_table_size("users".to_string(), 1000);
        stats.add_index("users".to_string(), "age".to_string());
        let planner = PhysicalPlanner::new().with_statistics(stats);

        let physical = planner.plan(&logical).expect("plan");
        match physical.root {
            PhysicalOperator::IndexScan(op) => {
                assert_eq!(op.index_name, "idx_users_age");
            }
            other => panic!("expected IndexScan, got {:?}", other),
        }
    }

    #[test]
    fn storage_catalog_index_name_and_composite_order_drive_selection() {
        let scan = PlanNode::new(PlanType::Scan)
            .with_schema("events".to_string())
            .with_condition(FilterCondition::Comparison {
                left: Expression::FieldAccess(FieldAccess {
                    path: vec!["app".to_string()],
                }),
                operator: ComparisonOperator::Equal,
                right: Expression::Literal(Literal::String("api".to_string())),
            });
        let logical = LogicalPlan {
            root: scan,
            schema_registry: None,
        };
        let mut stats = Statistics::new();
        stats.set_table_size("events".to_string(), 10_000);
        stats.add_index_definition(ScalarIndexDefinition {
            name: "events_by_app_time".to_string(),
            schema: "events".to_string(),
            fields: vec!["app".to_string(), "occurred_at".to_string()],
            unique: false,
        });
        let plan = PhysicalPlanner::new()
            .with_statistics(stats)
            .plan(&logical)
            .unwrap();
        match plan.root {
            PhysicalOperator::IndexScan(index) => {
                assert_eq!(index.index_name, "events_by_app_time");
                assert_eq!(index.access, IndexAccessKind::NativeTyped);
            }
            other => panic!("expected catalog index scan, got {other:?}"),
        }
    }

    #[test]
    fn composite_index_selection_prefers_the_longest_constrained_prefix() {
        let comparison = |field: &str, operator, literal| FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec![field.to_string()],
            }),
            operator,
            right: Expression::Literal(literal),
        };
        let condition = FilterCondition::And(
            Box::new(FilterCondition::And(
                Box::new(comparison(
                    "application",
                    ComparisonOperator::Equal,
                    Literal::String("checkout-api".to_string()),
                )),
                Box::new(comparison(
                    "environment",
                    ComparisonOperator::Equal,
                    Literal::String("production".to_string()),
                )),
            )),
            Box::new(comparison(
                "level_rank",
                ComparisonOperator::GreaterEqual,
                Literal::Integer(50),
            )),
        );
        let root = PlanNode::new(PlanType::Filter)
            .with_child(PlanNode::new(PlanType::Scan).with_schema("events".to_string()))
            .with_condition(condition);
        let logical = LogicalPlan {
            root,
            schema_registry: None,
        };
        let mut stats = Statistics::new();
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
            stats.add_index_definition(ScalarIndexDefinition {
                name: name.to_string(),
                schema: "events".to_string(),
                fields: fields.into_iter().map(str::to_string).collect(),
                unique: false,
            });
        }

        let plan = PhysicalPlanner::new()
            .with_statistics(stats)
            .plan(&logical)
            .unwrap();
        let PhysicalOperator::IndexScan(index) = plan.root else {
            panic!("expected native index scan");
        };
        assert_eq!(index.index_name, "events_by_level_time");
    }

    #[test]
    fn test_plan_scan_falls_back_to_seq_scan_when_index_missing() {
        let scan = PlanNode::new(PlanType::Scan)
            .with_schema("users".to_string())
            .with_condition(FilterCondition::Comparison {
                left: Expression::FieldAccess(FieldAccess {
                    path: vec!["age".to_string()],
                }),
                operator: ComparisonOperator::Equal,
                right: Expression::Literal(Literal::Integer(21)),
            });
        let logical = LogicalPlan {
            root: scan,
            schema_registry: None,
        };

        let mut stats = Statistics::new();
        stats.set_table_size("users".to_string(), 1000);
        let planner = PhysicalPlanner::new().with_statistics(stats);

        let physical = planner.plan(&logical).expect("plan");
        assert!(matches!(physical.root, PhysicalOperator::SeqScan(_)));
    }

    #[test]
    fn ordinary_logical_filter_selects_native_catalog_index_and_eliminates_recheck() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["level".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("error".to_string())),
        };
        let root = PlanNode::new(PlanType::Filter)
            .with_child(PlanNode::new(PlanType::Scan).with_schema("events".to_string()))
            .with_condition(condition);
        let logical = LogicalPlan {
            root,
            schema_registry: None,
        };
        let mut stats = Statistics::new();
        stats.add_index_definition(ScalarIndexDefinition {
            name: "events_by_level".to_string(),
            schema: "events".to_string(),
            fields: vec!["level".to_string()],
            unique: false,
        });

        let plan = PhysicalPlanner::new()
            .with_statistics(stats)
            .plan(&logical)
            .unwrap();
        let PhysicalOperator::IndexScan(index) = plan.root else {
            panic!("expected native index scan");
        };
        assert_eq!(index.index_name, "events_by_level");
        assert_eq!(index.access, IndexAccessKind::NativeTyped);
    }

    #[test]
    fn limit_pushdown_requires_every_composite_predicate_to_be_encoded() {
        let comparison = |field: &str, operator| FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec![field.to_string()],
            }),
            operator,
            right: Expression::Literal(Literal::Integer(1)),
        };
        let keys = vec!["app".to_string(), "sequence".to_string()];
        let exact_prefix_and_range = FilterCondition::And(
            Box::new(comparison("app", ComparisonOperator::Equal)),
            Box::new(comparison("sequence", ComparisonOperator::GreaterEqual)),
        );
        assert!(PhysicalPlanner::conditions_fully_enforced(
            &keys,
            &[exact_prefix_and_range]
        ));

        let residual_field = FilterCondition::And(
            Box::new(comparison("app", ComparisonOperator::Equal)),
            Box::new(comparison("message", ComparisonOperator::Equal)),
        );
        assert!(!PhysicalPlanner::conditions_fully_enforced(
            &keys,
            &[residual_field]
        ));

        let partially_encoded_prefix = FilterCondition::And(
            Box::new(comparison("app", ComparisonOperator::Equal)),
            Box::new(comparison("app", ComparisonOperator::GreaterThan)),
        );
        assert!(!PhysicalPlanner::conditions_fully_enforced(
            &keys,
            &[partially_encoded_prefix]
        ));
    }
}
