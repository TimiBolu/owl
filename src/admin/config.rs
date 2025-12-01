use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use std::fs;
use serde::{Deserialize, Serialize};
use toml;

#[derive(Debug, Serialize, Deserialize)]
pub struct OwlConfig {
    pub cluster: ClusterConfig,
    pub storage: StorageConfig,
    pub replication: ReplicationConfig,
    pub performance: PerformanceConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub node_id: u32,
    pub listen_address: String,
    pub peers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StorageConfig {
    pub data_dir: PathBuf,
    pub wal_dir: PathBuf,
    pub max_arena_size_gb: u64,
    pub compression_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplicationConfig {
    pub replication_factor: usize,
    pub election_timeout_ms: u64,
    pub heartbeat_interval_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PerformanceConfig {
    pub max_connections: usize,
    pub query_timeout_ms: u64,
    pub cache_size_mb: usize,
}

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new(config_path: &PathBuf) -> Result<Self> {
        Ok(Self {
            config_path: config_path.clone(),
        })
    }

    pub fn show(&self) -> Result<()> {
        println!("Owl Database Configuration");
        println!("==========================\n");
        
        let config = self.load_config()?;
        
        println!("Cluster Configuration:");
        println!("  Node ID: {}", config.cluster.node_id);
        println!("  Listen Address: {}", config.cluster.listen_address);
        println!("  Peers: {}", config.cluster.peers.join(", "));
        
        println!("\nStorage Configuration:");
        println!("  Data Directory: {}", config.storage.data_dir.display());
        println!("  WAL Directory: {}", config.storage.wal_dir.display());
        println!("  Max Arena Size: {} GB", config.storage.max_arena_size_gb);
        println!("  Compression: {}", if config.storage.compression_enabled { "Enabled" } else { "Disabled" });
        
        println!("\nReplication Configuration:");
        println!("  Replication Factor: {}", config.replication.replication_factor);
        println!("  Election Timeout: {} ms", config.replication.election_timeout_ms);
        println!("  Heartbeat Interval: {} ms", config.replication.heartbeat_interval_ms);
        
        println!("\nPerformance Configuration:");
        println!("  Max Connections: {}", config.performance.max_connections);
        println!("  Query Timeout: {} ms", config.performance.query_timeout_ms);
        println!("  Cache Size: {} MB", config.performance.cache_size_mb);
        
        Ok(())
    }

    pub fn validate(&self, path: &Path) -> Result<()> {
        println!("Validating configuration file: {}", path.display());
        println!();
        
        let content = fs::read_to_string(path)
            .context("Failed to read configuration file")?;
        
        let config: OwlConfig = toml::from_str(&content)
            .context("Failed to parse configuration file")?;
        
        println!("✓ Configuration file is valid TOML");
        
        // Validate values
        let mut warnings = Vec::new();
        
        if config.cluster.peers.is_empty() {
            warnings.push("No peers configured - running in single-node mode");
        }
        
        if config.replication.replication_factor < 3 {
            warnings.push("Replication factor < 3 - reduced fault tolerance");
        }
        
        if config.storage.max_arena_size_gb < 1 {
            warnings.push("Arena size < 1GB - may limit database capacity");
        }
        
        if config.performance.max_connections < 10 {
            warnings.push("Max connections < 10 - may limit concurrency");
        }
        
        if warnings.is_empty() {
            println!("✓ All configuration values are valid");
        } else {
            println!("⚠ Configuration is valid but has warnings:");
            for warning in warnings {
                println!("  - {}", warning);
            }
        }
        
        println!("\n✓ Configuration validation complete");
        
        Ok(())
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        println!("Setting configuration: {} = {}", key, value);
        
        let mut config = self.load_config()?;
        
        // Parse key path (e.g., "cluster.node_id")
        let parts: Vec<&str> = key.split('.').collect();
        
        match parts.as_slice() {
            ["cluster", "node_id"] => {
                config.cluster.node_id = value.parse()
                    .context("Invalid node_id value")?;
            }
            ["cluster", "listen_address"] => {
                config.cluster.listen_address = value.to_string();
            }
            ["storage", "max_arena_size_gb"] => {
                config.storage.max_arena_size_gb = value.parse()
                    .context("Invalid max_arena_size_gb value")?;
            }
            ["storage", "compression_enabled"] => {
                config.storage.compression_enabled = value.parse()
                    .context("Invalid compression_enabled value (use true/false)")?;
            }
            ["replication", "replication_factor"] => {
                config.replication.replication_factor = value.parse()
                    .context("Invalid replication_factor value")?;
            }
            ["performance", "max_connections"] => {
                config.performance.max_connections = value.parse()
                    .context("Invalid max_connections value")?;
            }
            ["performance", "cache_size_mb"] => {
                config.performance.cache_size_mb = value.parse()
                    .context("Invalid cache_size_mb value")?;
            }
            _ => anyhow::bail!("Unknown configuration key: {}", key),
        }
        
        self.save_config(&config)?;
        
        println!("✓ Configuration updated");
        println!("  Restart the cluster for changes to take effect");
        
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<()> {
        let config = self.load_config()?;
        
        let value = match key {
            "cluster.node_id" => config.cluster.node_id.to_string(),
            "cluster.listen_address" => config.cluster.listen_address.clone(),
            "storage.data_dir" => config.storage.data_dir.display().to_string(),
            "storage.max_arena_size_gb" => config.storage.max_arena_size_gb.to_string(),
            "storage.compression_enabled" => config.storage.compression_enabled.to_string(),
            "replication.replication_factor" => config.replication.replication_factor.to_string(),
            "replication.election_timeout_ms" => config.replication.election_timeout_ms.to_string(),
            "performance.max_connections" => config.performance.max_connections.to_string(),
            "performance.cache_size_mb" => config.performance.cache_size_mb.to_string(),
            _ => anyhow::bail!("Unknown configuration key: {}", key),
        };
        
        println!("{} = {}", key, value);
        
        Ok(())
    }

    // Helper methods
    fn load_config(&self) -> Result<OwlConfig> {
        if !self.config_path.exists() {
            return Ok(self.default_config());
        }
        
        let content = fs::read_to_string(&self.config_path)
            .context("Failed to read configuration file")?;
        
        toml::from_str(&content)
            .context("Failed to parse configuration file")
    }

    fn save_config(&self, config: &OwlConfig) -> Result<()> {
        let toml = toml::to_string_pretty(config)
            .context("Failed to serialize configuration")?;
        
        fs::write(&self.config_path, toml)
            .context("Failed to write configuration file")?;
        
        Ok(())
    }

    fn default_config(&self) -> OwlConfig {
        OwlConfig {
            cluster: ClusterConfig {
                node_id: 1,
                listen_address: "0.0.0.0:8080".to_string(),
                peers: vec![],
            },
            storage: StorageConfig {
                data_dir: PathBuf::from("/var/lib/owl/data"),
                wal_dir: PathBuf::from("/var/lib/owl/wal"),
                max_arena_size_gb: 100,
                compression_enabled: true,
            },
            replication: ReplicationConfig {
                replication_factor: 3,
                election_timeout_ms: 2000,
                heartbeat_interval_ms: 500,
            },
            performance: PerformanceConfig {
                max_connections: 1000,
                query_timeout_ms: 30000,
                cache_size_mb: 1024,
            },
        }
    }
}
