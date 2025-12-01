// Raft Consensus Implementation for Next-Gen Owl
//
// This module implements the Raft consensus algorithm for distributed coordination,
// leader election, and log replication across the Owl cluster.

use crate::simulation::{Clock, Network};
use crate::error::OQLError;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use rand::{Rng, SeedableRng};

pub type NodeId = u32;
pub type Term = u64;
pub type LogIndex = u64;

/// Raft node states
#[derive(Debug, Clone, PartialEq)]
pub enum RaftState {
    Follower { 
        leader: Option<NodeId>,
        last_heartbeat: Instant,
    },
    Candidate { 
        votes_received: HashSet<NodeId>,
        election_start: Instant,
    },
    Leader { 
        next_index: HashMap<NodeId, LogIndex>,
        match_index: HashMap<NodeId, LogIndex>,
        last_heartbeat_sent: Instant,
    },
}

/// Raft log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub term: Term,
    pub index: LogIndex,
    pub data: Vec<u8>,
}

/// RequestVote RPC message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVote {
    pub term: Term,
    pub candidate_id: NodeId,
    pub last_log_index: LogIndex,
    pub last_log_term: Term,
}

/// RequestVote response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVoteResponse {
    pub term: Term,
    pub vote_granted: bool,
}

/// AppendEntries RPC message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntries {
    pub term: Term,
    pub leader_id: NodeId,
    pub prev_log_index: LogIndex,
    pub prev_log_term: Term,
    pub entries: Vec<LogEntry>,
    pub leader_commit: LogIndex,
}

/// AppendEntries response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
    pub term: Term,
    pub success: bool,
    pub match_index: LogIndex,
}

/// InstallSnapshot RPC message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSnapshot {
    pub term: Term,
    pub leader_id: NodeId,
    pub last_included_index: LogIndex,
    pub last_included_term: Term,
    pub offset: u64,
    pub data: Vec<u8>,
    pub done: bool,
}

/// InstallSnapshot response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSnapshotResponse {
    pub term: Term,
}

/// Snapshot metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    pub last_included_index: LogIndex,
    pub last_included_term: Term,
    pub size: u64,
    pub checksum: u32,
}

/// Raft configuration
#[derive(Debug, Clone)]
pub struct RaftConfig {
    pub election_timeout_min: Duration,
    pub election_timeout_max: Duration,
    pub heartbeat_interval: Duration,
    pub rpc_timeout: Duration,
}

impl Default for RaftConfig {
    fn default() -> Self {
        Self {
            election_timeout_min: Duration::from_millis(150),
            election_timeout_max: Duration::from_millis(300),
            heartbeat_interval: Duration::from_millis(50),
            rpc_timeout: Duration::from_millis(100),
        }
    }
}

/// Main Raft node implementation
pub struct RaftNode<C: Clock, N: Network> {
    // Node identity
    pub id: NodeId,
    pub peers: Vec<NodeId>,
    
    // Persistent state
    pub current_term: Term,
    pub voted_for: Option<NodeId>,
    pub log: Vec<LogEntry>,
    
    // Volatile state
    pub commit_index: LogIndex,
    pub last_applied: LogIndex,
    pub state: RaftState,
    
    // Snapshot state
    pub snapshot_metadata: Option<SnapshotMetadata>,
    pub snapshot_data: Vec<u8>,
    
    // Dependencies
    clock: Arc<C>,
    network: Arc<N>,
    config: RaftConfig,
    
    // Election timeout
    election_timeout: Duration,
    
    // Internal state
    inner: Arc<Mutex<RaftNodeInner>>,
}

struct RaftNodeInner {
    rng: rand::rngs::StdRng,
}

