// Tests for Cost-Based Query Optimizer

use owl_db::document::{Document, Value};
use owl_db::error::OQLResult;
use owl_db::optimizer::{
    CostModel, CostParameters, QueryOptimizer, StatisticsManager,
};
use owl_db::parser::{ComparisonOperator, Expression, FieldAccess, Literal};
use owl_db::planner::logical::{LogicalPlan, LogicalPlanner};
use owl_db::planner::operators::*;
use owl_db::planner::physical::{PhysicalPlan, PhysicalPlanner, Statistics};
use owl_db::planner::types::{FilterCondition, PlanNode, PlanType, ProjectionField, SortSpec};
use owl_db::schema::{FieldType, Schema, SchemaRegistry};
use owl_db::storage::create_storage;
use std::collections::HashMap;

#[test]
fn test_cost_model_basic() {
    let model = CostModel::new();
    
    // Test sequential scan cost
    let scan = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 1000,
    };
    
    let cost = model.estimate_cost(&PhysicalOperator::SeqScan(scan));
    assert!(cost.total_cost > 0.0);
    assert_eq!(cost.rows, 1000);
    assert_eq!(cost.startup_cost, 0.0);
}

#[test]
fn test_cost_model_filter() {
    let model = CostModel::new();
    
    let scan = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 1000,
    };
    
    let filter = FilterOperator {
        condition: FilterCondition::Comparison {
            left: Expression::Literal(Literal::Integer(1)),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(1)),
        },
        input: Box::new(PhysicalOperator::SeqScan(scan)),
        selectivity: 0.1,
    };
    
    let cost = model.estimate_cost(&PhysicalOperator::Filter(filter));
    assert!(cost.total_cost > 0.0);
    assert_eq!(cost.rows, 100); // 1000 * 0.1 selectivity
}

#[test]
fn test_cost_model_sort() {
    let model = CostModel::new();
    
    let scan = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 1000,
    };
    
    let sort = SortOperator {
        sort_specs: vec![],
        input: Box::new(PhysicalOperator::SeqScan(scan)),
    };
    
    let cost = model.estimate_cost(&PhysicalOperator::Sort(sort));
    assert!(cost.total_cost > 0.0);
    assert!(cost.startup_cost > 0.0); // Sort must complete before outputting
    assert_eq!(cost.rows, 1000);
}

#[test]
fn test_cost_model_limit() {
    let model = CostModel::new();
    
    let scan = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 1000,
    };
    
    let limit = LimitOperator {
        limit: 10,
        input: Box::new(PhysicalOperator::SeqScan(scan.clone())),
    };
    
    let limit_cost = model.estimate_cost(&PhysicalOperator::Limit(limit));
    let scan_cost = model.estimate_cost(&PhysicalOperator::SeqScan(scan));
    
    assert_eq!(limit_cost.rows, 10);
    assert!(limit_cost.total_cost < scan_cost.total_cost); // Limit reduces cost
}

#[test]
fn test_cost_model_nested_loop_join() {
    let model = CostModel::new();
    
    let left = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 100,
    };
    
    let right = SeqScanOperator {
        schema: "orders".to_string(),
        estimated_rows: 1000,
    };
    
    let join = NestedLoopJoinOperator {
        left: Box::new(PhysicalOperator::SeqScan(left)),
        right: Box::new(PhysicalOperator::SeqScan(right)),
        join_condition: None,
    };
    
    let cost = model.estimate_cost(&PhysicalOperator::NestedLoopJoin(join));
    assert!(cost.total_cost > 0.0);
    assert!(cost.rows > 0); // Join produces output
}

#[test]
fn test_cost_model_hash_join() {
    let model = CostModel::new();
    
    let left = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 100,
    };
    
    let right = SeqScanOperator {
        schema: "orders".to_string(),
        estimated_rows: 1000,
    };
    
    let join = HashJoinOperator {
        left: Box::new(PhysicalOperator::SeqScan(left)),
        right: Box::new(PhysicalOperator::SeqScan(right)),
        join_condition: None,
    };
    
    let cost = model.estimate_cost(&PhysicalOperator::HashJoin(join));
    assert!(cost.total_cost > 0.0);
    assert!(cost.startup_cost > 0.0); // Hash join has build phase
}

#[test]
fn test_cost_model_join_comparison() {
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
    
    let nl_cost = model.estimate_cost(&PhysicalOperator::NestedLoopJoin(nested_loop));
    let hj_cost = model.estimate_cost(&PhysicalOperator::HashJoin(hash_join));
    
    // Hash join should be cheaper for this case
    assert!(hj_cost.total_cost < nl_cost.total_cost);
}

