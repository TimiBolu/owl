// Distributed OQL Infrastructure
//
// This module contains the distributed components for Next-Gen Owl:
// - GlobalObjRef: 128-bit distributed pointers
// - ConsistentHashRing: Shard distribution with virtual nodes
// - ShardManager: Cluster coordination and rebalancing
// - DistributedStorage: Storage adapter with cross-shard navigation
// - DistributedQueryPlanner: Cross-shard query optimization

pub mod consistent_hash;
pub mod global_objref;
pub mod oql_distributed;
pub mod query_planner;
pub mod raft;
pub mod raft_cluster;
pub mod shard_manager;
pub mod storage_adapter;
pub mod wal;
pub mod wal_first_storage;

pub use consistent_hash::{ConsistentHashRing, DistributionStats, ShardId};
pub use global_objref::{GlobalObjRef, NavigationResult};
pub use oql_distributed::{ClusterManager, DistributedOQLEngine, OQLExecutionResult};
pub use query_planner::{CrossShardAnalysis, DistributedPlan, DistributedQueryPlanner};
pub use raft::{
    AppendEntries, AppendEntriesResponse, InstallSnapshot, InstallSnapshotResponse, LogEntry,
    LogIndex, NodeId, RaftConfig, RaftNode, RaftState, RequestVote, RequestVoteResponse,
    SnapshotMetadata, Term,
};
pub use raft_cluster::{ClusterStatus, NodeStatus, RaftCluster};
pub use shard_manager::{ClusterHealth, NodeInfo, ShardInfo, ShardManager};
pub use storage_adapter::{CacheStats, DistributedStorage, Network};
pub use wal::{CheckpointMetadata, DistributedWAL, TxnId, WALEntry, WALEntryType, LSN};
pub use wal_first_storage::{Reservation, SerializedDocument, WALFirstStorage};
