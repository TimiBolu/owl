use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use super::{BackupManager, ClusterManager, ConfigManager, HealthChecker, IndexAdmin};

/// Owl Database Administration CLI
#[derive(Parser)]
#[command(name = "owl-admin")]
#[command(about = "Owl Database Administration Tool", long_about = None)]
pub struct AdminCli {
    /// Configuration file path
    #[arg(short, long, default_value = "owl.toml")]
    config: PathBuf,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Admin access token (or set OWL_ADMIN_TOKEN env var)
    #[arg(long)]
    token: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Cluster management commands
    Cluster {
        #[command(subcommand)]
        action: ClusterAction,
    },
    /// Health check commands
    Health {
        #[command(subcommand)]
        action: HealthAction,
    },
    /// Backup and restore commands
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Index management commands
    Index {
        #[command(subcommand)]
        action: IndexAction,
    },
}

#[derive(Subcommand)]
enum ClusterAction {
    /// Show cluster status
    Status,
    /// List all nodes in the cluster
    Nodes,
    /// Add a new node to the cluster
    AddNode {
        /// Node address (host:port)
        #[arg(short, long)]
        address: String,
        /// Node ID
        #[arg(short, long)]
        id: Option<u32>,
    },
    /// Remove a node from the cluster
    RemoveNode {
        /// Node ID to remove
        node_id: u32,
    },
    /// Rebalance data across nodes
    Rebalance {
        /// Dry run (don't actually move data)
        #[arg(short, long)]
        dry_run: bool,
    },
    /// Show shard distribution
    Shards,
}

#[derive(Subcommand)]
enum HealthAction {
    /// Check overall cluster health
    Check,
    /// Monitor cluster health continuously
    Monitor {
        /// Refresh interval in seconds
        #[arg(short, long, default_value = "5")]
        interval: u64,
    },
    /// Check specific node health
    Node {
        /// Node ID to check
        node_id: u32,
    },
    /// Check replication status
    Replication,
}

#[derive(Subcommand)]
enum BackupAction {
    /// Create a new backup
    Create {
        /// Backup destination path
        #[arg(short, long)]
        destination: PathBuf,
        /// Backup type (full or incremental)
        #[arg(short, long, default_value = "full")]
        backup_type: String,
    },
    /// List available backups
    List {
        /// Backup location
        #[arg(short, long)]
        location: PathBuf,
    },
    /// Restore from backup
    Restore {
        /// Backup source path
        #[arg(short, long)]
        source: PathBuf,
        /// Point-in-time recovery LSN (optional)
        #[arg(short, long)]
        lsn: Option<u64>,
        /// Validate restore plan without modifying data directories
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Verify backup integrity
    Verify {
        /// Backup path to verify
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,
    /// Validate configuration file
    Validate {
        /// Configuration file to validate
        path: PathBuf,
    },
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        /// Configuration value
        value: String,
    },
    /// Get a configuration value
    Get {
        /// Configuration key
        key: String,
    },
}

#[derive(Subcommand)]
enum IndexAction {
    /// Rebuild persisted indexes from stored documents
    Rebuild {
        /// Path to database file (e.g. data/dev.owl)
        #[arg(short, long)]
        db_path: PathBuf,
        /// Arena size in MB to open the database
        #[arg(short, long, default_value = "128")]
        size_mb: usize,
        /// Path to index catalog JSON (defaults to <db_path>.indexes.json)
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Index backend (zig|memory)
        #[arg(long, default_value = "zig")]
        backend: String,
    },
    /// List indexes from a catalog file
    List {
        /// Path to index catalog JSON
        #[arg(short, long)]
        catalog: PathBuf,
    },
}

impl AdminCli {
    pub fn run() -> Result<()> {
        let cli = AdminCli::parse();
        Self::authorize(cli.token.as_deref())?;

        // Initialize logging
        if cli.verbose {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .init();
        } else {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::INFO)
                .init();
        }

