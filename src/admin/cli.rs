use clap::{Parser, Subcommand};
use anyhow::Result;
use std::path::PathBuf;

use super::{ClusterManager, HealthChecker, BackupManager, ConfigManager};

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

impl AdminCli {
    pub fn run() -> Result<()> {
        let cli = AdminCli::parse();
        
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
            BackupAction::Create { destination, backup_type } => {
                manager.create_backup(&destination, &backup_type)
            }
            BackupAction::List { location } => manager.list_backups(&location),
            BackupAction::Restore { source, lsn } => manager.restore(&source, lsn),
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
}
