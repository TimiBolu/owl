use anyhow::Result;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthReport {
    pub overall_status: HealthStatus,
    pub node_health: Vec<NodeHealth>,
    pub replication_lag: Vec<ReplicationLag>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Critical,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeHealth {
    pub node_id: u32,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub disk_usage: f64,
    pub query_latency_p99: f64,
    pub status: HealthStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplicationLag {
    pub follower_id: u32,
    pub leader_id: u32,
    pub lag_ms: u64,
}

pub struct HealthChecker {
    config_path: PathBuf,
}

impl HealthChecker {
    pub fn new(config_path: &PathBuf) -> Result<Self> {
        Ok(Self {
            config_path: config_path.clone(),
        })
    }

    pub fn check_cluster(&self) -> Result<()> {
        println!("Owl Database Health Check");
        println!("=========================\n");
        
        let report = self.generate_health_report()?;
        
        println!("Overall Status: {:?}\n", report.overall_status);
        
        println!("Node Health:");
        println!("{:<8} {:<10} {:<12} {:<12} {:<15} {:<10}",
            "Node ID", "Status", "CPU %", "Memory %", "Disk %", "P99 Latency");
        println!("{}", "-".repeat(75));
        
        for node in &report.node_health {
            println!("{:<8} {:<10} {:<12.1} {:<12.1} {:<15.1} {:<10.2}ms",
                node.node_id,
                format!("{:?}", node.status),
                node.cpu_usage,
                node.memory_usage,
                node.disk_usage,
                node.query_latency_p99
            );
        }
        
        if !report.replication_lag.is_empty() {
            println!("\nReplication Status:");
            println!("{:<12} {:<12} {:<15}", "Follower", "Leader", "Lag (ms)");
            println!("{}", "-".repeat(40));
            
            for lag in &report.replication_lag {
                println!("{:<12} {:<12} {:<15}",
                    format!("Node {}", lag.follower_id),
                    format!("Node {}", lag.leader_id),
                    lag.lag_ms
                );
            }
        }
        
        if !report.warnings.is_empty() {
            println!("\nWarnings:");
            for warning in &report.warnings {
                println!("  ⚠ {}", warning);
            }
        }
        
        println!("\n✓ Health check complete");
        
        Ok(())
    }

    pub fn monitor(&self, interval: u64) -> Result<()> {
        println!("Starting health monitor (refresh every {}s)", interval);
        println!("Press Ctrl+C to stop\n");
        
        loop {
            // Clear screen (ANSI escape code)
            print!("\x1B[2J\x1B[1;1H");
            
            let report = self.generate_health_report()?;
            
            println!("Owl Database Health Monitor - {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
            println!("Overall Status: {:?}\n", report.overall_status);
            
            println!("{:<8} {:<10} {:<10} {:<10} {:<10} {:<12}",
                "Node", "Status", "CPU %", "Mem %", "Disk %", "P99 (ms)");
            println!("{}", "-".repeat(70));
            
            for node in &report.node_health {
                let status_icon = match node.status {
                    HealthStatus::Healthy => "✓",
                    HealthStatus::Warning => "⚠",
                    HealthStatus::Critical => "✗",
                };
                
                println!("{:<8} {:<10} {:<10.1} {:<10.1} {:<10.1} {:<12.2}",
                    format!("{} {}", status_icon, node.node_id),
                    format!("{:?}", node.status),
                    node.cpu_usage,
                    node.memory_usage,
                    node.disk_usage,
                    node.query_latency_p99
                );
            }
            
            if !report.warnings.is_empty() {
                println!("\nActive Warnings: {}", report.warnings.len());
            }
            
            thread::sleep(Duration::from_secs(interval));
        }
    }

    pub fn check_node(&self, node_id: u32) -> Result<()> {
        println!("Health Check for Node {}", node_id);
        println!("========================\n");
        
        let report = self.generate_health_report()?;
        
        if let Some(node) = report.node_health.iter().find(|n| n.node_id == node_id) {
            println!("Status: {:?}", node.status);
            println!("CPU Usage: {:.1}%", node.cpu_usage);
            println!("Memory Usage: {:.1}%", node.memory_usage);
            println!("Disk Usage: {:.1}%", node.disk_usage);
            println!("P99 Query Latency: {:.2}ms", node.query_latency_p99);
            
            println!("\nDetailed Metrics:");
            println!("  Active Connections: {}", self.get_active_connections(node_id)?);
            println!("  Queries/sec: {}", self.get_qps(node_id)?);
            println!("  Writes/sec: {}", self.get_wps(node_id)?);
            println!("  Cache Hit Rate: {:.1}%", self.get_cache_hit_rate(node_id)?);
            
            if matches!(node.status, HealthStatus::Warning | HealthStatus::Critical) {
                println!("\nIssues Detected:");
                if node.cpu_usage > 80.0 {
                    println!("  ⚠ High CPU usage");
                }
                if node.memory_usage > 85.0 {
                    println!("  ⚠ High memory usage");
                }
                if node.disk_usage > 90.0 {
                    println!("  ⚠ High disk usage");
                }
                if node.query_latency_p99 > 10.0 {
                    println!("  ⚠ High query latency");
                }
            }
        } else {
            println!("Node {} not found in cluster", node_id);
        }
        
        Ok(())
    }

    pub fn check_replication(&self) -> Result<()> {
        println!("Replication Status");
        println!("==================\n");
        
        let report = self.generate_health_report()?;
        
        if report.replication_lag.is_empty() {
            println!("No replication lag detected");
            println!("All followers are in sync with leader");
        } else {
            println!("{:<12} {:<12} {:<15} {:<10}", "Follower", "Leader", "Lag (ms)", "Status");
            println!("{}", "-".repeat(50));
            
            for lag in &report.replication_lag {
                let status = if lag.lag_ms < 10 {
                    "✓ Healthy"
                } else if lag.lag_ms < 100 {
                    "⚠ Warning"
                } else {
                    "✗ Critical"
                };
                
                println!("{:<12} {:<12} {:<15} {:<10}",
                    format!("Node {}", lag.follower_id),
                    format!("Node {}", lag.leader_id),
                    lag.lag_ms,
                    status
                );
            }
            
            let max_lag = report.replication_lag.iter().map(|l| l.lag_ms).max().unwrap_or(0);
            println!("\nMaximum Lag: {}ms", max_lag);
            
            if max_lag > 100 {
                println!("\n⚠ WARNING: High replication lag detected");
                println!("  Consider investigating network issues or follower performance");
            }
        }
        
        Ok(())
    }

    // Helper methods
    fn generate_health_report(&self) -> Result<HealthReport> {
        // In a real implementation, this would query actual cluster metrics
        // For now, return mock data
        Ok(HealthReport {
            overall_status: HealthStatus::Healthy,
            node_health: vec![
                NodeHealth {
                    node_id: 1,
                    cpu_usage: 45.2,
                    memory_usage: 62.8,
                    disk_usage: 58.3,
                    query_latency_p99: 0.8,
                    status: HealthStatus::Healthy,
                },
                NodeHealth {
                    node_id: 2,
                    cpu_usage: 52.1,
                    memory_usage: 68.5,
                    disk_usage: 61.2,
                    query_latency_p99: 0.9,
                    status: HealthStatus::Healthy,
                },
                NodeHealth {
                    node_id: 3,
                    cpu_usage: 48.7,
                    memory_usage: 65.3,
                    disk_usage: 59.8,
                    query_latency_p99: 0.7,
                    status: HealthStatus::Healthy,
                },
            ],
            replication_lag: vec![
                ReplicationLag {
                    follower_id: 2,
                    leader_id: 1,
                    lag_ms: 5,
                },
                ReplicationLag {
                    follower_id: 3,
                    leader_id: 1,
                    lag_ms: 8,
                },
            ],
            warnings: vec![],
        })
    }

    fn get_active_connections(&self, _node_id: u32) -> Result<usize> {
        Ok(42)
    }

    fn get_qps(&self, _node_id: u32) -> Result<usize> {
        Ok(15234)
    }

    fn get_wps(&self, _node_id: u32) -> Result<usize> {
        Ok(8521)
    }

    fn get_cache_hit_rate(&self, _node_id: u32) -> Result<f64> {
        Ok(94.5)
    }
}
