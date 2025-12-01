// Physical Query Planner
// Converts logical plan to physical execution plan with operator selection

use crate::error::{OQLError, OQLResult};
use crate::planner::logical::LogicalPlan;
use crate::planner::operators::*;
use crate::planner::types::{PlanNode, PlanType};
use crate::schema::SchemaRegistry;

/// Physical query plan
#[derive(Debug, Clone)]
pub struct PhysicalPlan {
    /// Root physical operator
    pub root: PhysicalOperator,
    /// Schema registry for metadata
    pub schema_registry: Option<SchemaRegistry>,
}

/// Physical query planner
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
    
    /// Convert a logical plan to a physical plan
    pub fn plan(&self, logical_plan: &LogicalPlan) -> OQLResult<PhysicalPlan> {
        let root = self.plan_node(&logical_plan.root)?;
        
        Ok(PhysicalPlan {
            root,
            schema_registry: logical_plan.schema_registry.clone(),
        })
    }
    
    /// Convert a logical plan node to a physical operator
    fn plan_node(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        match node.plan_type {
            PlanType::Scan => self.plan_scan(node),
            PlanType::Filter => self.plan_filter(node),
            PlanType::Project => self.plan_project(node),
            PlanType::Sort => self.plan_sort(node),
            PlanType::Limit => self.plan_limit(node),
            PlanType::GroupBy => self.plan_group_by(node),
            PlanType::Having => self.plan_having(node),
            PlanType::Aggregate => self.plan_aggregate(node),
            PlanType::Insert => self.plan_insert(node),
            PlanType::Update => self.plan_update(node),
            PlanType::Delete => self.plan_delete(node),
            PlanType::Join => self.plan_join(node),
        }
    }
    
    /// Plan a scan operation - choose between SeqScan and IndexScan
    fn plan_scan(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let schema = node.metadata.schema.clone()
            .ok_or_else(|| OQLError::Generic {
                message: "Scan node missing schema".to_string(),
            })?;
        
        // Check if we have an index available and a condition to use it
        if let Some(stats) = &self.statistics {
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
    fn extract_index_field(&self, condition: &crate::planner::types::FilterCondition) -> Option<String> {
        use crate::planner::types::FilterCondition;
        use crate::parser::Expression;
        
        match condition {
            FilterCondition::Comparison { left, operator: _, right: _ } => {
                // Extract field from left side
                if let Expression::FieldAccess(field_access) = left {
                    if !field_access.path.is_empty() {
                        return Some(field_access.path[0].clone());
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
    
    /// Plan a filter operation
    fn plan_filter(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let condition = node.metadata.condition.clone()
            .ok_or_else(|| OQLError::Generic {
                message: "Filter node missing condition".to_string(),
            })?;
        
        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Filter node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };
        
        Ok(PhysicalOperator::Filter(FilterOperator {
            condition,
            input,
            selectivity: 0.1, // Default selectivity estimate
        }))
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
        
        Ok(PhysicalOperator::Sort(SortOperator {
            sort_specs,
            input,
        }))
    }
    
    /// Plan a limit operation
    fn plan_limit(&self, node: &PlanNode) -> OQLResult<PhysicalOperator> {
        let limit = node.metadata.limit
            .ok_or_else(|| OQLError::Generic {
                message: "Limit node missing limit value".to_string(),
            })?;
        
        let input = if node.children.is_empty() {
            return Err(OQLError::Generic {
                message: "Limit node missing input".to_string(),
            });
        } else {
            Box::new(self.plan_node(&node.children[0])?)
        };
        
        Ok(PhysicalOperator::Limit(LimitOperator {
            limit,
            input,
        }))
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
        let condition = node.metadata.condition.clone()
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
        let schema = node.metadata.schema.clone()
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
        
        Ok(PhysicalOperator::Delete(DeleteOperator {
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
}

impl Default for Statistics {
    fn default() -> Self {
        Self::new()
    }
}