        match cli.command {
            Commands::Cluster { action } => Self::handle_cluster(action, &cli.config),
            Commands::Health { action } => Self::handle_health(action, &cli.config),
            Commands::Backup { action } => Self::handle_backup(action, &cli.config),
            Commands::Config { action } => Self::handle_config(action, &cli.config),
            Commands::Index { action } => Self::handle_index(action, &cli.config),
        }
    }

    fn handle_cluster(action: ClusterAction, config_path: &PathBuf) -> Result<()> {
        let manager = ClusterManager::new(config_path)?;

        match action {
            ClusterAction::Status => manager.show_status(),
            ClusterAction::Nodes => manager.list_nodes(),
            ClusterAction::AddNode { address, id } => manager.add_node(&address, id),
            ClusterAction::RemoveNode { node_id } => manager.remove_node(node_id),
            ClusterAction::Rebalance { dry_run } => manager.rebalance(dry_run),
            ClusterAction::Shards => manager.show_shards(),
        }
    }

    fn handle_health(action: HealthAction, config_path: &PathBuf) -> Result<()> {
        let checker = HealthChecker::new(config_path)?;

        match action {
            HealthAction::Check => checker.check_cluster(),
            HealthAction::Monitor { interval } => checker.monitor(interval),
            HealthAction::Node { node_id } => checker.check_node(node_id),
            HealthAction::Replication => checker.check_replication(),
        }
    }

    fn handle_backup(action: BackupAction, config_path: &PathBuf) -> Result<()> {
        let manager = BackupManager::new(config_path)?;

        match action {
            BackupAction::Create {
                destination,
                backup_type,
            } => manager.create_backup(&destination, &backup_type),
            BackupAction::List { location } => manager.list_backups(&location),
            BackupAction::Restore {
                source,
                lsn,
                dry_run,
            } => manager.restore_with_options(&source, lsn, dry_run),
            BackupAction::Verify { path } => manager.verify(&path),
        }
    }

    fn handle_config(action: ConfigAction, config_path: &PathBuf) -> Result<()> {
        let manager = ConfigManager::new(config_path)?;

        match action {
            ConfigAction::Show => manager.show(),
            ConfigAction::Validate { path } => manager.validate(&path),
            ConfigAction::Set { key, value } => manager.set(&key, &value),
            ConfigAction::Get { key } => manager.get(&key),
        }
    }

    fn handle_index(action: IndexAction, config_path: &PathBuf) -> Result<()> {
        let manager = IndexAdmin::new(config_path)?;
        match action {
            IndexAction::Rebuild {
                db_path,
                size_mb,
                catalog,
                backend,
            } => manager.rebuild(&db_path, size_mb, catalog, &backend),
            IndexAction::List { catalog } => manager.list(&catalog),
        }
    }

    fn authorize(provided_token: Option<&str>) -> Result<()> {
        let required = match std::env::var("OWL_ADMIN_TOKEN") {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(()),
        };

        let candidate = provided_token
            .map(|s| s.to_string())
            .or_else(|| std::env::var("OWL_ADMIN_TOKEN_INPUT").ok())
            .unwrap_or_default();
        if candidate != required {
            bail!("unauthorized: missing or invalid admin token");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AdminCli;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_authorize_allows_when_no_required_token() {
        let _guard = env_lock().lock().expect("env lock");
        std::env::remove_var("OWL_ADMIN_TOKEN");
        std::env::remove_var("OWL_ADMIN_TOKEN_INPUT");
        let result = AdminCli::authorize(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_authorize_rejects_missing_or_wrong_token() {
        let _guard = env_lock().lock().expect("env lock");
        std::env::set_var("OWL_ADMIN_TOKEN", "secret");
        std::env::remove_var("OWL_ADMIN_TOKEN_INPUT");
        assert!(AdminCli::authorize(None).is_err());
        assert!(AdminCli::authorize(Some("wrong")).is_err());
    }

    #[test]
    fn test_authorize_accepts_matching_token() {
        let _guard = env_lock().lock().expect("env lock");
        std::env::set_var("OWL_ADMIN_TOKEN", "secret");
        std::env::remove_var("OWL_ADMIN_TOKEN_INPUT");
        assert!(AdminCli::authorize(Some("secret")).is_ok());
    }
}
