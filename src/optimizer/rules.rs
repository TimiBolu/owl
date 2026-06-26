// Optimization Rules for Query Optimization
//
// This module provides optimization rules that transform query plans
// to improve performance. Rules include:
// - Predicate pushdown (move filters closer to scans)
// - Join reordering (choose optimal join order)
// - Index selection (use indexes when beneficial)
// - Projection pushdown (eliminate unnecessary columns early)

use crate::error::OQLResult;
use crate::optimizer::cost_model::CostModel;
use crate::optimizer::statistics::StatisticsManager;
use crate::planner::logical::LogicalPlan;
use crate::planner::operators::PhysicalOperator;
use crate::planner::physical::PhysicalPlan;
use crate::planner::types::{PlanNode, PlanType};

/// Query optimizer that applies optimization rules
pub struct QueryOptimizer {
    /// Cost model for cost-based decisions
    cost_model: CostModel,
    /// Statistics for cardinality estimation
    stats: Option<StatisticsManager>,
    /// Optimization level (0 = none, 1 = basic, 2 = aggressive)
    optimization_level: u8,
}

impl QueryOptimizer {
    /// Create a new query optimizer
    pub fn new() -> Self {
        Self {
            cost_model: CostModel::new(),
            stats: None,
            optimization_level: 2, // Aggressive by default
        }
    }

    /// Create an optimizer with statistics
    pub fn with_statistics(stats: StatisticsManager) -> Self {
        Self {
            cost_model: CostModel::with_statistics(stats.clone()),
            stats: Some(stats),
            optimization_level: 2,
        }
    }

    /// Set optimization level
    pub fn with_optimization_level(mut self, level: u8) -> Self {
        self.optimization_level = level;
        self
    }

    /// Optimize a logical plan
    pub fn optimize_logical(&self, plan: &LogicalPlan) -> OQLResult<LogicalPlan> {
        if self.optimization_level == 0 {
            return Ok(plan.clone());
        }

        let mut optimized = plan.clone();

        // Apply logical optimization rules
        optimized.root = self.apply_predicate_pushdown(&optimized.root)?;
        optimized.root = self.apply_projection_pushdown(&optimized.root)?;

        if self.optimization_level >= 2 {
            optimized.root = self.apply_join_reordering(&optimized.root)?;
        }

        Ok(optimized)
    }

    /// Optimize a physical plan
    pub fn optimize_physical(&self, plan: &PhysicalPlan) -> OQLResult<PhysicalPlan> {
        if self.optimization_level == 0 {
            return Ok(plan.clone());
        }

        let mut optimized = plan.clone();

        // Apply physical optimization rules
        optimized.root = self.apply_index_selection(&optimized.root)?;
        optimized.root = self.apply_join_algorithm_selection(&optimized.root)?;

        Ok(optimized)
    }

    /// Apply predicate pushdown optimization
    /// Move filters as close to scans as possible to reduce data early
    fn apply_predicate_pushdown(&self, node: &PlanNode) -> OQLResult<PlanNode> {
        match node.plan_type {
            PlanType::Filter => {
                // Check if we can push this filter down
                if !node.children.is_empty() {
                    let child = &node.children[0];

                    match child.plan_type {
                        PlanType::Scan => {
                            // Filter directly on scan - already optimal
                            Ok(node.clone())
                        }
                        PlanType::Project => {
                            // Try to push filter below projection
                            if self.can_push_filter_through_project(node, child) {
                                self.push_filter_through_project(node, child)
                            } else {
                                Ok(node.clone())
                            }
                        }
                        PlanType::Join => {
                            // Try to push filter into join branches
                            self.push_filter_into_join(node, child)
                        }
                        _ => {
                            // Recursively optimize children
                            let mut optimized = node.clone();
                            optimized.children = node
                                .children
                                .iter()
                                .map(|c| self.apply_predicate_pushdown(c))
                                .collect::<OQLResult<Vec<_>>>()?;
                            Ok(optimized)
                        }
                    }
                } else {
                    Ok(node.clone())
                }
            }
            _ => {
                // Recursively optimize children
                let mut optimized = node.clone();
                optimized.children = node
                    .children
                    .iter()
                    .map(|c| self.apply_predicate_pushdown(c))
                    .collect::<OQLResult<Vec<_>>>()?;
                Ok(optimized)
            }
        }
    }

    /// Check if filter can be pushed through projection
    fn can_push_filter_through_project(&self, _filter: &PlanNode, project: &PlanNode) -> bool {
        // For simplicity, allow pushdown if filter references projected fields
        // In a real implementation, we'd check field dependencies
        !project.metadata.projections.is_empty()
    }

    /// Push filter through projection
    fn push_filter_through_project(
        &self,
        filter: &PlanNode,
        project: &PlanNode,
    ) -> OQLResult<PlanNode> {
        // Create new plan: Project(Filter(child))
        if project.children.is_empty() {
            return Ok(filter.clone());
        }

        let mut new_filter = filter.clone();
        new_filter.children = vec![project.children[0].clone()];

        let mut new_project = project.clone();
        new_project.children = vec![new_filter];

        Ok(new_project)
    }

