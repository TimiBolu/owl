// Standalone Raft tests
use std::sync::Arc;
use std::time::Duration;

// Mock implementations for testing
pub trait Clock: Send + Sync {
    fn now(&self) -> std::time::Instant;
    fn sleep(&self, duration: Duration);
}

pub trait Network: Send + Sync {
    fn send(&self, peer: u32, message: Vec<u8>) -> Result<(), String>;
}

pub struct MockClock {
    start_time: std::time::Instant,
}

impl MockClock {
    pub fn new() -> Self {
        Self {
            start_time: std::time::Instant::now(),
        }
    }
}

impl Clock for MockClock {
    fn now(&self) -> std::time::Instant {
        self.start_time
    }
    
    fn sleep(&self, _duration: Duration) {
        // No-op for testing
    }
}

pub struct MockNetwork;

impl MockNetwork {
    pub fn new() -> Self {
        Self
    }
}

impl Network for MockNetwork {
    fn send(&self, _peer: u32, _message: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}

// Simple error type for testing
#[derive(Debug)]
pub enum TestError {
    NetworkPartition,
    ReplicationTimeout,
}

// Include the Raft implementation inline for testing
pub type NodeId = u32;
pub type Term = u64;
pub type LogIndex = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum RaftState {
    Follower { 
        leader: Option<NodeId>,
        last_heartbeat: std::time::Instant,
    },
    Candidate { 
        votes_received: std::collections::HashSet<NodeId>,
        election_start: std::time::Instant,
    },
    Leader { 
        next_index: std::collections::HashMap<NodeId, LogIndex>,
        match_index: std::collections::HashMap<NodeId, LogIndex>,
        last_heartbeat_sent: std::time::Instant,
    },
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub term: Term,
    pub index: LogIndex,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RaftConfig {
    pub election_timeout_min: Duration,
    pub election_timeout_max: Duration,
    pub heartbeat_interval: Duration,
}

impl Default for RaftConfig {
    fn default() -> Self {
        Self {
            election_timeout_min: Duration::from_millis(150),
            election_timeout_max: Duration::from_millis(300),
            heartbeat_interval: Duration::from_millis(50),
        }
    }
}

pub struct RaftNode<C: Clock, N: Network> {
    pub id: NodeId,
    pub peers: Vec<NodeId>,
    pub current_term: Term,
    pub voted_for: Option<NodeId>,
    pub log: Vec<LogEntry>,
    pub commit_index: LogIndex,
    pub last_applied: LogIndex,
    pub state: RaftState,
    clock: Arc<C>,
    network: Arc<N>,
    config: RaftConfig,
    election_timeout: Duration,
}

impl<C: Clock, N: Network> RaftNode<C, N> {
    pub fn new(
        id: NodeId,
        peers: Vec<NodeId>,
        clock: Arc<C>,
        network: Arc<N>,
        config: RaftConfig,
    ) -> Self {
        let election_timeout = Duration::from_millis(200); // Fixed for testing
        
        Self {
            id,
            peers,
            current_term: 0,
            voted_for: None,
            log: vec![],
            commit_index: 0,
            last_applied: 0,
            state: RaftState::Follower {
                leader: None,
                last_heartbeat: clock.now(),
            },
            clock,
            network,
            config,
            election_timeout,
        }
    }
    
    pub fn is_leader(&self) -> bool {
        matches!(self.state, RaftState::Leader { .. })
    }
    
    pub fn get_term(&self) -> Term {
        self.current_term
    }
    
    pub fn start_election(&mut self) -> Result<(), TestError> {
        self.current_term += 1;
        self.voted_for = Some(self.id);
        
        let mut votes = std::collections::HashSet::new();
        votes.insert(self.id);
        
        self.state = RaftState::Candidate {
            votes_received: votes,
            election_start: self.clock.now(),
        };
        
        Ok(())
    }
    
    pub fn append_entry(&mut self, data: Vec<u8>) -> Result<LogIndex, TestError> {
        if !self.is_leader() {
            return Err(TestError::ReplicationTimeout);
        }
        
        let entry = LogEntry {
            term: self.current_term,
            index: self.log.len() as LogIndex + 1,
            data,
        };
        
        let index = entry.index;
        self.log.push(entry);
        
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn create_test_node(id: NodeId, peers: Vec<NodeId>) -> RaftNode<MockClock, MockNetwork> {
        let clock = Arc::new(MockClock::new());
        let network = Arc::new(MockNetwork::new());
        let config = RaftConfig::default();
        
        RaftNode::new(id, peers, clock, network, config)
    }
    
    #[test]
    fn test_initial_state() {
        let node = create_test_node(1, vec![2, 3]);
        
        assert_eq!(node.id, 1);
        assert_eq!(node.current_term, 0);
        assert_eq!(node.voted_for, None);
        assert!(matches!(node.state, RaftState::Follower { .. }));
        assert!(!node.is_leader());
    }
    
    #[test]
    fn test_start_election() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        node.start_election().unwrap();
        
        assert_eq!(node.current_term, 1);
        assert_eq!(node.voted_for, Some(1));
        assert!(matches!(node.state, RaftState::Candidate { .. }));
        
        if let RaftState::Candidate { ref votes_received, .. } = node.state {
            assert!(votes_received.contains(&1));
            assert_eq!(votes_received.len(), 1);
        }
    }
    
    #[test]
    fn test_append_entry_as_leader() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        // Manually set as leader for testing
        node.state = RaftState::Leader {
            next_index: std::collections::HashMap::new(),
            match_index: std::collections::HashMap::new(),
            last_heartbeat_sent: node.clock.now(),
        };
        node.current_term = 1;
        
        let data = b"test data".to_vec();
        let index = node.append_entry(data.clone()).unwrap();
        
        assert_eq!(index, 1);
        assert_eq!(node.log.len(), 1);
        assert_eq!(node.log[0].data, data);
        assert_eq!(node.log[0].term, 1);
    }
    
    #[test]
    fn test_append_entry_as_follower_fails() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        let data = b"test data".to_vec();
        let result = node.append_entry(data);
        
        assert!(result.is_err());
    }
}