// Physical Operator Tests
// Tests for iterator-based physical operators

use owl_db::document::{Document, Value};
use owl_db::error::OQLResult;
use owl_db::execution::operator::{PhysicalOperatorExec, PhysicalOperatorCollect};
use owl_db::execution::scan::{SeqScanExec, FilterExec, ProjectExec, SortExec, LimitExec};
use owl_db::execution::join::{NestedLoopJoinExec, HashJoinExec};
use owl_db::execution::aggregate::{GroupByExec, AggregateExec};
use owl_db::parser::{ComparisonOperator, Expression, FieldAccess, Literal};
use owl_db::planner::types::{AggregateFunction, AggregateSpec, FilterCondition, ProjectionField, SortSpec};
use owl_db::storage::create_storage;
use std::collections::HashMap;

fn create_test_document(schema: &str, id: &str, name: &str, age: i64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("age".to_string(), Value::Integer(age));
    
    Document::with_id(
        id.to_string(),
        schema.to_string(),
        fields,
    )
}

#[test]
fn test_seq_scan_operator() -> OQLResult<()> {
    let storage = create_storage("test_seq_scan_op.owl", 10 * 1024 * 1024)?;
    
    // Create test documents
    for i in 0..5 {
        let doc = create_test_document("users", &format!("user{}", i), &format!("User{}", i), 20 + i);
        storage.put_document(&doc)?;
    }
    
    // Create and execute scan
    let mut scan = SeqScanExec::new("users".to_string());
    scan.open(storage.clone())?;
    
    let results = scan.collect_all()?;
    assert_eq!(results.len(), 5);
    
    scan.close()?;
    std::fs::remove_file("test_seq_scan_op.owl").ok();
    Ok(())
}

#[test]
fn test_filter_operator() -> OQLResult<()> {
    let storage = create_storage("test_filter_op.owl", 10 * 1024 * 1024)?;
    
    // Create test documents
    for i in 0..10 {
        let doc = create_test_document("users", &format!("user{}", i), &format!("User{}", i), 20 + i);
        storage.put_document(&doc)?;
    }
    
    // Create filter: age > 25
    let condition = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["age".to_string()],
        }),
        operator: ComparisonOperator::GreaterThan,
        right: Expression::Literal(Literal::Integer(25)),
    };
    
    let input = Box::new(SeqScanExec::new("users".to_string()));
    let mut filter = FilterExec::new(condition, input);
    
    filter.open(storage.clone())?;
    let results = filter.collect_all()?;
    
    // Should have 4 users with age > 25 (26, 27, 28, 29)
    assert_eq!(results.len(), 4);
    
    filter.close()?;
    std::fs::remove_file("test_filter_op.owl").ok();
    Ok(())
}

#[test]
fn test_project_operator() -> OQLResult<()> {
    let storage = create_storage("test_project_op.owl", 10 * 1024 * 1024)?;
    
    // Create test document
    let doc = create_test_document("users", "user1", "Alice", 28);
    storage.put_document(&doc)?;
    
    // Create projection: select name only
    let projections = vec![
        ProjectionField {
            expression: Expression::FieldAccess(FieldAccess {
                path: vec!["name".to_string()],
            }),
            alias: None,
            field_type: None,
        },
    ];
    
    let input = Box::new(SeqScanExec::new("users".to_string()));
    let mut project = ProjectExec::new(projections, input);
    
    project.open(storage.clone())?;
    let results = project.collect_all()?;
    
    assert_eq!(results.len(), 1);
    assert!(results[0].fields.contains_key("name"));
    assert!(!results[0].fields.contains_key("age")); // age should not be projected
    
    project.close()?;
    std::fs::remove_file("test_project_op.owl").ok();
    Ok(())
}

#[test]
fn test_sort_operator() -> OQLResult<()> {
    let storage = create_storage("test_sort_op.owl", 10 * 1024 * 1024)?;
    
    // Create test documents in random order
    let ages = vec![25, 30, 20, 35, 22];
    for (i, age) in ages.iter().enumerate() {
        let doc = create_test_document("users", &format!("user{}", i), &format!("User{}", i), *age);
        storage.put_document(&doc)?;
    }
    
    // Create sort: order by age ascending
    let sort_specs = vec![
        SortSpec {
            expression: Expression::FieldAccess(FieldAccess {
                path: vec!["age".to_string()],
            }),
            direction: owl_db::parser::SortDirection::Asc,
        },
    ];
    
    let input = Box::new(SeqScanExec::new("users".to_string()));
    let mut sort = SortExec::new(sort_specs, input);
    
    sort.open(storage.clone())?;
    let results = sort.collect_all()?;
    
    // Check sorted order: 20, 22, 25, 30, 35
    assert_eq!(results.len(), 5);
    assert_eq!(results[0].fields.get("age"), Some(&Value::Integer(20)));
    assert_eq!(results[1].fields.get("age"), Some(&Value::Integer(22)));
    assert_eq!(results[2].fields.get("age"), Some(&Value::Integer(25)));
    assert_eq!(results[3].fields.get("age"), Some(&Value::Integer(30)));
    assert_eq!(results[4].fields.get("age"), Some(&Value::Integer(35)));
    
    sort.close()?;
    std::fs::remove_file("test_sort_op.owl").ok();
    Ok(())
}

