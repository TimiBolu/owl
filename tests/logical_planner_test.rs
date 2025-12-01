// Tests for logical query planner

use owl_db::document::Value;
use owl_db::error::OQLResult;
use owl_db::parser::{ComparisonOperator, Condition, Expression, FieldAccess, Literal, OQLQuery, Operation, SelectBlock, SelectField, SortBlock, SortDirection, SortField, WhereBlock, GroupByBlock, InsertBlock, InsertField, UpdateBlock, UpdateField};
use owl_db::planner::logical::{LogicalPlanner, LogicalPlan};
use owl_db::planner::types::{PlanType, AggregateFunction};
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
    users_fields.insert("created_at".to_string(), FieldType::DateTime);
    
    let users_schema = Schema::new("users".to_string(), users_fields);
    registry.register(users_schema).unwrap();
    
    // Create orders schema with reference to users
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

#[test]
fn test_simple_scan_plan() {
    let planner = LogicalPlanner::new();
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Scan);
    assert_eq!(plan.root().metadata.schema, Some("users".to_string()));
}

#[test]
fn test_where_filter_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
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
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Filter);
    assert!(plan.root().metadata.condition.is_some());
    assert_eq!(plan.root().children.len(), 1);
    assert_eq!(plan.root().children[0].plan_type, PlanType::Scan);
}

#[test]
fn test_select_projection_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Select(SelectBlock {
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
            }),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Project);
    assert_eq!(plan.root().metadata.projections.len(), 2);
    assert_eq!(plan.root().metadata.projections[0].alias, Some("user_name".to_string()));
    assert_eq!(plan.root().metadata.projections[1].alias, Some("user_age".to_string()));
}

#[test]
fn test_sort_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Sort(SortBlock {
                fields: vec![
                    SortField {
                        expression: Expression::FieldAccess(FieldAccess {
                            path: vec!["age".to_string()],
                        }),
                        direction: SortDirection::Desc,
                    },
                ],
            }),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Sort);
    assert_eq!(plan.root().metadata.sort_specs.len(), 1);
    assert_eq!(plan.root().metadata.sort_specs[0].direction, SortDirection::Desc);
}

#[test]
fn test_limit_plan() {
    let planner = LogicalPlanner::new();
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Limit(10),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Limit);
    assert_eq!(plan.root().metadata.limit, Some(10));
}

#[test]
fn test_group_by_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::GroupBy(GroupByBlock {
                fields: vec![
                    FieldAccess {
                        path: vec!["age".to_string()],
                    },
                ],
            }),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::GroupBy);
    assert_eq!(plan.root().metadata.group_by_fields, vec!["age".to_string()]);
}

#[test]
fn test_aggregate_count_plan() {
    let planner = LogicalPlanner::new();
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Count,
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Aggregate);
    assert_eq!(plan.root().metadata.aggregates.len(), 1);
    assert!(matches!(plan.root().metadata.aggregates[0].function, AggregateFunction::Count));
}

#[test]
fn test_aggregate_sum_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "orders".to_string(),
        operations: vec![
            Operation::Sum(FieldAccess {
                path: vec!["amount".to_string()],
            }),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Aggregate);
    assert_eq!(plan.root().metadata.aggregates.len(), 1);
    assert!(matches!(plan.root().metadata.aggregates[0].function, AggregateFunction::Sum));
}

#[test]
fn test_insert_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Insert(InsertBlock {
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
            }),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Insert);
    assert_eq!(plan.root().metadata.field_updates.len(), 2);
    assert!(plan.root().metadata.field_updates.contains_key("name"));
    assert!(plan.root().metadata.field_updates.contains_key("age"));
}

#[test]
fn test_update_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
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
                fields: vec![
                    UpdateField {
                        name: "age".to_string(),
                        value: Expression::Literal(Literal::Integer(31)),
                    },
                ],
            }),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Update);
    assert_eq!(plan.root().metadata.field_updates.len(), 1);
    assert!(plan.root().metadata.field_updates.contains_key("age"));
}

#[test]
fn test_delete_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
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
    
    let plan = planner.plan(&query).unwrap();
    
    assert_eq!(plan.root().plan_type, PlanType::Delete);
    assert_eq!(plan.root().children.len(), 1);
    assert_eq!(plan.root().children[0].plan_type, PlanType::Filter);
}

#[test]
fn test_complex_query_plan() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
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
                fields: vec![
                    SelectField {
                        alias: Some("name".to_string()),
                        expression: Expression::FieldAccess(FieldAccess {
                            path: vec!["name".to_string()],
                        }),
                    },
                ],
            }),
            Operation::Sort(SortBlock {
                fields: vec![
                    SortField {
                        expression: Expression::FieldAccess(FieldAccess {
                            path: vec!["name".to_string()],
                        }),
                        direction: SortDirection::Asc,
                    },
                ],
            }),
            Operation::Limit(10),
        ],
    };
    
    let plan = planner.plan(&query).unwrap();
    
    // Check the plan structure (operations are nested)
    assert_eq!(plan.root().plan_type, PlanType::Limit);
    assert_eq!(plan.root().children[0].plan_type, PlanType::Sort);
    assert_eq!(plan.root().children[0].children[0].plan_type, PlanType::Project);
    assert_eq!(plan.root().children[0].children[0].children[0].plan_type, PlanType::Filter);
    assert_eq!(plan.root().children[0].children[0].children[0].children[0].plan_type, PlanType::Scan);
}

#[test]
fn test_type_checking_invalid_comparison() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    // Try to use string operator on integer field
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["age".to_string()],
                    }),
                    operator: ComparisonOperator::Contains,
                    right: Expression::Literal(Literal::String("test".to_string())),
                },
            }),
        ],
    };
    
    let result = planner.plan(&query);
    assert!(result.is_err());
}

#[test]
fn test_type_checking_incompatible_types() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    // Try to compare integer with string
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["age".to_string()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::String("not a number".to_string())),
                },
            }),
        ],
    };
    
    let result = planner.plan(&query);
    assert!(result.is_err());
}

#[test]
fn test_schema_validation_missing_field() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    // Try to access non-existent field
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["nonexistent_field".to_string()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::String("value".to_string())),
                },
            }),
        ],
    };
    
    let result = planner.plan(&query);
    assert!(result.is_err());
}

#[test]
fn test_reference_navigation_type_inference() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    // Navigate through reference: orders.customer.name
    let query = OQLQuery {
        collection: "orders".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["customer".to_string(), "name".to_string()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::String("Alice".to_string())),
                },
            }),
        ],
    };
    
    let result = planner.plan(&query);
    assert!(result.is_ok());
}

#[test]
fn test_aggregate_on_non_numeric_field() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    // Try to sum a string field
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Sum(FieldAccess {
                path: vec!["name".to_string()],
            }),
        ],
    };
    
    let result = planner.plan(&query);
    assert!(result.is_err());
}

#[test]
fn test_sort_on_unsortable_type() {
    let registry = create_test_schema_registry();
    let planner = LogicalPlanner::with_schema_registry(registry);
    
    // Try to sort by boolean field (not sortable in our implementation)
    let query = OQLQuery {
        collection: "users".to_string(),
        operations: vec![
            Operation::Sort(SortBlock {
                fields: vec![
                    SortField {
                        expression: Expression::FieldAccess(FieldAccess {
                            path: vec!["active".to_string()],
                        }),
                        direction: SortDirection::Asc,
                    },
                ],
            }),
        ],
    };
    
    let result = planner.plan(&query);
    assert!(result.is_err());
}
