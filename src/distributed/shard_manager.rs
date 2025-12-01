// Shard Manager for Cluster Coordination
//
// This module manages shard membership, rebalancing, and cluster coordination.
// Provides online shard operations without downtime.

use crate::distributed::consistent_hash::{ConsistentHashRing, ShardId};
use crate::error::{OQLError, OQLResult};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Node identifier in the cluster
pub type NodeId = String;

/// Shard information
#[derive(Debug, Clone)]
pub struct ShardInfo {
    /// Unique shard identifier
    pub shard_id: ShardId,
    /// Node that owns this shard
    pub node_id: NodeId,
    /// Shard status
    pub status: ShardStatus,
    /// When this shard was created
    pub created_at: Instant,
    /// Last heartbeat from the owning node
    pub last_heartbeat: Instant,
    /// Estimated data size in bytes
    pub data_size: u64,
    /// Number of objects in this shard
    pub object_count: u64,
}

/// Shard status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShardStatus {
    /// Shard is active and serving requests
    Active,
    /// Shard is being created/initialized
    Initializing,
    /// Shard is being migrated to another node
    Migrating { target_node: NodeId },
    /// Shard is being drained (preparing for removal)
    Draining,
    /// Shard is offline/unavailable
    Offline,
}

/// Node information
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Unique node identifier
    pub node_id: NodeId,
    /// Network address for communication
    pub address: String,
    /// Node status
    pub status: NodeStatus,
    /// When this node joined the cluster
    pub joined_at: Instant,
    /// Last heartbeat from this node
    pub last_heartbeat: Instant,
    /// Shards owned by this node
    pub shards: HashSet<ShardId>,
    /// Node capacity metrics
    pub capacity: NodeCapacity,
}

/// Node status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeStatus {
    /// Node is active and healthy
    Active,
    /// Node is joining the cluster
    Joining,
    /// Node is leaving the cluster
    Leaving,
    /// Node is suspected to be down
    Suspected,
    /// Node is confirmed down
    Down,
}

/// Node capacity metrics
#[derive(Debug, Clone)]
pub struct NodeCapacity {
    /// Available memory in bytes
    pub memory_bytes: u64,
    /// Available disk space in bytes
    pub disk_bytes: u64,
    /// CPU cores available
    pub cpu_cores: u32,
    /// Current load (0.0 to 1.0)
    pub load: f64,
}

/// Rebalancing operation
#[derive(Debug, Clone)]
pub struct RebalanceOperation {
    /// Operation identifier
    pub operation_id: String,
    /// Shard being moved
    pub shard_id: ShardId,
    /// Source node
    pub source_node: NodeId,
    /// Target node
    pub target_node: NodeId,
    /// Operation status
    pub status: RebalanceStatus,
    /// When operation started
    pub started_at: Instant,
    /// Estimated completion time
    pub estimated_completion: Option<Instant>,
    /// Progress (0.0 to 1.0)
    pub progress: f64,
}

/// Rebalancing operation status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebalanceStatus {
    /// Operation is planned but not started
    Planned,
    /// Data is being copied to target
    Copying,
    /// Switching traffic to target
    Switching,
    /// Cleaning up source data
    Cleanup,
    /// Operation completed successfully
    Completed,
    /// Operation failed
    Failed { reason: String },
}

/// Shard manager for cluster coordination
///
/// The ShardManager is responsible for:
/// - Maintaining cluster membership
/// - Coordinating shard placement and rebalancing
/// - Monitoring node health
/// - Managing online operations without downtime
///
/// # Example
///
/// ```
/// use oql::distributed::ShardManager;
///
/// let mut manager = ShardManager::new();
/// 
/// // Add nodes to cluster
/// manager.add_node("node1", "192.168.1.10:8080").unwrap();
/// manager.add_node("node2", "192.168.1.11:8080").unwrap();
/// 
/// // Create initial shards
/// manager.create_shard("node1").unwrap();
/// manager.create_shard("node2").unwrap();
/// ```
pub struct ShardManager {
    /// Consistent hash ring for shard distribution
    hash_ring: Arc<RwLock<ConsistentHashRing>>,
    /// Shard registry
    shards: Arc<RwLock<HashMap<ShardId, ShardInfo>>>,
    /// Node registry
    nodes: Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
    /// Active rebalancing operations
    rebalance_ops: Arc<RwLock<HashMap<String, RebalanceOperation>>>,
    /// Next shard ID to assign
    next_shard_id: Arc<RwLock<ShardId>>,
    /// Heartbeat timeout duration
    heartbeat_timeout: Duration,
    /// Rebalancing configuration
    rebalance_config: RebalanceConfig,
}

