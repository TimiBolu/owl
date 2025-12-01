// Tests for physical query planner

use owl_db::parser::{
    ComparisonOperator, Condition, Expression, FieldAccess, Literal, OQLQuery, Operation,
    SelectBlock, SelectField, SortBlock, SortDirection, SortField, WhereBlock, GroupByBlock,
    InsertBlock, InsertField, UpdateBlock, UpdateField,
};
use owl_db::planner::logical::LogicalPlanner;
use owl_db::planner::operators::*;
use owl_db::planner::physical::{PhysicalPlanner, Statistics};
use owl_db::schema::{FieldType, ReferenceType, Schema, SchemaRegistry};
use std::collections::HashMap;

fn create_test_schema_registry() -> SchemaRegistry {
    let mut registry = SchemaRegistry::new();
    
    // Create users schema
    let mut users_fields = HashMap::new();
    users_fields.insert("name".to_string(), FieldType::String);
    users_fields.insert("age".to_string(), FieldType::Integer);
    users_fields.insert("email".to_string(), FieldType::String);
    users_fields.insert("active".to_string(), FieldType::Boolean);
    
    let users_schema = Schema::new("users".to_string(), users_fields);
    registry.register(users_schema).unwrap();
    
    // Create orders schema
    let mut orders_fields = HashMap::new();
    orders_fields.insert("order_id".to_string(), FieldType::String);
    orders_fields.insert("amount".to_string(), FieldType::Float);
    orders_fields.insert(
        "customer".to_string(),
        FieldType::Reference(ReferenceType::new("users".to_string())),
    );
    
    let orders_schema = Schema::new("orders".to_string(), orders_fields);
    registry.register(orders_schema).unwrap();
    
    registry
}

fn create_test_statistics() -> Statistics {
    let mut stats = Statistics::new();
    stats.set_table_size("users".to_string(), 10000);
    stats.set_table_size("orders".to_string(), 50000);
    stats.add_index("users".to_string(), "email".to_string());
    stats
}

#[test]
fn test_simple_seq_scan() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::SeqScan(op) => {
            assert_eq!(op.schema, "users");
            assert!(op.estimated_rows > 0);
        }
        _ => panic!("Expected SeqScan operator"),
    }
}

#[test]
fn test_filter_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::Where(WhereBlock {
            condition: Condition::Comparison {
                left: Expression::FieldAccess(FieldAccess {
                    path: vec!["age".to_string()],
                }),
                operator: ComparisonOperator::GreaterThan,
                right: Expression::Literal(Literal::Integer(18)),
            },
        })],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Filter(op) => {
            assert!(op.selectivity > 0.0 && op.selectivity <= 1.0);
            match &*op.input {
                PhysicalOperator::SeqScan(scan_op) => {
                    assert_eq!(scan_op.schema, "users");
                }
                _ => panic!("Expected SeqScan as input to Filter"),
            }
        }
        _ => panic!("Expected Filter operator"),
    }
}

#[test]
fn test_project_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::Select(SelectBlock {
            fields: vec![
                SelectField {
                    alias: Some("user_name".to_string()),
                    expression: Expression::FieldAccess(FieldAccess {
                        path: vec!["name".to_string()],
                    }),
                },
                SelectField {
                    alias: Some("user_age".to_string()),
                    expression: Expression::FieldAccess(FieldAccess {
                        path: vec!["age".to_string()],
                    }),
                },
            ],
        })],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Project(op) => {
            assert_eq!(op.projections.len(), 2);
            assert_eq!(op.projections[0].alias, Some("user_name".to_string()));
            assert_eq!(op.projections[1].alias, Some("user_age".to_string()));
        }
        _ => panic!("Expected Project operator"),
    }
}

