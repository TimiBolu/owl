use owl_db::error::OQLResult;
use owl_db::schema::{Schema, FieldType, SchemaRegistry};
use owl_db::schema::evolution::{SchemaVersion, SchemaEvolution, CompatibilityLevel};
use owl_db::schema::migration::{Migration, MigrationPlan, MigrationExecutor};
use owl_db::storage::ZigStore;
use std::collections::HashMap;
use std::path::PathBuf;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "schema-migrate")]
#[command(about = "OQL Schema Migration Tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to the database
    #[arg(short, long, default_value = "./oql_data")]
    database: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    /// Check compatibility between two schema versions
    Check {
        /// Schema name
        schema: String,
        
        /// From version
        #[arg(short, long)]
        from: u32,
        
        /// To version
        #[arg(short, long)]
        to: u32,
    },
    
    /// Generate a migration plan
    Plan {
        /// Schema name
        schema: String,
        
        /// From version
        #[arg(short, long)]
        from: u32,
        
        /// To version
        #[arg(short, long)]
        to: u32,
        
        /// Output file for migration plan
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    
    /// Execute a migration
    Migrate {
        /// Schema name
        schema: String,
        
        /// Collection name
        collection: String,
        
        /// From version
        #[arg(short, long)]
        from: u32,
        
        /// To version
        #[arg(short, long)]
        to: u32,
        
        /// Dry run (don't apply changes)
        #[arg(long)]
        dry_run: bool,
    },
    
    /// List all schema versions
    List {
        /// Schema name (optional, lists all if not provided)
        schema: Option<String>,
    },
    
    /// Show schema version details
    Show {
        /// Schema name
        schema: String,
        
        /// Version number
        #[arg(short, long)]
        version: u32,
    },
}

fn main() -> OQLResult<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check { schema, from, to } => {
            check_compatibility(&cli.database, &schema, from, to)?;
        }
        Commands::Plan { schema, from, to, output } => {
            generate_plan(&cli.database, &schema, from, to, output)?;
        }
        Commands::Migrate { schema, collection, from, to, dry_run } => {
            execute_migration(&cli.database, &schema, &collection, from, to, dry_run)?;
        }
        Commands::List { schema } => {
            list_versions(&cli.database, schema.as_deref())?;
        }
        Commands::Show { schema, version } => {
            show_version(&cli.database, &schema, version)?;
        }
    }

    Ok(())
}

fn check_compatibility(
    _db_path: &PathBuf,
    schema_name: &str,
    from_version: u32,
    to_version: u32,
) -> OQLResult<()> {
    println!("Checking compatibility for schema '{}' from v{} to v{}", 
        schema_name, from_version, to_version);

    // In a real implementation, load versions from storage
    let evolution = SchemaEvolution::new();
    
    // For demo purposes, create sample schemas
    let old_schema = create_sample_schema(schema_name, from_version);
    let new_schema = create_sample_schema(schema_name, to_version);

    let compat = evolution.check_compatibility(&old_schema, &new_schema);

    match compat.level {
        CompatibilityLevel::FullyCompatible => {
            println!("✓ Fully compatible - no migration needed");
        }
        CompatibilityLevel::BackwardCompatible => {
            println!("⚠ Backward compatible - migration recommended");
            if !compat.warnings.is_empty() {
                println!("\nWarnings:");
                for warning in &compat.warnings {
                    println!("  - {}", warning);
                }
            }
        }
        CompatibilityLevel::Breaking => {
            println!("✗ Breaking changes detected - migration required");
            if !compat.issues.is_empty() {
                println!("\nIssues:");
                for issue in &compat.issues {
                    println!("  - {}", issue);
                }
            }
        }
    }

    if compat.migration_required {
        println!("\nMigration is required to upgrade from v{} to v{}", from_version, to_version);
    }

    Ok(())
}

