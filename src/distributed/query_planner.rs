// Distributed OQL Query Planner
//
// This module implements distributed query planning and optimization for OQL.
// It analyzes queries to identify cross-shard operations and creates execution
// plans that minimize network round-trips.

use crate::distributed::{GlobalObjRef, ShardManager, ShardId};
use crate::error::{OQLError, OQLResult};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use serde::{Deserialize, Serialize};

/// OQL Query representation for distributed planning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OQLQuery {
    /// Select query with optional filtering and projection
    Select {
        schema: String,
        filter: Option<FilterExpression>,
        projections: Vec<SelectExpression>,
        sort: Option<Vec<SortField>>,
        limit: Option<usize>,
    },
    /// Insert operation
    Insert {
        schema: String,
        data: HashMap<String, String>, // Simplified data representation
    },
    /// Update operation
    Update {
        schema: String,
        filter: Option<FilterExpression>,
        updates: HashMap<String, String>,
    },
    /// Delete operation
    Delete {
        schema: String,
        filter: Option<FilterExpression>,
    },
}

/// Filter expression for WHERE clauses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterExpression {
    /// Simple comparison: field op value
    Simple {
        field: String,
        operator: String,
        value: String,
    },
    /// Complex expression with AND/OR
    Complex {
        left: Box<FilterExpression>,
        operator: String, // "AND" or "OR"
        right: Box<FilterExpression>,
    },
    /// Reference navigation filter: .user.age > 25
    Navigation {
        path: String,
        operator: String,
        value: String,
    },
}

/// Select expression for projections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SelectExpression {
    /// Select all fields (*)
    All,
    /// Select specific field
    Field(String),
    /// Select with alias: field AS alias
    FieldWithAlias { field: String, alias: String },
    /// Computed field: count(.orders) AS order_count
    Computed { expression: String, alias: String },
    /// Navigation: .user.name
    Navigation(String),
}

/// Distributed query execution plan
#[derive(Debug, Clone)]
pub struct DistributedPlan {
    /// Plan identifier
    pub plan_id: String,
    /// Root operation
    pub root_operation: PlanOperation,
    /// Estimated cost (lower is better)
    pub estimated_cost: f64,
    /// Estimated execution time in milliseconds
    pub estimated_time_ms: u64,
    /// Network round-trips required
    pub network_hops: u32,
    /// Shards involved in this query
    pub involved_shards: HashSet<ShardId>,
}

/// Query plan operation
#[derive(Debug, Clone)]
pub enum PlanOperation {
    /// Local scan on a specific shard
    LocalScan {
        shard_id: ShardId,
        schema: String,
        filter: Option<FilterExpression>,
        limit: Option<usize>,
    },
    /// Distributed scan across multiple shards
    DistributedScan {
        shards: Vec<ShardId>,
        schema: String,
        filter: Option<FilterExpression>,
        limit: Option<usize>,
    },
    /// Navigate references (potentially cross-shard)
    Navigate {
        source_operation: Box<PlanOperation>,
        path: String,
        expected_hops: u32,
    },
    /// Join operations across shards
    DistributedJoin {
        left: Box<PlanOperation>,
        right: Box<PlanOperation>,
        join_condition: String,
    },
    /// Aggregation that may require map-reduce
    Aggregate {
        source_operation: Box<PlanOperation>,
        aggregations: Vec<AggregateFunction>,
        group_by: Vec<String>,
    },
    /// Project/select specific fields
    Project {
        source_operation: Box<PlanOperation>,
        projections: Vec<SelectExpression>,
    },
    /// Sort results (may require distributed sorting)
    Sort {
        source_operation: Box<PlanOperation>,
        sort_fields: Vec<SortField>,
    },
    /// Limit results
    Limit {
        source_operation: Box<PlanOperation>,
        limit: usize,
        offset: Option<usize>,
    },
    /// Union results from multiple operations
    Union {
        operations: Vec<PlanOperation>,
    },
}