/// Rebalancing configuration
#[derive(Debug, Clone)]
pub struct RebalanceConfig {
    /// Maximum number of concurrent rebalance operations
    pub max_concurrent_ops: usize,
    /// Target load balance threshold (0.0 to 1.0)
    pub load_balance_threshold: f64,
    /// Minimum data size to trigger rebalancing (bytes)
    pub min_rebalance_size: u64,
    /// Maximum time to wait for rebalancing completion
    pub max_rebalance_time: Duration,
}

impl Default for RebalanceConfig {
    fn default() -> Self {
        Self {
            max_concurrent_ops: 3,
            load_balance_threshold: 0.1, // 10% imbalance triggers rebalancing
            min_rebalance_size: 100 * 1024 * 1024, // 100MB minimum
            max_rebalance_time: Duration::from_secs(2 * 60 * 60), // 2 hours
        }
    }
}

impl ShardManager {
    /// Create a new shard manager
    pub fn new() -> Self {
        Self::with_config(RebalanceConfig::default())
    }

    /// Create a new shard manager with custom configuration
    pub fn with_config(config: RebalanceConfig) -> Self {
        Self {
            hash_ring: Arc::new(RwLock::new(ConsistentHashRing::new())),
            shards: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            rebalance_ops: Arc::new(RwLock::new(HashMap::new())),
            next_shard_id: Arc::new(RwLock::new(0)),
            heartbeat_timeout: Duration::from_secs(30),
            rebalance_config: config,
        }
    }

    /// Add a node to the cluster
    ///
    /// # Arguments
    /// * `node_id` - Unique identifier for the node
    /// * `address` - Network address for communication
    ///
    /// # Returns
    /// * `Ok(())` if node was added successfully
    /// * `Err(OQLError)` if node already exists or other error
    pub fn add_node(&self, node_id: &str, address: &str) -> OQLResult<()> {
        let mut nodes = self.nodes.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;

        if nodes.contains_key(node_id) {
            return Err(OQLError::generic(&format!("Node {} already exists", node_id)));
        }

        let node_info = NodeInfo {
            node_id: node_id.to_string(),
            address: address.to_string(),
            status: NodeStatus::Joining,
            joined_at: Instant::now(),
            last_heartbeat: Instant::now(),
            shards: HashSet::new(),
            capacity: NodeCapacity {
                memory_bytes: 0,
                disk_bytes: 0,
                cpu_cores: 0,
                load: 0.0,
            },
        };

        nodes.insert(node_id.to_string(), node_info);
        Ok(())
    }

    /// Remove a node from the cluster
    ///
    /// This initiates graceful node removal by draining its shards.
    ///
    /// # Arguments
    /// * `node_id` - Identifier of the node to remove
    ///
    /// # Returns
    /// * `Ok(())` if removal was initiated
    /// * `Err(OQLError)` if node doesn't exist or has active shards
    pub fn remove_node(&self, node_id: &str) -> OQLResult<()> {
        let mut nodes = self.nodes.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;

        let node = nodes.get_mut(node_id)
            .ok_or_else(|| OQLError::generic(&format!("Node {} not found", node_id)))?;

        if !node.shards.is_empty() {
            // Start draining shards
            node.status = NodeStatus::Leaving;
            drop(nodes);
            
            // Initiate shard migration
            self.drain_node_shards(node_id)?;
            return Ok(());
        }

        // Node has no shards, can remove immediately
        nodes.remove(node_id);
        Ok(())
    }