#[test]
fn test_sort_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::Sort(SortBlock {
            fields: vec![SortField {
                expression: Expression::FieldAccess(FieldAccess {
                    path: vec!["age".to_string()],
                }),
                direction: SortDirection::Desc,
            }],
        })],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Sort(op) => {
            assert_eq!(op.sort_specs.len(), 1);
            assert_eq!(op.sort_specs[0].direction, SortDirection::Desc);
        }
        _ => panic!("Expected Sort operator"),
    }
}

#[test]
fn test_limit_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::Limit(10)],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Limit(op) => {
            assert_eq!(op.limit, 10);
        }
        _ => panic!("Expected Limit operator"),
    }
}

#[test]
fn test_group_by_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::GroupBy(GroupByBlock {
            fields: vec![FieldAccess {
                path: vec!["age".to_string()],
            }],
        })],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::GroupBy(op) => {
            assert_eq!(op.group_by_fields, vec!["age".to_string()]);
        }
        _ => panic!("Expected GroupBy operator"),
    }
}

#[test]
fn test_aggregate_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::Count],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Aggregate(op) => {
            assert_eq!(op.aggregates.len(), 1);
        }
        _ => panic!("Expected Aggregate operator"),
    }
}

#[test]
fn test_insert_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::Insert(InsertBlock {
            fields: vec![
                InsertField {
                    name: "name".to_string(),
                    value: Expression::Literal(Literal::String("Alice".to_string())),
                },
                InsertField {
                    name: "age".to_string(),
                    value: Expression::Literal(Literal::Integer(30)),
                },
            ],
        })],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Insert(op) => {
            assert_eq!(op.schema, "users");
            assert_eq!(op.field_updates.len(), 2);
        }
        _ => panic!("Expected Insert operator"),
    }
}

#[test]
fn test_update_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["name".to_string()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::String("Alice".to_string())),
                },
            }),
            Operation::Update(UpdateBlock {
                fields: vec![UpdateField {
                    name: "age".to_string(),
                    value: Expression::Literal(Literal::Integer(31)),
                }],
            }),
        ],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Update(op) => {
            assert_eq!(op.field_updates.len(), 1);
            match &*op.input {
                PhysicalOperator::Filter(_) => {}
                _ => panic!("Expected Filter as input to Update"),
            }
        }
        _ => panic!("Expected Update operator"),
    }
}

#[test]
fn test_delete_operator() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["age".to_string()],
                    }),
                    operator: ComparisonOperator::LessThan,
                    right: Expression::Literal(Literal::Integer(18)),
                },
            }),
            Operation::Delete,
        ],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::Delete(op) => {
            match &*op.input {
                PhysicalOperator::Filter(_) => {}
                _ => panic!("Expected Filter as input to Delete"),
            }
        }
        _ => panic!("Expected Delete operator"),
    }
}

#[test]
fn test_complex_query_plan() {
    let registry = create_test_schema_registry();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["age".to_string()],
                    }),
                    operator: ComparisonOperator::GreaterThan,
                    right: Expression::Literal(Literal::Integer(18)),
                },
            }),
            Operation::Select(SelectBlock {
                fields: vec![SelectField {
                    alias: Some("name".to_string()),
                    expression: Expression::FieldAccess(FieldAccess {
                        path: vec!["name".to_string()],
                    }),
                }],
            }),
            Operation::Sort(SortBlock {
                fields: vec![SortField {
                    expression: Expression::FieldAccess(FieldAccess {
                        path: vec!["name".to_string()],
                    }),
                    direction: SortDirection::Asc,
                }],
            }),
            Operation::Limit(10),
        ],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    // Verify the plan structure
    match physical_plan.root() {
        PhysicalOperator::Limit(limit_op) => {
            assert_eq!(limit_op.limit, 10);
            match &*limit_op.input {
                PhysicalOperator::Sort(sort_op) => {
                    match &*sort_op.input {
                        PhysicalOperator::Project(project_op) => {
                            match &*project_op.input {
                                PhysicalOperator::Filter(filter_op) => {
                                    match &*filter_op.input {
                                        PhysicalOperator::SeqScan(scan_op) => {
                                            assert_eq!(scan_op.schema, "users");
                                        }
                                        _ => panic!("Expected SeqScan at bottom"),
                                    }
                                }
                                _ => panic!("Expected Filter"),
                            }
                        }
                        _ => panic!("Expected Project"),
                    }
                }
                _ => panic!("Expected Sort"),
            }
        }
        _ => panic!("Expected Limit at top"),
    }
}