fn generate_plan(
    _db_path: &PathBuf,
    schema_name: &str,
    from_version: u32,
    to_version: u32,
    output: Option<PathBuf>,
) -> OQLResult<()> {
    println!("Generating migration plan for schema '{}' from v{} to v{}", 
        schema_name, from_version, to_version);

    // In a real implementation, load versions from storage
    let old_schema = create_sample_schema(schema_name, from_version);
    let new_schema = create_sample_schema(schema_name, to_version);

    let from_ver = SchemaVersion::new(old_schema, from_version, "Old version".to_string());
    let to_ver = SchemaVersion::new(new_schema, to_version, "New version".to_string());

    let plan = MigrationPlan::from_versions(&from_ver, &to_ver, vec![]);

    println!("\nMigration Plan:");
    println!("  Schema: {}", plan.schema_name);
    println!("  From: v{}", plan.from_version);
    println!("  To: v{}", plan.to_version);
    println!("  Scripts: {}", plan.scripts.len());

    for (i, script) in plan.scripts.iter().enumerate() {
        println!("\n  Script {}:", i + 1);
        println!("    ID: {}", script.id);
        println!("    Description: {}", script.description);
        println!("    Steps: {}", script.steps.len());
    }

    if let Some(output_path) = output {
        let json = serde_json::to_string_pretty(&plan.scripts)
            .map_err(|e| owl_db::error::OQLError::Serialization(e))?;
        std::fs::write(&output_path, json)
            .map_err(|e| owl_db::error::OQLError::Io(e))?;
        println!("\nMigration plan saved to: {}", output_path.display());
    }

    Ok(())
}

fn execute_migration(
    db_path: &PathBuf,
    schema_name: &str,
    collection: &str,
    from_version: u32,
    to_version: u32,
    dry_run: bool,
) -> OQLResult<()> {
    if dry_run {
        println!("DRY RUN: Validating migration for schema '{}' from v{} to v{}", 
            schema_name, from_version, to_version);
    } else {
        println!("Executing migration for schema '{}' from v{} to v{}", 
            schema_name, from_version, to_version);
    }

    // Create store
    let store = ZigStore::new(db_path.to_str().unwrap(), 100_000_000)?;
    
    // Create migration plan
    let old_schema = create_sample_schema(schema_name, from_version);
    let new_schema = create_sample_schema(schema_name, to_version);
    
    let from_ver = SchemaVersion::new(old_schema, from_version, "Old version".to_string());
    let to_ver = SchemaVersion::new(new_schema, to_version, "New version".to_string());
    
    let plan = MigrationPlan::from_versions(&from_ver, &to_ver, vec![]);

    if dry_run {
        // Dry run - validate without applying
        let mut executor = MigrationExecutor::new(store);
        let issues = executor.dry_run(collection, &plan)?;
        
        if issues.is_empty() {
            println!("✓ Dry run successful - no issues found");
        } else {
            println!("✗ Dry run found issues:");
            for issue in issues {
                println!("  - {}", issue);
            }
        }
    } else {
        // Execute migration
        let mut executor = MigrationExecutor::new(store);
        let migration = Migration::new(schema_name.to_string(), from_version, to_version);
        
        let result = executor.execute(collection, &plan, migration)?;
        
        match result.status {
            owl_db::schema::migration::MigrationStatus::Completed { documents_migrated, .. } => {
                println!("✓ Migration completed successfully");
                println!("  Documents migrated: {}", documents_migrated);
            }
            owl_db::schema::migration::MigrationStatus::Failed { error, .. } => {
                println!("✗ Migration failed: {}", error);
            }
            _ => {
                println!("⚠ Migration in unexpected state");
            }
        }
    }

    Ok(())
}

fn list_versions(_db_path: &PathBuf, schema_name: Option<&str>) -> OQLResult<()> {
    if let Some(name) = schema_name {
        println!("Versions for schema '{}':", name);
        // In a real implementation, load from storage
        println!("  v1 - Initial version");
        println!("  v2 - Added email field");
        println!("  v3 - Made email optional");
    } else {
        println!("All schemas:");
        // In a real implementation, load from storage
        println!("  users (3 versions)");
        println!("  orders (2 versions)");
        println!("  products (1 version)");
    }

    Ok(())
}

fn show_version(_db_path: &PathBuf, schema_name: &str, version: u32) -> OQLResult<()> {
    println!("Schema: {}", schema_name);
    println!("Version: {}", version);
    
    // In a real implementation, load from storage
    let schema = create_sample_schema(schema_name, version);
    
    println!("\nFields:");
    for (name, field_type) in &schema.fields {
        println!("  {} : {}", name, field_type.type_name());
    }

    Ok(())
}

// Helper function to create sample schemas for demonstration
fn create_sample_schema(name: &str, version: u32) -> Schema {
    let mut fields = HashMap::new();
    
    // Base fields
    fields.insert("id".to_string(), FieldType::String);
    fields.insert("name".to_string(), FieldType::String);
    
    // Version-specific fields
    if version >= 2 {
        fields.insert("email".to_string(), FieldType::String);
    }
    if version >= 3 {
        // Make email optional in v3
        fields.remove("email");
        fields.insert("email".to_string(), FieldType::Optional(Box::new(FieldType::String)));
    }
    
    Schema::new(name.to_string(), fields)
}