#[test]
fn test_cost_model_custom_parameters() {
    let params = CostParameters {
        seq_page_cost: 2.0,
        random_page_cost: 8.0,
        cpu_tuple_cost: 0.02,
        cpu_operator_cost: 0.005,
        memory_cost: 0.002,
        page_size: 8192,
        avg_tuple_size: 512,
    };
    
    let model = CostModel::with_parameters(params.clone());
    
    let scan = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 1000,
    };
    
    let cost = model.estimate_cost(&PhysicalOperator::SeqScan(scan));
    assert!(cost.total_cost > 0.0);
}

#[test]
fn test_optimizer_creation() {
    let optimizer = QueryOptimizer::new();
    assert!(optimizer.cost_model().estimate_cost(&PhysicalOperator::SeqScan(
        SeqScanOperator {
            schema: "test".to_string(),
            estimated_rows: 100,
        }
    )).total_cost > 0.0);
}

#[test]
fn test_optimizer_with_statistics() {
    let stats = StatisticsManager::new();
    let optimizer = QueryOptimizer::with_statistics(stats);
    
    let scan = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 1000,
    };
    
    let cost = optimizer.cost_model().estimate_cost(&PhysicalOperator::SeqScan(scan));
    assert!(cost.total_cost > 0.0);
}

#[test]
fn test_predicate_pushdown() {
    let optimizer = QueryOptimizer::new();
    
    // Create a logical plan: Filter(Scan)
    let scan = PlanNode::new(PlanType::Scan)
        .with_schema("users".to_string());
    
    let filter = PlanNode::new(PlanType::Filter)
        .with_child(scan)
        .with_condition(FilterCondition::Comparison {
            left: Expression::Literal(Literal::Integer(1)),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(1)),
        });
    
    let plan = LogicalPlan {
        root: filter,
        schema_registry: None,
    };
    
    let optimized = optimizer.optimize_logical(&plan).unwrap();
    
    // Plan structure should be preserved (filter on scan is already optimal)
    assert_eq!(optimized.root.plan_type, PlanType::Filter);
    assert_eq!(optimized.root.children.len(), 1);
    assert_eq!(optimized.root.children[0].plan_type, PlanType::Scan);
}

#[test]
fn test_join_reordering() {
    let optimizer = QueryOptimizer::new();
    
    // Create a join with different sized tables
    let mut large_table = PlanNode::new(PlanType::Scan)
        .with_schema("large_table".to_string());
    large_table.metadata.estimated_cardinality = Some(10000);
    
    let mut small_table = PlanNode::new(PlanType::Scan)
        .with_schema("small_table".to_string());
    small_table.metadata.estimated_cardinality = Some(100);
    
    let join = PlanNode::new(PlanType::Join)
        .with_child(large_table)
        .with_child(small_table);
    
    let plan = LogicalPlan {
        root: join,
        schema_registry: None,
    };
    
    let optimized = optimizer.optimize_logical(&plan).unwrap();
    
    // Smaller table should be on the left
    assert_eq!(optimized.root.children[0].metadata.schema, Some("small_table".to_string()));
    assert_eq!(optimized.root.children[1].metadata.schema, Some("large_table".to_string()));
}

#[test]
fn test_join_algorithm_selection() {
    let optimizer = QueryOptimizer::new();
    
    let left = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 100,
    };
    
    let right = SeqScanOperator {
        schema: "orders".to_string(),
        estimated_rows: 1000,
    };
    
    // Start with nested loop join
    let nl_join = PhysicalOperator::NestedLoopJoin(NestedLoopJoinOperator {
        left: Box::new(PhysicalOperator::SeqScan(left)),
        right: Box::new(PhysicalOperator::SeqScan(right)),
        join_condition: None,
    });
    
    let plan = PhysicalPlan {
        root: nl_join,
        schema_registry: None,
    };
    
    let optimized = optimizer.optimize_physical(&plan).unwrap();
    
    // Should convert to hash join (cheaper for this case)
    match optimized.root {
        PhysicalOperator::HashJoin(_) => {
            // Success - optimizer chose hash join
        }
        _ => panic!("Expected hash join after optimization"),
    }
}

