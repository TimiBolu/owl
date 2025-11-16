// Simple test to verify Zig storage integration works

use owl_db::storage::{create_storage, DocumentStore};
use owl_db::document::{Document, DocumentId, Value};
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing Zig Storage Integration...\n");

    // Create storage
    println!("1. Creating Zig storage (10MB)...");
    let storage = create_storage("test_integration.owl", 10 * 1024 * 1024)?;
    println!("   ✓ Storage created successfully\n");

    // Create a test document
    println!("2. Creating test document...");
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Alice".to_string()));
    fields.insert("age".to_string(), Value::Integer(30));
    fields.insert("active".to_string(), Value::Boolean(true));

    let doc = Document::new_with_id(
        DocumentId::String("user1".to_string()),
        "users".to_string(),
        fields,
    );
    println!("   ✓ Document created\n");

    // Store document
    println!("3. Storing document...");
    storage.put_document(&doc)?;
    println!("   ✓ Document stored\n");

    // Retrieve document
    println!("4. Retrieving document...");
    let retrieved = storage.get_document("users", &doc.id)?;
    match retrieved {
        Some(d) => {
            println!("   ✓ Document retrieved:");
            println!("     - ID: {:?}", d.id);
            println!("     - Schema: {}", d.schema);
            println!("     - Fields: {} fields", d.fields.len());
        }
        None => {
            println!("   ✗ Document not found!");
            return Err("Document not found".into());
        }
    }

    // Scan documents
    println!("\n5. Scanning all documents in schema...");
    let docs = storage.scan_documents("users")?;
    println!("   ✓ Found {} document(s)\n", docs.len());

    // Delete document
    println!("6. Deleting document...");
    let deleted = storage.delete_document("users", &doc.id)?;
    if deleted {
        println!("   ✓ Document deleted\n");
    } else {
        println!("   ✗ Document not found for deletion\n");
    }

    // Verify deletion
    println!("7. Verifying deletion...");
    let retrieved_after_delete = storage.get_document("users", &doc.id)?;
    if retrieved_after_delete.is_none() {
        println!("   ✓ Document successfully deleted\n");
    } else {
        println!("   ✗ Document still exists!\n");
        return Err("Document was not deleted".into());
    }

    // Cleanup
    std::fs::remove_file("test_integration.owl").ok();

    println!("✅ All tests passed! Zig storage integration is working!\n");
    Ok(())
}