    /// Push filter into join branches
    fn push_filter_into_join(&self, filter: &PlanNode, _join: &PlanNode) -> OQLResult<PlanNode> {
        // Analyze filter condition to determine which join branch it applies to
        // For simplicity, keep filter above join for now
        // In a real implementation, we'd decompose the filter and push parts down
        Ok(filter.clone())
    }

    /// Apply projection pushdown optimization
    /// Eliminate unnecessary columns as early as possible
    fn apply_projection_pushdown(&self, node: &PlanNode) -> OQLResult<PlanNode> {
        // Recursively optimize children first
        let mut optimized = node.clone();
        optimized.children = node
            .children
            .iter()
            .map(|c| self.apply_projection_pushdown(c))
            .collect::<OQLResult<Vec<_>>>()?;

        // For now, keep projections as-is
        // In a real implementation, we'd track required columns and push projections down
        Ok(optimized)
    }

    /// Apply join reordering optimization
    /// Reorder joins to minimize intermediate result sizes
    fn apply_join_reordering(&self, node: &PlanNode) -> OQLResult<PlanNode> {
        match node.plan_type {
            PlanType::Join => {
                if node.children.len() >= 2 {
                    // For two-way joins, consider swapping left and right
                    let left = &node.children[0];
                    let right = &node.children[1];

                    // Estimate sizes
                    let left_size = self.estimate_node_size(left);
                    let right_size = self.estimate_node_size(right);

                    // Put smaller relation on the left (for hash join build side)
                    if right_size < left_size {
                        let mut swapped = node.clone();
                        swapped.children = vec![right.clone(), left.clone()];
                        return Ok(swapped);
                    }
                }
                Ok(node.clone())
            }
            _ => {
                // Recursively optimize children
                let mut optimized = node.clone();
                optimized.children = node
                    .children
                    .iter()
                    .map(|c| self.apply_join_reordering(c))
                    .collect::<OQLResult<Vec<_>>>()?;
                Ok(optimized)
            }
        }
    }

    /// Apply index selection optimization
    /// Choose to use indexes when they reduce cost
    fn apply_index_selection(&self, operator: &PhysicalOperator) -> OQLResult<PhysicalOperator> {
        match operator {
            PhysicalOperator::SeqScan(_scan_op) => {
                // Check if an index scan would be cheaper
                // For now, keep sequential scan
                // In a real implementation, we'd check available indexes and their selectivity
                Ok(operator.clone())
            }
            PhysicalOperator::Filter(filter_op) => {
                // Recursively optimize input
                let optimized_input = self.apply_index_selection(&filter_op.input)?;

                let mut optimized = filter_op.clone();
                optimized.input = Box::new(optimized_input);
                Ok(PhysicalOperator::Filter(optimized))
            }
            PhysicalOperator::Project(project_op) => {
                let optimized_input = self.apply_index_selection(&project_op.input)?;
                let mut optimized = project_op.clone();
                optimized.input = Box::new(optimized_input);
                Ok(PhysicalOperator::Project(optimized))
            }
            PhysicalOperator::Sort(sort_op) => {
                let optimized_input = self.apply_index_selection(&sort_op.input)?;
                let mut optimized = sort_op.clone();
                optimized.input = Box::new(optimized_input);
                Ok(PhysicalOperator::Sort(optimized))
            }
            PhysicalOperator::Limit(limit_op) => {
                let optimized_input = self.apply_index_selection(&limit_op.input)?;
                let mut optimized = limit_op.clone();
                optimized.input = Box::new(optimized_input);
                Ok(PhysicalOperator::Limit(optimized))
            }
            _ => Ok(operator.clone()),
        }
    }

    /// Apply join algorithm selection
    /// Choose between nested loop and hash join based on cost
    fn apply_join_algorithm_selection(
        &self,
        operator: &PhysicalOperator,
    ) -> OQLResult<PhysicalOperator> {
        match operator {
            PhysicalOperator::NestedLoopJoin(nl_op) => {
                // Consider converting to hash join
                let nl_cost = self.cost_model.estimate_cost(operator);

                // Create equivalent hash join
                let hash_join =
                    PhysicalOperator::HashJoin(crate::planner::operators::HashJoinOperator {
                        left: nl_op.left.clone(),
                        right: nl_op.right.clone(),
                        join_condition: nl_op.join_condition.clone(),
                    });

                let hj_cost = self.cost_model.estimate_cost(&hash_join);

                // Choose cheaper algorithm
                if hj_cost.total_cost < nl_cost.total_cost {
                    Ok(hash_join)
                } else {
                    Ok(operator.clone())
                }
            }
            PhysicalOperator::HashJoin(hj_op) => {
                // Consider converting to nested loop join
                let hj_cost = self.cost_model.estimate_cost(operator);

                // Create equivalent nested loop join
                let nl_join = PhysicalOperator::NestedLoopJoin(
                    crate::planner::operators::NestedLoopJoinOperator {
                        left: hj_op.left.clone(),
                        right: hj_op.right.clone(),
                        join_condition: hj_op.join_condition.clone(),
                    },
                );

                let nl_cost = self.cost_model.estimate_cost(&nl_join);

                // Choose cheaper algorithm
                if nl_cost.total_cost < hj_cost.total_cost {
                    Ok(nl_join)
                } else {
                    Ok(operator.clone())
                }
            }
            PhysicalOperator::Filter(filter_op) => {
                let optimized_input = self.apply_join_algorithm_selection(&filter_op.input)?;
                let mut optimized = filter_op.clone();
                optimized.input = Box::new(optimized_input);
                Ok(PhysicalOperator::Filter(optimized))
            }
            PhysicalOperator::Project(project_op) => {
                let optimized_input = self.apply_join_algorithm_selection(&project_op.input)?;
                let mut optimized = project_op.clone();
                optimized.input = Box::new(optimized_input);
                Ok(PhysicalOperator::Project(optimized))
            }
            _ => Ok(operator.clone()),
        }
    }