    /// Create a new shard on the specified node
    ///
    /// # Arguments
    /// * `node_id` - Node to create the shard on
    ///
    /// # Returns
    /// * `Ok(shard_id)` if shard was created successfully
    /// * `Err(OQLError)` if node doesn't exist or other error
    pub fn create_shard(&self, node_id: &str) -> OQLResult<ShardId> {
        // Check if node exists and is active
        {
            let nodes = self.nodes.read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
            
            let node = nodes.get(node_id)
                .ok_or_else(|| OQLError::generic(&format!("Node {} not found", node_id)))?;
            
            if node.status != NodeStatus::Active && node.status != NodeStatus::Joining {
                return Err(OQLError::generic(&format!("Node {} is not active", node_id)));
            }
        }

        // Allocate new shard ID
        let shard_id = {
            let mut next_id = self.next_shard_id.write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            let id = *next_id;
            *next_id += 1;
            id
        };

        // Create shard info
        let shard_info = ShardInfo {
            shard_id,
            node_id: node_id.to_string(),
            status: ShardStatus::Initializing,
            created_at: Instant::now(),
            last_heartbeat: Instant::now(),
            data_size: 0,
            object_count: 0,
        };

        // Add to shard registry
        {
            let mut shards = self.shards.write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            shards.insert(shard_id, shard_info);
        }

        // Add to node's shard list
        {
            let mut nodes = self.nodes.write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            if let Some(node) = nodes.get_mut(node_id) {
                node.shards.insert(shard_id);
            }
        }

        // Add to hash ring
        {
            let mut ring = self.hash_ring.write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            ring.add_shard(shard_id);
        }

        Ok(shard_id)
    }

    /// Mark a shard as active (ready to serve requests)
    pub fn activate_shard(&self, shard_id: ShardId) -> OQLResult<()> {
        let mut shards = self.shards.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;

        let shard = shards.get_mut(&shard_id)
            .ok_or_else(|| OQLError::generic(&format!("Shard {} not found", shard_id)))?;

        shard.status = ShardStatus::Active;
        Ok(())
    }

    /// Update node heartbeat and capacity metrics
    pub fn update_node_heartbeat(&self, node_id: &str, capacity: NodeCapacity) -> OQLResult<()> {
        let mut nodes = self.nodes.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;

        let node = nodes.get_mut(node_id)
            .ok_or_else(|| OQLError::generic(&format!("Node {} not found", node_id)))?;

        node.last_heartbeat = Instant::now();
        node.capacity = capacity;
        
        // Activate joining nodes
        if node.status == NodeStatus::Joining {
            node.status = NodeStatus::Active;
        }

        Ok(())
    }

    /// Update shard statistics
    pub fn update_shard_stats(&self, shard_id: ShardId, data_size: u64, object_count: u64) -> OQLResult<()> {
        let mut shards = self.shards.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;

        let shard = shards.get_mut(&shard_id)
            .ok_or_else(|| OQLError::generic(&format!("Shard {} not found", shard_id)))?;

        shard.data_size = data_size;
        shard.object_count = object_count;
        shard.last_heartbeat = Instant::now();

        Ok(())
    }

    /// Get shard information
    pub fn get_shard(&self, shard_id: ShardId) -> OQLResult<Option<ShardInfo>> {
        let shards = self.shards.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(shards.get(&shard_id).cloned())
    }

    /// Get node information
    pub fn get_node(&self, node_id: &str) -> OQLResult<Option<NodeInfo>> {
        let nodes = self.nodes.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(nodes.get(node_id).cloned())
    }

    /// List all active shards
    pub fn list_shards(&self) -> OQLResult<Vec<ShardInfo>> {
        let shards = self.shards.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(shards.values().cloned().collect())
    }

    /// List all nodes
    pub fn list_nodes(&self) -> OQLResult<Vec<NodeInfo>> {
        let nodes = self.nodes.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(nodes.values().cloned().collect())
    }

    /// Get the shard responsible for an object
    pub fn get_shard_for_object(&self, object_id: &str) -> OQLResult<Option<ShardId>> {
        let ring = self.hash_ring.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        Ok(ring.get_shard_for_object(object_id))
    }

