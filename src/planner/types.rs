// Type definitions for query planning

use crate::parser::{ComparisonOperator, Expression, FieldAccess, SortDirection};
use crate::schema::FieldType;
use crate::storage::VectorMetricType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Type of plan node
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanType {
    /// Scan all documents in a collection
    Scan,
    /// Filter documents based on a condition
    Filter,
    /// Project specific fields
    Project,
    /// Sort documents
    Sort,
    /// Limit number of documents
    Limit,
    /// Skip a number of documents
    Offset,
    /// Group documents by fields
    GroupBy,
    /// Filter grouped documents
    Having,
    /// Aggregate values
    Aggregate,
    /// Insert new documents
    Insert,
    /// Update existing documents
    Update,
    /// Delete documents
    Delete,
    /// Vector nearest-neighbor search
    VectorNearest,
    /// Join two collections
    Join,
}

/// A node in the query plan tree
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanNode {
    /// Type of this plan node
    pub plan_type: PlanType,
    /// Child nodes (inputs to this operation)
    pub children: Vec<PlanNode>,
    /// Additional metadata for this node
    pub metadata: PlanMetadata,
}

/// Metadata for a plan node
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanMetadata {
    /// Schema/collection name
    pub schema: Option<String>,
    /// Fields involved in this operation
    pub fields: Vec<String>,
    /// Filter condition (for Filter nodes)
    pub condition: Option<FilterCondition>,
    /// Projection fields (for Project nodes)
    pub projections: Vec<ProjectionField>,
    /// Sort specifications (for Sort nodes)
    pub sort_specs: Vec<SortSpec>,
    /// Limit value (for Limit nodes)
    pub limit: Option<u64>,
    /// Rows to skip (for Offset nodes)
    pub offset: Option<u64>,
    /// Group by fields (for GroupBy nodes)
    pub group_by_fields: Vec<String>,
    /// Aggregate functions (for Aggregate nodes)
    pub aggregates: Vec<AggregateSpec>,
    /// Insert/Update fields
    pub field_updates: HashMap<String, Expression>,
    /// Estimated cardinality (number of rows)
    pub estimated_cardinality: Option<u64>,
    /// Estimated cost
    pub estimated_cost: Option<f64>,
    /// Vector search field path
    pub vector_field: Option<Vec<String>>,
    /// Vector query expression
    pub vector_query: Option<Expression>,
    /// Vector metric
    pub vector_metric: Option<VectorMetricType>,
    /// Requested top-k
    pub vector_k: Option<u64>,
    /// Optional ANN candidate count
    pub vector_num_candidates: Option<u64>,
    /// Force exact mode
    pub vector_exact: Option<bool>,
}

impl Default for PlanMetadata {
    fn default() -> Self {
        Self {
            schema: None,
            fields: Vec::new(),
            condition: None,
            projections: Vec::new(),
            sort_specs: Vec::new(),
            limit: None,
            offset: None,
            group_by_fields: Vec::new(),
            aggregates: Vec::new(),
            field_updates: HashMap::new(),
            estimated_cardinality: None,
            estimated_cost: None,
            vector_field: None,
            vector_query: None,
            vector_metric: None,
            vector_k: None,
            vector_num_candidates: None,
            vector_exact: None,
        }
    }
}

/// Filter condition in the plan
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilterCondition {
    /// Comparison between two expressions
    Comparison {
        left: Expression,
        operator: ComparisonOperator,
        right: Expression,
    },
    /// Logical AND
    And(Box<FilterCondition>, Box<FilterCondition>),
    /// Logical OR
    Or(Box<FilterCondition>, Box<FilterCondition>),
    /// Logical NOT
    Not(Box<FilterCondition>),
}

/// Projection field specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionField {
    /// Field expression
    pub expression: Expression,
    /// Alias for the field
    pub alias: Option<String>,
    /// Inferred type
    pub field_type: Option<FieldType>,
}

