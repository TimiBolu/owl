// Distributed OQL Infrastructure
//
// This module contains the distributed components for Next-Gen Owl:
// - GlobalObjRef: 128-bit distributed pointers
// - ConsistentHashRing: Shard distribution with virtual nodes
// - ShardManager: Cluster coordination and rebalancing
// - DistributedStorage: Storage adapter with cross-shard navigation
// - DistributedQueryPlanner: Cross-shard query optimization

pub mod global_objref;
pub mod consistent_hash;
pub mod shard_manager;
pub mod storage_adapter;
pub mod query_planner;
pub mod wal;
pub mod wal_first_storage;
pub mod oql_distributed;
pub mod raft;
pub mod raft_cluster;

pub use global_objref::{GlobalObjRef, NavigationResult};
pub use consistent_hash::{ConsistentHashRing, DistributionStats, ShardId};
pub use shard_manager::{ShardManager, ShardInfo, NodeInfo, ClusterHealth};
pub use storage_adapter::{DistributedStorage, Network, CacheStats};
pub use query_planner::{DistributedQueryPlanner, DistributedPlan, CrossShardAnalysis};
pub use wal::{DistributedWAL, WALEntry, WALEntryType, CheckpointMetadata, LSN, TxnId};
pub use wal_first_storage::{WALFirstStorage, Reservation, SerializedDocument};
pub use oql_distributed::{DistributedOQLEngine, OQLExecutionResult, ClusterManager};
pub use raft::{RaftNode, RaftState, RaftConfig, LogEntry, NodeId, Term, LogIndex, 
               RequestVote, RequestVoteResponse, AppendEntries, AppendEntriesResponse,
               InstallSnapshot, InstallSnapshotResponse, SnapshotMetadata};
pub use raft_cluster::{RaftCluster, ClusterStatus, NodeStatus};