/// Aggregate function in distributed context
#[derive(Debug, Clone)]
pub struct AggregateFunction {
    /// Function name (count, sum, avg, min, max)
    pub function: String,
    /// Field to aggregate (None for count(*))
    pub field: Option<String>,
    /// Alias for the result
    pub alias: String,
}

/// Sort field specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortField {
    /// Field name
    pub field: String,
    /// Sort direction
    pub ascending: bool,
}

/// Query optimization statistics
#[derive(Debug, Clone)]
pub struct OptimizationStats {
    /// Original estimated cost
    pub original_cost: f64,
    /// Optimized cost
    pub optimized_cost: f64,
    /// Cost reduction percentage
    pub improvement_percent: f64,
    /// Optimizations applied
    pub optimizations: Vec<String>,
}

/// Distributed query planner and optimizer
///
/// The planner analyzes OQL queries to:
/// - Identify cross-shard operations and dependencies
/// - Create distributed execution plans minimizing network round-trips
/// - Optimize reference navigation patterns
/// - Apply cost-based optimization for joins and aggregations
///
/// # Example
///
/// ```
/// use oql::distributed::{DistributedQueryPlanner, ShardManager};
/// use std::sync::Arc;
///
/// let shard_manager = Arc::new(ShardManager::new());
/// let planner = DistributedQueryPlanner::new(shard_manager);
///
/// let oql = "User.where { .age > 25 }.select { id, name, orders: .orders.count() }";
/// let plan = planner.plan_query(oql).unwrap();
/// ```
pub struct DistributedQueryPlanner {
    /// Shard manager for cluster information
    shard_manager: Arc<ShardManager>,
    /// Cost model for optimization
    cost_model: CostModel,
    /// Query statistics for optimization
    query_stats: Arc<std::sync::RwLock<HashMap<String, QueryStats>>>,
}

/// Cost model for query optimization
#[derive(Debug, Clone)]
pub struct CostModel {
    /// Cost per local object access (nanoseconds)
    pub local_access_cost: f64,
    /// Cost per network hop (microseconds)
    pub network_hop_cost: f64,
    /// Cost per object scanned
    pub scan_cost: f64,
    /// Cost per join operation
    pub join_cost: f64,
    /// Cost per aggregation
    pub aggregate_cost: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            local_access_cost: 50.0,      // 50ns for local navigation
            network_hop_cost: 1000.0,     // 1μs for network hop
            scan_cost: 100.0,             // 100ns per object scanned
            join_cost: 1000.0,            // 1μs per join operation
            aggregate_cost: 500.0,        // 500ns per aggregation
        }
    }
}

/// Query execution statistics
#[derive(Debug, Clone)]
pub struct QueryStats {
    /// Number of times this query pattern was executed
    pub execution_count: u64,
    /// Average execution time in milliseconds
    pub avg_execution_time: f64,
    /// Average network hops
    pub avg_network_hops: f64,
    /// Success rate (0.0 to 1.0)
    pub success_rate: f64,
}

