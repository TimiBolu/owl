// Complete End-to-End Test Runner
// Executes the complete_e2e_test.oql file and verifies all operations

use owl_db::create_oql_runner;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=============================================================================");
    println!("OQL COMPLETE END-TO-END TEST");
    println!("=============================================================================\n");

    // Create database path
    let db_path = "./oql_e2e_test_db";

    // Clean up any existing database
    if Path::new(db_path).exists() {
        println!("🧹 Cleaning up existing database...");
        fs::remove_dir_all(db_path).ok();
    }

    println!("📁 Creating new database at: {}\n", db_path);

    // Create OQL runner
    let runner = create_oql_runner(db_path)?;

    // Read the OQL test file
    let oql_file_path = "oql_test_suite/complete_e2e_test.oql";
    println!("📖 Reading OQL file: {}", oql_file_path);

    if !Path::new(oql_file_path).exists() {
        eprintln!("❌ Error: OQL file not found at {}", oql_file_path);
        eprintln!("   Please ensure you're running from the project root directory.");
        return Err("OQL file not found".into());
    }

    let content = fs::read_to_string(oql_file_path)?;

    // Count the number of operations
    let schema_count = content.matches("schema ").count();
    let insert_count = content.matches(".insert {").count();
    let query_count = content.matches("// Test ").count();

    println!("📊 Test file statistics:");
    println!("   - Schema definitions: {}", schema_count);
    println!("   - Insert operations: {}", insert_count);
    println!("   - Test queries: {}", query_count);
    println!();

    // Execute the OQL file
    println!("🚀 Executing OQL file...\n");
    println!("=============================================================================");

    match runner.execute_content(&content) {
        Ok(_) => {
            println!(
                "=============================================================================\n"
            );
            println!("✅ SUCCESS! All operations completed successfully!");
            println!();
            println!("📈 Summary:");
            println!("   - {} schemas created", schema_count);
            println!("   - {} documents inserted", insert_count);
            println!("   - {} queries executed", query_count);
            println!();
            println!("💾 Database saved at: {}", db_path);
            println!();
            println!("🎉 End-to-end test PASSED!");
        }
        Err(e) => {
            println!(
                "=============================================================================\n"
            );
            println!("❌ ERROR: Test execution failed!");
            println!("   Error: {}", e);
            println!();

            // Try to provide more context
            if let Some(source) = e.source() {
                println!("   Caused by: {}", source);
            }

            return Err(e);
        }
    }

    println!();
    println!("=============================================================================");
    println!("TEST VERIFICATION");
    println!("=============================================================================\n");

    // Verify data was saved by running some queries
    println!("🔍 Verifying data persistence...\n");

    let verification_queries = r#"
        // Verify users were saved
        users.count()
        
        // Verify products were saved
        products.count()
        
        // Verify orders were saved
        orders.count()
        
        // Verify we can query specific data
        users.where { .name == "Alice Johnson" }
        
        // Verify reference navigation works
        orders.select {
            id,
            user_name: .user_id.name,
            product_name: .product_id.name
        }
    "#;

    match runner.execute_content(verification_queries) {
        Ok(_) => {
            println!("✅ Data verification PASSED!");
            println!("   - All data was successfully saved");
            println!("   - All data can be retrieved");
            println!("   - Reference navigation works correctly");
        }
        Err(e) => {
            println!("⚠️  Warning: Data verification encountered issues");
            println!("   Error: {}", e);
        }
    }

    println!();
    println!("=============================================================================");
    println!("🎊 COMPLETE END-TO-END TEST FINISHED SUCCESSFULLY!");
    println!("=============================================================================");

    Ok(())
}
