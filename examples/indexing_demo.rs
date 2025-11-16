// OQL Database Indexing System Demo
// Demonstrates automatic index creation and query optimization

use owl_db::{
    Document, DocumentStore, IndexRangeType, OQLResult, Schema, FieldType, Value,
};
use owl_db::storage::RocksDBStore;
use std::collections::HashMap;
use tempfile::TempDir;

fn main() -> OQLResult<()> {
    println!("=== OQL Database Indexing System Demo ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new().unwrap();
    let store = RocksDBStore::new(temp_dir.path())?;
    
    // Create schema with various field types
    let mut fields = HashMap::new();
    fields.insert("employee_id".to_string(), FieldType::String);
    fields.insert("name".to_string(), FieldType::String);
    fields.insert("age".to_string(), FieldType::Integer);
    fields.insert("email".to_string(), FieldType::String);
    fields.insert("department".to_string(), FieldType::String);
    fields.insert("salary".to_string(), FieldType::Float);
    
    let schema = Schema::new("employees".to_string(), fields);
    
    // Store schema (this automatically creates indexes)
    store.put_schema(&schema)?;
    
    println!("1. Schema created with automatic indexes");
    store.with_index_manager(|index_manager| {
        let indexes = index_manager.list_indexes("employees");
        println!("   Created {} indexes:", indexes.len());
        for index in indexes {
            println!("   - {} on fields: {:?}", index.name, index.fields);
        }
        Ok(())
    })?;
    
    // Create sample employee documents
    let employees = vec![
        create_employee("emp_1", "E001", "Alice Johnson", 30, "alice@company.com", "Engineering", 75000.0),
        create_employee("emp_2", "E002", "Bob Smith", 25, "bob@company.com", "Engineering", 65000.0),
        create_employee("emp_3", "E003", "Carol Davis", 35, "carol@company.com", "Marketing", 70000.0),
        create_employee("emp_4", "E004", "David Wilson", 28, "david@company.com", "Engineering", 68000.0),
        create_employee("emp_5", "E005", "Eve Brown", 32, "eve@company.com", "Sales", 72000.0),
    ];
    
    // Insert documents (automatically indexed)
    println!("\n2. Inserting {} employee documents...", employees.len());
    for employee in &employees {
        store.put_document(employee)?;
    }
    println!("   All documents inserted and indexed");
    
    // Demonstrate index-based queries
    println!("\n3. Demonstrating index-based queries:");
    
    // Query by name (uses name index)
    println!("\n   Query: Find employee named 'Alice Johnson'");
    let alice_docs = store.query_with_index(
        "employees",
        &["name".to_string()],
        &[Value::String("Alice Johnson".to_string())],
    )?;
    println!("   Found {} employee(s):", alice_docs.len());
    for doc in &alice_docs {
        print_employee_summary(doc);
    }
    
    // Query by department (uses department index)
    println!("\n   Query: Find all Engineering employees");
    let eng_docs = store.query_with_index(
        "employees",
        &["department".to_string()],
        &[Value::String("Engineering".to_string())],
    )?;
    println!("   Found {} employee(s):", eng_docs.len());
    for doc in &eng_docs {
        print_employee_summary(doc);
    }
    
    // Create a composite index for more complex queries
    println!("\n4. Creating composite index on department + age");
    store.with_index_manager(|index_manager| {
        let composite_index = index_manager.create_index(
            "employees",
            vec!["department".to_string(), "age".to_string()],
            false,
            false,
        )?;
        println!("   Created composite index: {}", composite_index.name);
        Ok(())
    })?;
    
    // Re-index existing documents for the new composite index
    println!("   Re-indexing existing documents...");
    for employee in &employees {
        store.with_index_manager(|index_manager| {
            index_manager.index_document(employee)
        })?;
    }
    
    // Query using composite index
    println!("\n   Query: Find Engineering employees aged 25");
    let doc_ids = store.with_index_manager(|index_manager| {
        if let Some(composite_index) = index_manager.find_best_index(
            "employees", 
            &["department".to_string(), "age".to_string()]
        ) {
            let composite_index_clone = composite_index.clone();
            let doc_ids = index_manager.query_index(
                &composite_index_clone,
                &[
                    Value::String("Engineering".to_string()),
                    Value::Integer(25),
                ],
                IndexRangeType::Exact,
            )?;
            Ok(doc_ids)
        } else {
            Ok(Vec::new())
        }
    })?;
    
    println!("   Found {} employee(s) using composite index:", doc_ids.len());
    for doc_id in doc_ids {
        if let Some(doc) = store.get_document("employees", &doc_id)? {
            print_employee_summary(&doc);
        }
    }
    
    // Show index usage statistics
    println!("\n5. Index usage statistics:");
    store.with_index_manager(|index_manager| {
        let indexes = index_manager.list_indexes("employees");
        for index in indexes {
            if let Some(stats) = index_manager.get_usage_stats(&index.name) {
                println!("   {}: {} queries, {} hits, {} misses, {:.2}ms avg",
                    index.name,
                    stats.query_count,
                    stats.hit_count,
                    stats.miss_count,
                    stats.avg_scan_time_ms
                );
            }
        }
        Ok(())
    })?;
    
    // Demonstrate unique index
    println!("\n6. Creating unique index on employee_id field");
    store.with_index_manager(|index_manager| {
        // First drop the existing non-unique index
        let dropped = index_manager.drop_index("employees", "idx_employees_employee_id")?;
        if dropped {
            println!("   Dropped existing non-unique employee_id index");
        }
        
        // Create unique index
        let unique_index = index_manager.create_index(
            "employees",
            vec!["employee_id".to_string()],
            true, // unique
            false,
        )?;
        println!("   Created unique index: {}", unique_index.name);
        
        // Re-index existing documents
        for employee in &employees {
            index_manager.index_document(employee)?;
        }
        
        // Try to insert duplicate employee_id (should fail)
        let duplicate_employee = create_employee(
            "emp_6", 
            "E001", // Duplicate employee_id
            "Frank Miller", 
            40, 
            "frank@company.com",
            "HR", 
            80000.0
        );
        
        println!("   Attempting to insert employee with duplicate employee_id...");
        match index_manager.index_document(&duplicate_employee) {
            Ok(_) => println!("   ERROR: Duplicate was allowed!"),
            Err(e) => println!("   SUCCESS: Duplicate rejected - {}", e),
        }
        
        Ok(())
    })?;
    
    println!("\n=== Demo completed successfully! ===");
    Ok(())
}

fn create_employee(id: &str, employee_id: &str, name: &str, age: i64, email: &str, department: &str, salary: f64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("employee_id".to_string(), Value::String(employee_id.to_string()));
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("age".to_string(), Value::Integer(age));
    fields.insert("email".to_string(), Value::String(email.to_string()));
    fields.insert("department".to_string(), Value::String(department.to_string()));
    fields.insert("salary".to_string(), Value::Float(salary));
    
    Document::with_id(id.to_string(), "employees".to_string(), fields)
}

fn print_employee_summary(doc: &Document) {
    let name = match doc.get_field("name") {
        Some(Value::String(s)) => s.clone(),
        _ => "Unknown".to_string(),
    };
    let age = match doc.get_field("age") {
        Some(Value::Integer(i)) => i.to_string(),
        _ => "Unknown".to_string(),
    };
    let department = match doc.get_field("department") {
        Some(Value::String(s)) => s.clone(),
        _ => "Unknown".to_string(),
    };
    
    println!("     - {} (age {}, {})", name, age, department);
}