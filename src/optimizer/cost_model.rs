// Cost Model for Query Optimization
//
// This module provides cost estimation for physical operators to enable
// cost-based query optimization. Costs are estimated based on:
// - I/O costs (disk reads/writes)
// - CPU costs (comparisons, computations)
// - Memory costs (sorting, hashing)
// - Network costs (for distributed queries - future)

use crate::planner::operators::*;
use crate::optimizer::statistics::StatisticsManager;

/// Cost model for query optimization
pub struct CostModel {
    /// Statistics manager for cardinality estimation
    stats: Option<StatisticsManager>,
    /// Cost parameters
    params: CostParameters,
}

/// Cost parameters for tuning the cost model
#[derive(Debug, Clone)]
pub struct CostParameters {
    /// Cost per sequential page read
    pub seq_page_cost: f64,
    /// Cost per random page read
    pub random_page_cost: f64,
    /// Cost per CPU operation (comparison, arithmetic)
    pub cpu_tuple_cost: f64,
    /// Cost per operator invocation
    pub cpu_operator_cost: f64,
    /// Cost per byte of memory used
    pub memory_cost: f64,
    /// Page size in bytes
    pub page_size: u64,
    /// Average tuple size in bytes
    pub avg_tuple_size: u64,
}

impl Default for CostParameters {
    fn default() -> Self {
        Self {
            seq_page_cost: 1.0,      // Sequential I/O is cheap
            random_page_cost: 4.0,   // Random I/O is expensive
            cpu_tuple_cost: 0.01,    // CPU operations are cheap
            cpu_operator_cost: 0.0025, // Operator overhead
            memory_cost: 0.001,      // Memory is relatively cheap
            page_size: 4096,         // 4KB pages
            avg_tuple_size: 256,     // Average document size
        }
    }
}

/// Cost estimate for an operator
#[derive(Debug, Clone)]
pub struct CostEstimate {
    /// Startup cost (cost before first tuple)
    pub startup_cost: f64,
    /// Total cost (cost to produce all tuples)
    pub total_cost: f64,
    /// Estimated number of output rows
    pub rows: u64,
    /// Estimated width of output rows in bytes
    pub width: u64,
}

impl CostModel {
    /// Create a new cost model
    pub fn new() -> Self {
        Self {
            stats: None,
            params: CostParameters::default(),
        }
    }
    
    /// Create a cost model with statistics
    pub fn with_statistics(stats: StatisticsManager) -> Self {
        Self {
            stats: Some(stats),
            params: CostParameters::default(),
        }
    }
    
    /// Create a cost model with custom parameters
    pub fn with_parameters(params: CostParameters) -> Self {
        Self {
            stats: None,
            params,
        }
    }
    
    /// Estimate the cost of a physical operator
    pub fn estimate_cost(&self, operator: &PhysicalOperator) -> CostEstimate {
        match operator {
            PhysicalOperator::SeqScan(op) => self.cost_seq_scan(op),
            PhysicalOperator::IndexScan(op) => self.cost_index_scan(op),
            PhysicalOperator::Filter(op) => self.cost_filter(op),
            PhysicalOperator::Project(op) => self.cost_project(op),
            PhysicalOperator::Sort(op) => self.cost_sort(op),
            PhysicalOperator::Limit(op) => self.cost_limit(op),
            PhysicalOperator::GroupBy(op) => self.cost_group_by(op),
            PhysicalOperator::Having(op) => self.cost_having(op),
            PhysicalOperator::Aggregate(op) => self.cost_aggregate(op),
            PhysicalOperator::NestedLoopJoin(op) => self.cost_nested_loop_join(op),
            PhysicalOperator::HashJoin(op) => self.cost_hash_join(op),
            PhysicalOperator::Insert(op) => self.cost_insert(op),
            PhysicalOperator::Update(op) => self.cost_update(op),
            PhysicalOperator::Delete(op) => self.cost_delete(op),
        }
    }
    
    /// Cost a sequential scan
    fn cost_seq_scan(&self, op: &SeqScanOperator) -> CostEstimate {
        let rows = op.estimated_rows;
        let pages = self.estimate_pages(rows);
        
        // Sequential scan: read all pages sequentially
        let io_cost = pages as f64 * self.params.seq_page_cost;
        let cpu_cost = rows as f64 * self.params.cpu_tuple_cost;
        
        CostEstimate {
            startup_cost: 0.0,
            total_cost: io_cost + cpu_cost,
            rows,
            width: self.params.avg_tuple_size,
        }
    }
    