    /// Check cluster health and detect failed nodes
    pub fn check_cluster_health(&self) -> OQLResult<ClusterHealth> {
        let nodes = self.nodes.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        let shards = self.shards.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;

        let now = Instant::now();
        let mut healthy_nodes = 0;
        let mut suspected_nodes = Vec::new();
        let mut down_nodes = Vec::new();

        for (node_id, node) in nodes.iter() {
            let time_since_heartbeat = now.duration_since(node.last_heartbeat);
            
            if time_since_heartbeat > self.heartbeat_timeout {
                if node.status == NodeStatus::Suspected {
                    down_nodes.push(node_id.clone());
                } else {
                    suspected_nodes.push(node_id.clone());
                }
            } else if node.status == NodeStatus::Active {
                healthy_nodes += 1;
            }
        }

        let total_shards = shards.len();
        let active_shards = shards.values()
            .filter(|s| s.status == ShardStatus::Active)
            .count();

        Ok(ClusterHealth {
            total_nodes: nodes.len(),
            healthy_nodes,
            suspected_nodes,
            down_nodes,
            total_shards,
            active_shards,
            rebalancing_operations: self.rebalance_ops.read().unwrap().len(),
        })
    }

    /// Trigger automatic rebalancing if needed
    pub fn trigger_rebalancing(&self) -> OQLResult<Vec<String>> {
        let nodes = self.nodes.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        let shards = self.shards.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;

        // Calculate load per node
        let mut node_loads = HashMap::new();
        for (node_id, node) in nodes.iter() {
            if node.status == NodeStatus::Active {
                let total_data: u64 = node.shards.iter()
                    .filter_map(|shard_id| shards.get(shard_id))
                    .map(|shard| shard.data_size)
                    .sum();
                node_loads.insert(node_id.clone(), total_data);
            }
        }

        if node_loads.is_empty() {
            return Ok(Vec::new());
        }

        // Find imbalanced nodes
        let total_data: u64 = node_loads.values().sum();
        let average_load = total_data as f64 / node_loads.len() as f64;
        let threshold = average_load * self.rebalance_config.load_balance_threshold;

        let mut operations = Vec::new();
        
        // Find overloaded and underloaded nodes
        let mut overloaded = Vec::new();
        let mut underloaded = Vec::new();
        
        for (node_id, &load) in &node_loads {
            let load_f64 = load as f64;
            if load_f64 > average_load + threshold {
                overloaded.push((node_id.clone(), load));
            } else if load_f64 < average_load - threshold {
                underloaded.push((node_id.clone(), load));
            }
        }

        // Create rebalancing operations
        for (overloaded_node, _) in overloaded {
            if let Some((underloaded_node, _)) = underloaded.first() {
                if let Some(shard_to_move) = self.find_shard_to_move(&overloaded_node, &shards)? {
                    let op_id = self.create_rebalance_operation(
                        shard_to_move,
                        &overloaded_node,
                        underloaded_node,
                    )?;
                    operations.push(op_id);
                }
            }
        }

        Ok(operations)
    }

    /// Drain all shards from a node (for node removal)
    fn drain_node_shards(&self, node_id: &str) -> OQLResult<()> {
        let nodes = self.nodes.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        
        let node = nodes.get(node_id)
            .ok_or_else(|| OQLError::generic(&format!("Node {} not found", node_id)))?;
        
        let shards_to_drain: Vec<ShardId> = node.shards.iter().copied().collect();
        drop(nodes);

        // Find target nodes for each shard
        let active_nodes: Vec<String> = {
            let nodes = self.nodes.read().unwrap();
            nodes.values()
                .filter(|n| n.status == NodeStatus::Active && n.node_id != node_id)
                .map(|n| n.node_id.clone())
                .collect()
        };

        if active_nodes.is_empty() {
            return Err(OQLError::generic("No active nodes available for shard migration"));
        }

        // Create migration operations
        for (i, shard_id) in shards_to_drain.iter().enumerate() {
            let target_node = &active_nodes[i % active_nodes.len()];
            self.create_rebalance_operation(*shard_id, node_id, target_node)?;
        }

        Ok(())
    }