#[test]
fn test_limit_operator() -> OQLResult<()> {
    let storage = create_storage("test_limit_op.owl", 10 * 1024 * 1024)?;
    
    // Create test documents
    for i in 0..10 {
        let doc = create_test_document("users", &format!("user{}", i), &format!("User{}", i), 20 + i);
        storage.put_document(&doc)?;
    }
    
    // Create limit: limit 3
    let input = Box::new(SeqScanExec::new("users".to_string()));
    let mut limit = LimitExec::new(3, input);
    
    limit.open(storage.clone())?;
    let results = limit.collect_all()?;
    
    assert_eq!(results.len(), 3);
    
    limit.close()?;
    std::fs::remove_file("test_limit_op.owl").ok();
    Ok(())
}

#[test]
fn test_aggregate_count_operator() -> OQLResult<()> {
    let storage = create_storage("test_agg_count_op.owl", 10 * 1024 * 1024)?;
    
    // Create test documents
    for i in 0..7 {
        let doc = create_test_document("users", &format!("user{}", i), &format!("User{}", i), 20 + i);
        storage.put_document(&doc)?;
    }
    
    // Create aggregate: count()
    let aggregates = vec![
        AggregateSpec {
            function: AggregateFunction::Count,
            field: None,
            alias: Some("total".to_string()),
        },
    ];
    
    let input = Box::new(SeqScanExec::new("users".to_string()));
    let mut agg = AggregateExec::new(aggregates, input);
    
    agg.open(storage.clone())?;
    let results = agg.collect_all()?;
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].fields.get("total"), Some(&Value::Integer(7)));
    
    agg.close()?;
    std::fs::remove_file("test_agg_count_op.owl").ok();
    Ok(())
}

#[test]
fn test_combined_operators() -> OQLResult<()> {
    let storage = create_storage("test_combined_op.owl", 10 * 1024 * 1024)?;
    
    // Create test documents
    for i in 0..10 {
        let doc = create_test_document("users", &format!("user{}", i), &format!("User{}", i), 20 + i);
        storage.put_document(&doc)?;
    }
    
    // Build pipeline: scan -> filter (age > 23) -> sort (age desc) -> limit 3
    let scan = Box::new(SeqScanExec::new("users".to_string()));
    
    let filter_condition = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["age".to_string()],
        }),
        operator: ComparisonOperator::GreaterThan,
        right: Expression::Literal(Literal::Integer(23)),
    };
    let filter = Box::new(FilterExec::new(filter_condition, scan));
    
    let sort_specs = vec![
        SortSpec {
            expression: Expression::FieldAccess(FieldAccess {
                path: vec!["age".to_string()],
            }),
            direction: owl_db::parser::SortDirection::Desc,
        },
    ];
    let sort = Box::new(SortExec::new(sort_specs, filter));
    
    let mut limit = LimitExec::new(3, sort);
    
    // Execute pipeline
    limit.open(storage.clone())?;
    let results = limit.collect_all()?;
    
    // Should get top 3 users with age > 23, sorted descending
    // Ages: 29, 28, 27
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].fields.get("age"), Some(&Value::Integer(29)));
    assert_eq!(results[1].fields.get("age"), Some(&Value::Integer(28)));
    assert_eq!(results[2].fields.get("age"), Some(&Value::Integer(27)));
    
    limit.close()?;
    std::fs::remove_file("test_combined_op.owl").ok();
    Ok(())
}

#[test]
fn test_operator_reset() -> OQLResult<()> {
    let storage = create_storage("test_op_reset.owl", 10 * 1024 * 1024)?;
    
    // Create test documents
    for i in 0..3 {
        let doc = create_test_document("users", &format!("user{}", i), &format!("User{}", i), 20 + i);
        storage.put_document(&doc)?;
    }
    
    // Create scan
    let mut scan = SeqScanExec::new("users".to_string());
    scan.open(storage.clone())?;
    
    // First iteration
    let results1 = scan.collect_all()?;
    assert_eq!(results1.len(), 3);
    
    // Reset and iterate again
    scan.reset()?;
    let results2 = scan.collect_all()?;
    assert_eq!(results2.len(), 3);
    
    scan.close()?;
    std::fs::remove_file("test_op_reset.owl").ok();
    Ok(())
}
