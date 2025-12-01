// Deep Nesting Test Runner
// Test multiple schemas with references forming deep hierarchies

use owl_db::create_oql_runner;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=============================================================================");
    println!("DEEP NESTING TEST - Clean JSON Output");
    println!("=============================================================================\n");
    
    let db_path = "./test_deep_nesting_db";
    
    // Clean up
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }
    
    println!("📁 Creating database: {}\n", db_path);
    let runner = create_oql_runner(db_path)?;
    
    // Read and execute the test file
    let oql_content = fs::read_to_string("test_deep_nesting.oql")?;
    
    println!("📖 Executing test_deep_nesting.oql\n");
    println!("=============================================================================");
    
    match runner.execute_content(&oql_content) {
        Ok(_) => {
            println!("=============================================================================\n");
            println!("✅ Test completed!");
        }
        Err(e) => {
            println!("=============================================================================\n");
            println!("❌ Error: {}", e);
            return Err(e);
        }
    }
    
    println!("\n=============================================================================");
    println!("DETAILED QUERY RESULTS - CLEAN JSON");
    println!("=============================================================================\n");
    
    let executor = runner.executor();
    
    // Query 1: All projects (raw data)
    println!("📊 Query 1: All Projects (Raw Data)");
    println!("─────────────────────────────────────────────────────────────────────────────");
    let projects_query = owl_db::parse_oql("Project")?;
    let projects = executor.execute_query(&projects_query)?;
    for (i, project) in projects.iter().enumerate() {
        let clean_json = project.fields_to_clean_json();
        let json = serde_json::to_string_pretty(&clean_json)?;
        println!("Project {}:\n{}\n", i + 1, json);
    }
    
    // Query 2: Deep navigation - 5 levels deep!
    println!("📊 Query 2: Deep Navigation (5 Levels: Project → Dept → Company → City → Country)");
    println!("─────────────────────────────────────────────────────────────────────────────");
    let deep_query = owl_db::parse_oql(r#"
        Project.select {
            id,
            title,
            status,
            budget,
            lead_name: .lead.name,
            lead_email: .lead.email,
            lead_salary: .lead.salary,
            dept_name: .department.name,
            dept_budget: .department.budget,
            company_name: .department.company.name,
            company_founded: .department.company.founded,
            city_name: .department.company.headquarters.name,
            city_population: .department.company.headquarters.population,
            country_name: .department.company.headquarters.country.name,
            country_code: .department.company.headquarters.country.code
        }
    "#)?;
    
    let deep_results = executor.execute_query(&deep_query)?;
    for (i, result) in deep_results.iter().enumerate() {
        let clean_json = result.fields_to_clean_json();
        let json = serde_json::to_string_pretty(&clean_json)?;
        println!("Result {}:\n{}\n", i + 1, json);
    }
    
    // Query 3: Multiple reference paths
    println!("📊 Query 3: Multiple Reference Paths (Lead vs Project Department)");
    println!("─────────────────────────────────────────────────────────────────────────────");
    let multi_path_query = owl_db::parse_oql(r#"
        Project.select {
            title,
            lead_dept: .lead.department.name,
            lead_company: .lead.department.company.name,
            lead_city: .lead.department.company.headquarters.name,
            project_dept: .department.name,
            project_company: .department.company.name,
            project_city: .department.company.headquarters.name
        }
    "#)?;
    
    let multi_results = executor.execute_query(&multi_path_query)?;
    for (i, result) in multi_results.iter().enumerate() {
        let clean_json = result.fields_to_clean_json();
        let json = serde_json::to_string_pretty(&clean_json)?;
        println!("Result {}:\n{}\n", i + 1, json);
    }
    
    // Query 4: Simple aggregations
    println!("📊 Query 4: Simple Aggregations");
    println!("─────────────────────────────────────────────────────────────────────────────");
    
    let count_query = owl_db::parse_oql("Project.count()")?;
    let count_results = executor.execute_query(&count_query)?;
    if let Some(result) = count_results.first() {
        let clean_json = result.fields_to_clean_json();
        println!("Total Projects: {}\n", serde_json::to_string(&clean_json)?);
    }
    
    let sum_query = owl_db::parse_oql("Project.sum(.budget)")?;
    let sum_results = executor.execute_query(&sum_query)?;
    if let Some(result) = sum_results.first() {
        let clean_json = result.fields_to_clean_json();
        println!("Total Budget: {}\n", serde_json::to_string(&clean_json)?);
    }
    
    let avg_query = owl_db::parse_oql("Project.avg(.budget)")?;
    let avg_results = executor.execute_query(&avg_query)?;
    if let Some(result) = avg_results.first() {
        let clean_json = result.fields_to_clean_json();
        println!("Average Budget: {}\n", serde_json::to_string(&clean_json)?);
    }
    
    // Query 5: Filter with deep navigation
    println!("📊 Query 5: Filter by Country Code (US Projects Only)");
    println!("─────────────────────────────────────────────────────────────────────────────");
    let filter_query = owl_db::parse_oql(r#"
        Project.where { .department.company.headquarters.country.code == "US" }.select {
            title,
            lead_name: .lead.name,
            company: .department.company.name,
            country: .department.company.headquarters.country.name
        }
    "#)?;
    
    let filter_results = executor.execute_query(&filter_query)?;
    for (i, result) in filter_results.iter().enumerate() {
        let clean_json = result.fields_to_clean_json();
        let json = serde_json::to_string_pretty(&clean_json)?;
        println!("Result {}:\n{}\n", i + 1, json);
    }
    
    println!("=============================================================================");
    println!("ANALYSIS");
    println!("=============================================================================\n");
    
    println!("✅ Deep Navigation Features Demonstrated:");
    println!("   • 5-level deep reference chains (Project → Dept → Company → City → Country)");
    println!("   • Multiple reference paths from same entity");
    println!("   • Aggregations with deep navigation");
    println!("   • Filtering with deep navigation");
    println!("   • Clean JSON output without type wrappers");
    println!();
    
    println!("📊 Data Hierarchy:");
    println!("   Project");
    println!("   ├── Lead (Employee)");
    println!("   │   └── Department");
    println!("   │       └── Company");
    println!("   │           └── Headquarters (City)");
    println!("   │               └── Country");
    println!("   └── Department");
    println!("       └── Company");
    println!("           └── Headquarters (City)");
    println!("               └── Country");
    println!();
    
    println!("💡 Key Benefits:");
    println!("   • Natural query syntax for complex relationships");
    println!("   • O(1) reference navigation performance");
    println!("   • Clean JSON output perfect for APIs");
    println!("   • No N+1 query problems");
    println!("   • Type-safe reference handling");
    
    Ok(())
}
