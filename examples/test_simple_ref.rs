// Simple Reference Test Runner
// Diagnose reference navigation issues

use owl_db::create_oql_runner;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=============================================================================");
    println!("SIMPLE REFERENCE TEST");
    println!("=============================================================================\n");

    let db_path = "./test_ref_db";

    // Clean up
    if std::path::Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }

    println!("📁 Creating database: {}\n", db_path);
    let runner = create_oql_runner(db_path)?;

    // Read and execute the test file
    let oql_content = fs::read_to_string("test_simple_ref.oql")?;

    println!("📖 Executing test_simple_ref.oql\n");
    println!("=============================================================================");

    match runner.execute_content(&oql_content) {
        Ok(_) => {
            println!(
                "=============================================================================\n"
            );
            println!("✅ Test completed!");
        }
        Err(e) => {
            println!(
                "=============================================================================\n"
            );
            println!("❌ Error: {}", e);
            return Err(e);
        }
    }

    println!("\n=============================================================================");
    println!("DETAILED QUERY RESULTS");
    println!("=============================================================================\n");

    // Now let's query and show the actual data
    let executor = runner.executor();

    println!("📊 Query 1: All Users");
    println!("─────────────────────────────────────────────────────────────────────────────");
    let users_query = owl_db::parse_oql("User")?;
    let users = executor.execute_query(&users_query)?;
    for (i, user) in users.iter().enumerate() {
        let clean_json = user.fields_to_clean_json();
        let json = serde_json::to_string_pretty(&clean_json)?;
        println!("User {}:\n{}\n", i + 1, json);
    }

    println!("📊 Query 2: All Posts");
    println!("─────────────────────────────────────────────────────────────────────────────");
    let posts_query = owl_db::parse_oql("Post")?;
    let posts = executor.execute_query(&posts_query)?;
    for (i, post) in posts.iter().enumerate() {
        let clean_json = post.fields_to_clean_json();
        let json = serde_json::to_string_pretty(&clean_json)?;
        println!("Post {}:\n{}\n", i + 1, json);
    }

    println!("📊 Query 3: Posts with Author Navigation");
    println!("─────────────────────────────────────────────────────────────────────────────");
    let nav_query = owl_db::parse_oql(
        r#"
        Post.select {
            id,
            title,
            author_name: .author.name,
            author_email: .author.email
        }
    "#,
    )?;
    let nav_results = executor.execute_query(&nav_query)?;
    for (i, result) in nav_results.iter().enumerate() {
        let clean_json = result.fields_to_clean_json();
        let json = serde_json::to_string_pretty(&clean_json)?;
        println!("Result {}:\n{}\n", i + 1, json);
    }

    println!("=============================================================================");
    println!("ANALYSIS");
    println!("=============================================================================\n");

    // Check what's actually stored in the author field
    if let Some(post) = posts.first() {
        println!("🔍 Inspecting first post's 'author' field:");
        if let Some(author_value) = post.fields.get("author") {
            println!("   Internal Type: {:?}", author_value);
            println!(
                "   Raw JSON: {}",
                serde_json::to_string_pretty(author_value)?
            );
            println!(
                "   Clean JSON: {}",
                serde_json::to_string_pretty(&author_value.to_clean_json())?
            );
        } else {
            println!("   ❌ No 'author' field found!");
        }
    }

    println!("\n💡 What we need to check:");
    println!("   1. Is 'author' stored as a String or Reference?");
    println!("   2. Does the schema parser recognize 'User' as a reference type?");
    println!("   3. Does the insert convert 'u1' string to an ObjRef?");
    println!("   4. Does reference navigation work with the stored value?");

    Ok(())
}