    /// Cost an index scan
    fn cost_index_scan(&self, op: &IndexScanOperator) -> CostEstimate {
        let rows = op.estimated_rows;
        
        // Index scan: random I/O for each tuple
        let io_cost = rows as f64 * self.params.random_page_cost;
        let cpu_cost = rows as f64 * self.params.cpu_tuple_cost;
        
        CostEstimate {
            startup_cost: self.params.random_page_cost, // Index lookup
            total_cost: io_cost + cpu_cost,
            rows,
            width: self.params.avg_tuple_size,
        }
    }
    
    /// Cost a filter operation
    fn cost_filter(&self, op: &FilterOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Filter: evaluate condition for each input tuple
        let cpu_cost = input_cost.rows as f64 * self.params.cpu_operator_cost;
        let output_rows = (input_cost.rows as f64 * op.selectivity) as u64;
        
        CostEstimate {
            startup_cost: input_cost.startup_cost,
            total_cost: input_cost.total_cost + cpu_cost,
            rows: output_rows,
            width: input_cost.width,
        }
    }
    
    /// Cost a projection operation
    fn cost_project(&self, op: &ProjectOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Project: evaluate expressions for each tuple
        let cpu_cost = input_cost.rows as f64 
            * op.projections.len() as f64 
            * self.params.cpu_operator_cost;
        
        // Estimate output width based on number of projections
        let output_width = (self.params.avg_tuple_size * op.projections.len() as u64) 
            / (op.projections.len().max(1) as u64);
        
        CostEstimate {
            startup_cost: input_cost.startup_cost,
            total_cost: input_cost.total_cost + cpu_cost,
            rows: input_cost.rows,
            width: output_width,
        }
    }
    
    /// Cost a sort operation
    fn cost_sort(&self, op: &SortOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Sort: O(n log n) comparisons
        let rows = input_cost.rows;
        let comparisons = if rows > 0 {
            rows as f64 * (rows as f64).log2()
        } else {
            0.0
        };
        let cpu_cost = comparisons * self.params.cpu_operator_cost;
        
        // Memory cost for sorting
        let memory_cost = (rows * input_cost.width) as f64 * self.params.memory_cost;
        
        CostEstimate {
            startup_cost: input_cost.total_cost + cpu_cost + memory_cost, // Must sort all before output
            total_cost: input_cost.total_cost + cpu_cost + memory_cost,
            rows,
            width: input_cost.width,
        }
    }
    
    /// Cost a limit operation
    fn cost_limit(&self, op: &LimitOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Limit: can stop early
        let output_rows = input_cost.rows.min(op.limit);
        let cost_fraction = output_rows as f64 / input_cost.rows.max(1) as f64;
        
        CostEstimate {
            startup_cost: input_cost.startup_cost,
            total_cost: input_cost.startup_cost + (input_cost.total_cost - input_cost.startup_cost) * cost_fraction,
            rows: output_rows,
            width: input_cost.width,
        }
    }
    
    /// Cost a group by operation
    fn cost_group_by(&self, op: &GroupByOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // GroupBy: hash table construction + aggregation
        let rows = input_cost.rows;
        let cpu_cost = rows as f64 * self.params.cpu_operator_cost * 2.0; // Hash + aggregate
        let memory_cost = (rows * input_cost.width) as f64 * self.params.memory_cost;
        
        // Estimate number of groups (assume 10% of rows are unique groups)
        let output_rows = (rows as f64 * 0.1).max(1.0) as u64;
        
        CostEstimate {
            startup_cost: input_cost.total_cost + cpu_cost + memory_cost,
            total_cost: input_cost.total_cost + cpu_cost + memory_cost,
            rows: output_rows,
            width: input_cost.width,
        }
    }
    
    /// Cost a having operation
    fn cost_having(&self, op: &HavingOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Having: filter after grouping (assume 50% selectivity)
        let cpu_cost = input_cost.rows as f64 * self.params.cpu_operator_cost;
        let output_rows = (input_cost.rows as f64 * 0.5) as u64;
        
        CostEstimate {
            startup_cost: input_cost.startup_cost,
            total_cost: input_cost.total_cost + cpu_cost,
            rows: output_rows,
            width: input_cost.width,
        }
    }
    