#[test]
fn test_optimization_levels() {
    let optimizer_none = QueryOptimizer::new().with_optimization_level(0);
    let optimizer_basic = QueryOptimizer::new().with_optimization_level(1);
    let optimizer_aggressive = QueryOptimizer::new().with_optimization_level(2);
    
    let scan = PlanNode::new(PlanType::Scan)
        .with_schema("users".to_string());
    
    let plan = LogicalPlan {
        root: scan,
        schema_registry: None,
    };
    
    // Level 0 should not optimize
    let result = optimizer_none.optimize_logical(&plan).unwrap();
    assert_eq!(result.root.plan_type, PlanType::Scan);
    
    // Level 1 and 2 should optimize
    let result = optimizer_basic.optimize_logical(&plan).unwrap();
    assert_eq!(result.root.plan_type, PlanType::Scan);
    
    let result = optimizer_aggressive.optimize_logical(&plan).unwrap();
    assert_eq!(result.root.plan_type, PlanType::Scan);
}

#[test]
fn test_cost_estimate_incremental() {
    let model = CostModel::new();
    
    let scan = SeqScanOperator {
        schema: "users".to_string(),
        estimated_rows: 1000,
    };
    
    let cost = model.estimate_cost(&PhysicalOperator::SeqScan(scan));
    let incremental = cost.incremental_cost();
    
    assert_eq!(incremental, cost.total_cost - cost.startup_cost);
}

#[test]
fn test_optimizer_with_real_query() -> OQLResult<()> {
    // Create schema
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), FieldType::Integer);
    fields.insert("name".to_string(), FieldType::String);
    fields.insert("age".to_string(), FieldType::Integer);
    
    let schema = Schema::new("users".to_string(), fields);
    
    let mut registry = SchemaRegistry::new();
    registry.register(schema)?;
    
    // Create optimizer with statistics
    let stats = StatisticsManager::new();
    let optimizer = QueryOptimizer::with_statistics(stats);
    
    // Create a simple query plan
    let scan = PlanNode::new(PlanType::Scan)
        .with_schema("users".to_string());
    
    let filter = PlanNode::new(PlanType::Filter)
        .with_child(scan)
        .with_condition(FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["age".to_string()],
            }),
            operator: ComparisonOperator::GreaterThan,
            right: Expression::Literal(Literal::Integer(30)),
        });
    
    let plan = LogicalPlan {
        root: filter,
        schema_registry: Some(registry),
    };
    
    // Optimize the plan
    let optimized = optimizer.optimize_logical(&plan)?;
    
    // Verify optimization succeeded
    assert_eq!(optimized.root.plan_type, PlanType::Filter);
    
    Ok(())
}

#[test]
fn test_complex_join_optimization() {
    let optimizer = QueryOptimizer::new();
    
    // Create three-table join scenario
    let mut users = PlanNode::new(PlanType::Scan)
        .with_schema("users".to_string());
    users.metadata.estimated_cardinality = Some(1000);
    
    let mut orders = PlanNode::new(PlanType::Scan)
        .with_schema("orders".to_string());
    orders.metadata.estimated_cardinality = Some(10000);
    
    let mut products = PlanNode::new(PlanType::Scan)
        .with_schema("products".to_string());
    products.metadata.estimated_cardinality = Some(100);
    
    // Join users and orders
    let join1 = PlanNode::new(PlanType::Join)
        .with_child(users)
        .with_child(orders);
    
    // Join result with products
    let join2 = PlanNode::new(PlanType::Join)
        .with_child(join1)
        .with_child(products);
    
    let plan = LogicalPlan {
        root: join2,
        schema_registry: None,
    };
    
    let optimized = optimizer.optimize_logical(&plan).unwrap();
    
    // Verify plan structure
    assert_eq!(optimized.root.plan_type, PlanType::Join);
}

#[test]
fn test_projection_with_filter() {
    let optimizer = QueryOptimizer::new();
    
    let scan = PlanNode::new(PlanType::Scan)
        .with_schema("users".to_string());
    
    let project = PlanNode::new(PlanType::Project)
        .with_child(scan)
        .with_projections(vec![
            ProjectionField {
                expression: Expression::FieldAccess(FieldAccess {
                    path: vec!["name".to_string()],
                }),
                alias: None,
                field_type: Some(FieldType::String),
            },
        ]);
    
    let filter = PlanNode::new(PlanType::Filter)
        .with_child(project)
        .with_condition(FilterCondition::Comparison {
            left: Expression::Literal(Literal::Integer(1)),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(1)),
        });
    
    let plan = LogicalPlan {
        root: filter,
        schema_registry: None,
    };
    
    let optimized = optimizer.optimize_logical(&plan).unwrap();
    
    // Verify optimization succeeded
    assert!(optimized.root.plan_type == PlanType::Filter || optimized.root.plan_type == PlanType::Project);
}
