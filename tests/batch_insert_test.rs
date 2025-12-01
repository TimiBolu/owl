use owl_db::{create_oql_runner, parse_oql};
use std::fs;

#[test]
fn test_insert_many_basic() {
    let db_path = "./test_insert_many_db";
    
    // Clean up
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }
    
    let runner = create_oql_runner(db_path).unwrap();
    
    // Define schema
    runner.execute_content(r#"
        schema User {
            id: String,
            name: String,
            age: Integer,
            active: Boolean
        }
    "#).unwrap();
    
    // Insert multiple users in one operation
    let result = runner.execute_content(r#"
        User.insert [
            { id: "user_001", name: "Alice", age: 28, active: true },
            { id: "user_002", name: "Bob", age: 35, active: true },
            { id: "user_003", name: "Carol", age: 42, active: false }
        ]
    "#);
    
    assert!(result.is_ok(), "Batch insert should succeed");
    
    // Verify all documents were inserted
    let executor = runner.executor();
    let query = parse_oql("User").unwrap();
    let results = executor.execute_query(&query).unwrap();
    
    assert_eq!(results.len(), 3, "Should have 3 users");
    
    // Verify individual documents
    let names: Vec<String> = results.iter()
        .filter_map(|doc| doc.fields.get("name"))
        .filter_map(|v| v.as_string())
        .map(|s| s.to_string())
        .collect();
    
    assert!(names.contains(&"Alice".to_string()));
    assert!(names.contains(&"Bob".to_string()));
    assert!(names.contains(&"Carol".to_string()));
    
    // Clean up
    fs::remove_file(db_path).ok();
}

#[test]
fn test_insert_many_with_references() {
    let db_path = "./test_insert_many_ref_db";
    
    // Clean up
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }
    
    let runner = create_oql_runner(db_path).unwrap();
    
    // Define schemas
    runner.execute_content(r#"
        schema Department {
            id: String,
            name: String
        }
        
        schema Employee {
            id: String,
            name: String,
            department: Department
        }
    "#).unwrap();
    
    // Insert departments
    runner.execute_content(r#"
        Department.insert [
            { id: "eng", name: "Engineering" },
            { id: "sales", name: "Sales" }
        ]
    "#).unwrap();
    
    // Insert employees with references
    let result = runner.execute_content(r#"
        Employee.insert [
            { id: "emp1", name: "Alice", department: "eng" },
            { id: "emp2", name: "Bob", department: "sales" },
            { id: "emp3", name: "Carol", department: "eng" }
        ]
    "#);
    
    assert!(result.is_ok(), "Batch insert with references should succeed");
    
    // Verify with navigation
    let executor = runner.executor();
    let query = parse_oql(r#"
        Employee.select {
            name,
            dept_name: .department.name
        }
    "#).unwrap();
    
    let results = executor.execute_query(&query).unwrap();
    assert_eq!(results.len(), 3, "Should have 3 employees");
    
    // Clean up
    fs::remove_file(db_path).ok();
}

#[test]
fn test_insert_many_empty_array() {
    let db_path = "./test_insert_many_empty_db";
    
    // Clean up
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }
    
    let runner = create_oql_runner(db_path).unwrap();
    
    // Define schema
    runner.execute_content(r#"
        schema User {
            id: String,
            name: String
        }
    "#).unwrap();
    
    // Try to insert empty array - should fail
    let result = runner.execute_content(r#"
        User.insert []
    "#);
    
    assert!(result.is_err(), "Empty array should fail");
    
    // Clean up
    fs::remove_file(db_path).ok();
}

#[test]
fn test_insert_many_large_batch() {
    let db_path = "./test_insert_many_large_db";
    
    // Clean up
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }
    
    let runner = create_oql_runner(db_path).unwrap();
    
    // Define schema
    runner.execute_content(r#"
        schema User {
            id: String,
            name: String,
            index: Integer
        }
    "#).unwrap();
    
    // Build a large batch insert (100 documents)
    let mut insert_stmt = String::from("User.insert [\n");
    for i in 0..100 {
        if i > 0 {
            insert_stmt.push_str(",\n");
        }
        insert_stmt.push_str(&format!(
            "    {{ id: \"user_{:03}\", name: \"User {}\", index: {} }}",
            i, i, i
        ));
    }
    insert_stmt.push_str("\n]");
    
    let result = runner.execute_content(&insert_stmt);
    assert!(result.is_ok(), "Large batch insert should succeed");
    
    // Verify count
    let executor = runner.executor();
    let query = parse_oql("User.count()").unwrap();
    let results = executor.execute_query(&query).unwrap();
    
    if let Some(doc) = results.first() {
        if let Some(count_value) = doc.fields.get("count") {
            if let Some(count) = count_value.as_integer() {
                assert_eq!(count, 100, "Should have 100 users");
            }
        }
    }
    
    // Clean up
    fs::remove_file(db_path).ok();
}

#[test]
fn test_insert_single_vs_insert_many() {
    let db_path1 = "./test_insert_single_db";
    let db_path2 = "./test_insert_many_db2";
    
    // Clean up
    if std::path::Path::new(db_path1).exists() {
        fs::remove_file(db_path1).ok();
    }
    if std::path::Path::new(db_path2).exists() {
        fs::remove_file(db_path2).ok();
    }
    
    let runner1 = create_oql_runner(db_path1).unwrap();
    let runner2 = create_oql_runner(db_path2).unwrap();
    
    // Define schema in both
    let schema = r#"
        schema User {
            id: String,
            name: String
        }
    "#;
    
    runner1.execute_content(schema).unwrap();
    runner2.execute_content(schema).unwrap();
    
    // Single inserts
    runner1.execute_content(r#"User.insert { id: "u1", name: "Alice" }"#).unwrap();
    runner1.execute_content(r#"User.insert { id: "u2", name: "Bob" }"#).unwrap();
    runner1.execute_content(r#"User.insert { id: "u3", name: "Carol" }"#).unwrap();
    
    // Batch insert
    runner2.execute_content(r#"
        User.insert [
            { id: "u1", name: "Alice" },
            { id: "u2", name: "Bob" },
            { id: "u3", name: "Carol" }
        ]
    "#).unwrap();
    
    // Both should have same result
    let query = parse_oql("User").unwrap();
    
    let results1 = runner1.executor().execute_query(&query).unwrap();
    let results2 = runner2.executor().execute_query(&query).unwrap();
    
    assert_eq!(results1.len(), results2.len(), "Both should have same count");
    assert_eq!(results1.len(), 3, "Should have 3 users");
    
    // Clean up
    fs::remove_file(db_path1).ok();
    fs::remove_file(db_path2).ok();
}