/// Sort specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SortSpec {
    /// Expression to sort by
    pub expression: Expression,
    /// Sort direction
    pub direction: SortDirection,
}

/// Aggregate function specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateSpec {
    /// Aggregate function name
    pub function: AggregateFunction,
    /// Field to aggregate
    pub field: Option<FieldAccess>,
    /// Alias for the result
    pub alias: Option<String>,
}

/// Aggregate function types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AggregateFunction {
    Exists,
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl PlanNode {
    /// Create a new plan node
    pub fn new(plan_type: PlanType) -> Self {
        Self {
            plan_type,
            children: Vec::new(),
            metadata: PlanMetadata::default(),
        }
    }

    /// Add a child node
    pub fn with_child(mut self, child: PlanNode) -> Self {
        self.children.push(child);
        self
    }

    /// Set the schema
    pub fn with_schema(mut self, schema: String) -> Self {
        self.metadata.schema = Some(schema);
        self
    }

    /// Set the condition
    pub fn with_condition(mut self, condition: FilterCondition) -> Self {
        self.metadata.condition = Some(condition);
        self
    }

    /// Set projections
    pub fn with_projections(mut self, projections: Vec<ProjectionField>) -> Self {
        self.metadata.projections = projections;
        self
    }

    /// Set sort specifications
    pub fn with_sort_specs(mut self, sort_specs: Vec<SortSpec>) -> Self {
        self.metadata.sort_specs = sort_specs;
        self
    }

    /// Set limit
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.metadata.limit = Some(limit);
        self
    }

    pub fn with_offset(mut self, offset: u64) -> Self {
        self.metadata.offset = Some(offset);
        self
    }

    /// Set group by fields
    pub fn with_group_by(mut self, fields: Vec<String>) -> Self {
        self.metadata.group_by_fields = fields;
        self
    }

    /// Set aggregates
    pub fn with_aggregates(mut self, aggregates: Vec<AggregateSpec>) -> Self {
        self.metadata.aggregates = aggregates;
        self
    }

    /// Set field updates
    pub fn with_field_updates(mut self, updates: HashMap<String, Expression>) -> Self {
        self.metadata.field_updates = updates;
        self
    }

    /// Set vector nearest parameters
    pub fn with_vector_nearest(
        mut self,
        field: Vec<String>,
        query: Expression,
        metric: VectorMetricType,
        k: u64,
        num_candidates: Option<u64>,
        exact: Option<bool>,
    ) -> Self {
        self.metadata.vector_field = Some(field);
        self.metadata.vector_query = Some(query);
        self.metadata.vector_metric = Some(metric);
        self.metadata.vector_k = Some(k);
        self.metadata.vector_num_candidates = num_candidates;
        self.metadata.vector_exact = exact;
        self
    }

    /// Get estimated cardinality
    pub fn cardinality(&self) -> Option<u64> {
        self.metadata.estimated_cardinality
    }

    /// Get estimated cost
    pub fn cost(&self) -> Option<f64> {
        self.metadata.estimated_cost
    }
}

impl FilterCondition {
    /// Convert from parser Condition
    pub fn from_parser_condition(condition: &crate::parser::Condition) -> Self {
        match condition {
            crate::parser::Condition::Comparison {
                left,
                operator,
                right,
            } => FilterCondition::Comparison {
                left: left.clone(),
                operator: operator.clone(),
                right: right.clone(),
            },
            crate::parser::Condition::And(left, right) => FilterCondition::And(
                Box::new(Self::from_parser_condition(left)),
                Box::new(Self::from_parser_condition(right)),
            ),
            crate::parser::Condition::Or(left, right) => FilterCondition::Or(
                Box::new(Self::from_parser_condition(left)),
                Box::new(Self::from_parser_condition(right)),
            ),
            crate::parser::Condition::Not(inner) => {
                FilterCondition::Not(Box::new(Self::from_parser_condition(inner)))
            }
        }
    }
}
