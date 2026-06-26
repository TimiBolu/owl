// Raft Cluster Implementation for Next-Gen Owl
//
// This module provides a high-level interface for managing a Raft cluster,
// including automatic leader election, replication, and client operations.

use crate::distributed::raft::{NodeId, RaftConfig, RaftNode};
use crate::error::OQLError;
use crate::simulation::{Clock, Network};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// High-level Raft cluster interface
#[allow(dead_code)]
pub struct RaftCluster<C: Clock, N: Network> {
    nodes: HashMap<NodeId, Arc<Mutex<RaftNode<C, N>>>>,
    config: RaftConfig,
    clock: Arc<C>,
    network: Arc<N>,
}

impl<C: Clock, N: Network> RaftCluster<C, N> {
    /// Create a new Raft cluster
    pub fn new(node_ids: Vec<NodeId>, clock: Arc<C>, network: Arc<N>, config: RaftConfig) -> Self {
        let mut nodes = HashMap::new();

        for &id in &node_ids {
            let peers: Vec<NodeId> = node_ids
                .iter()
                .filter(|&&peer| peer != id)
                .copied()
                .collect();
            let node = RaftNode::new(id, peers, clock.clone(), network.clone(), config.clone());
            nodes.insert(id, Arc::new(Mutex::new(node)));
        }

        Self {
            nodes,
            config,
            clock,
            network,
        }
    }

    /// Write data to the cluster (goes to leader)
    pub async fn write(&self, data: Vec<u8>) -> Result<u64, OQLError> {
        let leader_id = self.find_leader().ok_or(OQLError::StorageError {
            message: "No leader".to_string(),
        })?;

        if let Some(leader_node) = self.nodes.get(&leader_id) {
            let mut leader = leader_node.lock().unwrap();
            leader.append_entry(data)
        } else {
            Err(OQLError::StorageError {
                message: "No leader".to_string(),
            })
        }
    }

    /// Read data from any node (strong consistency)
    pub async fn read(&self, query: &str) -> Result<Vec<u8>, OQLError> {
        // For strong consistency, we could read from leader only
        // For now, return a placeholder
        Ok(format!("Query result for: {}", query).into_bytes())
    }

    /// Find the current leader
    pub fn find_leader(&self) -> Option<NodeId> {
        for (id, node_ref) in &self.nodes {
            let node = node_ref.lock().unwrap();
            if node.is_leader() {
                return Some(*id);
            }
        }
        None
    }

    /// Get cluster status
    pub fn get_cluster_status(&self) -> ClusterStatus {
        let mut status = ClusterStatus {
            nodes: HashMap::new(),
            leader: self.find_leader(),
            term: 0,
        };

        for (id, node_ref) in &self.nodes {
            let node = node_ref.lock().unwrap();
            status.nodes.insert(
                *id,
                NodeStatus {
                    id: *id,
                    state: format!("{:?}", node.get_state()),
                    term: node.get_term(),
                    log_length: node.log_len(),
                    is_leader: node.is_leader(),
                },
            );

            // Use highest term as cluster term
            if node.get_term() > status.term {
                status.term = node.get_term();
            }
        }

        status
    }

    /// Trigger election timeout on a specific node (for testing)
    pub fn trigger_election(&self, node_id: NodeId) -> Result<(), OQLError> {
        if let Some(node_ref) = self.nodes.get(&node_id) {
            let mut node = node_ref.lock().unwrap();
            node.start_election()
        } else {
            Err(OQLError::StorageError {
                message: "Node not found".to_string(),
            })
        }
    }

    /// Step the cluster forward (process timeouts, send heartbeats)
    pub fn step(&self) -> Result<(), OQLError> {
        for node_ref in self.nodes.values() {
            let mut node = node_ref.lock().unwrap();

            // Check for election timeout
            if node.is_election_timeout() && !node.is_leader() {
                node.start_election()?;
            }

            // Send heartbeats if leader
            if node.is_leader() && node.should_send_heartbeat() {
                node.send_heartbeats()?;
            }
        }

        Ok(())
    }

    /// Get node by ID
    pub fn get_node(&self, node_id: NodeId) -> Option<Arc<Mutex<RaftNode<C, N>>>> {
        self.nodes.get(&node_id).cloned()
    }

    /// Get all node IDs
    pub fn get_node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().copied().collect()
    }

    /// Wait for leader election to complete
    pub async fn wait_for_leader(&self, timeout: Duration) -> Result<NodeId, OQLError> {
        let start = self.clock.now();

        loop {
            if let Some(leader) = self.find_leader() {
                return Ok(leader);
            }

            if self.clock.now().duration_since(start) > timeout {
                return Err(OQLError::StorageError {
                    message: "Timeout waiting for leader".to_string(),
                });
            }

            // Step the cluster
            self.step()?;

            // Small delay
            self.clock.sleep(Duration::from_millis(10));
        }
    }
}

/// Cluster status information
#[derive(Debug, Clone)]
pub struct ClusterStatus {
    pub nodes: HashMap<NodeId, NodeStatus>,
    pub leader: Option<NodeId>,
    pub term: u64,
}

/// Individual node status
#[derive(Debug, Clone)]
pub struct NodeStatus {
    pub id: NodeId,
    pub state: String,
    pub term: u64,
    pub log_length: usize,
    pub is_leader: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::{SimulatedClock, SimulatedNetwork};

    fn create_test_cluster(node_count: usize) -> RaftCluster<SimulatedClock, SimulatedNetwork> {
        let clock = Arc::new(SimulatedClock::new(42));
        let network = Arc::new(SimulatedNetwork::new(42));
        let config = RaftConfig::default();
        let node_ids: Vec<NodeId> = (1..=node_count as u32).collect();

        RaftCluster::new(node_ids, clock, network, config)
    }

    #[test]
    fn test_cluster_creation() {
        let cluster = create_test_cluster(3);

        assert_eq!(cluster.get_node_ids().len(), 3);
        assert!(cluster.find_leader().is_none()); // No leader initially

        let status = cluster.get_cluster_status();
        assert_eq!(status.nodes.len(), 3);
        assert_eq!(status.leader, None);
        assert_eq!(status.term, 0);
    }

    #[tokio::test]
    async fn test_leader_election() {
        let cluster = create_test_cluster(3);

        // Trigger election on node 1
        cluster.trigger_election(1).unwrap();

        // Wait for leader election
        let leader = cluster
            .wait_for_leader(Duration::from_secs(1))
            .await
            .unwrap();

        assert_eq!(leader, 1);

        let status = cluster.get_cluster_status();
        assert_eq!(status.leader, Some(1));
        assert!(status.term > 0);
    }

    #[tokio::test]
    async fn test_write_to_cluster() {
        let cluster = create_test_cluster(3);

        // Elect leader
        cluster.trigger_election(1).unwrap();
        let _leader = cluster
            .wait_for_leader(Duration::from_secs(1))
            .await
            .unwrap();

        // Write data
        let data = b"test data".to_vec();
        let index = cluster.write(data).await.unwrap();

        assert_eq!(index, 1);
    }

    #[tokio::test]
    async fn test_read_from_cluster() {
        let cluster = create_test_cluster(3);

        // Read should work even without leader (placeholder implementation)
        let result = cluster.read("SELECT * FROM users").await.unwrap();

        assert!(!result.is_empty());
    }
}
