// Index Scan Operator Tests
// Tests for index-based query execution

use owl_db::{
    execution::IndexScanExec,
    parser::{ComparisonOperator, Expression, FieldAccess, Literal},
    planner::types::FilterCondition,
};

#[test]
fn test_index_scan_creation() {
    let condition = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["id".to_string()],
        }),
        operator: ComparisonOperator::Equal,
        right: Expression::Literal(Literal::Integer(1)),
    };
    
    let scan = IndexScanExec::new(
        "users".to_string(),
        "idx_users_id".to_string(),
        vec![condition],
    );
    
    // Just verify it was created successfully
    drop(scan);
}

#[test]
fn test_extract_key_range_equality() {
    let condition = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["id".to_string()],
        }),
        operator: ComparisonOperator::Equal,
        right: Expression::Literal(Literal::Integer(42)),
    };
    
    let scan = IndexScanExec::new(
        "users".to_string(),
        "idx_users_id".to_string(),
        vec![condition],
    );
    
    let (start, end) = scan.extract_key_range().unwrap();
    assert_eq!(start, 42);
    assert_eq!(end, 42);
}

#[test]
fn test_extract_key_range_less_than() {
    let condition = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["age".to_string()],
        }),
        operator: ComparisonOperator::LessThan,
        right: Expression::Literal(Literal::Integer(30)),
    };
    
    let scan = IndexScanExec::new(
        "users".to_string(),
        "idx_users_age".to_string(),
        vec![condition],
    );
    
    let (start, end) = scan.extract_key_range().unwrap();
    assert_eq!(start, 0);
    assert_eq!(end, 29);
}

#[test]
fn test_extract_key_range_greater_equal() {
    let condition = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["score".to_string()],
        }),
        operator: ComparisonOperator::GreaterEqual,
        right: Expression::Literal(Literal::Integer(100)),
    };
    
    let scan = IndexScanExec::new(
        "users".to_string(),
        "idx_users_score".to_string(),
        vec![condition],
    );
    
    let (start, end) = scan.extract_key_range().unwrap();
    assert_eq!(start, 100);
    assert_eq!(end, u64::MAX);
}

#[test]
fn test_extract_key_range_no_conditions() {
    let scan = IndexScanExec::new(
        "users".to_string(),
        "idx_users_id".to_string(),
        vec![],
    );
    
    let (start, end) = scan.extract_key_range().unwrap();
    assert_eq!(start, 0);
    assert_eq!(end, u64::MAX);
}

#[test]
fn test_index_scan_multiple_conditions() {
    let condition1 = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["age".to_string()],
        }),
        operator: ComparisonOperator::GreaterEqual,
        right: Expression::Literal(Literal::Integer(25)),
    };
    
    let condition2 = FilterCondition::Comparison {
        left: Expression::FieldAccess(FieldAccess {
            path: vec!["age".to_string()],
        }),
        operator: ComparisonOperator::LessEqual,
        right: Expression::Literal(Literal::Integer(35)),
    };
    
    let scan = IndexScanExec::new(
        "users".to_string(),
        "idx_users_age".to_string(),
        vec![condition1, condition2],
    );
    
    // For now, we only use the first condition
    let (start, end) = scan.extract_key_range().unwrap();
    assert_eq!(start, 25);
    assert_eq!(end, u64::MAX);
}
