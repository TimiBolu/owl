// Execute OQL File Example
// Shows how to execute OQL files using existing parse_oql

use owl_db::{parse_oql, QueryExecutor};
use owl_db::storage::RocksDBStore;
use std::sync::Arc;
use std::fs;
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        println!("Usage: {} <oql_file>", args[0]);
        println!("Example: {} oql_test_suite/01_basic/crud_operations.oql", args[0]);
        return Ok(());
    }
    
    let file_path = &args[1];
    println!("🦉 Executing OQL file: {}", file_path);
    
    // Setup using existing utilities
    let store = Arc::new(RocksDBStore::new("./temp_db")?);
    let executor = QueryExecutor::new(store);
    
    // Read and execute file
    let content = fs::read_to_string(file_path)?;
    execute_oql_content(&executor, &content)?;
    
    // Cleanup
    std::fs::remove_dir_all("./temp_db").ok();
    
    Ok(())
}

fn execute_oql_content(executor: &QueryExecutor<RocksDBStore>, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    let statements = parse_oql_statements(content)?;
    let mut statement_count = 0;
    let mut success_count = 0;
    
    for statement in statements {
        statement_count += 1;
        
        // Handle schema definitions
        if statement.trim().starts_with("schema ") {
            println!("📋 Schema definition: {}", extract_schema_name(&statement));
            // TODO: Parse and create schema
            success_count += 1;
            continue;
        }
        
        // Skip empty statements
        if statement.trim().is_empty() {
            statement_count -= 1;
            continue;
        }
        
        // Execute OQL statement
        match parse_oql(&statement) {
            Ok(query) => {
                match executor.execute_query(&query) {
                    Ok(documents) => {
                        success_count += 1;
                        println!("✅ {} -> {} documents", 
                            statement.chars().take(50).collect::<String>().replace('\n', " "), 
                            documents.len()
                        );
                    }
                    Err(e) => {
                        println!("❌ Execution failed: {} -> {}", 
                            statement.chars().take(50).collect::<String>().replace('\n', " "), 
                            e
                        );
                    }
                }
            }
            Err(e) => {
                println!("❌ Parse failed: {} -> {}", 
                    statement.chars().take(50).collect::<String>().replace('\n', " "), 
                    e
                );
            }
        }
    }
    
    println!("\n📊 Summary: {}/{} statements executed successfully", success_count, statement_count);
    
    Ok(())
}

fn parse_oql_statements(content: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut statements = Vec::new();
    let mut current_statement = String::new();
    let mut brace_count = 0;
    let mut in_string = false;
    let mut escape_next = false;
    
    for line in content.lines() {
        let trimmed = line.trim();
        
        // Skip comments and empty lines when not building a statement
        if current_statement.is_empty() && (trimmed.is_empty() || trimmed.starts_with("//")) {
            continue;
        }
        
        // Add line to current statement
        if !current_statement.is_empty() {
            current_statement.push('\n');
        }
        current_statement.push_str(line);
        
        // Count braces to determine statement boundaries
        for ch in line.chars() {
            if escape_next {
                escape_next = false;
                continue;
            }
            
            match ch {
                '\\' if in_string => escape_next = true,
                '"' => in_string = !in_string,
                '{' if !in_string => brace_count += 1,
                '}' if !in_string => brace_count -= 1,
                _ => {}
            }
        }
        
        // Check if statement is complete
        let is_complete = if trimmed.starts_with("schema ") {
            // Schema statements end when braces are balanced
            brace_count == 0 && current_statement.contains('{')
        } else if current_statement.trim().contains('.') {
            // OQL statements end when braces are balanced or it's a simple query
            brace_count == 0 && (current_statement.contains('{') || !current_statement.contains('{'))
        } else {
            // Simple statements (like collection names) end immediately
            !trimmed.is_empty() && brace_count == 0
        };
        
        if is_complete {
            statements.push(current_statement.trim().to_string());
            current_statement.clear();
            brace_count = 0;
            in_string = false;
        }
    }
    
    // Add any remaining statement
    if !current_statement.trim().is_empty() {
        statements.push(current_statement.trim().to_string());
    }
    
    Ok(statements)
}

fn extract_schema_name(statement: &str) -> String {
    statement
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("unknown")
        .to_string()
}