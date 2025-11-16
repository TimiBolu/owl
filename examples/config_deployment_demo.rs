// Configuration and Deployment Demo
// Demonstrates environment-specific configurations, backup/restore, and data migration

use owl_db::*;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

fn main() -> OQLResult<()> {
    println!("=== OQL Configuration and Deployment Demo ===\n");

    // Demo 1: Environment-specific configurations
    demo_environment_configs()?;

    // Demo 2: Configuration file handling
    demo_config_files()?;

    // Demo 3: Environment variable configuration
    demo_env_config()?;

    // Demo 4: Backup and restore
    demo_backup_restore()?;

    // Demo 5: Data migration
    demo_data_migration()?;

    println!("\n=== Configuration and Deployment Demo Complete ===");
    Ok(())
}

fn demo_environment_configs() -> OQLResult<()> {
    println!("--- Demo 1: Environment-Specific Configurations ---");

    let environments = vec![
        Environment::Development,
        Environment::Testing,
        Environment::Staging,
        Environment::Production,
    ];

    for env in environments {
        let config = DatabaseConfig::for_environment(env.clone());
        
        println!("📋 {:?} Environment:", env);
        println!("  Auth enabled: {}", config.security.enable_auth);
        println!("  TLS enabled: {}", config.security.enable_tls);
        println!("  Log level: {:?}", config.logging.level);
        println!("  Query cache: {}", config.performance.enable_query_cache);
        println!("  Backups enabled: {}", config.storage.backup.enabled);
        println!("  Rate limiting: {}", config.security.rate_limiting.enabled);
        println!("  Monitoring: {}", config.monitoring.enable_performance_monitoring);
        println!("  Data directory: {}", config.storage.data_dir.display());
        println!();
    }

    println!("✓ Environment configurations demo complete\n");
    Ok(())
}

fn demo_config_files() -> OQLResult<()> {
    println!("--- Demo 2: Configuration File Handling ---");

    let temp_dir = TempDir::new().unwrap();

    // Create a custom configuration
    let mut config = DatabaseConfig::for_environment(Environment::Production);
    config.network.port = 9090;
    config.network.bind_address = "0.0.0.0".to_string();
    config.performance.max_connections = 500;
    config.storage.data_dir = temp_dir.path().join("production_data").to_path_buf();
    config.security.access_control.admin_users = vec![
        "admin@example.com".to_string(),
        "ops@example.com".to_string(),
    ];
    // Disable TLS for demo (or provide cert paths)
    config.security.enable_tls = false;

    // Save configuration in different formats
    let json_path = temp_dir.path().join("config.json");
    let toml_path = temp_dir.path().join("config.toml");

    config.save_to_file(&json_path, ConfigFormat::JSON)?;
    config.save_to_file(&toml_path, ConfigFormat::TOML)?;

    println!("✓ Saved configuration files:");
    println!("  JSON: {}", json_path.display());
    println!("  TOML: {}", toml_path.display());

    // Show file contents
    println!("\n📄 JSON Configuration (first 300 chars):");
    let json_content = fs::read_to_string(&json_path).unwrap();
    println!("{}", &json_content[..json_content.len().min(300)]);
    if json_content.len() > 300 {
        println!("...");
    }

    println!("\n📄 TOML Configuration (first 300 chars):");
    let toml_content = fs::read_to_string(&toml_path).unwrap();
    println!("{}", &toml_content[..toml_content.len().min(300)]);
    if toml_content.len() > 300 {
        println!("...");
    }

    // Load configuration back
    let loaded_json_config = DatabaseConfig::load_from_file(&json_path)?;
    let loaded_toml_config = DatabaseConfig::load_from_file(&toml_path)?;

    println!("\n✓ Loaded configurations:");
    println!("  JSON - Port: {}, Max connections: {}", 
        loaded_json_config.network.port, 
        loaded_json_config.performance.max_connections
    );
    println!("  TOML - Port: {}, Max connections: {}", 
        loaded_toml_config.network.port, 
        loaded_toml_config.performance.max_connections
    );

    // Validate configurations
    loaded_json_config.validate()?;
    loaded_toml_config.validate()?;
    println!("✓ All configurations are valid");

    println!("✓ Configuration file handling demo complete\n");
    Ok(())
}

