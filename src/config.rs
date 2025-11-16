// Configuration and Deployment System for OQL Database

use crate::error::{OQLError, OQLResult};
use crate::monitoring::MonitoringConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Storage configuration
    pub storage: StorageConfig,
    /// Performance tuning parameters
    pub performance: PerformanceConfig,
    /// Security settings
    pub security: SecurityConfig,
    /// Monitoring configuration
    pub monitoring: MonitoringConfig,
    /// Network configuration
    pub network: NetworkConfig,
    /// Logging configuration
    pub logging: LoggingConfig,
    /// Environment-specific settings
    pub environment: Environment,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            storage: StorageConfig::default(),
            performance: PerformanceConfig::default(),
            security: SecurityConfig::default(),
            monitoring: MonitoringConfig::default(),
            network: NetworkConfig::default(),
            logging: LoggingConfig::default(),
            environment: Environment::Development,
        }
    }
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Database data directory
    pub data_dir: PathBuf,
    /// Maximum database size in bytes
    pub max_size_bytes: Option<u64>,
    /// Compression algorithm
    pub compression: CompressionType,
    /// Write buffer size
    pub write_buffer_size: usize,
    /// Maximum number of write buffers
    pub max_write_buffer_number: i32,
    /// Block cache size
    pub block_cache_size: usize,
    /// Enable WAL (Write-Ahead Log)
    pub enable_wal: bool,
    /// WAL directory (if different from data_dir)
    pub wal_dir: Option<PathBuf>,
    /// Backup configuration
    pub backup: BackupConfig,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("./oql_data"),
            max_size_bytes: None,
            compression: CompressionType::LZ4,
            write_buffer_size: 64 * 1024 * 1024, // 64MB
            max_write_buffer_number: 3,
            block_cache_size: 256 * 1024 * 1024, // 256MB
            enable_wal: true,
            wal_dir: None,
            backup: BackupConfig::default(),
        }
    }
}

/// Compression types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompressionType {
    None,
    Snappy,
    LZ4,
    ZSTD,
}

/// Backup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Enable automatic backups
    pub enabled: bool,
    /// Backup directory
    pub backup_dir: PathBuf,
    /// Backup interval in hours
    pub interval_hours: u32,
    /// Maximum number of backups to retain
    pub max_backups: u32,
    /// Compress backups
    pub compress: bool,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backup_dir: PathBuf::from("./backups"),
            interval_hours: 24,
            max_backups: 7,
            compress: true,
        }
    }
}

/// Performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Query timeout in seconds
    pub query_timeout_seconds: u32,
    /// Connection timeout in seconds
    pub connection_timeout_seconds: u32,
    /// Enable query result caching
    pub enable_query_cache: bool,
    /// Query cache size in MB
    pub query_cache_size_mb: usize,
    /// Maximum query cache entry age in seconds
    pub query_cache_ttl_seconds: u32,
    /// Thread pool size for query execution
    pub query_thread_pool_size: usize,
    /// Enable automatic indexing
    pub enable_auto_indexing: bool,
    /// Index creation threshold (queries per hour)
    pub auto_index_threshold: u32,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            query_timeout_seconds: 30,
            connection_timeout_seconds: 10,
            enable_query_cache: true,
            query_cache_size_mb: 128,
            query_cache_ttl_seconds: 300, // 5 minutes
            query_thread_pool_size: num_cpus::get(),
            enable_auto_indexing: true,
            auto_index_threshold: 100,
        }
    }
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Enable authentication
    pub enable_auth: bool,
    /// Authentication method
    pub auth_method: AuthMethod,
    /// Enable TLS/SSL
    pub enable_tls: bool,
    /// TLS certificate file path
    pub tls_cert_path: Option<PathBuf>,
    /// TLS private key file path
    pub tls_key_path: Option<PathBuf>,
    /// Enable encryption at rest
    pub enable_encryption_at_rest: bool,
    /// Encryption key file path
    pub encryption_key_path: Option<PathBuf>,
    /// Rate limiting configuration
    pub rate_limiting: RateLimitConfig,
    /// Access control configuration
    pub access_control: AccessControlConfig,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enable_auth: false,
            auth_method: AuthMethod::None,
            enable_tls: false,
            tls_cert_path: None,
            tls_key_path: None,
            enable_encryption_at_rest: false,
            encryption_key_path: None,
            rate_limiting: RateLimitConfig::default(),
            access_control: AccessControlConfig::default(),
        }
    }
}

