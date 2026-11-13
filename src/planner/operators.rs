// Physical Operators
// Defines the physical execution operators for query plans

use crate::parser::Expression;
use crate::planner::types::{AggregateSpec, FilterCondition, ProjectionField, SortSpec};
use crate::storage::ScalarIndexDirection;
use crate::storage::VectorMetricType;
use std::collections::HashMap;

/// Physical operator types
#[derive(Debug, Clone)]
pub enum PhysicalOperator {
    /// Sequential scan of a collection
    SeqScan(SeqScanOperator),
    /// Storage-owned predicate evaluation and final-field materialization.
    StorageScan(StorageScanOperator),
    /// Index scan of a collection
    IndexScan(IndexScanOperator),
    /// Deduplicating union of independently indexable predicate branches.
    IndexUnion(IndexUnionOperator),
    /// Filter rows based on a condition
    Filter(FilterOperator),
    /// Project specific fields
    Project(ProjectOperator),
    /// Sort rows
    Sort(SortOperator),
    /// Limit number of rows
    Limit(LimitOperator),
    /// Skip rows
    Offset(OffsetOperator),
    /// Group rows by fields
    GroupBy(GroupByOperator),
    /// Filter grouped rows
    Having(HavingOperator),
    /// Aggregate values
    Aggregate(AggregateOperator),
    /// Insert new rows
    Insert(InsertOperator),
    /// Update existing rows
    Update(UpdateOperator),
    /// Delete rows
    Delete(DeleteOperator),
    /// Vector nearest-neighbor search
    VectorKnn(VectorKnnOperator),
    /// Nested loop join
    NestedLoopJoin(NestedLoopJoinOperator),
    /// Hash join
    HashJoin(HashJoinOperator),
}

/// Sequential scan operator
#[derive(Debug, Clone)]
pub struct SeqScanOperator {
    /// Schema/collection to scan
    pub schema: String,
    /// Estimated number of rows
    pub estimated_rows: u64,
}

#[derive(Debug, Clone)]
pub struct StorageScanOperator {
    pub schema: String,
    pub filters: Vec<FilterCondition>,
    /// `None` materializes complete matching documents; `Some([])` preserves
    /// only identity and system metadata.
    pub projection: Option<Vec<String>>,
    pub estimated_rows: u64,
}

/// Index scan operator
#[derive(Debug, Clone)]
pub struct IndexScanOperator {
    /// Schema/collection to scan
    pub schema: String,
    /// Index to use
    pub index_name: String,
    /// Index key conditions
    pub key_conditions: Vec<FilterCondition>,
    /// Estimated number of rows
    pub estimated_rows: u64,
    pub access: IndexAccessKind,
    /// Authoritative ordered key fields for native indexes.
    pub key_fields: Vec<String>,
    /// Optional selective index whose postings are intersected before records
    /// are decoded from the ordered access path.
    pub prefilter_index: Option<String>,
    /// Planner-proven safe row limit to apply inside the postings scan.
    pub pushed_limit: Option<u64>,
    /// Top-level fields required while decoding indexed records.
    pub pushed_projection: Vec<String>,
    /// Physical traversal direction through the ordered key space.
    pub direction: ScalarIndexDirection,
}