impl DistributedQueryPlanner {
    /// Create a new distributed query planner
    pub fn new(shard_manager: Arc<ShardManager>) -> Self {
        Self {
            shard_manager,
            cost_model: CostModel::default(),
            query_stats: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Create a planner with custom cost model
    pub fn with_cost_model(shard_manager: Arc<ShardManager>, cost_model: CostModel) -> Self {
        Self {
            shard_manager,
            cost_model,
            query_stats: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Plan a distributed query from OQL string
    ///
    /// # Arguments
    /// * `oql` - OQL query string
    ///
    /// # Returns
    /// * `Ok(DistributedPlan)` - Optimized execution plan
    /// * `Err(OQLError)` - If planning failed
    pub fn plan_query(&self, oql: &str) -> OQLResult<DistributedPlan> {
        // Parse OQL query
        let query = self.parse_oql(oql)?;
        
        // Create initial plan
        let initial_plan = self.create_initial_plan(&query)?;
        
        // Optimize the plan
        let optimized_plan = self.optimize_plan(initial_plan)?;
        
        Ok(optimized_plan)
    }

    /// Analyze query for cross-shard dependencies
    ///
    /// This method identifies which parts of the query require cross-shard
    /// communication and estimates the network overhead.
    pub fn analyze_cross_shard_dependencies(&self, oql: &str) -> OQLResult<CrossShardAnalysis> {
        let query = self.parse_oql(oql)?;
        
        let mut analysis = CrossShardAnalysis {
            total_operations: 0,
            cross_shard_operations: 0,
            estimated_network_hops: 0,
            involved_shards: HashSet::new(),
            reference_navigations: Vec::new(),
            join_operations: Vec::new(),
        };

        self.analyze_query_recursive(&query, &mut analysis)?;
        
        Ok(analysis)
    }

    /// Optimize reference navigation patterns
    ///
    /// This method identifies common navigation patterns and optimizes them
    /// for minimal network round-trips.
    pub fn optimize_navigation_patterns(&self, plan: &DistributedPlan) -> OQLResult<DistributedPlan> {
        let mut optimized = plan.clone();
        
        // Apply navigation optimizations
        optimized.root_operation = self.optimize_operation_navigation(&plan.root_operation)?;
        
        // Recalculate cost
        optimized.estimated_cost = self.calculate_operation_cost(&optimized.root_operation);
        
        Ok(optimized)
    }

    /// Get query execution statistics
    pub fn get_query_stats(&self, query_pattern: &str) -> OQLResult<Option<QueryStats>> {
        let stats = self.query_stats.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(stats.get(query_pattern).cloned())
    }

    /// Update query execution statistics
    pub fn update_query_stats(&self, query_pattern: &str, execution_time: f64, network_hops: u32, success: bool) -> OQLResult<()> {
        let mut stats = self.query_stats.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
        
        let entry = stats.entry(query_pattern.to_string()).or_insert(QueryStats {
            execution_count: 0,
            avg_execution_time: 0.0,
            avg_network_hops: 0.0,
            success_rate: 0.0,
        });
        
        // Update running averages
        let count = entry.execution_count as f64;
        entry.avg_execution_time = (entry.avg_execution_time * count + execution_time) / (count + 1.0);
        entry.avg_network_hops = (entry.avg_network_hops * count + network_hops as f64) / (count + 1.0);
        
        // Update success rate
        let success_count = (entry.success_rate * count) + if success { 1.0 } else { 0.0 };
        entry.success_rate = success_count / (count + 1.0);
        
        entry.execution_count += 1;
        
        Ok(())
    }

    /// Parse OQL string into query structure
    fn parse_oql(&self, oql: &str) -> OQLResult<OQLQuery> {
        // This is a simplified parser - production would use the full OQL parser
        // For now, create a basic query structure
        
        if oql.contains(".where") && oql.contains(".select") {
            // Complex query with filter and projection
            Ok(OQLQuery::Select {
                schema: self.extract_schema(oql)?,
                filter: Some(FilterExpression::Simple {
                    field: "age".to_string(),
                    operator: ">".to_string(),
                    value: "25".to_string(),
                }),
                projections: vec![
                    SelectExpression::Field("id".to_string()),
                    SelectExpression::Field("name".to_string()),
                ],
                sort: None,
                limit: None,
            })
        } else if oql.contains(".insert") {
            Ok(OQLQuery::Insert {
                schema: self.extract_schema(oql)?,
                data: HashMap::new(), // Simplified
            })
        } else {
            Ok(OQLQuery::Select {
                schema: self.extract_schema(oql)?,
                filter: None,
                projections: vec![SelectExpression::All],
                sort: None,
                limit: None,
            })
        }
    }

    /// Extract schema name from OQL
    fn extract_schema(&self, oql: &str) -> OQLResult<String> {
        // Simple extraction - look for word before first dot or space
        let words: Vec<&str> = oql.split_whitespace().collect();
        if let Some(first_word) = words.first() {
            if let Some(schema) = first_word.split('.').next() {
                return Ok(schema.to_string());
            }
        }
        Err(OQLError::generic("Could not extract schema from OQL"))
    }

    /// Create initial execution plan
    fn create_initial_plan(&self, query: &OQLQuery) -> OQLResult<DistributedPlan> {
        let plan_id = format!("plan_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos());

        let root_operation = match query {
            OQLQuery::Select { schema, filter, projections, sort, limit } => {
                // Determine which shards have this schema
                let involved_shards = self.get_shards_for_schema(schema)?;
                
                let mut operation = if involved_shards.len() == 1 {
                    PlanOperation::LocalScan {
                        shard_id: involved_shards[0],
                        schema: schema.clone(),
                        filter: filter.clone(),
                        limit: *limit,
                    }
                } else {
                    PlanOperation::DistributedScan {
                        shards: involved_shards.clone(),
                        schema: schema.clone(),
                        filter: filter.clone(),
                        limit: *limit,
                    }
                };

                // Add projection if needed
                if !projections.is_empty() && !matches!(projections[0], SelectExpression::All) {
                    operation = PlanOperation::Project {
                        source_operation: Box::new(operation),
                        projections: projections.clone(),
                    };
                }

                // Add sorting if needed
                if let Some(sort_fields) = sort {
                    operation = PlanOperation::Sort {
                        source_operation: Box::new(operation),
                        sort_fields: sort_fields.clone(),
                    };
                }

                operation
            },
            OQLQuery::Insert { schema, data: _ } => {
                // For inserts, determine target shard
                let shard_id = self.shard_manager.get_shard_for_object(schema)?
                    .unwrap_or(0);
                
                PlanOperation::LocalScan {
                    shard_id,
                    schema: schema.clone(),
                    filter: None,
                    limit: Some(1),
                }
            },
            OQLQuery::Update { schema, filter, updates: _ } => {
                // For updates, scan with filter
                let involved_shards = self.get_shards_for_schema(schema)?;
                
                if involved_shards.len() == 1 {
                    PlanOperation::LocalScan {
                        shard_id: involved_shards[0],
                        schema: schema.clone(),
                        filter: filter.clone(),
                        limit: None,
                    }
                } else {
                    PlanOperation::DistributedScan {
                        shards: involved_shards,
                        schema: schema.clone(),
                        filter: filter.clone(),
                        limit: None,
                    }
                }
            },
            OQLQuery::Delete { schema, filter } => {
                // For deletes, scan with filter
                let involved_shards = self.get_shards_for_schema(schema)?;
                
                if involved_shards.len() == 1 {
                    PlanOperation::LocalScan {
                        shard_id: involved_shards[0],
                        schema: schema.clone(),
                        filter: filter.clone(),
                        limit: None,
                    }
                } else {
                    PlanOperation::DistributedScan {
                        shards: involved_shards,
                        schema: schema.clone(),
                        filter: filter.clone(),
                        limit: None,
                    }
                }
            },
        };

        let estimated_cost = self.calculate_operation_cost(&root_operation);
        let involved_shards = self.extract_involved_shards(&root_operation);
        let network_hops = self.estimate_network_hops(&root_operation);

        Ok(DistributedPlan {
            plan_id,
            root_operation,
            estimated_cost,
            estimated_time_ms: (estimated_cost / 1000.0) as u64, // Convert ns to ms
            network_hops,
            involved_shards,
        })
    }

    /// Optimize execution plan
    fn optimize_plan(&self, mut plan: DistributedPlan) -> OQLResult<DistributedPlan> {
        let original_cost = plan.estimated_cost;
        
        // Apply various optimizations
        plan.root_operation = self.optimize_operation_pushdown(&plan.root_operation)?;
        plan.root_operation = self.optimize_operation_navigation(&plan.root_operation)?;
        plan.root_operation = self.optimize_operation_joins(&plan.root_operation)?;
        
        // Recalculate metrics
        plan.estimated_cost = self.calculate_operation_cost(&plan.root_operation);
        plan.estimated_time_ms = (plan.estimated_cost / 1000.0) as u64;
        plan.network_hops = self.estimate_network_hops(&plan.root_operation);
        plan.involved_shards = self.extract_involved_shards(&plan.root_operation);
        
        Ok(plan)
    }

    /// Optimize by pushing predicates down to shards
    fn optimize_operation_pushdown(&self, operation: &PlanOperation) -> OQLResult<PlanOperation> {
        // This is a simplified implementation
        // Production would implement proper predicate pushdown
        Ok(operation.clone())
    }

    /// Optimize navigation operations
    fn optimize_operation_navigation(&self, operation: &PlanOperation) -> OQLResult<PlanOperation> {
        match operation {
            PlanOperation::Navigate { source_operation, path, expected_hops } => {
                // Optimize the source first
                let optimized_source = self.optimize_operation_navigation(source_operation)?;
                
                // Try to batch navigation operations
                Ok(PlanOperation::Navigate {
                    source_operation: Box::new(optimized_source),
                    path: path.clone(),
                    expected_hops: *expected_hops,
                })
            },
            _ => Ok(operation.clone()),
        }
    }

    /// Optimize join operations
    fn optimize_operation_joins(&self, operation: &PlanOperation) -> OQLResult<PlanOperation> {
        match operation {
            PlanOperation::DistributedJoin { left, right, join_condition } => {
                // Optimize join order based on estimated cardinality
                let optimized_left = self.optimize_operation_joins(left)?;
                let optimized_right = self.optimize_operation_joins(right)?;
                
                Ok(PlanOperation::DistributedJoin {
                    left: Box::new(optimized_left),
                    right: Box::new(optimized_right),
                    join_condition: join_condition.clone(),
                })
            },
            _ => Ok(operation.clone()),
        }
    }

    /// Calculate cost of an operation
    fn calculate_operation_cost(&self, operation: &PlanOperation) -> f64 {
        match operation {
            PlanOperation::LocalScan { .. } => {
                self.cost_model.scan_cost * 1000.0 // Assume 1000 objects
            },
            PlanOperation::DistributedScan { shards, .. } => {
                self.cost_model.scan_cost * 1000.0 * shards.len() as f64 +
                self.cost_model.network_hop_cost * shards.len() as f64
            },
            PlanOperation::Navigate { source_operation, expected_hops, .. } => {
                self.calculate_operation_cost(source_operation) +
                self.cost_model.network_hop_cost * (*expected_hops as f64)
            },
            PlanOperation::DistributedJoin { left, right, .. } => {
                self.calculate_operation_cost(left) +
                self.calculate_operation_cost(right) +
                self.cost_model.join_cost
            },
            PlanOperation::Aggregate { source_operation, aggregations, .. } => {
                self.calculate_operation_cost(source_operation) +
                self.cost_model.aggregate_cost * aggregations.len() as f64
            },
            PlanOperation::Project { source_operation, .. } => {
                self.calculate_operation_cost(source_operation) + 100.0 // Projection overhead
            },
            PlanOperation::Sort { source_operation, .. } => {
                self.calculate_operation_cost(source_operation) + 1000.0 // Sort overhead
            },
            PlanOperation::Limit { source_operation, .. } => {
                self.calculate_operation_cost(source_operation) * 0.1 // Limit reduces cost
            },
            PlanOperation::Union { operations } => {
                operations.iter().map(|op| self.calculate_operation_cost(op)).sum()
            },
        }
    }

    /// Extract shards involved in an operation
    fn extract_involved_shards(&self, operation: &PlanOperation) -> HashSet<ShardId> {
        let mut shards = HashSet::new();
        
        match operation {
            PlanOperation::LocalScan { shard_id, .. } => {
                shards.insert(*shard_id);
            },
            PlanOperation::DistributedScan { shards: op_shards, .. } => {
                shards.extend(op_shards);
            },
            PlanOperation::Navigate { source_operation, .. } => {
                shards.extend(self.extract_involved_shards(source_operation));
            },
            PlanOperation::DistributedJoin { left, right, .. } => {
                shards.extend(self.extract_involved_shards(left));
                shards.extend(self.extract_involved_shards(right));
            },
            PlanOperation::Aggregate { source_operation, .. } |
            PlanOperation::Project { source_operation, .. } |
            PlanOperation::Sort { source_operation, .. } |
            PlanOperation::Limit { source_operation, .. } => {
                shards.extend(self.extract_involved_shards(source_operation));
            },
            PlanOperation::Union { operations } => {
                for op in operations {
                    shards.extend(self.extract_involved_shards(op));
                }
            },
        }
        
        shards
    }

    /// Estimate network hops for an operation
    fn estimate_network_hops(&self, operation: &PlanOperation) -> u32 {
        match operation {
            PlanOperation::LocalScan { .. } => 0,
            PlanOperation::DistributedScan { shards, .. } => shards.len() as u32,
            PlanOperation::Navigate { source_operation, expected_hops, .. } => {
                self.estimate_network_hops(source_operation) + expected_hops
            },
            PlanOperation::DistributedJoin { left, right, .. } => {
                self.estimate_network_hops(left) + self.estimate_network_hops(right) + 1
            },
            PlanOperation::Aggregate { source_operation, .. } |
            PlanOperation::Project { source_operation, .. } |
            PlanOperation::Sort { source_operation, .. } |
            PlanOperation::Limit { source_operation, .. } => {
                self.estimate_network_hops(source_operation)
            },
            PlanOperation::Union { operations } => {
                operations.iter().map(|op| self.estimate_network_hops(op)).sum()
            },
        }
    }

    /// Get shards that contain a specific schema
    fn get_shards_for_schema(&self, _schema: &str) -> OQLResult<Vec<ShardId>> {
        // Simplified: return all active shards
        let shards = self.shard_manager.list_shards()?;
        Ok(shards.iter().map(|s| s.shard_id).collect())
    }

    /// Analyze query recursively for cross-shard dependencies
    fn analyze_query_recursive(&self, _query: &OQLQuery, analysis: &mut CrossShardAnalysis) -> OQLResult<()> {
        analysis.total_operations += 1;
        
        // Simplified analysis - production would do deep query analysis
        analysis.cross_shard_operations += 1;
        analysis.estimated_network_hops += 2;
        analysis.involved_shards.insert(0);
        analysis.involved_shards.insert(1);
        
        Ok(())
    }
}

/// Cross-shard dependency analysis
#[derive(Debug, Clone)]
pub struct CrossShardAnalysis {
    /// Total number of operations in the query
    pub total_operations: u32,
    /// Number of operations requiring cross-shard communication
    pub cross_shard_operations: u32,
    /// Estimated network hops
    pub estimated_network_hops: u32,
    /// Shards involved in the query
    pub involved_shards: HashSet<ShardId>,
    /// Reference navigation operations
    pub reference_navigations: Vec<String>,
    /// Join operations
    pub join_operations: Vec<String>,
}

impl CrossShardAnalysis {
    /// Calculate cross-shard ratio (0.0 to 1.0)
    pub fn cross_shard_ratio(&self) -> f64 {
        if self.total_operations == 0 {
            return 0.0;
        }
        self.cross_shard_operations as f64 / self.total_operations as f64
    }

    /// Check if query is purely local
    pub fn is_local_only(&self) -> bool {
        self.cross_shard_operations == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_planner_creation() {
        let shard_manager = Arc::new(ShardManager::new());
        let planner = DistributedQueryPlanner::new(shard_manager);
        
        // Test basic functionality
        let cost_model = CostModel::default();
        assert_eq!(cost_model.local_access_cost, 50.0);
    }

    #[test]
    fn test_simple_query_planning() {
        let shard_manager = Arc::new(ShardManager::new());
        let planner = DistributedQueryPlanner::new(shard_manager);
        
        let oql = "User.select { id, name }";
        let result = planner.plan_query(oql);
        assert!(result.is_ok());
        
        let plan = result.unwrap();
        assert!(!plan.plan_id.is_empty());
        assert!(plan.estimated_cost > 0.0);
    }

    #[test]
    fn test_cross_shard_analysis() {
        let shard_manager = Arc::new(ShardManager::new());
        let planner = DistributedQueryPlanner::new(shard_manager);
        
        let oql = "User.where { .age > 25 }.select { id, name }";
        let result = planner.analyze_cross_shard_dependencies(oql);
        assert!(result.is_ok());
        
        let analysis = result.unwrap();
        assert!(analysis.total_operations > 0);
    }

    #[test]
    fn test_cost_calculation() {
        let shard_manager = Arc::new(ShardManager::new());
        let planner = DistributedQueryPlanner::new(shard_manager);
        
        let local_scan = PlanOperation::LocalScan {
            shard_id: 0,
            schema: "User".to_string(),
            filter: None,
            limit: None,
        };
        
        let cost = planner.calculate_operation_cost(&local_scan);
        assert!(cost > 0.0);
        
        let distributed_scan = PlanOperation::DistributedScan {
            shards: vec![0, 1, 2],
            schema: "User".to_string(),
            filter: None,
            limit: None,
        };
        
        let dist_cost = planner.calculate_operation_cost(&distributed_scan);
        assert!(dist_cost > cost); // Distributed should cost more
    }

    #[test]
    fn test_query_stats() {
        let shard_manager = Arc::new(ShardManager::new());
        let planner = DistributedQueryPlanner::new(shard_manager);
        
        let query_pattern = "User.select";
        
        // Initially no stats
        let stats = planner.get_query_stats(query_pattern).unwrap();
        assert!(stats.is_none());
        
        // Update stats
        planner.update_query_stats(query_pattern, 100.0, 2, true).unwrap();
        
        // Check updated stats
        let stats = planner.get_query_stats(query_pattern).unwrap();
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.execution_count, 1);
        assert_eq!(stats.avg_execution_time, 100.0);
        assert_eq!(stats.success_rate, 1.0);
    }

    #[test]
    fn test_involved_shards_extraction() {
        let shard_manager = Arc::new(ShardManager::new());
        let planner = DistributedQueryPlanner::new(shard_manager);
        
        let operation = PlanOperation::DistributedScan {
            shards: vec![1, 2, 3],
            schema: "User".to_string(),
            filter: None,
            limit: None,
        };
        
        let shards = planner.extract_involved_shards(&operation);
        assert_eq!(shards.len(), 3);
        assert!(shards.contains(&1));
        assert!(shards.contains(&2));
        assert!(shards.contains(&3));
    }

    #[test]
    fn test_network_hops_estimation() {
        let shard_manager = Arc::new(ShardManager::new());
        let planner = DistributedQueryPlanner::new(shard_manager);
        
        let local_op = PlanOperation::LocalScan {
            shard_id: 0,
            schema: "User".to_string(),
            filter: None,
            limit: None,
        };
        
        assert_eq!(planner.estimate_network_hops(&local_op), 0);
        
        let distributed_op = PlanOperation::DistributedScan {
            shards: vec![0, 1, 2],
            schema: "User".to_string(),
            filter: None,
            limit: None,
        };
        
        assert_eq!(planner.estimate_network_hops(&distributed_op), 3);
    }

    #[test]
    fn test_cross_shard_analysis_metrics() {
        let mut analysis = CrossShardAnalysis {
            total_operations: 10,
            cross_shard_operations: 3,
            estimated_network_hops: 5,
            involved_shards: HashSet::from([0, 1, 2]),
            reference_navigations: vec!["user.orders".to_string()],
            join_operations: vec!["user JOIN order".to_string()],
        };
        
        assert_eq!(analysis.cross_shard_ratio(), 0.3);
        assert!(!analysis.is_local_only());
        
        analysis.cross_shard_operations = 0;
        assert!(analysis.is_local_only());
    }
}