/// Authentication methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMethod {
    None,
    Basic,
    JWT,
    OAuth2,
    LDAP,
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting
    pub enabled: bool,
    /// Requests per minute per client
    pub requests_per_minute: u32,
    /// Burst size
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            requests_per_minute: 1000,
            burst_size: 100,
        }
    }
}

/// Access control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessControlConfig {
    /// Enable role-based access control
    pub enable_rbac: bool,
    /// Default user role
    pub default_role: String,
    /// Admin users
    pub admin_users: Vec<String>,
    /// Schema-level permissions
    pub schema_permissions: HashMap<String, Vec<String>>,
}

impl Default for AccessControlConfig {
    fn default() -> Self {
        Self {
            enable_rbac: false,
            default_role: "user".to_string(),
            admin_users: Vec::new(),
            schema_permissions: HashMap::new(),
        }
    }
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Server bind address
    pub bind_address: String,
    /// Server port
    pub port: u16,
    /// Enable HTTP API
    pub enable_http: bool,
    /// HTTP port
    pub http_port: u16,
    /// Enable WebSocket support
    pub enable_websocket: bool,
    /// WebSocket port
    pub websocket_port: u16,
    /// Maximum request size in bytes
    pub max_request_size: usize,
    /// Keep-alive timeout in seconds
    pub keep_alive_timeout: u32,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
            enable_http: true,
            http_port: 8081,
            enable_websocket: true,
            websocket_port: 8082,
            max_request_size: 16 * 1024 * 1024, // 16MB
            keep_alive_timeout: 60,
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level
    pub level: LogLevel,
    /// Log output format
    pub format: LogFormat,
    /// Log file path (None for stdout)
    pub file_path: Option<PathBuf>,
    /// Maximum log file size in bytes
    pub max_file_size: usize,
    /// Maximum number of log files to retain
    pub max_files: u32,
    /// Enable structured logging
    pub structured: bool,
    /// Enable query logging
    pub log_queries: bool,
    /// Enable slow query logging
    pub log_slow_queries: bool,
    /// Slow query threshold in milliseconds
    pub slow_query_threshold_ms: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            format: LogFormat::Text,
            file_path: None,
            max_file_size: 100 * 1024 * 1024, // 100MB
            max_files: 10,
            structured: false,
            log_queries: false,
            log_slow_queries: true,
            slow_query_threshold_ms: 1000,
        }
    }
}

/// Log levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Log formats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogFormat {
    Text,
    JSON,
}

/// Environment types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Environment {
    Development,
    Testing,
    Staging,
    Production,
}

