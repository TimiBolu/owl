// OQL Streaming and Pagination Demo
// Demonstrates memory-efficient processing of large datasets

use owl_db::{
    Document, DocumentStore, FieldType, OQLResult, QueryExecutor, Schema, Value,
    parse_oql, StreamConfig, StreamCursor, BatchProcessor,
};
use owl_db::storage::RocksDBStore;
use std::collections::HashMap;
use tempfile::TempDir;

fn main() -> OQLResult<()> {
    println!("=== OQL Streaming and Pagination Demo ===\n");
    
    // Create temporary database
    let temp_dir = TempDir::new().unwrap();
    let store = RocksDBStore::new(temp_dir.path())?;
    let executor = QueryExecutor::new(std::sync::Arc::new(store));
    
    // Create schema
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), FieldType::String);
    fields.insert("age".to_string(), FieldType::Integer);
    fields.insert("department".to_string(), FieldType::String);
    fields.insert("salary".to_string(), FieldType::Float);
    fields.insert("employee_id".to_string(), FieldType::String);
    
    let schema = Schema::new("employees".to_string(), fields);
    executor.store().put_schema(&schema)?;
    
    // Create a large dataset for demonstration
    println!("1. Creating large dataset (1000 employees)...");
    let employees = create_large_employee_dataset(1000);
    
    println!("   Inserting {} employees in batches...", employees.len());
    for (i, employee) in employees.iter().enumerate() {
        executor.store().put_document(employee)?;
        
        if (i + 1) % 100 == 0 {
            println!("   Inserted {} employees", i + 1);
        }
    }
    
    println!("   Dataset creation completed!\n");
    
    // Demonstrate streaming queries
    println!("2. Demonstrating streaming queries:");
    
    let query_str = "employees.where { .department == \"Engineering\" }";
    println!("   Query: {}", query_str);
    
    let query = parse_oql(query_str).map_err(|e| owl_db::OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    
    // Stream with small batch size for demonstration
    let config = StreamConfig {
        batch_size: 50,
        limit: Some(200), // Limit to first 200 results
        cursor: None,
        prefetch: true,
        memory_limit: Some(10 * 1024 * 1024), // 10MB limit
    };
    
    println!("   Streaming with batch size: {}", config.batch_size);
    
    let mut stream = executor.execute_query_stream(&query, config)?;
    let mut count = 0;
    let mut engineering_employees = Vec::new();
    
    // Process stream in chunks
    while let Some(doc_result) = stream.next() {
        let doc = doc_result?;
        engineering_employees.push(doc);
        count += 1;
        
        if count % 25 == 0 {
            println!("     Processed {} documents so far...", count);
        }
        
        // Limit for demo purposes
        if count >= 100 {
            break;
        }
    }
    
    println!("   Streamed {} engineering employees", count);
    
    // Demonstrate pagination
    println!("\n3. Demonstrating pagination:");
    
    let page_size = 20;
    let mut current_cursor: Option<StreamCursor> = None;
    let mut page_number = 1;
    
    loop {
        println!("   Page {}: (page size: {})", page_number, page_size);
        
        let page_result = executor.execute_paginated_query(&query, page_size, current_cursor.clone())?;
        
        println!("     Documents in this page: {}", page_result.documents.len());
        println!("     Has next page: {}", page_result.page_info.has_next_page);
        println!("     Has previous page: {}", page_result.page_info.has_previous_page);
        
        // Show first few employees in this page
        for (i, doc) in page_result.documents.iter().take(3).enumerate() {
            let name = doc.get_field("name").unwrap_or(&Value::Null);
            let dept = doc.get_field("department").unwrap_or(&Value::Null);
            println!("       {}. {} ({})", i + 1, format_value(name), format_value(dept));
        }
        
        if page_result.documents.len() > 3 {
            println!("       ... and {} more", page_result.documents.len() - 3);
        }
        
        // Move to next page
        if let Some(next_cursor) = page_result.next_cursor {
            current_cursor = Some(next_cursor);
            page_number += 1;
            
            // Limit pages for demo
            if page_number > 3 {
                println!("   ... (stopping at page 3 for demo)");
                break;
            }
        } else {
            println!("   No more pages available");
            break;
        }
    }
    
    // Demonstrate cursor encoding/decoding
    println!("\n4. Demonstrating cursor persistence:");
    
    if let Some(cursor) = current_cursor {
        let encoded_cursor = cursor.encode()?;
        println!("   Encoded cursor: {}...", &encoded_cursor[..50]);
        
        let decoded_cursor = StreamCursor::decode(&encoded_cursor)?;
        println!("   Decoded cursor position: {}", decoded_cursor.position);
        println!("   Cursor schema: {}", decoded_cursor.schema);
        println!("   Cursor created at: {}", decoded_cursor.created_at);
    }
    
    // Demonstrate batch processing
    println!("\n5. Demonstrating batch processing:");
    
    let batch_processor = BatchProcessor::new(executor.store().clone(), 100);
    let all_employees_query = parse_oql("employees").map_err(|e| owl_db::OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    
    let mut total_salary = 0.0;
    let mut employee_count = 0;
    let mut department_counts = HashMap::new();
    
    println!("   Processing all employees in batches of 100...");
    
    batch_processor.process_batches(all_employees_query, |batch| {
        println!("     Processing batch of {} employees", batch.len());
        
        for doc in batch {
            employee_count += 1;
            
            if let Some(Value::Float(salary)) = doc.get_field("salary") {
                total_salary += salary;
            }
            
            if let Some(Value::String(dept)) = doc.get_field("department") {
                *department_counts.entry(dept.clone()).or_insert(0) += 1;
            }
        }
        
        Ok(())
    })?;
    
    println!("   Batch processing completed!");
    println!("   Total employees processed: {}", employee_count);
    println!("   Average salary: ${:.2}", total_salary / employee_count as f64);
    println!("   Department distribution:");
    
    for (dept, count) in department_counts {
        println!("     {}: {} employees", dept, count);
    }
    
    // Demonstrate memory-efficient transformations
    println!("\n6. Demonstrating memory-efficient transformations:");
    
    let transform_query = parse_oql("employees.where { .salary > 75000 }").map_err(|e| owl_db::OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    
    println!("   Transforming high-salary employees to summary format...");
    
    let summaries = batch_processor.transform_batches(transform_query, |doc| {
        let name = match doc.get_field("name") {
            Some(Value::String(s)) => s.clone(),
            _ => "Unknown".to_string(),
        };
        
        let salary = match doc.get_field("salary") {
            Some(Value::Float(f)) => *f,
            _ => 0.0,
        };
        
        let department = match doc.get_field("department") {
            Some(Value::String(s)) => s.clone(),
            _ => "Unknown".to_string(),
        };
        
        Ok(format!("{} (${:.0}) - {}", name, salary, department))
    })?;
    
    println!("   Transformed {} high-salary employees:", summaries.len());
    for (i, summary) in summaries.iter().take(5).enumerate() {
        println!("     {}. {}", i + 1, summary);
    }
    
    if summaries.len() > 5 {
        println!("     ... and {} more", summaries.len() - 5);
    }
    
    // Performance comparison
    println!("\n7. Performance benefits:");
    println!("   ✓ Streaming: Process datasets larger than available memory");
    println!("   ✓ Pagination: Efficient navigation through large result sets");
    println!("   ✓ Batch processing: Optimal memory usage for bulk operations");
    println!("   ✓ Cursor-based pagination: Consistent results even with concurrent modifications");
    println!("   ✓ Memory limits: Prevent out-of-memory errors with large datasets");
    
    println!("\n=== Streaming and Pagination Demo completed! ===");
    Ok(())
}

fn create_large_employee_dataset(count: usize) -> Vec<Document> {
    let departments = vec!["Engineering", "Marketing", "Sales", "HR", "Finance"];
    let first_names = vec!["Alice", "Bob", "Carol", "David", "Eve", "Frank", "Grace", "Henry", "Ivy", "Jack"];
    let last_names = vec!["Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis", "Rodriguez", "Martinez"];
    
    (0..count)
        .map(|i| {
            let first_name = &first_names[i % first_names.len()];
            let last_name = &last_names[(i / first_names.len()) % last_names.len()];
            let name = format!("{} {}", first_name, last_name);
            
            let department = departments[i % departments.len()].to_string();
            let age = 22 + (i % 40) as i64; // Ages 22-61
            let base_salary = 50000.0 + (i % 50000) as f64; // Salaries 50k-100k
            let employee_id = format!("EMP{:06}", i + 1);
            
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String(name));
            fields.insert("age".to_string(), Value::Integer(age));
            fields.insert("department".to_string(), Value::String(department));
            fields.insert("salary".to_string(), Value::Float(base_salary));
            fields.insert("employee_id".to_string(), Value::String(employee_id));
            
            Document::with_id(format!("emp_{}", i + 1), "employees".to_string(), fields)
        })
        .collect()
}

fn format_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => format!("{:.2}", f),
        Value::Boolean(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => "unknown".to_string(),
    }
}