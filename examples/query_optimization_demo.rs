// OQL Query Optimization Engine Demo
// Demonstrates query planning, index recommendations, and result caching

use owl_db::{
    Document, DocumentStore, FieldType, OQLError, OQLResult, QueryExecutor, Schema, Value,
    parse_oql,
};
use owl_db::storage::RocksDBStore;
use std::collections::HashMap;
use tempfile::TempDir;

fn main() -> OQLResult<()> {
    println!("=== OQL Query Optimization Engine Demo ===\n");
    
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
    
    let schema = Schema::new("employees".to_string(), fields);
    executor.store().put_schema(&schema)?;
    
    // Create sample data
    let employees = vec![
        create_employee("emp_1", "Alice Johnson", 30, "Engineering", 75000.0),
        create_employee("emp_2", "Bob Smith", 25, "Engineering", 65000.0),
        create_employee("emp_3", "Carol Davis", 35, "Marketing", 70000.0),
        create_employee("emp_4", "David Wilson", 28, "Engineering", 68000.0),
        create_employee("emp_5", "Eve Brown", 32, "Sales", 72000.0),
    ];
    
    println!("1. Inserting sample employee data...");
    for employee in &employees {
        executor.store().put_document(employee)?;
    }
    println!("   Inserted {} employees", employees.len());
    
    // Demonstrate query optimization
    println!("\n2. Demonstrating query optimization:");
    
    // Parse some example queries
    let queries = vec![
        "employees.where { .name == \"Alice Johnson\" }",
        "employees.where { .department == \"Engineering\" }",
        "employees.where { .age > 30 }",
        "employees.where { .salary > 70000 }",
    ];
    
    for (i, query_str) in queries.iter().enumerate() {
        println!("\n   Query {}: {}", i + 1, query_str);
        
        // Parse the query
        let query = parse_oql(query_str).map_err(|e| OQLError::Generic {
            message: format!("Parse error: {}", e),
        })?;
        
        // Create query plan with optimization
        executor.store().with_index_manager(|index_manager| {
            let plan = executor.optimizer().create_query_plan(&query, index_manager)?;
            
            println!("     Estimated cost: {:.2}", plan.estimated_cost);
            println!("     Cacheable: {}", plan.cacheable);
            println!("     Index usage: {} indexes", plan.index_usage.len());
            
            for index_usage in &plan.index_usage {
                println!("       - Index: {} (fields: {:?})", 
                    index_usage.index.name, 
                    index_usage.index.fields
                );
                println!("         Operation: {:?}, Selectivity: {:.2}", 
                    index_usage.operation_type, 
                    index_usage.selectivity
                );
            }
            
            println!("     Optimizations applied: {}", plan.optimizations.len());
            for opt in &plan.optimizations {
                println!("       - {}: {} (improvement: {:.1}x)", 
                    format!("{:?}", opt.optimization_type),
                    opt.description,
                    opt.improvement_factor
                );
            }
            
            Ok(())
        })?;
    }
    
    // Execute queries and show caching behavior
    println!("\n3. Demonstrating query caching:");
    
    let test_query = "employees.where { .name == \"Alice Johnson\" }";
    println!("   Executing query: {}", test_query);
    
    // First execution (cache miss)
    let start = std::time::Instant::now();
    let query = parse_oql(test_query).map_err(|e| OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    let _results1 = executor.execute_query(&query)?;
    let first_duration = start.elapsed();
    println!("     First execution: {:?} (cache miss)", first_duration);
    
    // Second execution (should be cached, but our current implementation doesn't 
    // integrate caching with execute_query yet - this is a demonstration)
    let start = std::time::Instant::now();
    let _results2 = executor.execute_query(&query)?;
    let second_duration = start.elapsed();
    println!("     Second execution: {:?}", second_duration);
    
    // Show cache statistics
    let cache_stats = executor.optimizer().get_cache_stats()?;
    println!("     Cache stats: {} entries, {:.2}% hit rate", 
        cache_stats.total_entries, 
        cache_stats.hit_rate * 100.0
    );
    
    // Show query execution statistics
    println!("\n4. Query execution statistics:");
    let query_stats = executor.get_query_stats()?;
    
    for (query_hash, stats) in query_stats.iter().take(3) {
        println!("   Query: {}...", &query_hash[..50.min(query_hash.len())]);
        println!("     Executions: {}", stats.execution_count);
        println!("     Avg time: {:?}", stats.avg_execution_time);
        println!("     Documents scanned: {}", stats.documents_scanned);
        println!("     Documents returned: {}", stats.documents_returned);
        println!("     Index hits/misses: {}/{}", stats.index_hits, stats.index_misses);
    }
    
    // Show index recommendations
    println!("\n5. Index recommendations based on query patterns:");
    let recommendations = executor.get_index_recommendations()?;
    
    if recommendations.is_empty() {
        println!("   No recommendations yet (execute more queries to generate recommendations)");
    } else {
        for (i, rec) in recommendations.iter().enumerate() {
            println!("   Recommendation {}:", i + 1);
            println!("     Schema: {}", rec.schema);
            println!("     Fields: {:?}", rec.fields);
            println!("     Unique: {}", rec.unique);
            println!("     Benefit score: {:.2}", rec.benefit_score);
            println!("     Estimated usage: {}", rec.estimated_usage);
            println!("     Benefiting queries: {}", rec.benefiting_queries.len());
        }
    }
    
    // Demonstrate manual index creation based on recommendations
    println!("\n6. Creating indexes based on recommendations:");
    
    executor.store().with_index_manager(|index_manager| {
        // Create some manual indexes for demonstration (using fields that don't have auto-indexes)
        let salary_index = index_manager.create_index(
            "employees",
            vec!["salary".to_string()],
            false,
            false,
        )?;
        println!("   Created index: {}", salary_index.name);
        
        let dept_age_index = index_manager.create_index(
            "employees",
            vec!["department".to_string(), "age".to_string()],
            false,
            false,
        )?;
        println!("   Created composite index: {}", dept_age_index.name);
        
        Ok(())
    })?;
    
    // Re-run query planning with new indexes
    println!("\n7. Query planning with new indexes:");
    
    let complex_query = "employees.where { .salary > 65000 }";
    println!("   Query: {}", complex_query);
    
    let query = parse_oql(complex_query).map_err(|e| OQLError::Generic {
        message: format!("Parse error: {}", e),
    })?;
    executor.store().with_index_manager(|index_manager| {
        let plan = executor.optimizer().create_query_plan(&query, index_manager)?;
        
        println!("     Estimated cost: {:.2}", plan.estimated_cost);
        println!("     Index usage: {} indexes", plan.index_usage.len());
        
        for index_usage in &plan.index_usage {
            println!("       - Using index: {} for fields {:?}", 
                index_usage.index.name, 
                index_usage.query_fields
            );
        }
        
        Ok(())
    })?;
    
    // Show performance comparison
    println!("\n8. Performance comparison:");
    println!("   With proper indexes, queries can be 10-100x faster");
    println!("   Cache hits can be 1000x faster than disk access");
    println!("   Composite indexes enable efficient multi-field queries");
    
    println!("\n=== Query Optimization Demo completed! ===");
    Ok(())
}

fn create_employee(id: &str, name: &str, age: i64, department: &str, salary: f64) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("age".to_string(), Value::Integer(age));
    fields.insert("department".to_string(), Value::String(department.to_string()));
    fields.insert("salary".to_string(), Value::Float(salary));
    
    Document::with_id(id.to_string(), "employees".to_string(), fields)
}