    /// Cost an aggregate operation
    fn cost_aggregate(&self, op: &AggregateOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Aggregate: process all input rows
        let cpu_cost = input_cost.rows as f64 
            * op.aggregates.len() as f64 
            * self.params.cpu_operator_cost;
        
        CostEstimate {
            startup_cost: input_cost.total_cost + cpu_cost,
            total_cost: input_cost.total_cost + cpu_cost,
            rows: 1, // Aggregates produce one row
            width: op.aggregates.len() as u64 * 8, // Each aggregate is ~8 bytes
        }
    }
    
    /// Cost a nested loop join
    fn cost_nested_loop_join(&self, op: &NestedLoopJoinOperator) -> CostEstimate {
        let left_cost = self.estimate_cost(&op.left);
        let right_cost = self.estimate_cost(&op.right);
        
        // Nested loop: for each left tuple, scan all right tuples
        let total_cost = left_cost.total_cost 
            + (left_cost.rows as f64 * right_cost.total_cost);
        
        let cpu_cost = (left_cost.rows * right_cost.rows) as f64 * self.params.cpu_operator_cost;
        
        // Estimate join output (assume 10% selectivity)
        let output_rows = (left_cost.rows as f64 * right_cost.rows as f64 * 0.1) as u64;
        
        CostEstimate {
            startup_cost: left_cost.startup_cost,
            total_cost: total_cost + cpu_cost,
            rows: output_rows,
            width: left_cost.width + right_cost.width,
        }
    }
    
    /// Cost a hash join
    fn cost_hash_join(&self, op: &HashJoinOperator) -> CostEstimate {
        let left_cost = self.estimate_cost(&op.left);
        let right_cost = self.estimate_cost(&op.right);
        
        // Hash join: build hash table on smaller side, probe with larger side
        let build_cost = left_cost.total_cost;
        let probe_cost = right_cost.total_cost;
        
        // CPU cost for hashing and probing
        let cpu_cost = (left_cost.rows + right_cost.rows) as f64 * self.params.cpu_operator_cost * 2.0;
        
        // Memory cost for hash table
        let memory_cost = (left_cost.rows * left_cost.width) as f64 * self.params.memory_cost;
        
        // Estimate join output (assume 10% selectivity)
        let output_rows = (left_cost.rows as f64 * right_cost.rows as f64 * 0.1) as u64;
        
        CostEstimate {
            startup_cost: build_cost + cpu_cost + memory_cost,
            total_cost: build_cost + probe_cost + cpu_cost + memory_cost,
            rows: output_rows,
            width: left_cost.width + right_cost.width,
        }
    }
    
    /// Cost an insert operation
    fn cost_insert(&self, _op: &InsertOperator) -> CostEstimate {
        // Insert: write one page
        let io_cost = self.params.random_page_cost;
        let cpu_cost = self.params.cpu_tuple_cost;
        
        CostEstimate {
            startup_cost: 0.0,
            total_cost: io_cost + cpu_cost,
            rows: 1,
            width: self.params.avg_tuple_size,
        }
    }
    
    /// Cost an update operation
    fn cost_update(&self, op: &UpdateOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Update: read + write for each tuple
        let io_cost = input_cost.rows as f64 * self.params.random_page_cost * 2.0;
        let cpu_cost = input_cost.rows as f64 * self.params.cpu_tuple_cost;
        
        CostEstimate {
            startup_cost: input_cost.startup_cost,
            total_cost: input_cost.total_cost + io_cost + cpu_cost,
            rows: input_cost.rows,
            width: input_cost.width,
        }
    }
    
    /// Cost a delete operation
    fn cost_delete(&self, op: &DeleteOperator) -> CostEstimate {
        let input_cost = self.estimate_cost(&op.input);
        
        // Delete: write for each tuple
        let io_cost = input_cost.rows as f64 * self.params.random_page_cost;
        let cpu_cost = input_cost.rows as f64 * self.params.cpu_tuple_cost;
        
        CostEstimate {
            startup_cost: input_cost.startup_cost,
            total_cost: input_cost.total_cost + io_cost + cpu_cost,
            rows: input_cost.rows,
            width: input_cost.width,
        }
    }
    
    /// Estimate number of pages for a given number of rows
    fn estimate_pages(&self, rows: u64) -> u64 {
        let tuples_per_page = self.params.page_size / self.params.avg_tuple_size;
        if tuples_per_page == 0 {
            return rows;
        }
        (rows + tuples_per_page - 1) / tuples_per_page
    }
    