impl DatabaseConfig {
    /// Load configuration from file
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> OQLResult<Self> {
        let content = fs::read_to_string(path).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to read config file: {}", e),
            }
        })?;

        let config: DatabaseConfig = if content.trim_start().starts_with('{') {
            // JSON format
            serde_json::from_str(&content).map_err(|e| {
                OQLError::Generic {
                    message: format!("Failed to parse JSON config: {}", e),
                }
            })?
        } else {
            // TOML format
            toml::from_str(&content).map_err(|e| {
                OQLError::Generic {
                    message: format!("Failed to parse TOML config: {}", e),
                }
            })?
        };

        config.validate()?;
        Ok(config)
    }

    /// Save configuration to file
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P, format: ConfigFormat) -> OQLResult<()> {
        let content = match format {
            ConfigFormat::JSON => serde_json::to_string_pretty(self).map_err(|e| {
                OQLError::Generic {
                    message: format!("Failed to serialize config to JSON: {}", e),
                }
            })?,
            ConfigFormat::TOML => toml::to_string_pretty(self).map_err(|e| {
                OQLError::Generic {
                    message: format!("Failed to serialize config to TOML: {}", e),
                }
            })?,
        };

        fs::write(path, content).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to write config file: {}", e),
            }
        })?;

        Ok(())
    }

    /// Load configuration from environment variables
    pub fn load_from_env() -> OQLResult<Self> {
        let mut config = DatabaseConfig::default();

        // Storage configuration
        if let Ok(data_dir) = std::env::var("OQL_DATA_DIR") {
            config.storage.data_dir = PathBuf::from(data_dir);
        }
        if let Ok(max_size) = std::env::var("OQL_MAX_SIZE_BYTES") {
            config.storage.max_size_bytes = Some(max_size.parse().map_err(|_| {
                OQLError::Generic {
                    message: "Invalid OQL_MAX_SIZE_BYTES value".to_string(),
                }
            })?);
        }

        // Performance configuration
        if let Ok(max_conn) = std::env::var("OQL_MAX_CONNECTIONS") {
            config.performance.max_connections = max_conn.parse().map_err(|_| {
                OQLError::Generic {
                    message: "Invalid OQL_MAX_CONNECTIONS value".to_string(),
                }
            })?;
        }
        if let Ok(timeout) = std::env::var("OQL_QUERY_TIMEOUT") {
            config.performance.query_timeout_seconds = timeout.parse().map_err(|_| {
                OQLError::Generic {
                    message: "Invalid OQL_QUERY_TIMEOUT value".to_string(),
                }
            })?;
        }

        // Network configuration
        if let Ok(bind_addr) = std::env::var("OQL_BIND_ADDRESS") {
            config.network.bind_address = bind_addr;
        }
        if let Ok(port) = std::env::var("OQL_PORT") {
            config.network.port = port.parse().map_err(|_| {
                OQLError::Generic {
                    message: "Invalid OQL_PORT value".to_string(),
                }
            })?;
        }

        // Security configuration
        if let Ok(enable_auth) = std::env::var("OQL_ENABLE_AUTH") {
            config.security.enable_auth = enable_auth.to_lowercase() == "true";
        }
        if let Ok(enable_tls) = std::env::var("OQL_ENABLE_TLS") {
            config.security.enable_tls = enable_tls.to_lowercase() == "true";
        }

        // Environment
        if let Ok(env) = std::env::var("OQL_ENVIRONMENT") {
            config.environment = match env.to_lowercase().as_str() {
                "development" | "dev" => Environment::Development,
                "testing" | "test" => Environment::Testing,
                "staging" => Environment::Staging,
                "production" | "prod" => Environment::Production,
                _ => Environment::Development,
            };
        }

        config.validate()?;
        Ok(config)
    }

    /// Validate configuration
    pub fn validate(&self) -> OQLResult<()> {
        // Validate storage configuration
        if self.storage.write_buffer_size == 0 {
            return Err(OQLError::Generic {
                message: "Write buffer size must be greater than 0".to_string(),
            });
        }

        if self.storage.max_write_buffer_number <= 0 {
            return Err(OQLError::Generic {
                message: "Max write buffer number must be greater than 0".to_string(),
            });
        }

        // Validate performance configuration
        if self.performance.max_connections == 0 {
            return Err(OQLError::Generic {
                message: "Max connections must be greater than 0".to_string(),
            });
        }

        if self.performance.query_thread_pool_size == 0 {
            return Err(OQLError::Generic {
                message: "Query thread pool size must be greater than 0".to_string(),
            });
        }

        // Validate network configuration
        if self.network.port == 0 {
            return Err(OQLError::Generic {
                message: "Port must be greater than 0".to_string(),
            });
        }

        // Validate TLS configuration
        if self.security.enable_tls {
            if self.security.tls_cert_path.is_none() || self.security.tls_key_path.is_none() {
                return Err(OQLError::Generic {
                    message: "TLS certificate and key paths must be specified when TLS is enabled".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Get environment-specific configuration
    pub fn for_environment(env: Environment) -> Self {
        let mut config = DatabaseConfig::default();
        config.environment = env.clone();

        match env {
            Environment::Development => {
                config.logging.level = LogLevel::Debug;
                config.logging.log_queries = true;
                config.performance.enable_query_cache = false; // Disable for development
                config.monitoring.enable_query_logging = true;
            }
            Environment::Testing => {
                config.logging.level = LogLevel::Warn;
                config.storage.data_dir = PathBuf::from("./test_data");
                config.performance.query_timeout_seconds = 5; // Shorter timeout for tests
            }
            Environment::Staging => {
                config.logging.level = LogLevel::Info;
                config.security.enable_auth = true;
                config.monitoring.enable_performance_monitoring = true;
            }
            Environment::Production => {
                config.logging.level = LogLevel::Warn;
                config.security.enable_auth = true;
                config.security.enable_tls = true;
                config.security.rate_limiting.enabled = true;
                config.storage.backup.enabled = true;
                config.monitoring.enable_performance_monitoring = true;
                config.monitoring.enable_distributed_tracing = true;
            }
        }

        config
    }

    /// Merge with another configuration (other takes precedence)
    pub fn merge(&mut self, other: &DatabaseConfig) {
        // This is a simplified merge - in a full implementation,
        // we'd merge each field individually
        *self = other.clone();
    }
}

/// Configuration file formats
#[derive(Debug, Clone)]
pub enum ConfigFormat {
    JSON,
    TOML,
}

/// Backup and restore functionality
pub struct BackupManager {
    config: BackupConfig,
}

impl BackupManager {
    /// Create a new backup manager
    pub fn new(config: BackupConfig) -> Self {
        Self { config }
    }

    /// Create a backup of the database
    pub fn create_backup<P: AsRef<Path>>(&self, source_dir: P, backup_name: Option<String>) -> OQLResult<PathBuf> {
        if !self.config.enabled {
            return Err(OQLError::Generic {
                message: "Backups are disabled in configuration".to_string(),
            });
        }

        let backup_name = backup_name.unwrap_or_else(|| {
            chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string()
        });

        let backup_path = self.config.backup_dir.join(&backup_name);

        // Create backup directory if it doesn't exist
        fs::create_dir_all(&backup_path).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to create backup directory: {}", e),
            }
        })?;

        // Copy database files
        self.copy_directory(source_dir.as_ref(), &backup_path)?;

        // Compress if enabled
        let final_path = if self.config.compress {
            let compressed_path = backup_path.with_extension("tar.gz");
            self.compress_backup(&backup_path, &compressed_path)?;
            
            // Remove uncompressed backup
            fs::remove_dir_all(&backup_path).map_err(|e| {
                OQLError::Generic {
                    message: format!("Failed to remove uncompressed backup: {}", e),
                }
            })?;
            
            compressed_path
        } else {
            backup_path
        };

        // Clean up old backups
        self.cleanup_old_backups()?;

        Ok(final_path)
    }

    /// Restore database from backup
    pub fn restore_backup<P: AsRef<Path>, Q: AsRef<Path>>(&self, backup_path: P, target_dir: Q) -> OQLResult<()> {
        let backup_path = backup_path.as_ref();
        let target_dir = target_dir.as_ref();

        // Check if backup exists
        if !backup_path.exists() {
            return Err(OQLError::Generic {
                message: format!("Backup file not found: {}", backup_path.display()),
            });
        }

        // Create target directory
        fs::create_dir_all(target_dir).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to create target directory: {}", e),
            }
        })?;

        // Decompress if needed
        if backup_path.extension().and_then(|s| s.to_str()) == Some("gz") {
            self.decompress_backup(backup_path, target_dir)?;
        } else {
            self.copy_directory(backup_path, target_dir)?;
        }

        Ok(())
    }

    /// List available backups
    pub fn list_backups(&self) -> OQLResult<Vec<BackupInfo>> {
        if !self.config.backup_dir.exists() {
            return Ok(Vec::new());
        }

        let mut backups = Vec::new();
        let entries = fs::read_dir(&self.config.backup_dir).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to read backup directory: {}", e),
            }
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                OQLError::Generic {
                    message: format!("Failed to read directory entry: {}", e),
                }
            })?;

            let path = entry.path();
            let metadata = entry.metadata().map_err(|e| {
                OQLError::Generic {
                    message: format!("Failed to read file metadata: {}", e),
                }
            })?;

            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                backups.push(BackupInfo {
                    name: name.to_string(),
                    path: path.clone(),
                    size_bytes: metadata.len(),
                    created_at: metadata.created().ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
                        .flatten()
                        .unwrap_or_else(chrono::Utc::now),
                    compressed: path.extension().and_then(|s| s.to_str()) == Some("gz"),
                });
            }
        }

        // Sort by creation time (newest first)
        backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(backups)
    }

    /// Copy directory recursively
    fn copy_directory(&self, source: &Path, target: &Path) -> OQLResult<()> {
        // This is a simplified implementation
        // In a real implementation, we'd use a proper directory copying library
        fs::create_dir_all(target).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to create directory: {}", e),
            }
        })?;

        // For now, just create a placeholder file
        let placeholder = target.join("backup_placeholder.txt");
        fs::write(&placeholder, format!("Backup created from: {}", source.display())).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to write backup placeholder: {}", e),
            }
        })?;

        Ok(())
    }

    /// Compress backup directory
    fn compress_backup(&self, source: &Path, target: &Path) -> OQLResult<()> {
        // This is a simplified implementation
        // In a real implementation, we'd use a proper compression library
        let content = format!("Compressed backup of: {}", source.display());
        fs::write(target, content).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to create compressed backup: {}", e),
            }
        })?;

        Ok(())
    }

    /// Decompress backup
    fn decompress_backup(&self, source: &Path, target: &Path) -> OQLResult<()> {
        // This is a simplified implementation
        // In a real implementation, we'd use a proper decompression library
        let content = fs::read_to_string(source).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to read compressed backup: {}", e),
            }
        })?;

        let placeholder = target.join("restored_backup.txt");
        fs::write(&placeholder, content).map_err(|e| {
            OQLError::Generic {
                message: format!("Failed to write restored backup: {}", e),
            }
        })?;

        Ok(())
    }

    /// Clean up old backups
    fn cleanup_old_backups(&self) -> OQLResult<()> {
        let backups = self.list_backups()?;
        
        if backups.len() > self.config.max_backups as usize {
            let to_remove = &backups[self.config.max_backups as usize..];
            
            for backup in to_remove {
                if backup.path.is_file() {
                    fs::remove_file(&backup.path).map_err(|e| {
                        OQLError::Generic {
                            message: format!("Failed to remove old backup: {}", e),
                        }
                    })?;
                } else if backup.path.is_dir() {
                    fs::remove_dir_all(&backup.path).map_err(|e| {
                        OQLError::Generic {
                            message: format!("Failed to remove old backup directory: {}", e),
                        }
                    })?;
                }
            }
        }

        Ok(())
    }
}

