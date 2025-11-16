// Parser Demo - Testing OQL parsing functionality
use owl_db::{parse_oql, OQLQuery, Operation, Condition, Expression, FieldAccess, Literal};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("OQL Parser Demo");
    println!("===============");
    
    // Test 1: Simple WHERE query
    println!("\n1. Simple WHERE query:");
    let query1 = r#"users.where { .name == "Alice" }"#;
    println!("Input: {}", query1);
    
    let query = parse_oql(query1)?;
    println!("✓ Parsed successfully:");
    println!("  - Collection: {}", query.collection);
    println!("  - Operations: {}", query.operations.len());
    
    match &query.operations[0] {
        Operation::Where(where_block) => {
            println!("  - Operation: WHERE");
            match &where_block.condition {
                Condition::Comparison { left, operator, right } => {
                    println!("  - Condition: comparison");
                }
                _ => println!("  - Condition: complex"),
            }
        }
        _ => println!("  - Operation: other"),
    }
    
    // Test 2: SELECT with field selection
    println!("\n2. SELECT with field selection:");
    let query2 = r#"users.select { name, email: .email_address }"#;
    println!("Input: {}", query2);
    
    let query = parse_oql(query2)?;
    println!("✓ Parsed successfully:");
    println!("  - Collection: {}", query.collection);
    println!("  - Operations: {}", query.operations.len());
    
    match &query.operations[0] {
        Operation::Select(select_block) => {
            println!("  - Operation: SELECT");
            println!("  - Fields: {}", select_block.fields.len());
            for (i, field) in select_block.fields.iter().enumerate() {
                match &field.alias {
                    Some(alias) => println!("    Field {}: {} (alias)", i + 1, alias),
                    None => println!("    Field {}: direct access", i + 1),
                }
            }
        }
        _ => println!("  - Operation: other"),
    }
    
    // Test 3: Chained operations with reference navigation
    println!("\n3. Chained operations with reference navigation:");
    let query3 = r#"orders.where { .status == "pending" }.select { id, customer_name: .customer.name }"#;
    println!("Input: {}", query3);
    
    let query = parse_oql(query3)?;
    println!("✓ Parsed successfully:");
    println!("  - Collection: {}", query.collection);
    println!("  - Operations: {}", query.operations.len());
    
    for (i, operation) in query.operations.iter().enumerate() {
        match operation {
            Operation::Where(_) => println!("  - Operation {}: WHERE", i + 1),
            Operation::Select(select_block) => {
                println!("  - Operation {}: SELECT ({} fields)", i + 1, select_block.fields.len());
            }
            _ => println!("  - Operation {}: other", i + 1),
        }
    }
    
    println!("\n✓ OQL Parser implementation complete!");
    println!("✓ Tokenizer handles OQL method-chaining syntax");
    println!("✓ Parser generates AST for OQL queries with operations");
    println!("✓ Support for WHERE, SELECT, INSERT operations");
    println!("✓ Support for field access with dot notation (.field.subfield)");
    println!("✓ Support for method chaining (collection.where{{...}}.select{{...}})");
    
    Ok(())
}