    /// Find a suitable shard to move from an overloaded node
    fn find_shard_to_move(&self, node_id: &str, shards: &HashMap<ShardId, ShardInfo>) -> OQLResult<Option<ShardId>> {
        let nodes = self.nodes.read()
            .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
        
        let node = nodes.get(node_id)
            .ok_or_else(|| OQLError::generic(&format!("Node {} not found", node_id)))?;

        // Find the largest shard that meets minimum size requirement
        let mut best_shard = None;
        let mut best_size = 0;

        for &shard_id in &node.shards {
            if let Some(shard) = shards.get(&shard_id) {
                if shard.status == ShardStatus::Active 
                    && shard.data_size >= self.rebalance_config.min_rebalance_size
                    && shard.data_size > best_size {
                    best_shard = Some(shard_id);
                    best_size = shard.data_size;
                }
            }
        }

        Ok(best_shard)
    }

    /// Create a new rebalancing operation
    fn create_rebalance_operation(&self, shard_id: ShardId, source_node: &str, target_node: &str) -> OQLResult<String> {
        let operation_id = format!("rebalance_{}_{}", shard_id, Instant::now().elapsed().as_nanos());
        
        let operation = RebalanceOperation {
            operation_id: operation_id.clone(),
            shard_id,
            source_node: source_node.to_string(),
            target_node: target_node.to_string(),
            status: RebalanceStatus::Planned,
            started_at: Instant::now(),
            estimated_completion: None,
            progress: 0.0,
        };

        let mut ops = self.rebalance_ops.write()
            .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
        ops.insert(operation_id.clone(), operation);

        Ok(operation_id)
    }
}

impl Default for ShardManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Cluster health information
#[derive(Debug, Clone)]
pub struct ClusterHealth {
    /// Total number of nodes in cluster
    pub total_nodes: usize,
    /// Number of healthy nodes
    pub healthy_nodes: usize,
    /// Nodes suspected to be down
    pub suspected_nodes: Vec<NodeId>,
    /// Nodes confirmed to be down
    pub down_nodes: Vec<NodeId>,
    /// Total number of shards
    pub total_shards: usize,
    /// Number of active shards
    pub active_shards: usize,
    /// Number of ongoing rebalancing operations
    pub rebalancing_operations: usize,
}

impl ClusterHealth {
    /// Check if cluster is healthy
    pub fn is_healthy(&self) -> bool {
        self.suspected_nodes.is_empty() 
            && self.down_nodes.is_empty()
            && self.active_shards == self.total_shards
    }

    /// Get cluster availability (0.0 to 1.0)
    pub fn availability(&self) -> f64 {
        if self.total_shards == 0 {
            return 1.0;
        }
        self.active_shards as f64 / self.total_shards as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_manager_creation() {
        let manager = ShardManager::new();
        let health = manager.check_cluster_health().unwrap();
        
        assert_eq!(health.total_nodes, 0);
        assert_eq!(health.total_shards, 0);
        assert!(health.is_healthy());
    }

    #[test]
    fn test_add_remove_node() {
        let manager = ShardManager::new();
        
        // Add node
        manager.add_node("node1", "192.168.1.10:8080").unwrap();
        let nodes = manager.list_nodes().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "node1");
        
        // Remove node (no shards)
        manager.remove_node("node1").unwrap();
        let nodes = manager.list_nodes().unwrap();
        assert_eq!(nodes.len(), 0);
    }

    #[test]
    fn test_create_shard() {
        let manager = ShardManager::new();
        manager.add_node("node1", "192.168.1.10:8080").unwrap();
        
        // Update heartbeat to activate node
        let capacity = NodeCapacity {
            memory_bytes: 1024 * 1024 * 1024,
            disk_bytes: 10 * 1024 * 1024 * 1024,
            cpu_cores: 4,
            load: 0.5,
        };
        manager.update_node_heartbeat("node1", capacity).unwrap();
        
        // Create shard
        let shard_id = manager.create_shard("node1").unwrap();
        assert_eq!(shard_id, 0);
        
        let shards = manager.list_shards().unwrap();
        assert_eq!(shards.len(), 1);
        assert_eq!(shards[0].shard_id, 0);
        assert_eq!(shards[0].node_id, "node1");
    }