    /// Compare two operators and return the cheaper one
    pub fn choose_cheaper<'a>(
        &self,
        op1: &'a PhysicalOperator,
        op2: &'a PhysicalOperator,
    ) -> &'a PhysicalOperator {
        let cost1 = self.estimate_cost(op1);
        let cost2 = self.estimate_cost(op2);
        
        if cost1.total_cost <= cost2.total_cost {
            op1
        } else {
            op2
        }
    }
}

impl Default for CostModel {
    fn default() -> Self {
        Self::new()
    }
}

impl CostEstimate {
    /// Create a new cost estimate
    pub fn new(startup_cost: f64, total_cost: f64, rows: u64, width: u64) -> Self {
        Self {
            startup_cost,
            total_cost,
            rows,
            width,
        }
    }
    
    /// Get the incremental cost (total - startup)
    pub fn incremental_cost(&self) -> f64 {
        self.total_cost - self.startup_cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::types::FilterCondition;

    #[test]
    fn test_cost_model_creation() {
        let model = CostModel::new();
        assert_eq!(model.params.seq_page_cost, 1.0);
        assert_eq!(model.params.random_page_cost, 4.0);
    }

    #[test]
    fn test_seq_scan_cost() {
        let model = CostModel::new();
        let op = SeqScanOperator {
            schema: "users".to_string(),
            estimated_rows: 1000,
        };
        
        let cost = model.cost_seq_scan(&op);
        assert!(cost.total_cost > 0.0);
        assert_eq!(cost.rows, 1000);
        assert_eq!(cost.startup_cost, 0.0);
    }

    #[test]
    fn test_filter_cost() {
        let model = CostModel::new();
        let scan = SeqScanOperator {
            schema: "users".to_string(),
            estimated_rows: 1000,
        };
        
        let filter = FilterOperator {
            condition: FilterCondition::Comparison {
                left: crate::parser::Expression::Literal(crate::document::Value::Integer(1)),
                operator: crate::parser::ComparisonOperator::Equal,
                right: crate::parser::Expression::Literal(crate::document::Value::Integer(1)),
            },
            input: Box::new(PhysicalOperator::SeqScan(scan)),
            selectivity: 0.1,
        };
        
        let cost = model.cost_filter(&filter);
        assert!(cost.total_cost > 0.0);
        assert_eq!(cost.rows, 100); // 1000 * 0.1 selectivity
    }

    #[test]
    fn test_sort_cost() {
        let model = CostModel::new();
        let scan = SeqScanOperator {
            schema: "users".to_string(),
            estimated_rows: 1000,
        };
        
        let sort = SortOperator {
            sort_specs: vec![],
            input: Box::new(PhysicalOperator::SeqScan(scan)),
        };
        
        let cost = model.cost_sort(&sort);
        assert!(cost.total_cost > 0.0);
        assert!(cost.startup_cost > 0.0); // Sort has startup cost
        assert_eq!(cost.rows, 1000);
    }

    #[test]
    fn test_join_cost_comparison() {
        let model = CostModel::new();
        
        let left = SeqScanOperator {
            schema: "users".to_string(),
            estimated_rows: 100,
        };
        let right = SeqScanOperator {
            schema: "orders".to_string(),
            estimated_rows: 1000,
        };
        
        let nested_loop = NestedLoopJoinOperator {
            left: Box::new(PhysicalOperator::SeqScan(left.clone())),
            right: Box::new(PhysicalOperator::SeqScan(right.clone())),
            join_condition: None,
        };
        
        let hash_join = HashJoinOperator {
            left: Box::new(PhysicalOperator::SeqScan(left)),
            right: Box::new(PhysicalOperator::SeqScan(right)),
            join_condition: None,
        };
        
        let nl_cost = model.cost_nested_loop_join(&nested_loop);
        let hj_cost = model.cost_hash_join(&hash_join);
        
        // Hash join should be cheaper for this case
        assert!(hj_cost.total_cost < nl_cost.total_cost);
    }

    #[test]
    fn test_limit_cost() {
        let model = CostModel::new();
        let scan = SeqScanOperator {
            schema: "users".to_string(),
            estimated_rows: 1000,
        };
        
        let limit = LimitOperator {
            limit: 10,
            input: Box::new(PhysicalOperator::SeqScan(scan)),
        };
        
        let cost = model.cost_limit(&limit);
        assert_eq!(cost.rows, 10);
        // Limit should reduce total cost
        let scan_cost = model.cost_seq_scan(&SeqScanOperator {
            schema: "users".to_string(),
            estimated_rows: 1000,
        });
        assert!(cost.total_cost < scan_cost.total_cost);
    }
}