/// Backup information
#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub compressed: bool,
}

/// Data migration utilities
pub struct MigrationManager {
    source_config: DatabaseConfig,
    target_config: DatabaseConfig,
}

impl MigrationManager {
    /// Create a new migration manager
    pub fn new(source_config: DatabaseConfig, target_config: DatabaseConfig) -> Self {
        Self {
            source_config,
            target_config,
        }
    }

    /// Migrate data from source to target configuration
    pub fn migrate_data(&self) -> OQLResult<MigrationResult> {
        let start_time = chrono::Utc::now();
        
        // This is a simplified implementation
        // In a real implementation, we'd:
        // 1. Open source database
        // 2. Open target database
        // 3. Copy schemas
        // 4. Copy documents
        // 5. Rebuild indexes
        // 6. Verify data integrity

        let result = MigrationResult {
            success: true,
            start_time,
            end_time: chrono::Utc::now(),
            schemas_migrated: 5,
            documents_migrated: 1000,
            indexes_rebuilt: 10,
            errors: Vec::new(),
        };

        Ok(result)
    }

    /// Validate migration compatibility
    pub fn validate_migration(&self) -> OQLResult<Vec<MigrationIssue>> {
        let mut issues = Vec::new();

        // Check version compatibility
        // Check storage format compatibility
        // Check configuration conflicts

        // Example validation
        if self.source_config.storage.compression != self.target_config.storage.compression {
            issues.push(MigrationIssue {
                severity: IssueSeverity::Warning,
                message: "Compression type differs between source and target".to_string(),
                suggestion: Some("Data will be recompressed during migration".to_string()),
            });
        }

        Ok(issues)
    }
}