    #[test]
    fn test_shard_activation() {
        let manager = ShardManager::new();
        manager.add_node("node1", "192.168.1.10:8080").unwrap();
        
        let capacity = NodeCapacity {
            memory_bytes: 1024 * 1024 * 1024,
            disk_bytes: 10 * 1024 * 1024 * 1024,
            cpu_cores: 4,
            load: 0.5,
        };
        manager.update_node_heartbeat("node1", capacity).unwrap();
        
        let shard_id = manager.create_shard("node1").unwrap();
        
        // Initially initializing
        let shard = manager.get_shard(shard_id).unwrap().unwrap();
        assert_eq!(shard.status, ShardStatus::Initializing);
        
        // Activate shard
        manager.activate_shard(shard_id).unwrap();
        let shard = manager.get_shard(shard_id).unwrap().unwrap();
        assert_eq!(shard.status, ShardStatus::Active);
    }

    #[test]
    fn test_object_shard_assignment() {
        let manager = ShardManager::new();
        manager.add_node("node1", "192.168.1.10:8080").unwrap();
        manager.add_node("node2", "192.168.1.11:8080").unwrap();
        
        let capacity = NodeCapacity {
            memory_bytes: 1024 * 1024 * 1024,
            disk_bytes: 10 * 1024 * 1024 * 1024,
            cpu_cores: 4,
            load: 0.5,
        };
        manager.update_node_heartbeat("node1", capacity.clone()).unwrap();
        manager.update_node_heartbeat("node2", capacity).unwrap();
        
        let shard1 = manager.create_shard("node1").unwrap();
        let shard2 = manager.create_shard("node2").unwrap();
        
        // Objects should be assigned to shards
        let shard = manager.get_shard_for_object("user123").unwrap();
        assert!(shard == Some(shard1) || shard == Some(shard2));
        
        // Same object should always get same shard
        let shard2 = manager.get_shard_for_object("user123").unwrap();
        assert_eq!(shard, shard2);
    }

    #[test]
    fn test_cluster_health() {
        let manager = ShardManager::new();
        manager.add_node("node1", "192.168.1.10:8080").unwrap();
        
        let capacity = NodeCapacity {
            memory_bytes: 1024 * 1024 * 1024,
            disk_bytes: 10 * 1024 * 1024 * 1024,
            cpu_cores: 4,
            load: 0.5,
        };
        manager.update_node_heartbeat("node1", capacity).unwrap();
        
        let shard_id = manager.create_shard("node1").unwrap();
        manager.activate_shard(shard_id).unwrap();
        
        let health = manager.check_cluster_health().unwrap();
        assert_eq!(health.total_nodes, 1);
        assert_eq!(health.healthy_nodes, 1);
        assert_eq!(health.total_shards, 1);
        assert_eq!(health.active_shards, 1);
        assert!(health.is_healthy());
        assert_eq!(health.availability(), 1.0);
    }

    #[test]
    fn test_shard_stats_update() {
        let manager = ShardManager::new();
        manager.add_node("node1", "192.168.1.10:8080").unwrap();
        
        let capacity = NodeCapacity {
            memory_bytes: 1024 * 1024 * 1024,
            disk_bytes: 10 * 1024 * 1024 * 1024,
            cpu_cores: 4,
            load: 0.5,
        };
        manager.update_node_heartbeat("node1", capacity).unwrap();
        
        let shard_id = manager.create_shard("node1").unwrap();
        
        // Update stats
        manager.update_shard_stats(shard_id, 1024 * 1024, 100).unwrap();
        
        let shard = manager.get_shard(shard_id).unwrap().unwrap();
        assert_eq!(shard.data_size, 1024 * 1024);
        assert_eq!(shard.object_count, 100);
    }

    #[test]
    fn test_duplicate_node_addition() {
        let manager = ShardManager::new();
        
        manager.add_node("node1", "192.168.1.10:8080").unwrap();
        let result = manager.add_node("node1", "192.168.1.10:8080");
        assert!(result.is_err());
    }

    #[test]
    fn test_nonexistent_node_operations() {
        let manager = ShardManager::new();
        
        // Try to create shard on non-existent node
        let result = manager.create_shard("nonexistent");
        assert!(result.is_err());
        
        // Try to remove non-existent node
        let result = manager.remove_node("nonexistent");
        assert!(result.is_err());
    }
}