impl<C: Clock, N: Network> RaftNode<C, N> {
    pub fn new(
        id: NodeId,
        peers: Vec<NodeId>,
        clock: Arc<C>,
        network: Arc<N>,
        config: RaftConfig,
    ) -> Self {
        let mut rng = rand::rngs::StdRng::from_entropy();
        let election_timeout = Self::random_election_timeout(&mut rng, &config);
        
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
            snapshot_metadata: None,
            snapshot_data: vec![],
            clock,
            network,
            config,
            election_timeout,
            inner: Arc::new(Mutex::new(RaftNodeInner {
                rng: rand::rngs::StdRng::from_entropy(),
            })),
        }
    }
    
    /// Generate random election timeout
    fn random_election_timeout(rng: &mut impl Rng, config: &RaftConfig) -> Duration {
        let min_ms = config.election_timeout_min.as_millis() as u64;
        let max_ms = config.election_timeout_max.as_millis() as u64;
        let timeout_ms = rng.gen_range(min_ms..=max_ms);
        Duration::from_millis(timeout_ms)
    }
    
    /// Check if election timeout has elapsed
    pub fn is_election_timeout(&self) -> bool {
        match &self.state {
            RaftState::Follower { last_heartbeat, .. } => {
                self.clock.now().duration_since(*last_heartbeat) > self.election_timeout
            }
            RaftState::Candidate { election_start, .. } => {
                self.clock.now().duration_since(*election_start) > self.election_timeout
            }
            RaftState::Leader { .. } => false,
        }
    }
    
    /// Start leader election
    pub fn start_election(&mut self) -> Result<(), OQLError> {
        // Increment term and vote for self
        self.current_term += 1;
        self.voted_for = Some(self.id);
        
        // Reset election timeout
        {
            let mut inner = self.inner.lock().unwrap();
            self.election_timeout = Self::random_election_timeout(&mut inner.rng, &self.config);
        }
        
        // Transition to candidate state
        let mut votes = HashSet::new();
        votes.insert(self.id); // Vote for self
        
        self.state = RaftState::Candidate {
            votes_received: votes,
            election_start: self.clock.now(),
        };
        
        // Send RequestVote RPCs to all peers
        let last_log_index = self.get_last_log_index();
        let last_log_term = self.get_last_log_term();
        
        let request = RequestVote {
            term: self.current_term,
            candidate_id: self.id,
            last_log_index,
            last_log_term,
        };
        
        for &peer in &self.peers {
            self.send_request_vote(peer, request.clone())?;
        }
        
        Ok(())
    }
    
    /// Send RequestVote RPC to a peer
    fn send_request_vote(&self, peer: NodeId, request: RequestVote) -> Result<(), OQLError> {
        let message = serde_json::to_vec(&request)
            .map_err(|_| OQLError::StorageError { message: "Serialization failed".to_string() })?;
        
        self.network.send(self.id, peer, message)
            .map_err(|_| OQLError::StorageError { message: "Send failed".to_string() })?;
        
        Ok(())
    }
    
    /// Handle RequestVote RPC
    pub fn handle_request_vote(&mut self, request: RequestVote) -> RequestVoteResponse {
        let mut vote_granted = false;
        
        // Update term if we see a higher term
        if request.term > self.current_term {
            self.current_term = request.term;
            self.voted_for = None;
            self.become_follower(None);
        }
        
        // Grant vote if conditions are met
        if request.term == self.current_term {
            let can_vote = self.voted_for.is_none() || self.voted_for == Some(request.candidate_id);
            let log_up_to_date = self.is_log_up_to_date(request.last_log_index, request.last_log_term);
            
            if can_vote && log_up_to_date {
                vote_granted = true;
                self.voted_for = Some(request.candidate_id);
                self.reset_election_timeout();
            }
        }
        
        RequestVoteResponse {
            term: self.current_term,
            vote_granted,
        }
    }
    
    /// Check if candidate's log is at least as up-to-date as ours
    fn is_log_up_to_date(&self, last_log_index: LogIndex, last_log_term: Term) -> bool {
        let our_last_index = self.get_last_log_index();
        let our_last_term = self.get_last_log_term();
        
        // Candidate's log is up-to-date if:
        // 1. Last term is higher, OR
        // 2. Last term is same and index is at least as high
        last_log_term > our_last_term || 
        (last_log_term == our_last_term && last_log_index >= our_last_index)
    }
    
    /// Handle RequestVote response
    pub fn handle_request_vote_response(&mut self, from: NodeId, response: RequestVoteResponse) -> Result<(), OQLError> {
        // Update term if we see a higher term
        if response.term > self.current_term {
            self.current_term = response.term;
            self.voted_for = None;
            self.become_follower(None);
            return Ok(());
        }
        
        // Only process if we're still a candidate and terms match
        if let RaftState::Candidate { ref mut votes_received, .. } = self.state {
            if response.term == self.current_term && response.vote_granted {
                votes_received.insert(from);
                
                // Check if we have majority
                let majority = (self.peers.len() + 1) / 2 + 1;
                if votes_received.len() >= majority {
                    self.become_leader()?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Transition to follower state
    fn become_follower(&mut self, leader: Option<NodeId>) {
        self.state = RaftState::Follower {
            leader,
            last_heartbeat: self.clock.now(),
        };
        self.reset_election_timeout();
    }
    
    /// Transition to leader state
    fn become_leader(&mut self) -> Result<(), OQLError> {
        let last_log_index = self.get_last_log_index();
        
        let mut next_index = HashMap::new();
        let mut match_index = HashMap::new();
        
        for &peer in &self.peers {
            next_index.insert(peer, last_log_index + 1);
            match_index.insert(peer, 0);
        }
        
        self.state = RaftState::Leader {
            next_index,
            match_index,
            last_heartbeat_sent: self.clock.now(),
        };
        
        // Send initial heartbeat
        self.send_heartbeats()?;
        
        Ok(())
    }
    
    /// Reset election timeout with new random value
    fn reset_election_timeout(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        self.election_timeout = Self::random_election_timeout(&mut inner.rng, &self.config);
        
        if let RaftState::Follower { ref mut last_heartbeat, .. } = self.state {
            *last_heartbeat = self.clock.now();
        }
    }
    
    /// Send heartbeats to all followers (empty AppendEntries)
    pub fn send_heartbeats(&mut self) -> Result<(), OQLError> {
        if let RaftState::Leader { ref mut last_heartbeat_sent, .. } = self.state {
            *last_heartbeat_sent = self.clock.now();
            
            for &peer in &self.peers {
                self.send_append_entries(peer, vec![])?;
            }
        }
        
        Ok(())
    }
    
    /// Send AppendEntries RPC to a peer
    fn send_append_entries(&self, peer: NodeId, entries: Vec<LogEntry>) -> Result<(), OQLError> {
        if let RaftState::Leader { ref next_index, .. } = self.state {
            let next_idx = next_index.get(&peer).copied().unwrap_or(1);
            let prev_log_index = if next_idx > 1 { next_idx - 1 } else { 0 };
            
            let prev_log_term = if prev_log_index == 0 {
                0
            } else if let Some(entry) = self.get_entry_at_index(prev_log_index) {
                entry.term
            } else if let Some(ref metadata) = self.snapshot_metadata {
                if prev_log_index == metadata.last_included_index {
                    metadata.last_included_term
                } else {
                    // Need to send snapshot instead
                    return self.send_snapshot(peer);
                }
            } else {
                0
            };
            
            let request = AppendEntries {
                term: self.current_term,
                leader_id: self.id,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit: self.commit_index,
            };
            
            let message = serde_json::to_vec(&request)
                .map_err(|_| OQLError::StorageError { message: "Serialization failed".to_string() })?;
            
            self.network.send(self.id, peer, message)
                .map_err(|_| OQLError::StorageError { message: "Send failed".to_string() })?;
        }
        
        Ok(())
    }
    
    /// Check if we should send heartbeats
    pub fn should_send_heartbeat(&self) -> bool {
        if let RaftState::Leader { last_heartbeat_sent, .. } = &self.state {
            self.clock.now().duration_since(*last_heartbeat_sent) >= self.config.heartbeat_interval
        } else {
            false
        }
    }
    
    /// Get current leader ID
    pub fn get_leader(&self) -> Option<NodeId> {
        match &self.state {
            RaftState::Follower { leader, .. } => *leader,
            RaftState::Candidate { .. } => None,
            RaftState::Leader { .. } => Some(self.id),
        }
    }
    
    /// Check if this node is the leader
    pub fn is_leader(&self) -> bool {
        matches!(self.state, RaftState::Leader { .. })
    }
    
    /// Get current term
    pub fn get_term(&self) -> Term {
        self.current_term
    }
    
    /// Get current state
    pub fn get_state(&self) -> &RaftState {
        &self.state
    }
    
    /// Append new entry to log (leader only)
    pub fn append_entry(&mut self, data: Vec<u8>) -> Result<LogIndex, OQLError> {
        if !self.is_leader() {
            return Err(OQLError::StorageError { message: "Not leader".to_string() });
        }
        
        let entry = LogEntry {
            term: self.current_term,
            index: self.get_last_log_index() + 1,
            data,
        };
        
        let index = entry.index;
        self.log.push(entry.clone());
        
        // Replicate to followers
        self.replicate_entry(entry)?;
        
        Ok(index)
    }
    
    /// Replicate entry to all followers
    fn replicate_entry(&self, entry: LogEntry) -> Result<(), OQLError> {
        for &peer in &self.peers {
            self.send_append_entries(peer, vec![entry.clone()])?;
        }
        Ok(())
    }
    
    /// Handle AppendEntries RPC
    pub fn handle_append_entries(&mut self, request: AppendEntries) -> AppendEntriesResponse {
        let mut success = false;
        let mut match_index = 0;
        
        // Update term if we see a higher term
        if request.term > self.current_term {
            self.current_term = request.term;
            self.voted_for = None;
            self.become_follower(Some(request.leader_id));
        }
        
        // Reset election timeout on valid heartbeat
        if request.term == self.current_term {
            self.reset_election_timeout();
            
            // Check log consistency
            if self.is_log_consistent(request.prev_log_index, request.prev_log_term) {
                success = true;
                
                // Append new entries
                if !request.entries.is_empty() {
                    self.append_entries_to_log(request.prev_log_index, request.entries);
                }
                
                match_index = self.get_last_log_index();
                
                // Update commit index
                if request.leader_commit > self.commit_index {
                    self.commit_index = std::cmp::min(request.leader_commit, self.get_last_log_index());
                }
            }
        }
        
        AppendEntriesResponse {
            term: self.current_term,
            success,
            match_index,
        }
    }
    
    /// Check if log is consistent at given index and term
    fn is_log_consistent(&self, prev_log_index: LogIndex, prev_log_term: Term) -> bool {
        if prev_log_index == 0 {
            return true; // No previous entry required
        }
        
        // Check if entry exists in our log
        if let Some(entry) = self.get_entry_at_index(prev_log_index) {
            entry.term == prev_log_term
        } else if let Some(ref metadata) = self.snapshot_metadata {
            // Entry might be in snapshot
            prev_log_index == metadata.last_included_index && prev_log_term == metadata.last_included_term
        } else {
            false // Entry not found
        }
    }
    
    /// Append entries to log, removing conflicts
    fn append_entries_to_log(&mut self, prev_log_index: LogIndex, entries: Vec<LogEntry>) {
        let log_start_index = self.get_log_start_index();
        
        if prev_log_index >= log_start_index {
            // Entry is in current log
            let start_index = (prev_log_index - log_start_index + 1) as usize;
            
            // Remove conflicting entries
            if start_index < self.log.len() {
                self.log.truncate(start_index);
            }
            
            // Append new entries
            self.log.extend(entries);
        } else {
            // Entry is in snapshot, replace entire log
            self.log = entries;
        }
    }
    
    /// Handle AppendEntries response from follower
    pub fn handle_append_entries_response(&mut self, from: NodeId, response: AppendEntriesResponse) -> Result<(), OQLError> {
        // Update term if we see a higher term
        if response.term > self.current_term {
            self.current_term = response.term;
            self.voted_for = None;
            self.become_follower(None);
            return Ok(());
        }
        
        // Only process if we're still leader and terms match
        if let RaftState::Leader { ref mut next_index, ref mut match_index, .. } = self.state {
            if response.term == self.current_term {
                if response.success {
                    // Update follower's progress
                    match_index.insert(from, response.match_index);
                    next_index.insert(from, response.match_index + 1);
                    
                    // Try to advance commit index
                    self.try_advance_commit_index();
                } else {
                    // Decrement next_index and retry
                    if let Some(next) = next_index.get_mut(&from) {
                        if *next > 1 {
                            *next -= 1;
                        }
                    }
                    
                    // Retry with earlier entries
                    self.retry_append_entries(from)?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Try to advance commit index based on follower progress
    fn try_advance_commit_index(&mut self) {
        if let RaftState::Leader { ref match_index, .. } = self.state {
            let mut indices: Vec<LogIndex> = match_index.values().cloned().collect();
            indices.push(self.get_last_log_index()); // Include leader's index
            indices.sort_unstable();
            
            // Find median (majority)
            let majority_index = indices.len() / 2;
            let new_commit_index = indices[majority_index];
            
            // Only advance if the entry is from current term
            if new_commit_index > self.commit_index {
                if let Some(entry) = self.get_entry_at_index(new_commit_index) {
                    if entry.term == self.current_term {
                        self.commit_index = new_commit_index;
                    }
                }
            }
        }
    }
    
    /// Retry AppendEntries with earlier entries
    fn retry_append_entries(&self, peer: NodeId) -> Result<(), OQLError> {
        if let RaftState::Leader { ref next_index, .. } = self.state {
            if let Some(&next) = next_index.get(&peer) {
                let log_start = self.get_log_start_index();
                let last_log_index = self.get_last_log_index();
                
                if next < log_start {
                    // Need to send snapshot
                    self.send_snapshot(peer)?;
                } else if next <= last_log_index {
                    // Send entries from log
                    let start_idx = (next - log_start) as usize;
                    let entries = self.log[start_idx..].to_vec();
                    self.send_append_entries(peer, entries)?;
                } else {
                    // Send empty heartbeat
                    self.send_append_entries(peer, vec![])?;
                }
            }
        }
        Ok(())
    }
    
    /// Get committed entries that haven't been applied yet
    pub fn get_committed_entries(&self) -> Vec<LogEntry> {
        let start = self.last_applied as usize;
        let end = self.commit_index as usize;
        
        if end > start && end <= self.log.len() {
            self.log[start..end].to_vec()
        } else {
            vec![]
        }
    }
    
    /// Mark entries as applied
    pub fn mark_applied(&mut self, up_to_index: LogIndex) {
        self.last_applied = std::cmp::max(self.last_applied, up_to_index);
    }
    
    /// Check if entry at index is committed
    pub fn is_committed(&self, index: LogIndex) -> bool {
        index <= self.commit_index
    }
    
    /// Get log length
    pub fn log_len(&self) -> usize {
        self.log.len()
    }
    
    /// Create snapshot of current state
    pub fn create_snapshot(&mut self, state_data: Vec<u8>) -> Result<(), OQLError> {
        if self.last_applied == 0 {
            return Ok(()); // Nothing to snapshot
        }
        
        let last_included_index = self.last_applied;
        let last_included_term = if let Some(entry) = self.log.get((last_included_index - 1) as usize) {
            entry.term
        } else if let Some(ref meta) = self.snapshot_metadata {
            meta.last_included_term
        } else {
            0
        };
        
        // Calculate checksum (simple hash for now)
        let checksum = Self::simple_hash(&state_data);
        
        self.snapshot_metadata = Some(SnapshotMetadata {
            last_included_index,
            last_included_term,
            size: state_data.len() as u64,
            checksum,
        });
        
        self.snapshot_data = state_data;
        
        // Compact log - remove entries up to last_included_index
        self.compact_log(last_included_index)?;
        
        Ok(())
    }
    
    /// Compact log by removing entries covered by snapshot
    fn compact_log(&mut self, last_included_index: LogIndex) -> Result<(), OQLError> {
        if last_included_index == 0 {
            return Ok(());
        }
        
        let entries_to_remove = std::cmp::min(last_included_index as usize, self.log.len());
        
        if entries_to_remove > 0 {
            self.log.drain(0..entries_to_remove);
        }
        
        Ok(())
    }
    
    /// Install snapshot from leader
    pub fn handle_install_snapshot(&mut self, request: InstallSnapshot) -> InstallSnapshotResponse {
        // Update term if we see a higher term
        if request.term > self.current_term {
            self.current_term = request.term;
            self.voted_for = None;
            self.become_follower(Some(request.leader_id));
        }
        
        // Reset election timeout
        if request.term == self.current_term {
            self.reset_election_timeout();
            
            if request.offset == 0 {
                // First chunk - initialize snapshot
                self.snapshot_data.clear();
            }
            
            // Append data chunk
            self.snapshot_data.extend_from_slice(&request.data);
            
            if request.done {
                // Snapshot complete - install it
                self.install_snapshot_data(request.last_included_index, request.last_included_term);
            }
        }
        
        InstallSnapshotResponse {
            term: self.current_term,
        }
    }
    
    /// Install completed snapshot data
    fn install_snapshot_data(&mut self, last_included_index: LogIndex, last_included_term: Term) {
        // Calculate checksum (simple hash for now)
        let checksum = Self::simple_hash(&self.snapshot_data);
        
        self.snapshot_metadata = Some(SnapshotMetadata {
            last_included_index,
            last_included_term,
            size: self.snapshot_data.len() as u64,
            checksum,
        });
        
        // Discard log entries covered by snapshot
        self.log.clear();
        
        // Update indices
        self.last_applied = last_included_index;
        self.commit_index = std::cmp::max(self.commit_index, last_included_index);
    }
    
    /// Send snapshot to a slow follower
    pub fn send_snapshot(&self, peer: NodeId) -> Result<(), OQLError> {
        if let Some(ref metadata) = self.snapshot_metadata {
            const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks
            
            let mut offset = 0;
            while offset < self.snapshot_data.len() {
                let end = std::cmp::min(offset + CHUNK_SIZE, self.snapshot_data.len());
                let chunk = &self.snapshot_data[offset..end];
                let done = end == self.snapshot_data.len();
                
                let request = InstallSnapshot {
                    term: self.current_term,
                    leader_id: self.id,
                    last_included_index: metadata.last_included_index,
                    last_included_term: metadata.last_included_term,
                    offset: offset as u64,
                    data: chunk.to_vec(),
                    done,
                };
                
                let message = serde_json::to_vec(&request)
                    .map_err(|_| OQLError::StorageError { message: "Serialization failed".to_string() })?;
                
                self.network.send(self.id, peer, message)
                    .map_err(|_| OQLError::StorageError { message: "Send failed".to_string() })?;
                
                offset = end;
            }
        }
        
        Ok(())
    }
    
    /// Check if we should create a snapshot
    pub fn should_create_snapshot(&self, max_log_size: usize) -> bool {
        self.log.len() > max_log_size && self.last_applied > 0
    }
    
    /// Get effective log start index (accounting for snapshots)
    pub fn get_log_start_index(&self) -> LogIndex {
        if let Some(ref metadata) = self.snapshot_metadata {
            metadata.last_included_index + 1
        } else {
            1
        }
    }
    
    /// Get entry at logical index (accounting for snapshots)
    pub fn get_entry_at_index(&self, index: LogIndex) -> Option<&LogEntry> {
        let start_index = self.get_log_start_index();
        if index >= start_index {
            let log_index = (index - start_index) as usize;
            self.log.get(log_index)
        } else {
            None // Entry is in snapshot
        }
    }
    
    /// Get last log index (accounting for snapshots)
    pub fn get_last_log_index(&self) -> LogIndex {
        if self.log.is_empty() {
            self.snapshot_metadata.as_ref().map(|m| m.last_included_index).unwrap_or(0)
        } else {
            self.get_log_start_index() + self.log.len() as LogIndex - 1
        }
    }
    
    /// Get last log term (accounting for snapshots)
    pub fn get_last_log_term(&self) -> Term {
        if let Some(last_entry) = self.log.last() {
            last_entry.term
        } else if let Some(ref metadata) = self.snapshot_metadata {
            metadata.last_included_term
        } else {
            0
        }
    }
    
    /// Get snapshot metadata
    pub fn get_snapshot_metadata(&self) -> Option<&SnapshotMetadata> {
        self.snapshot_metadata.as_ref()
    }
    
    /// Get snapshot data
    pub fn get_snapshot_data(&self) -> &[u8] {
        &self.snapshot_data
    }
    
    /// Simple hash function for checksums
    fn simple_hash(data: &[u8]) -> u32 {
        let mut hash = 0u32;
        for &byte in data {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::{SimulatedClock, SimulatedNetwork};
    use std::sync::Arc;
    
    fn create_test_node(id: NodeId, peers: Vec<NodeId>) -> RaftNode<SimulatedClock, SimulatedNetwork> {
        let clock = Arc::new(SimulatedClock::new(42));
        let network = Arc::new(SimulatedNetwork::new(42));
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
    fn test_handle_request_vote_grant() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        let request = RequestVote {
            term: 1,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };
        
        let response = node.handle_request_vote(request);
        
        assert_eq!(response.term, 1);
        assert!(response.vote_granted);
        assert_eq!(node.voted_for, Some(2));
        assert_eq!(node.current_term, 1);
    }
    
    #[test]
    fn test_handle_request_vote_deny_already_voted() {
        let mut node = create_test_node(1, vec![2, 3]);
        node.current_term = 1;
        node.voted_for = Some(3);
        
        let request = RequestVote {
            term: 1,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };
        
        let response = node.handle_request_vote(request);
        
        assert_eq!(response.term, 1);
        assert!(!response.vote_granted);
        assert_eq!(node.voted_for, Some(3)); // Unchanged
    }
    
    #[test]
    fn test_become_leader_with_majority() {
        let mut node = create_test_node(1, vec![2, 3]);
        node.start_election().unwrap();
        
        // Receive vote from node 2
        let response = RequestVoteResponse {
            term: 1,
            vote_granted: true,
        };
        node.handle_request_vote_response(2, response).unwrap();
        
        // Should become leader (2 out of 3 votes)
        assert!(node.is_leader());
        assert!(matches!(node.state, RaftState::Leader { .. }));
    }
    
    #[test]
    fn test_election_timeout() {
        let clock = Arc::new(SimulatedClock::new(42));
        let network = Arc::new(SimulatedNetwork::new(42));
        let config = RaftConfig {
            election_timeout_min: Duration::from_millis(100),
            election_timeout_max: Duration::from_millis(200),
            ..Default::default()
        };
        
        let mut node = RaftNode::new(1, vec![2, 3], clock.clone(), network, config);
        
        // Initially no timeout
        assert!(!node.is_election_timeout());
        
        // Advance time beyond election timeout
        clock.advance_time(Duration::from_millis(250));
        
        // Should timeout
        assert!(node.is_election_timeout());
    }
    
    #[test]
    fn test_append_entry_as_leader() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        // Become leader
        node.start_election().unwrap();
        let response = RequestVoteResponse { term: 1, vote_granted: true };
        node.handle_request_vote_response(2, response).unwrap();
        
        assert!(node.is_leader());
        
        // Append entry
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
        
        // Should fail as follower
        let data = b"test data".to_vec();
        let result = node.append_entry(data);
        
        assert!(result.is_err());
    }
    
    #[test]
    fn test_handle_append_entries_success() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        let entry = LogEntry {
            term: 1,
            index: 1,
            data: b"test data".to_vec(),
        };
        
        let request = AppendEntries {
            term: 1,
            leader_id: 2,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![entry.clone()],
            leader_commit: 0,
        };
        
        let response = node.handle_append_entries(request);
        
        assert!(response.success);
        assert_eq!(response.term, 1);
        assert_eq!(node.log.len(), 1);
        assert_eq!(node.log[0].data, entry.data);
    }
    
    #[test]
    fn test_handle_append_entries_log_inconsistency() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        let entry = LogEntry {
            term: 2,
            index: 2,
            data: b"test data".to_vec(),
        };
        
        let request = AppendEntries {
            term: 2,
            leader_id: 2,
            prev_log_index: 1, // We don't have entry at index 1
            prev_log_term: 1,
            entries: vec![entry],
            leader_commit: 0,
        };
        
        let response = node.handle_append_entries(request);
        
        assert!(!response.success);
        assert_eq!(response.term, 2);
        assert_eq!(node.log.len(), 0); // No entries added
    }
    
    #[test]
    fn test_commit_index_advancement() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        // Become leader
        node.start_election().unwrap();
        let response = RequestVoteResponse { term: 1, vote_granted: true };
        node.handle_request_vote_response(2, response).unwrap();
        
        // Add entry
        node.append_entry(b"test".to_vec()).unwrap();
        assert_eq!(node.commit_index, 0); // Not committed yet
        
        // Simulate successful replication from majority
        let append_response = AppendEntriesResponse {
            term: 1,
            success: true,
            match_index: 1,
        };
        
        node.handle_append_entries_response(2, append_response).unwrap();
        
        // Should advance commit index
        assert_eq!(node.commit_index, 1);
    }
    
    #[test]
    fn test_get_committed_entries() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        // Add some entries
        node.log.push(LogEntry { term: 1, index: 1, data: b"entry1".to_vec() });
        node.log.push(LogEntry { term: 1, index: 2, data: b"entry2".to_vec() });
        node.log.push(LogEntry { term: 1, index: 3, data: b"entry3".to_vec() });
        
        // Commit first two entries
        node.commit_index = 2;
        node.last_applied = 0;
        
        let committed = node.get_committed_entries();
        assert_eq!(committed.len(), 2);
        assert_eq!(committed[0].data, b"entry1");
        assert_eq!(committed[1].data, b"entry2");
        
        // Mark as applied
        node.mark_applied(2);
        
        // Should return empty now
        let committed = node.get_committed_entries();
        assert_eq!(committed.len(), 0);
    }
    
    #[test]
    fn test_create_snapshot() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        // Add some entries and mark as applied
        node.log.push(LogEntry { term: 1, index: 1, data: b"entry1".to_vec() });
        node.log.push(LogEntry { term: 1, index: 2, data: b"entry2".to_vec() });
        node.log.push(LogEntry { term: 1, index: 3, data: b"entry3".to_vec() });
        node.last_applied = 2;
        
        let state_data = b"snapshot_state".to_vec();
        node.create_snapshot(state_data.clone()).unwrap();
        
        // Check snapshot metadata
        let metadata = node.get_snapshot_metadata().unwrap();
        assert_eq!(metadata.last_included_index, 2);
        assert_eq!(metadata.last_included_term, 1);
        assert_eq!(metadata.size, state_data.len() as u64);
        
        // Check log compaction
        assert_eq!(node.log.len(), 1); // Only entry 3 should remain
        assert_eq!(node.log[0].index, 3);
        
        // Check snapshot data
        assert_eq!(node.get_snapshot_data(), state_data.as_slice());
    }
    
    #[test]
    fn test_install_snapshot() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        let snapshot_data = b"test_snapshot_data".to_vec();
        let request = InstallSnapshot {
            term: 2,
            leader_id: 2,
            last_included_index: 5,
            last_included_term: 2,
            offset: 0,
            data: snapshot_data.clone(),
            done: true,
        };
        
        let response = node.handle_install_snapshot(request);
        
        assert_eq!(response.term, 2);
        assert_eq!(node.current_term, 2);
        assert_eq!(node.last_applied, 5);
        assert_eq!(node.commit_index, 5);
        
        let metadata = node.get_snapshot_metadata().unwrap();
        assert_eq!(metadata.last_included_index, 5);
        assert_eq!(metadata.last_included_term, 2);
        
        assert_eq!(node.get_snapshot_data(), snapshot_data.as_slice());
    }
    
    #[test]
    fn test_should_create_snapshot() {
        let node = create_test_node(1, vec![2, 3]);
        
        // Should not create snapshot with empty log
        assert!(!node.should_create_snapshot(10));
        
        let mut node = node;
        
        // Add entries but not applied
        for i in 1..=15 {
            node.log.push(LogEntry { 
                term: 1, 
                index: i, 
                data: format!("entry{}", i).into_bytes() 
            });
        }
        
        // Should not create snapshot if nothing applied
        assert!(!node.should_create_snapshot(10));
        
        // Mark some as applied
        node.last_applied = 5;
        
        // Should create snapshot now
        assert!(node.should_create_snapshot(10));
    }
    
    #[test]
    fn test_get_entry_at_index_with_snapshot() {
        let mut node = create_test_node(1, vec![2, 3]);
        
        // Create snapshot covering first 3 entries
        node.snapshot_metadata = Some(SnapshotMetadata {
            last_included_index: 3,
            last_included_term: 1,
            size: 100,
            checksum: 0,
        });
        
        // Add entries after snapshot
        node.log.push(LogEntry { term: 1, index: 4, data: b"entry4".to_vec() });
        node.log.push(LogEntry { term: 1, index: 5, data: b"entry5".to_vec() });
        
        // Should not find entries in snapshot
        assert!(node.get_entry_at_index(1).is_none());
        assert!(node.get_entry_at_index(2).is_none());
        assert!(node.get_entry_at_index(3).is_none());
        
        // Should find entries in log
        assert!(node.get_entry_at_index(4).is_some());
        assert!(node.get_entry_at_index(5).is_some());
        assert_eq!(node.get_entry_at_index(4).unwrap().data, b"entry4");
        
        // Check indices
        assert_eq!(node.get_log_start_index(), 4);
        assert_eq!(node.get_last_log_index(), 5);
        assert_eq!(node.get_last_log_term(), 1);
    }
}