#[derive(Debug, Clone)]
pub struct IndexUnionOperator {
    pub inputs: Vec<PhysicalOperator>,
    pub estimated_rows: u64,
    /// Planner-proven common result order for every branch.
    pub order_fields: Vec<String>,
    pub direction: ScalarIndexDirection,
    /// Global row goal. Each ordered branch may safely receive the same goal.
    pub pushed_limit: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexAccessKind {
    NativeTyped,
    CompatibilityHashed,
}

/// Filter operator
#[derive(Debug, Clone)]
pub struct FilterOperator {
    /// Filter condition
    pub condition: FilterCondition,
    /// Input operator
    pub input: Box<PhysicalOperator>,
    /// Estimated selectivity (0.0 to 1.0)
    pub selectivity: f64,
}

/// Project operator
#[derive(Debug, Clone)]
pub struct ProjectOperator {
    /// Fields to project
    pub projections: Vec<ProjectionField>,
    /// Input operator
    pub input: Box<PhysicalOperator>,
}

/// Sort operator
#[derive(Debug, Clone)]
pub struct SortOperator {
    /// Sort specifications
    pub sort_specs: Vec<SortSpec>,
    /// Input operator
    pub input: Box<PhysicalOperator>,
}

/// Limit operator
#[derive(Debug, Clone)]
pub struct LimitOperator {
    /// Maximum number of rows
    pub limit: u64,
    /// Input operator
    pub input: Box<PhysicalOperator>,
}

#[derive(Debug, Clone)]
pub struct OffsetOperator {
    pub offset: u64,
    pub input: Box<PhysicalOperator>,
}

/// Group by operator
#[derive(Debug, Clone)]
pub struct GroupByOperator {
    /// Fields to group by
    pub group_by_fields: Vec<String>,
    /// Input operator
    pub input: Box<PhysicalOperator>,
}

/// Having operator
#[derive(Debug, Clone)]
pub struct HavingOperator {
    /// Filter condition for grouped rows
    pub condition: FilterCondition,
    /// Input operator
    pub input: Box<PhysicalOperator>,
}

/// Aggregate operator
#[derive(Debug, Clone)]
pub struct AggregateOperator {
    /// Aggregate specifications
    pub aggregates: Vec<AggregateSpec>,
    /// Input operator
    pub input: Box<PhysicalOperator>,
}

/// Insert operator
#[derive(Debug, Clone)]
pub struct InsertOperator {
    /// Schema to insert into
    pub schema: String,
    /// Field values to insert
    pub field_updates: HashMap<String, Expression>,
}

/// Update operator
#[derive(Debug, Clone)]
pub struct UpdateOperator {
    /// Field updates
    pub field_updates: HashMap<String, Expression>,
    /// Input operator (rows to update)
    pub input: Box<PhysicalOperator>,
}

/// Delete operator
#[derive(Debug, Clone)]
pub struct DeleteOperator {
    /// Input operator (rows to delete)
    pub input: Box<PhysicalOperator>,
}

/// Vector nearest-neighbor operator.
#[derive(Debug, Clone)]
pub struct VectorKnnOperator {
    pub schema: String,
    pub field_path: Vec<String>,
    pub query: Expression,
    pub metric: VectorMetricType,
    pub k: u64,
    pub num_candidates: Option<u64>,
    pub exact: Option<bool>,
    pub input: Box<PhysicalOperator>,
}

/// Nested loop join operator
#[derive(Debug, Clone)]
pub struct NestedLoopJoinOperator {
    /// Left input
    pub left: Box<PhysicalOperator>,
    /// Right input
    pub right: Box<PhysicalOperator>,
    /// Join condition
    pub join_condition: Option<FilterCondition>,
}

/// Hash join operator
#[derive(Debug, Clone)]
pub struct HashJoinOperator {
    /// Left input (build side)
    pub left: Box<PhysicalOperator>,
    /// Right input (probe side)
    pub right: Box<PhysicalOperator>,
    /// Join condition
    pub join_condition: Option<FilterCondition>,
}

impl PhysicalOperator {
    pub fn explain(&self) -> String {
        let mut output = String::new();
        self.explain_into(0, &mut output);
        output
    }

