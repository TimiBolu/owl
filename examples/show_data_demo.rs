// Visual Data Demonstration
// Shows exactly what data is written and retrieved in human-readable JSON format

use owl_db::create_oql_runner;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=============================================================================");
    println!("OQL DATA VISUALIZATION DEMO");
    println!("=============================================================================\n");

    let db_path = "./demo_visual_db";

    // Clean up any existing database
    if Path::new(db_path).exists() {
        fs::remove_file(db_path).ok();
    }

    println!("📁 Creating new database: {}\n", db_path);
    let runner = create_oql_runner(db_path)?;

    println!("=============================================================================");
    println!("STEP 1: DEFINE SCHEMA");
    println!("=============================================================================\n");

    let schema_oql = r#"
        schema users {
            id: String,
            name: String,
            email: String,
            age: Integer,
            active: Boolean
        }
        
        schema posts {
            id: String,
            author_id: Ref<users>,
            title: String,
            content: String,
            likes: Integer
        }
    "#;

    println!("📋 Schema Definition:");
    println!("{}", schema_oql);

    runner.execute_content(schema_oql)?;
    println!("✅ Schemas created!\n");

    println!("=============================================================================");
    println!("STEP 2: INSERT DATA");
    println!("=============================================================================\n");

    let insert_oql = r#"
        users.insert {
            id: "user_1",
            name: "Alice Johnson",
            email: "alice@example.com",
            age: 28,
            active: true
        }
        
        users.insert {
            id: "user_2",
            name: "Bob Smith",
            email: "bob@example.com",
            age: 35,
            active: true
        }
        
        users.insert {
            id: "user_3",
            name: "Carol White",
            email: "carol@example.com",
            age: 42,
            active: false
        }
        
        posts.insert {
            id: "post_1",
            author_id: "user_1",
            title: "My First Post",
            content: "Hello, this is my first post!",
            likes: 15
        }
        
        posts.insert {
            id: "post_2",
            author_id: "user_1",
            title: "Another Great Post",
            content: "Sharing more thoughts here.",
            likes: 23
        }
        
        posts.insert {
            id: "post_3",
            author_id: "user_2",
            title: "Bob's Thoughts",
            content: "Here are my thoughts on the topic.",
            likes: 8
        }
    "#;

    println!("📝 Inserting data...");
    runner.execute_content(insert_oql)?;
    println!("✅ Data inserted!\n");

    println!("=============================================================================");
    println!("STEP 3: RETRIEVE AND DISPLAY DATA");
    println!("=============================================================================\n");

    // Now let's retrieve and display the data in JSON format
    println!("📊 Query 1: All Users");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, "users")?;

    println!("\n📊 Query 2: All Posts");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, "posts")?;

    println!("\n📊 Query 3: Active Users Only");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, "users.where { .active == true }")?;

    println!("\n📊 Query 4: Users Older Than 30");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, "users.where { .age > 30 }")?;

    println!("\n📊 Query 5: Posts with Author Information (Reference Navigation)");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(
        &runner,
        r#"
        posts.select {
            post_id: id,
            title,
            author_name: .author_id.name,
            author_email: .author_id.email,
            likes
        }
    "#,
    )?;

    println!("\n📊 Query 6: Popular Posts (likes > 10)");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, "posts.where { .likes > 10 }")?;

    println!("\n📊 Query 7: Posts Sorted by Likes (Descending)");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, "posts.sort { .likes.desc }")?;

    println!("\n📊 Query 8: Top 2 Most Liked Posts");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, "posts.sort { .likes.desc }.limit(2)")?;

    println!("\n=============================================================================");
    println!("STEP 4: AGGREGATIONS");
    println!("=============================================================================\n");

    println!("📊 Total Users:");
    display_query_results(&runner, "users.count()")?;

    println!("\n📊 Total Posts:");
    display_query_results(&runner, "posts.count()")?;

    println!("\n📊 Average User Age:");
    display_query_results(&runner, "users.avg(.age)")?;

    println!("\n📊 Total Likes Across All Posts:");
    display_query_results(&runner, "posts.sum(.likes)")?;

    println!("\n📊 Average Likes Per Post:");
    display_query_results(&runner, "posts.avg(.likes)")?;

    println!("\n📊 Most Liked Post:");
    display_query_results(&runner, "posts.max(.likes)")?;

    println!("\n=============================================================================");
    println!("STEP 5: UPDATE DATA");
    println!("=============================================================================\n");

    println!("📝 Updating Alice's age to 29...");
    runner.execute_content(
        r#"
        users.where { .id == "user_1" }.update {
            age: 29
        }
    "#,
    )?;
    println!("✅ Updated!\n");

    println!("📊 Verify Update - Alice's Record:");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner, r#"users.where { .id == "user_1" }"#)?;

    println!("\n=============================================================================");
    println!("STEP 6: VERIFY DATA PERSISTENCE");
    println!("=============================================================================\n");

    // Close and reopen database
    drop(runner);
    println!("🔄 Closing database...");
    println!("🔄 Reopening database...\n");

    let runner2 = create_oql_runner(db_path)?;

    println!("📊 Data After Reopening - All Users:");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner2, "users")?;

    println!("\n📊 Data After Reopening - All Posts:");
    println!("─────────────────────────────────────────────────────────────────────────────");
    display_query_results(&runner2, "posts")?;

    println!("\n=============================================================================");
    println!("✅ DEMO COMPLETE!");
    println!("=============================================================================\n");

    println!("Summary:");
    println!("  ✓ Schemas defined and enforced");
    println!("  ✓ Data inserted with type validation");
    println!("  ✓ Queries executed with various filters");
    println!("  ✓ Reference navigation demonstrated");
    println!("  ✓ Aggregations calculated correctly");
    println!("  ✓ Updates applied successfully");
    println!("  ✓ Data persisted across sessions");
    println!();
    println!("💾 Database file: {}", db_path);
    println!("📊 All data shown in human-readable JSON format");

    Ok(())
}

fn display_query_results(
    runner: &owl_db::OQLRunner<owl_db::storage::zig_adapter::ZigStore>,
    query: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get the executor to access the store
    let executor = runner.executor();

    // Parse and execute the query
    let parsed = owl_db::parse_oql(query)?;
    let results = executor.execute_query(&parsed)?;

    // Display results as JSON
    if results.is_empty() {
        println!("  (no results)");
    } else {
        for (i, doc) in results.iter().enumerate() {
            // Convert document to JSON
            let json = serde_json::to_string_pretty(&doc.fields)?;
            println!("  Document {}:", i + 1);
            println!("  {}", json.replace("\n", "\n  "));
            if i < results.len() - 1 {
                println!();
            }
        }
        println!("\n  Total: {} document(s)", results.len());
    }

    Ok(())
}
