// Batch Insert Demo
// Demonstrates the performance and convenience of batch insert operations

use owl_db::create_oql_runner;
use std::fs;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=============================================================================");
    println!("BATCH INSERT DEMO");
    println!("=============================================================================\n");

    let db_path = "./batch_insert_demo_db";

    // Clean up
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }

    println!("📁 Creating database: {}\n", db_path);
    let runner = create_oql_runner(db_path)?;

    // Define schemas
    println!("📋 Defining schemas...");
    runner.execute_content(
        r#"
        schema Department {
            id: String,
            name: String,
            budget: Integer
        }
        
        schema Employee {
            id: String,
            name: String,
            email: String,
            department: Department,
            salary: Integer,
            active: Boolean
        }
        
        schema Project {
            id: String,
            title: String,
            lead: Employee,
            department: Department,
            budget: Integer,
            status: String
        }
    "#,
    )?;
    println!("✅ Schemas defined\n");

    println!("=============================================================================");
    println!("EXAMPLE 1: Basic Batch Insert");
    println!("=============================================================================\n");

    println!("📝 Inserting 3 departments in one operation...");
    let start = Instant::now();

    runner.execute_content(
        r#"
        Department.insert [
            { id: "eng", name: "Engineering", budget: 5000000 },
            { id: "sales", name: "Sales", budget: 3000000 },
            { id: "hr", name: "Human Resources", budget: 1500000 }
        ]
    "#,
    )?;

    let duration = start.elapsed();
    println!("✅ Inserted 3 departments in {:?}\n", duration);

    println!("=============================================================================");
    println!("EXAMPLE 2: Batch Insert with References");
    println!("=============================================================================\n");

    println!("📝 Inserting 5 employees with department references...");
    let start = Instant::now();

    runner.execute_content(r#"
        Employee.insert [
            { id: "emp1", name: "Alice Johnson", email: "alice@company.com", department: "eng", salary: 150000, active: true },
            { id: "emp2", name: "Bob Smith", email: "bob@company.com", department: "eng", salary: 140000, active: true },
            { id: "emp3", name: "Carol White", email: "carol@company.com", department: "sales", salary: 120000, active: true },
            { id: "emp4", name: "David Brown", email: "david@company.com", department: "sales", salary: 115000, active: true },
            { id: "emp5", name: "Eve Davis", email: "eve@company.com", department: "hr", salary: 95000, active: true }
        ]
    "#)?;

    let duration = start.elapsed();
    println!("✅ Inserted 5 employees in {:?}\n", duration);

    println!("=============================================================================");
    println!("EXAMPLE 3: Large Batch Insert (100 documents)");
    println!("=============================================================================\n");

    // Build a large batch insert
    let mut insert_stmt = String::from("Project.insert [\n");
    for i in 0..100 {
        if i > 0 {
            insert_stmt.push_str(",\n");
        }
        let dept = match i % 3 {
            0 => "eng",
            1 => "sales",
            _ => "hr",
        };
        let lead = format!("emp{}", (i % 5) + 1);
        let status = if i % 2 == 0 { "active" } else { "planning" };

        insert_stmt.push_str(&format!(
            "    {{ id: \"proj{:03}\", title: \"Project {}\", lead: \"{}\", department: \"{}\", budget: {}, status: \"{}\" }}",
            i, i, lead, dept, 100000 + (i * 10000), status
        ));
    }
    insert_stmt.push_str("\n]");

    println!("📝 Inserting 100 projects in one batch...");
    let start = Instant::now();

    runner.execute_content(&insert_stmt)?;

    let duration = start.elapsed();
    println!("✅ Inserted 100 projects in {:?}\n", duration);

    println!("=============================================================================");
    println!("QUERY RESULTS WITH CLEAN JSON");
    println!("=============================================================================\n");

    let executor = runner.executor();

    // Query 1: Count documents
    println!("📊 Document Counts:");
    println!("─────────────────────────────────────────────────────────────────────────────");

    let dept_query = owl_db::parse_oql("Department.count()")?;
    let dept_results = executor.execute_query(&dept_query)?;
    if let Some(doc) = dept_results.first() {
        let clean_json = doc.fields_to_clean_json();
        println!("Departments: {}", serde_json::to_string(&clean_json)?);
    }

    let emp_query = owl_db::parse_oql("Employee.count()")?;
    let emp_results = executor.execute_query(&emp_query)?;
    if let Some(doc) = emp_results.first() {
        let clean_json = doc.fields_to_clean_json();
        println!("Employees: {}", serde_json::to_string(&clean_json)?);
    }

    let proj_query = owl_db::parse_oql("Project.count()")?;
    let proj_results = executor.execute_query(&proj_query)?;
    if let Some(doc) = proj_results.first() {
        let clean_json = doc.fields_to_clean_json();
        println!("Projects: {}", serde_json::to_string(&clean_json)?);
    }

    println!();

    // Query 2: Sample employees with navigation
    println!("📊 Sample Employees (with department info):");
    println!("─────────────────────────────────────────────────────────────────────────────");

    let emp_query = owl_db::parse_oql(
        r#"
        Employee.limit(3).select {
            name,
            email,
            dept_name: .department.name,
            salary
        }
    "#,
    )?;

    let emp_results = executor.execute_query(&emp_query)?;
    for (i, emp) in emp_results.iter().enumerate() {
        let clean_json = emp.fields_to_clean_json();
        println!(
            "Employee {}:\n{}\n",
            i + 1,
            serde_json::to_string_pretty(&clean_json)?
        );
    }

    // Query 3: Sample projects
    println!("📊 Sample Projects:");
    println!("─────────────────────────────────────────────────────────────────────────────");

    let proj_query = owl_db::parse_oql(
        r#"
        Project.limit(5).select {
            title,
            lead_name: .lead.name,
            dept_name: .department.name,
            budget,
            status
        }
    "#,
    )?;

    let proj_results = executor.execute_query(&proj_query)?;
    for (i, proj) in proj_results.iter().enumerate() {
        let clean_json = proj.fields_to_clean_json();
        println!(
            "Project {}:\n{}\n",
            i + 1,
            serde_json::to_string_pretty(&clean_json)?
        );
    }

    println!("=============================================================================");
    println!("PERFORMANCE COMPARISON");
    println!("=============================================================================\n");

    println!("💡 Benefits of Batch Insert:");
    println!("   • Single transaction for all documents");
    println!("   • Reduced overhead per document");
    println!("   • Better throughput for large datasets");
    println!("   • Atomic operation (all or nothing)");
    println!("   • Cleaner, more readable code");
    println!();

    println!("📊 Syntax Comparison:");
    println!();
    println!("   Single Inserts (verbose):");
    println!("   ─────────────────────────");
    println!("   User.insert {{ id: \"u1\", name: \"Alice\" }}");
    println!("   User.insert {{ id: \"u2\", name: \"Bob\" }}");
    println!("   User.insert {{ id: \"u3\", name: \"Carol\" }}");
    println!();
    println!("   Batch Insert (clean):");
    println!("   ─────────────────────");
    println!("   User.insert [");
    println!("       {{ id: \"u1\", name: \"Alice\" }},");
    println!("       {{ id: \"u2\", name: \"Bob\" }},");
    println!("       {{ id: \"u3\", name: \"Carol\" }}");
    println!("   ]");
    println!();

    println!("=============================================================================");
    println!("✅ DEMO COMPLETE!");
    println!("=============================================================================\n");

    println!("Summary:");
    println!("  ✓ Inserted 3 departments");
    println!("  ✓ Inserted 5 employees with references");
    println!("  ✓ Inserted 100 projects in one batch");
    println!("  ✓ All with clean JSON output");
    println!("  ✓ Reference navigation works seamlessly");

    Ok(())
}
