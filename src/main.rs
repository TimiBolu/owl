// src/main.rs - OQL Database System
use owl_db::{execute_oql_file_simple, OQLRunner};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() >= 2 {
        // Execute OQL file from command line argument
        let file_path = &args[1];
        println!("🦉 Executing OQL file: {}", file_path);

        execute_oql_file_simple(file_path)?;
    } else {
        // Run embedded OQL demonstration
        println!("🦉 OQL Database System - Interactive Demo");
        println!("=========================================");

        // Create OQL runner with temporary database
        use owl_db::storage::RustStore;
        let store = std::sync::Arc::new(RustStore::new("./demo_db", 1024 * 1024 * 100)?);
        let runner = OQLRunner::new(store);

        // Execute embedded OQL content
        let demo_oql = r#"
            // Schema definitions
            schema users {
                id: String,
                name: String,
                email: String,
                age: Integer,
                created_at: String
            }
            
            schema orders {
                id: String,
                user_id: String,
                total: Float,
                status: String,
                created_at: String
            }
            
            // Insert sample data
            users.insert {
                id: "user_001",
                name: "Alice Smith",
                email: "alice@example.com",
                age: 28
            }
            
            users.insert {
                id: "user_002", 
                name: "Bob Jones",
                email: "bob@example.com",
                age: 35
            }
            
            orders.insert {
                id: "order_001",
                user_id: "user_001",
                total: 299.99,
                status: "completed"
            }
            
            orders.insert {
                id: "order_002",
                user_id: "user_002", 
                total: 150.00,
                status: "pending"
            }
            
            // Query the data
            users
            
            users.where { .age > 30 }
            
            users.where { .name == "Alice Smith" }
            
            orders.where { .status == "pending" }
            
            // Update operations
            users.where { .id == "user_001" }.update {
                age: 29
            }
            
            orders.where { .id == "order_002" }.update {
                status: "shipped"
            }
            
            // Final queries to see changes
            users.where { .id == "user_001" }
            
            orders.where { .status == "shipped" }
        "#;

        println!("\nExecuting embedded OQL demonstration...\n");
        runner.execute_content(demo_oql)?;

        // Cleanup
        std::fs::remove_dir_all("./demo_db").ok();

        println!("\n🎉 Demo completed successfully!");
        println!("\nTo run OQL files:");
        println!("  {} <oql_file>", args[0]);
        println!("  {} oql_test_suite/01_basic/crud_operations.oql", args[0]);
    }

    Ok(())
}
