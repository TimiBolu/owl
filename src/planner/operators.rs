// Physical Operators
// Defines the physical execution operators for query plans

use crate::parser::Expression;
use crate::planner::types::{AggregateSpec, FilterCondition, ProjectionField, SortSpec};
use crate::storage::VectorMetricType;
use std::collections::HashMap;

/// Physical operator types
#[derive(Debug, Clone)]
pub enum PhysicalOperator {
    /// Sequential scan of a collection
    SeqScan(SeqScanOperator),
    /// Index scan of a collection
    IndexScan(IndexScanOperator),
    /// Filter rows based on a condition
    Filter(FilterOperator),
    /// Project specific fields
    Project(ProjectOperator),
    /// Sort rows
    Sort(SortOperator),
    /// Limit number of rows
    Limit(LimitOperator),
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
    /// Estimate the cost of executing this operator
    pub fn estimate_cost(&self) -> f64 {
        match self {
            PhysicalOperator::SeqScan(op) => {
                // Cost is proportional to number of rows
                op.estimated_rows as f64
            }
            PhysicalOperator::IndexScan(op) => {
                // Index scan is cheaper than seq scan
                (op.estimated_rows as f64) * 0.1
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
            PhysicalOperator::IndexScan(op) => op.estimated_rows,
            PhysicalOperator::Filter(op) => {
                (op.input.estimate_rows() as f64 * op.selectivity) as u64
            }
            PhysicalOperator::Project(op) => op.input.estimate_rows(),
            PhysicalOperator::Sort(op) => op.input.estimate_rows(),
            PhysicalOperator::Limit(op) => op.limit.min(op.input.estimate_rows()),
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
            PhysicalOperator::IndexScan(_) => "IndexScan",
            PhysicalOperator::Filter(_) => "Filter",
            PhysicalOperator::Project(_) => "Project",
            PhysicalOperator::Sort(_) => "Sort",
            PhysicalOperator::Limit(_) => "Limit",
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
