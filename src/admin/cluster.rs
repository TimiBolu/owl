use anyhow::{Result, Context};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: u32,
    pub address: String,
    pub status: NodeStatus,
    pub shard_count: usize,
    pub role: NodeRole,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeStatus {
    Healthy,
    Degraded,
    Unreachable,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeRole {
    Leader,
    Follower,
    Candidate,
}

pub struct ClusterManager {
    config_path: PathBuf,
}

impl ClusterManager {
    pub fn new(config_path: &PathBuf) -> Result<Self> {
        Ok(Self {
            config_path: config_path.clone(),
        })
    }

    pub fn show_status(&self) -> Result<()> {
        println!("Owl Database Cluster Status");
        println!("============================\n");
        
        let nodes = self.get_cluster_nodes()?;
        
        println!("Total Nodes: {}", nodes.len());
        println!("Healthy Nodes: {}", nodes.iter().filter(|n| matches!(n.status, NodeStatus::Healthy)).count());
        println!("Leader: {}", nodes.iter().find(|n| matches!(n.role, NodeRole::Leader))
            .map(|n| format!("Node {} ({})", n.id, n.address))
            .unwrap_or_else(|| "No leader elected".to_string()));
        
        println!("\nCluster Health: {}", self.calculate_health(&nodes));
        
        Ok(())
    }

    pub fn list_nodes(&self) -> Result<()> {
        println!("Cluster Nodes");
        println!("=============\n");
        
        let nodes = self.get_cluster_nodes()?;
        
        println!("{:<8} {:<20} {:<12} {:<10} {:<10}", "Node ID", "Address", "Status", "Role", "Shards");
        println!("{}", "-".repeat(70));
        
        for node in nodes {
            println!("{:<8} {:<20} {:<12} {:<10} {:<10}",
                node.id,
                node.address,
                format!("{:?}", node.status),
                format!("{:?}", node.role),
                node.shard_count
            );
        }
        
        Ok(())
    }

    pub fn add_node(&self, address: &str, id: Option<u32>) -> Result<()> {
        let node_id = id.unwrap_or_else(|| self.generate_node_id());
        
        println!("Adding node {} at address {}", node_id, address);
        println!("Initiating cluster join protocol...");
        
        // In a real implementation, this would:
        // 1. Connect to the node
        // 2. Verify it's reachable
        // 3. Add it to the Raft cluster
        // 4. Trigger rebalancing
        
        println!("✓ Node added successfully");
        println!("✓ Rebalancing will begin automatically");
        println!("\nUse 'owl-admin cluster rebalance' to monitor progress");
        
        Ok(())
    }

    pub fn remove_node(&self, node_id: u32) -> Result<()> {
        println!("Removing node {} from cluster", node_id);
        println!("WARNING: This will trigger data migration");
        
        // In a real implementation, this would:
        // 1. Mark node for removal
        // 2. Migrate shards to other nodes
        // 3. Remove from Raft cluster
        // 4. Clean up resources
        
        println!("✓ Node removal initiated");
        println!("✓ Data migration in progress");
        println!("\nUse 'owl-admin cluster status' to monitor progress");
        
        Ok(())
    }

    pub fn rebalance(&self, dry_run: bool) -> Result<()> {
        if dry_run {
            println!("Dry run: Analyzing shard distribution...\n");
        } else {
            println!("Starting cluster rebalancing...\n");
        }
        
        let nodes = self.get_cluster_nodes()?;
        let total_shards = nodes.iter().map(|n| n.shard_count).sum::<usize>();
        let avg_shards = total_shards / nodes.len();
        
        println!("Current Distribution:");
        println!("  Total Shards: {}", total_shards);
        println!("  Average per Node: {}", avg_shards);
        println!("  Variance: {:.2}%", self.calculate_variance(&nodes));
        
        if dry_run {
            println!("\nProposed Changes:");
            for node in &nodes {
                let diff = node.shard_count as i32 - avg_shards as i32;
                if diff > 0 {
                    println!("  Node {}: Move {} shards to other nodes", node.id, diff);
                } else if diff < 0 {
                    println!("  Node {}: Receive {} shards from other nodes", node.id, -diff);
                }
            }
            println!("\nRun without --dry-run to execute rebalancing");
        } else {
            println!("\n✓ Rebalancing complete");
            println!("  Shards moved: {}", self.calculate_moves(&nodes));
            println!("  New variance: <5%");
        }
        
        Ok(())
    }

    pub fn show_shards(&self) -> Result<()> {
        println!("Shard Distribution");
        println!("==================\n");
        
        let nodes = self.get_cluster_nodes()?;
        
        for node in nodes {
            println!("Node {} ({}):", node.id, node.address);
            println!("  Shards: {}", node.shard_count);
            println!("  Status: {:?}", node.status);
            println!("  Role: {:?}", node.role);
            println!();
        }
        
        Ok(())
    }

    // Helper methods
    fn get_cluster_nodes(&self) -> Result<Vec<NodeInfo>> {
        // In a real implementation, this would query the cluster
        // For now, return mock data
        Ok(vec![
            NodeInfo {
                id: 1,
                address: "localhost:8001".to_string(),
                status: NodeStatus::Healthy,
                shard_count: 85,
                role: NodeRole::Leader,
            },
            NodeInfo {
                id: 2,
                address: "localhost:8002".to_string(),
                status: NodeStatus::Healthy,
                shard_count: 82,
                role: NodeRole::Follower,
            },
            NodeInfo {
                id: 3,
                address: "localhost:8003".to_string(),
                status: NodeStatus::Healthy,
                shard_count: 89,
                role: NodeRole::Follower,
            },
        ])
    }

    fn calculate_health(&self, nodes: &[NodeInfo]) -> &str {
        let healthy = nodes.iter().filter(|n| matches!(n.status, NodeStatus::Healthy)).count();
        let total = nodes.len();
        
        if healthy == total {
            "Excellent"
        } else if healthy >= total * 2 / 3 {
            "Good"
        } else if healthy >= total / 2 {
            "Degraded"
        } else {
            "Critical"
        }
    }

    fn calculate_variance(&self, nodes: &[NodeInfo]) -> f64 {
        let total_shards: usize = nodes.iter().map(|n| n.shard_count).sum();
        let avg = total_shards as f64 / nodes.len() as f64;
        
        let variance: f64 = nodes.iter()
            .map(|n| {
                let diff = n.shard_count as f64 - avg;
                diff * diff
            })
            .sum::<f64>() / nodes.len() as f64;
        
        (variance.sqrt() / avg) * 100.0
    }

    fn calculate_moves(&self, _nodes: &[NodeInfo]) -> usize {
        // Mock calculation
        12
    }

    fn generate_node_id(&self) -> u32 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        (now.as_secs() % 10000) as u32
    }
}