    fn explain_into(&self, depth: usize, output: &mut String) {
        use std::fmt::Write;
        let indent = "  ".repeat(depth);
        match self {
            PhysicalOperator::SeqScan(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}SeqScan collection={} estimated_rows={}",
                    operator.schema, operator.estimated_rows
                );
            }
            PhysicalOperator::StorageScan(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}StorageScan collection={} filters={} projection={:?} estimated_rows={}",
                    operator.schema,
                    operator.filters.len(),
                    operator.projection,
                    operator.estimated_rows
                );
            }
            PhysicalOperator::IndexScan(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}IndexScan collection={} index={} access={:?} direction={:?} key_fields={:?} prefilter_index={:?} pushed_limit={:?} pushed_projection={:?} estimated_rows={} conditions={:?}",
                    operator.schema,
                    operator.index_name,
                    operator.access,
                    operator.direction,
                    operator.key_fields,
                    operator.prefilter_index,
                    operator.pushed_limit,
                    operator.pushed_projection,
                    operator.estimated_rows,
                    operator.key_conditions
                );
            }
            PhysicalOperator::IndexUnion(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}IndexUnion branches={} order_fields={:?} direction={:?} pushed_limit={:?} estimated_rows={}",
                    operator.inputs.len(),
                    operator.order_fields,
                    operator.direction,
                    operator.pushed_limit,
                    operator.estimated_rows
                );
                for input in &operator.inputs {
                    input.explain_into(depth + 1, output);
                }
            }
            PhysicalOperator::Filter(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}Filter selectivity={}",
                    operator.selectivity
                );
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Project(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}Project fields={}",
                    operator.projections.len()
                );
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Sort(operator) => {
                let _ = writeln!(output, "{indent}Sort keys={}", operator.sort_specs.len());
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Limit(operator) => {
                let _ = writeln!(output, "{indent}Limit rows={}", operator.limit);
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Offset(operator) => {
                let _ = writeln!(output, "{indent}Offset rows={}", operator.offset);
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::GroupBy(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}GroupBy fields={}",
                    operator.group_by_fields.join(",")
                );
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Having(operator) => {
                let _ = writeln!(output, "{indent}Having");
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Aggregate(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}Aggregate expressions={}",
                    operator.aggregates.len()
                );
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Insert(operator) => {
                let _ = writeln!(output, "{indent}Insert collection={}", operator.schema);
            }
            PhysicalOperator::Update(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}Update fields={}",
                    operator.field_updates.len()
                );
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::Delete(operator) => {
                let _ = writeln!(output, "{indent}Delete");
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::VectorKnn(operator) => {
                let _ = writeln!(
                    output,
                    "{indent}VectorKnn collection={} field={} k={} candidates={:?}",
                    operator.schema,
                    operator.field_path.join("."),
                    operator.k,
                    operator.num_candidates
                );
                operator.input.explain_into(depth + 1, output);
            }
            PhysicalOperator::NestedLoopJoin(operator) => {
                let _ = writeln!(output, "{indent}NestedLoopJoin");
                operator.left.explain_into(depth + 1, output);
                operator.right.explain_into(depth + 1, output);
            }
            PhysicalOperator::HashJoin(operator) => {
                let _ = writeln!(output, "{indent}HashJoin");
                operator.left.explain_into(depth + 1, output);
                operator.right.explain_into(depth + 1, output);
            }
        }
    }

    /// Estimate the cost of executing this operator
    pub fn estimate_cost(&self) -> f64 {
        match self {
            PhysicalOperator::SeqScan(op) => {
                // Cost is proportional to number of rows
                op.estimated_rows as f64
            }
            PhysicalOperator::StorageScan(op) => op.estimated_rows as f64 * 0.5,
            PhysicalOperator::IndexScan(op) => {
                // Index scan is cheaper than seq scan
                (op.estimated_rows as f64) * 0.1
            }
            PhysicalOperator::IndexUnion(op) => {
                op.inputs
                    .iter()
                    .map(PhysicalOperator::estimate_cost)
                    .sum::<f64>()
                    + op.estimated_rows as f64 * 0.01
            }
            PhysicalOperator::Filter(op) => {
                // Cost of input plus filtering cost
                op.input.estimate_cost() + (op.input.estimate_rows() as f64 * 0.01)
            }
            PhysicalOperator::Project(op) => {
                // Cost of input plus projection cost
                op.input.estimate_cost() + (op.input.estimate_rows() as f64 * 0.01)
            }
            PhysicalOperator::Sort(op) => {
                // Sorting is expensive: O(n log n)
                let n = op.input.estimate_rows() as f64;
                op.input.estimate_cost() + (n * n.log2())
            }
            PhysicalOperator::Limit(op) => {
                // Limit is cheap, just stops early
                op.input.estimate_cost() * 0.1
            }
            PhysicalOperator::Offset(op) => {
                op.input.estimate_cost() + (op.offset.min(op.input.estimate_rows()) as f64 * 0.001)
            }
            PhysicalOperator::GroupBy(op) => {
                // Grouping requires hash table
                let n = op.input.estimate_rows() as f64;
                op.input.estimate_cost() + (n * 2.0)
            }
            PhysicalOperator::Having(op) => {
                // Similar to filter
                op.input.estimate_cost() + (op.input.estimate_rows() as f64 * 0.01)
            }
            PhysicalOperator::Aggregate(op) => {
                // Aggregation requires scanning all rows
                op.input.estimate_cost() + (op.input.estimate_rows() as f64)
            }
            PhysicalOperator::Insert(_) => {
                // Insert is relatively cheap
                10.0
            }
            PhysicalOperator::Update(op) => {
                // Cost of finding rows plus update cost
                op.input.estimate_cost() + (op.input.estimate_rows() as f64 * 2.0)
            }
            PhysicalOperator::Delete(op) => {
                // Cost of finding rows plus delete cost
                op.input.estimate_cost() + (op.input.estimate_rows() as f64 * 1.5)
            }
            PhysicalOperator::VectorKnn(op) => {
                let n = op.input.estimate_rows() as f64;
                let candidate_factor = if op.exact.unwrap_or(false) {
                    1.0
                } else {
                    op.num_candidates
                        .map(|c| (c as f64 / n.max(1.0)).clamp(0.05, 1.0))
                        .unwrap_or(0.25)
                };
                op.input.estimate_cost() + (n * candidate_factor)
            }
            PhysicalOperator::NestedLoopJoin(op) => {
                // Nested loop is expensive: O(n * m)
                let left_rows = op.left.estimate_rows() as f64;
                let right_rows = op.right.estimate_rows() as f64;
                op.left.estimate_cost() + op.right.estimate_cost() + (left_rows * right_rows)
            }
            PhysicalOperator::HashJoin(op) => {
                // Hash join is cheaper: O(n + m)
                let left_rows = op.left.estimate_rows() as f64;
                let right_rows = op.right.estimate_rows() as f64;
                op.left.estimate_cost() + op.right.estimate_cost() + (left_rows + right_rows) * 2.0
            }
        }
    }

    /// Estimate the number of rows this operator will produce
    pub fn estimate_rows(&self) -> u64 {
        match self {
            PhysicalOperator::SeqScan(op) => op.estimated_rows,
            PhysicalOperator::StorageScan(op) => op.estimated_rows,
            PhysicalOperator::IndexScan(op) => op.estimated_rows,
            PhysicalOperator::IndexUnion(op) => op.estimated_rows,
            PhysicalOperator::Filter(op) => {
                (op.input.estimate_rows() as f64 * op.selectivity) as u64
            }
            PhysicalOperator::Project(op) => op.input.estimate_rows(),
            PhysicalOperator::Sort(op) => op.input.estimate_rows(),
            PhysicalOperator::Limit(op) => op.limit.min(op.input.estimate_rows()),
            PhysicalOperator::Offset(op) => op.input.estimate_rows().saturating_sub(op.offset),
            PhysicalOperator::GroupBy(op) => {
                // Grouping typically reduces rows
                op.input.estimate_rows() / 10
            }
            PhysicalOperator::Having(op) => {
                // Having filters grouped rows
                op.input.estimate_rows() / 2
            }
            PhysicalOperator::Aggregate(_) => 1, // Aggregates produce single row
            PhysicalOperator::Insert(_) => 1,
            PhysicalOperator::Update(op) => op.input.estimate_rows(),
            PhysicalOperator::Delete(op) => op.input.estimate_rows(),
            PhysicalOperator::VectorKnn(op) => op.k.min(op.input.estimate_rows()),
            PhysicalOperator::NestedLoopJoin(op) => {
                // Join can produce many rows
                op.left.estimate_rows() * op.right.estimate_rows() / 10
            }
            PhysicalOperator::HashJoin(op) => {
                // Hash join similar to nested loop
                op.left.estimate_rows() * op.right.estimate_rows() / 10
            }
        }
    }

    /// Get the operator type as a string
    pub fn operator_type(&self) -> &str {
        match self {
            PhysicalOperator::SeqScan(_) => "SeqScan",
            PhysicalOperator::StorageScan(_) => "StorageScan",
            PhysicalOperator::IndexScan(_) => "IndexScan",
            PhysicalOperator::IndexUnion(_) => "IndexUnion",
            PhysicalOperator::Filter(_) => "Filter",
            PhysicalOperator::Project(_) => "Project",
            PhysicalOperator::Sort(_) => "Sort",
            PhysicalOperator::Limit(_) => "Limit",
            PhysicalOperator::Offset(_) => "Offset",
            PhysicalOperator::GroupBy(_) => "GroupBy",
            PhysicalOperator::Having(_) => "Having",
            PhysicalOperator::Aggregate(_) => "Aggregate",
            PhysicalOperator::Insert(_) => "Insert",
            PhysicalOperator::Update(_) => "Update",
            PhysicalOperator::Delete(_) => "Delete",
            PhysicalOperator::VectorKnn(_) => "VectorKnn",
            PhysicalOperator::NestedLoopJoin(_) => "NestedLoopJoin",
            PhysicalOperator::HashJoin(_) => "HashJoin",
        }
    }
}