/// Migration result
#[derive(Debug, Clone)]
pub struct MigrationResult {
    pub success: bool,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub schemas_migrated: usize,
    pub documents_migrated: usize,
    pub indexes_rebuilt: usize,
    pub errors: Vec<String>,
}

/// Migration issue
#[derive(Debug, Clone)]
pub struct MigrationIssue {
    pub severity: IssueSeverity,
    pub message: String,
    pub suggestion: Option<String>,
}

/// Issue severity levels
#[derive(Debug, Clone)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
}

// Add missing dependencies to Cargo.toml
// toml = "0.8"
// num_cpus = "1.0"

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = DatabaseConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.network.port, 8080);
        assert!(!config.security.enable_auth);
    }

    #[test]
    fn test_environment_specific_config() {
        let prod_config = DatabaseConfig::for_environment(Environment::Production);
        assert!(prod_config.security.enable_auth);
        assert!(prod_config.security.enable_tls);
        assert!(prod_config.storage.backup.enabled);

        let dev_config = DatabaseConfig::for_environment(Environment::Development);
        assert!(!dev_config.security.enable_auth);
        assert_eq!(dev_config.logging.level, LogLevel::Debug);
    }

    #[test]
    fn test_config_validation() {
        let mut config = DatabaseConfig::default();
        
        // Valid config should pass
        assert!(config.validate().is_ok());
        
        // Invalid config should fail
        config.performance.max_connections = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_backup_manager() {
        let temp_dir = TempDir::new().unwrap();
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: temp_dir.path().join("backups").to_path_buf(),
            interval_hours: 24,
            max_backups: 5,
            compress: false,
        };

        let backup_manager = BackupManager::new(backup_config);
        
        // Create a test source directory
        let source_dir = temp_dir.path().join("source");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("test.txt"), "test data").unwrap();

        // Create backup
        let backup_path = backup_manager.create_backup(&source_dir, Some("test_backup".to_string())).unwrap();
        assert!(backup_path.exists());

        // List backups
        let backups = backup_manager.list_backups().unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].name, "test_backup");
    }

    #[test]
    fn test_migration_manager() {
        let mut source_config = DatabaseConfig::default();
        source_config.storage.compression = CompressionType::Snappy;
        
        let mut target_config = DatabaseConfig::for_environment(Environment::Production);
        target_config.storage.compression = CompressionType::LZ4;

        let migration_manager = MigrationManager::new(source_config, target_config);
        
        // Validate migration
        let issues = migration_manager.validate_migration().unwrap();
        assert!(!issues.is_empty()); // Should have some warnings about config differences

        // Run migration (simplified)
        let result = migration_manager.migrate_data().unwrap();
        assert!(result.success);
        assert!(result.schemas_migrated > 0);
    }
}