#[test]
fn test_cost_estimation() {
    let registry = create_test_schema_registry();
    let stats = create_test_statistics();
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry)
        .with_statistics(stats);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["age".to_string()],
                    }),
                    operator: ComparisonOperator::GreaterThan,
                    right: Expression::Literal(Literal::Integer(18)),
                },
            }),
            Operation::Sort(SortBlock {
                fields: vec![SortField {
                    expression: Expression::FieldAccess(FieldAccess {
                        path: vec!["name".to_string()],
                    }),
                    direction: SortDirection::Asc,
                }],
            }),
        ],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    let cost = physical_plan.estimate_cost();
    assert!(cost > 0.0);
    
    // Sort should be more expensive than just filter
    let simple_query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![Operation::Where(WhereBlock {
            condition: Condition::Comparison {
                left: Expression::FieldAccess(FieldAccess {
                    path: vec!["age".to_string()],
                }),
                operator: ComparisonOperator::GreaterThan,
                right: Expression::Literal(Literal::Integer(18)),
            },
        })],
    };
    
    let simple_logical_plan = logical_planner.plan(&simple_query).unwrap();
    let simple_physical_plan = physical_planner.plan(&simple_logical_plan).unwrap();
    let simple_cost = simple_physical_plan.estimate_cost();
    
    assert!(cost > simple_cost, "Sort should be more expensive than filter");
}

#[test]
fn test_operator_type_names() {
    let seq_scan = PhysicalOperator::SeqScan(SeqScanOperator {
        schema: "test".to_string(),
        estimated_rows: 100,
    });
    assert_eq!(seq_scan.operator_type(), "SeqScan");
    
    let filter = PhysicalOperator::Filter(FilterOperator {
        condition: owl_db::planner::types::FilterCondition::Comparison {
            left: Expression::Literal(Literal::Integer(1)),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(1)),
        },
        input: Box::new(seq_scan.clone()),
        selectivity: 0.5,
    });
    assert_eq!(filter.operator_type(), "Filter");
}

#[test]
fn test_row_estimation() {
    let seq_scan = PhysicalOperator::SeqScan(SeqScanOperator {
        schema: "test".to_string(),
        estimated_rows: 1000,
    });
    assert_eq!(seq_scan.estimate_rows(), 1000);
    
    let filter = PhysicalOperator::Filter(FilterOperator {
        condition: owl_db::planner::types::FilterCondition::Comparison {
            left: Expression::Literal(Literal::Integer(1)),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(1)),
        },
        input: Box::new(seq_scan),
        selectivity: 0.1,
    });
    assert_eq!(filter.estimate_rows(), 100); // 1000 * 0.1
    
    let limit = PhysicalOperator::Limit(LimitOperator {
        limit: 10,
        input: Box::new(filter),
    });
    assert_eq!(limit.estimate_rows(), 10);
}

#[test]
fn test_statistics_usage() {
    let registry = create_test_schema_registry();
    let mut stats = Statistics::new();
    stats.set_table_size("users".to_string(), 5000);
    
    let logical_planner = LogicalPlanner::with_schema_registry(registry.clone());
    let physical_planner = PhysicalPlanner::with_schema_registry(registry)
        .with_statistics(stats);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![],
    };
    
    let logical_plan = logical_planner.plan(&query).unwrap();
    let physical_plan = physical_planner.plan(&logical_plan).unwrap();
    
    match physical_plan.root() {
        PhysicalOperator::SeqScan(op) => {
            assert_eq!(op.estimated_rows, 5000);
        }
        _ => panic!("Expected SeqScan"),
    }
}