fn demo_env_config() -> OQLResult<()> {
    println!("--- Demo 3: Environment Variable Configuration ---");

    // Set some environment variables for testing
    std::env::set_var("OQL_PORT", "8888");
    std::env::set_var("OQL_MAX_CONNECTIONS", "200");
    std::env::set_var("OQL_BIND_ADDRESS", "192.168.1.100");
    std::env::set_var("OQL_ENABLE_AUTH", "true");
    std::env::set_var("OQL_ENVIRONMENT", "staging");
    std::env::set_var("OQL_DATA_DIR", "/opt/oql/data");

    println!("🔧 Set environment variables:");
    println!("  OQL_PORT=8888");
    println!("  OQL_MAX_CONNECTIONS=200");
    println!("  OQL_BIND_ADDRESS=192.168.1.100");
    println!("  OQL_ENABLE_AUTH=true");
    println!("  OQL_ENVIRONMENT=staging");
    println!("  OQL_DATA_DIR=/opt/oql/data");

    // Load configuration from environment
    let env_config = DatabaseConfig::load_from_env()?;

    println!("\n📋 Configuration loaded from environment:");
    println!("  Environment: {:?}", env_config.environment);
    println!("  Port: {}", env_config.network.port);
    println!("  Max connections: {}", env_config.performance.max_connections);
    println!("  Bind address: {}", env_config.network.bind_address);
    println!("  Auth enabled: {}", env_config.security.enable_auth);
    println!("  Data directory: {}", env_config.storage.data_dir.display());

    // Clean up environment variables
    std::env::remove_var("OQL_PORT");
    std::env::remove_var("OQL_MAX_CONNECTIONS");
    std::env::remove_var("OQL_BIND_ADDRESS");
    std::env::remove_var("OQL_ENABLE_AUTH");
    std::env::remove_var("OQL_ENVIRONMENT");
    std::env::remove_var("OQL_DATA_DIR");

    println!("✓ Environment variable configuration demo complete\n");
    Ok(())
}