    /// Estimate the size of a plan node
    fn estimate_node_size(&self, node: &PlanNode) -> u64 {
        // Use statistics if available
        if let Some(stats) = &self.stats {
            if let Some(schema) = &node.metadata.schema {
                if let Ok(cardinality) = stats.get_cardinality(schema) {
                    return cardinality;
                }
            }
        }

        // Fallback to metadata estimate
        node.metadata.estimated_cardinality.unwrap_or(1000)
    }

    /// Get the cost model
    pub fn cost_model(&self) -> &CostModel {
        &self.cost_model
    }
}

impl Default for QueryOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Optimization statistics
#[derive(Debug, Clone)]
pub struct OptimizationStats {
    /// Number of rules applied
    pub rules_applied: usize,
    /// Original plan cost
    pub original_cost: f64,
    /// Optimized plan cost
    pub optimized_cost: f64,
    /// Cost improvement percentage
    pub improvement_pct: f64,
}

impl OptimizationStats {
    /// Create new optimization statistics
    pub fn new(original_cost: f64, optimized_cost: f64, rules_applied: usize) -> Self {
        let improvement_pct = if original_cost > 0.0 {
            ((original_cost - optimized_cost) / original_cost) * 100.0
        } else {
            0.0
        };

        Self {
            rules_applied,
            original_cost,
            optimized_cost,
            improvement_pct,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Expression;
    use crate::planner::types::FilterCondition;

    #[test]
    fn test_optimizer_creation() {
        let optimizer = QueryOptimizer::new();
        assert_eq!(optimizer.optimization_level, 2);
    }

    #[test]
    fn test_predicate_pushdown() {
        let optimizer = QueryOptimizer::new();

        // Create a plan: Filter(Scan)
        let scan = PlanNode::new(PlanType::Scan).with_schema("users".to_string());

        let filter = PlanNode::new(PlanType::Filter)
            .with_child(scan)
            .with_condition(FilterCondition::Comparison {
                left: Expression::Literal(crate::parser::Literal::Integer(1)),
                operator: crate::parser::ComparisonOperator::Equal,
                right: Expression::Literal(crate::parser::Literal::Integer(1)),
            });

        let optimized = optimizer.apply_predicate_pushdown(&filter).unwrap();

        // Filter on scan should remain as-is (already optimal)
        assert_eq!(optimized.plan_type, PlanType::Filter);
        assert_eq!(optimized.children.len(), 1);
        assert_eq!(optimized.children[0].plan_type, PlanType::Scan);
    }

    #[test]
    fn test_join_reordering() {
        let optimizer = QueryOptimizer::new();

        // Create a join with estimated sizes
        let mut left = PlanNode::new(PlanType::Scan).with_schema("large_table".to_string());
        left.metadata.estimated_cardinality = Some(10000);

        let mut right = PlanNode::new(PlanType::Scan).with_schema("small_table".to_string());
        right.metadata.estimated_cardinality = Some(100);

        let join = PlanNode::new(PlanType::Join)
            .with_child(left)
            .with_child(right);

        let optimized = optimizer.apply_join_reordering(&join).unwrap();

        // Smaller table should be on the left
        assert_eq!(
            optimized.children[0].metadata.schema,
            Some("small_table".to_string())
        );
        assert_eq!(
            optimized.children[1].metadata.schema,
            Some("large_table".to_string())
        );
    }

    #[test]
    fn test_optimization_level() {
        let optimizer = QueryOptimizer::new().with_optimization_level(0);
        assert_eq!(optimizer.optimization_level, 0);

        let optimizer = QueryOptimizer::new().with_optimization_level(2);
        assert_eq!(optimizer.optimization_level, 2);
    }

    #[test]
    fn test_optimization_stats() {
        let stats = OptimizationStats::new(100.0, 50.0, 3);
        assert_eq!(stats.original_cost, 100.0);
        assert_eq!(stats.optimized_cost, 50.0);
        assert_eq!(stats.improvement_pct, 50.0);
        assert_eq!(stats.rules_applied, 3);
    }
}