fn demo_backup_restore() -> OQLResult<()> {
    println!("--- Demo 4: Backup and Restore ---");

    let temp_dir = TempDir::new().unwrap();
    
    // Create backup configuration
    let backup_config = config::BackupConfig {
        enabled: true,
        backup_dir: temp_dir.path().join("backups").to_path_buf(),
        interval_hours: 6,
        max_backups: 10,
        compress: true,
    };

    let backup_manager = BackupManager::new(backup_config);

    // Create a mock database directory
    let db_dir = temp_dir.path().join("database");
    fs::create_dir_all(&db_dir).unwrap();
    fs::write(db_dir.join("data.db"), "mock database data").unwrap();
    fs::write(db_dir.join("index.idx"), "mock index data").unwrap();
    fs::write(db_dir.join("metadata.json"), r#"{"version": "1.0", "created": "2024-01-01"}"#).unwrap();

    println!("📁 Created mock database with files:");
    println!("  data.db (18 bytes)");
    println!("  index.idx (15 bytes)");
    println!("  metadata.json (44 bytes)");

    // Create backups
    let backup_names = vec!["daily_backup", "weekly_backup", "monthly_backup"];
    
    for backup_name in &backup_names {
        let backup_path = backup_manager.create_backup(&db_dir, Some(backup_name.to_string()))?;
        println!("✓ Created backup: {} -> {}", backup_name, backup_path.display());
        
        // Small delay to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // List backups
    let backups = backup_manager.list_backups()?;
    println!("\n📋 Available backups ({}):", backups.len());
    for backup in &backups {
        println!("  📦 {} ({} bytes, compressed: {}, created: {})",
            backup.name,
            backup.size_bytes,
            backup.compressed,
            backup.created_at.format("%Y-%m-%d %H:%M:%S")
        );
    }

    // Restore a backup
    if let Some(backup) = backups.first() {
        let restore_dir = temp_dir.path().join("restored");
        backup_manager.restore_backup(&backup.path, &restore_dir)?;
        
        println!("\n✓ Restored backup '{}' to: {}", backup.name, restore_dir.display());
        
        // Verify restored files
        if restore_dir.join("restored_backup.txt").exists() {
            let content = fs::read_to_string(restore_dir.join("restored_backup.txt")).unwrap();
            println!("  📄 Restored content: {}", content.lines().next().unwrap_or(""));
        }
    }

    println!("✓ Backup and restore demo complete\n");
    Ok(())
}

fn demo_data_migration() -> OQLResult<()> {
    println!("--- Demo 5: Data Migration ---");

    // Create source and target configurations
    let mut source_config = DatabaseConfig::for_environment(Environment::Development);
    source_config.storage.data_dir = "/old/database".into();
    source_config.storage.compression = config::CompressionType::Snappy;
    source_config.performance.max_connections = 50;

    let mut target_config = DatabaseConfig::for_environment(Environment::Production);
    target_config.storage.data_dir = "/new/database".into();
    target_config.storage.compression = config::CompressionType::LZ4;
    target_config.performance.max_connections = 500;

    println!("🔄 Migration Setup:");
    println!("  Source: {} -> {}", 
        source_config.storage.data_dir.display(),
        target_config.storage.data_dir.display()
    );
    println!("  Compression: {:?} -> {:?}", 
        source_config.storage.compression,
        target_config.storage.compression
    );
    println!("  Max connections: {} -> {}", 
        source_config.performance.max_connections,
        target_config.performance.max_connections
    );

    // Create migration manager
    let migration_manager = MigrationManager::new(source_config, target_config);

    // Validate migration
    let issues = migration_manager.validate_migration()?;
    println!("\n🔍 Migration validation ({} issues):", issues.len());
    for issue in &issues {
        let icon = match issue.severity {
            config::IssueSeverity::Info => "ℹ️",
            config::IssueSeverity::Warning => "⚠️",
            config::IssueSeverity::Error => "❌",
        };
        println!("  {} {:?}: {}", icon, issue.severity, issue.message);
        if let Some(suggestion) = &issue.suggestion {
            println!("     💡 {}", suggestion);
        }
    }

    // Run migration (simulated)
    println!("\n🚀 Starting data migration...");
    let result = migration_manager.migrate_data()?;

    println!("✅ Migration completed successfully!");
    println!("  Duration: {:.2}s", 
        (result.end_time - result.start_time).num_milliseconds() as f64 / 1000.0
    );
    println!("  Schemas migrated: {}", result.schemas_migrated);
    println!("  Documents migrated: {}", result.documents_migrated);
    println!("  Indexes rebuilt: {}", result.indexes_rebuilt);
    
    if !result.errors.is_empty() {
        println!("  ⚠️ Errors encountered: {}", result.errors.len());
        for error in &result.errors {
            println!("    - {}", error);
        }
    }

    // Show migration summary
    println!("\n📊 Migration Summary:");
    println!("  ✅ Success: {}", result.success);
    println!("  📈 Data throughput: {:.0} docs/sec", 
        result.documents_migrated as f64 / 
        (result.end_time - result.start_time).num_seconds() as f64
    );
    println!("  🔧 Index rebuild rate: {:.1} indexes/sec", 
        result.indexes_rebuilt as f64 / 
        (result.end_time - result.start_time).num_seconds() as f64
    );

    println!("✓ Data migration demo complete\n");
    Ok(())
}

/// Helper function to demonstrate configuration merging
#[allow(dead_code)]
fn demo_config_merging() -> OQLResult<()> {
    println!("--- Configuration Merging ---");

    let mut base_config = DatabaseConfig::for_environment(Environment::Development);
    let override_config = DatabaseConfig::for_environment(Environment::Production);

    println!("Base config - Auth: {}, TLS: {}", 
        base_config.security.enable_auth,
        base_config.security.enable_tls
    );

    base_config.merge(&override_config);

    println!("After merge - Auth: {}, TLS: {}", 
        base_config.security.enable_auth,
        base_config.security.enable_tls
    );

    Ok(())
}

/// Helper function to demonstrate configuration validation
#[allow(dead_code)]
fn demo_config_validation() -> OQLResult<()> {
    println!("--- Configuration Validation ---");

    let mut config = DatabaseConfig::default();
    
    // Test valid configuration
    match config.validate() {
        Ok(()) => println!("✅ Valid configuration"),
        Err(e) => println!("❌ Invalid configuration: {}", e),
    }

    // Test invalid configuration
    config.performance.max_connections = 0;
    match config.validate() {
        Ok(()) => println!("✅ Valid configuration"),
        Err(e) => println!("❌ Invalid configuration: {}", e),
    }

    Ok(())
}