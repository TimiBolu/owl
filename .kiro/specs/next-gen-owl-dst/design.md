# Design Document: Next-Gen Owl with Deterministic Simulation Testing

## Overview

This document describes the architecture and design for Next-Generation Owl Storage Engine with integrated Deterministic Simulation Testing. The design preserves Owl's unique O(1) graph navigation while adding distributed capabilities, strong durability, and world-class testing infrastructure.

## Architecture

### High-Level System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Client Applications                          │
│                    (OQL Query Interface)                         │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                    Query Layer (Rust)                            │
│  ┌──────────┬──────────┬──────────┬──────────┬──────────┐     │
│  │  Parser  │ Planner  │Optimizer │ Executor │ Functions│     │
│  └──────────┴──────────┴──────────┴──────────┴──────────┘     │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│              Distributed Coordination Layer (Rust)               │
│  ┌──────────┬──────────┬──────────┬──────────┬──────────┐     │
│  │   Raft   │  Shard   │   2PC    │  Router  │ Rebalance│     │
│  │Consensus │ Manager  │Coordinator│          │          │     │
│  └──────────┴──────────┴──────────┴──────────┴──────────┘     │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                Storage Engine Layer (Zig)                        │
│  ┌──────────┬──────────┬──────────┬──────────┬──────────┐     │
│  │  Arena   │   WAL    │   MVCC   │  B-Tree  │   GC     │     │
│  │Allocator │          │          │  Index   │          │     │
│  └──────────┴──────────┴──────────┴──────────┴──────────┘     │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│              Simulation Framework (Rust + Zig)                   │
│  ┌──────────┬──────────┬──────────┬──────────┬──────────┐     │
│  │Simulated │Simulated │Simulated │Simulated │ Failure  │     │
│  │  Clock   │Scheduler │   FS     │ Network  │ Injector │     │
│  └──────────┴──────────┴──────────┴──────────┴──────────┘     │
└─────────────────────────────────────────────────────────────────┘
```

## OQL Integration with Distributed Architecture

### OQL Query Execution in Distributed Environment

**Current OQL Syntax** (from analysis of .oql files):
```oql
// Schema definition
schema User {
    id: String,
    name: String,
    profile: {
        email: String,
        age: Int
    },
    orders: [Order]  // References to Order objects
}

schema Order {
    id: String,
    user_id: String,
    total: Float,
    items: [OrderItem]
}

// CRUD Operations
User.insert {
    id: "user1",
    name: "Alice Smith",
    profile: {
        email: "alice@example.com",
        age: 28
    }
}

// Reference navigation (O(1) with GlobalObjRef)
User.find(id: "user1").orders[0].items[2].product.name

// Complex queries with filtering
User.where { .profile.age > 25 && .profile.email != null }
    .select {
        id,
        name,
        email: .profile.email,
        order_count: count(.orders)
    }
    .sort { .order_count desc }
    .limit { 10 }
```

**Distributed OQL Execution**:
```zig
/// Distributed OQL query engine
pub const DistributedOQLEngine = struct {
    local_shard: u32,
    cluster: *ClusterManager,
    hash_ring: *ConsistentHashRing,
    
    /// Execute OQL query across distributed shards
    pub fn execute(self: *DistributedOQLEngine, oql: []const u8) !OQLResult {
        // 1. Parse OQL
        const ast = try OQLParser.parse(self.allocator, oql);
        defer ast.deinit();
        
        // 2. Plan distributed execution
        const plan = try self.planDistributedQuery(ast);
        defer plan.deinit();
        
        // 3. Execute across shards
        return self.executeDistributedPlan(plan);
    }
    
    /// Handle cross-shard reference navigation
    pub fn navigateCrossShard(
        self: *DistributedOQLEngine,
        start_ref: GlobalObjRef,
        path: []const u8  // OQL path like ".orders[0].items"
    ) !GlobalObjRef {
        var current_ref = start_ref;
        
        // Parse navigation path
        const path_segments = try OQLParser.parsePath(self.allocator, path);
        defer path_segments.deinit();
        
        for (path_segments.items) |segment| {
            if (current_ref.isLocal(self.local_shard)) {
                // Local navigation: <50ns (same as current Owl)
                current_ref = try self.navigateLocal(current_ref, segment);
            } else {
                // Remote navigation: <1μs (network call)
                current_ref = try self.navigateRemote(current_ref, segment);
            }
        }
        
        return current_ref;
    }
};
```

## Components

### 1. GlobalObjRef: 128-bit Distributed Pointers

**Purpose**: Enable O(1) navigation across distributed nodes

**Design**:
```zig
/// 128-bit global object reference extending current ObjRef
/// Layout: [Shard ID: 32 bits][Local Offset: 64 bits][Flags: 32 bits]
/// Compatible with existing ObjRef (local_offset field)
pub const GlobalObjRef = extern struct {
    shard_id: u32,      // Which shard owns this object (0-4B shards)
    local_offset: u64,  // Offset within shard's arena (same as ObjRef.offset)
    flags: u32,         // Compression, type hints, version info
    
    /// Check if reference is local to current shard
    pub inline fn isLocal(self: GlobalObjRef, current_shard: u32) bool {
        return self.shard_id == current_shard;
    }
    
    /// Convert to local ObjRef for same-shard navigation
    pub inline fn toLocal(self: GlobalObjRef) ObjRef {
        return ObjRef.init(self.local_offset);
    }
    
    /// O(1) navigation with OQL integration: <50ns for local, <1μs for remote
    pub fn navigateOQL(
        self: GlobalObjRef,
        arena: *Arena,
        network: *Network,
        path: []const u8  // OQL path like ".user.profile.name"
    ) !Object {
        if (self.isLocal(arena.shard_id)) {
            // Fast path: Direct pointer navigation (same as current Owl)
            const local_ref = self.toLocal();
            return arena.navigatePath(local_ref, path);
        } else {
            // Remote path: Network call with caching
            return network.navigateRemotePath(self, path);
        }
    }
    
    /// Swizzle for direct memory access (local only)
    pub inline fn swizzle(self: GlobalObjRef, base: [*]u8) [*]u8 {
        std.debug.assert(self.shard_id == 0); // Must be local
        return base + self.local_offset;
    }
};

/// Backward compatibility: Convert ObjRef to GlobalObjRef
pub fn objRefToGlobal(objref: ObjRef, shard_id: u32) GlobalObjRef {
    return GlobalObjRef{
        .shard_id = shard_id,
        .local_offset = objref.offset,
        .flags = 0,
    };
}
```

**Key Innovation**: Pointers encode location, enabling transparent distributed navigation

---

### 2. Consistent Hashing for Sharding

**Purpose**: Distribute data evenly with minimal rebalancing

**Design**:
```zig
/// Consistent hashing for OQL object distribution
pub const ConsistentHashRing = struct {
    /// Virtual nodes for smooth distribution (sorted by hash)
    vnodes: std.ArrayList(VNode),
    /// Number of virtual nodes per physical shard (default: 256)
    vnodes_per_shard: u32,
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    
    const VNode = struct {
        hash: u64,
        shard_id: u32,
    };
    
    /// Determine which shard owns an OQL object by ID
    pub fn getShardForObject(self: *const ConsistentHashRing, object_id: []const u8) u32 {
        // Use Wyhash for speed (same as Zig std.hash.Wyhash)
        const hash = std.hash.Wyhash.hash(0, object_id);
        
        // Binary search for next vnode in ring
        const idx = self.findNextVNode(hash);
        return self.vnodes.items[idx].shard_id;
    }
    
    /// Get shard for OQL schema type (for co-location)
    pub fn getShardForSchema(self: *const ConsistentHashRing, schema_name: []const u8) u32 {
        return self.getShardForObject(schema_name);
    }
    
    /// Add a new shard (triggers rebalancing)
    pub fn addShard(self: *ConsistentHashRing, shard_id: u32) !void {
        // Add virtual nodes for this shard
        var i: u32 = 0;
        while (i < self.vnodes_per_shard) : (i += 1) {
            const vnode_hash = self.hashVNode(shard_id, i);
            try self.vnodes.append(VNode{
                .hash = vnode_hash,
                .shard_id = shard_id,
            });
        }
        
        // Keep vnodes sorted by hash for binary search
        std.sort.sort(VNode, self.vnodes.items, {}, vnodeCompare);
    }
    
    /// Hash function for virtual nodes
    fn hashVNode(self: *const ConsistentHashRing, shard_id: u32, vnode_idx: u32) u64 {
        var hasher = std.hash.Wyhash.init(0);
        hasher.update(std.mem.asBytes(&shard_id));
        hasher.update(std.mem.asBytes(&vnode_idx));
        return hasher.final();
    }
    
    /// Binary search for next virtual node
    fn findNextVNode(self: *const ConsistentHashRing, hash: u64) usize {
        // Find first vnode with hash >= target
        for (self.vnodes.items, 0..) |vnode, i| {
            if (vnode.hash >= hash) return i;
        }
        // Wrap around to first vnode
        return 0;
    }
    
    fn vnodeCompare(_: void, a: VNode, b: VNode) bool {
        return a.hash < b.hash;
    }
};

/// OQL-aware shard assignment
pub fn getShardForOQLInsert(ring: *const ConsistentHashRing, oql: []const u8) !u32 {
    // Parse OQL to extract object ID
    const parsed = try OQLParser.parseInsert(oql);
    defer parsed.deinit();
    
    // Use object ID for shard assignment
    return ring.getShardForObject(parsed.object_id);
}
```

**Benefits**:
- Adding N nodes moves only 1/N of data
- Virtual nodes ensure even distribution
- Deterministic shard assignment

---

### 3. WAL-First Durability

**Purpose**: Guarantee no data loss after write acknowledgment

**Design**:
```rust
pub struct WALFirstStorage<C: Clock, F: FileSystem> {
    arena: Arena,
    wal: WAL<F>,
    clock: Arc<C>,
}

impl<C: Clock, F: FileSystem> WALFirstStorage<C, F> {
    /// Atomic write with durability guarantee
    pub fn write_atomic(&mut self, obj: &Object) -> Result<GlobalObjRef> {
        // 1. Reserve space in arena (don't write yet)
        let objref = self.arena.reserve(obj.size())?;
        
        // 2. Write to WAL with fsync (DURABLE)
        let lsn = self.wal.append_with_sync(obj, objref)?;
        
        // 3. Write to arena (now safe - WAL has it)
        self.arena.commit_reservation(objref, obj)?;
        
        // 4. Update in-memory index
        self.index.insert(obj.id, objref, lsn)?;
        
        Ok(objref)
    }
    
    /// Crash recovery from WAL
    pub fn recover(&mut self) -> Result<()> {
        // 1. Find latest checkpoint
        let checkpoint_lsn = self.find_latest_checkpoint()?;
        
        // 2. Load arena state at checkpoint
        self.arena.load_checkpoint(checkpoint_lsn)?;
        
        // 3. Replay WAL from checkpoint to end
        self.wal.replay_from(checkpoint_lsn, |entry| {
            self.arena.apply(entry)
        })?;
        
        Ok(())
    }
}
```

**Guarantee**: If write returns Ok, data is durable (survives crash/power loss)

---

### 4. Raft Consensus for Replication

**Purpose**: Maintain consistency across replicas with automatic failover

**Design**:
```rust
pub struct RaftNode<C: Clock, N: Network> {
    id: NodeId,
    state: RaftState,
    log: ReplicatedLog,
    clock: Arc<C>,
    network: Arc<N>,
}

pub enum RaftState {
    Follower { leader: Option<NodeId> },
    Candidate { votes_received: HashSet<NodeId> },
    Leader { next_index: HashMap<NodeId, LogIndex> },
}

impl<C: Clock, N: Network> RaftNode<C, N> {
    /// Replicate a write to followers
    pub fn replicate_write(&mut self, data: Vec<u8>) -> Result<()> {
        // 1. Append to local log
        let entry = LogEntry::new(self.current_term, data);
        self.log.append(entry)?;
        
        // 2. Send AppendEntries to all followers
        for follower in &self.followers {
            self.network.send(*follower, AppendEntries {
                term: self.current_term,
                entries: vec![entry.clone()],
                ...
            })?;
        }
        
        // 3. Wait for quorum (majority)
        self.wait_for_quorum()?;
        
        // 4. Commit and apply
        self.log.commit(entry.index)?;
        self.apply_to_state_machine(entry)?;
        
        Ok(())
    }
}
```

**Properties**:
- Leader election in <2 seconds
- Quorum writes prevent split-brain
- Automatic failover

---

### 5. Simulation Framework Architecture

**Purpose**: Enable deterministic testing of distributed scenarios

**Design**:
```rust
/// Main simulation controller
pub struct Simulation {
    seed: u64,
    clock: Arc<SimulatedClock>,
    scheduler: Arc<SimulatedScheduler>,
    filesystem: Arc<SimulatedFileSystem>,
    network: Arc<SimulatedNetwork>,
    rng: SeededRng,
}

impl Simulation {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            clock: Arc::new(SimulatedClock::new(seed)),
            scheduler: Arc::new(SimulatedScheduler::new(seed)),
            filesystem: Arc::new(SimulatedFileSystem::new(seed)),
            network: Arc::new(SimulatedNetwork::new(seed)),
            rng: SeededRng::from_seed(seed),
        }
    }
    
    /// Run a test scenario
    pub fn run<F, R>(&mut self, test: F) -> R
    where F: FnOnce(&mut Self) -> R
    {
        test(self)
    }
    
    /// Create a simulated Owl cluster
    pub fn create_cluster(&self, num_nodes: usize) -> Vec<OwlNode> {
        (0..num_nodes).map(|id| {
            OwlNode::new(
                id as NodeId,
                self.clock.clone(),
                self.scheduler.clone(),
                self.filesystem.clone(),
                self.network.clone(),
            )
        }).collect()
    }
}
```

**Key Insight**: All non-determinism controlled by seed → 100% reproducible tests

---

### 6. Dependency Injection Pattern

**Purpose**: Allow production and simulation code to coexist

**Design**:
```rust
/// Clock trait for time operations
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
    fn schedule(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>);
}

/// Production implementation
pub struct RealClock;
impl Clock for RealClock {
    fn now(&self) -> Instant { Instant::now() }
    fn sleep(&self, duration: Duration) { std::thread::sleep(duration) }
    ...
}

/// Simulation implementation
pub struct SimulatedClock {
    current_time: Arc<Mutex<Instant>>,
    events: Arc<Mutex<BinaryHeap<Event>>>,
}
impl Clock for SimulatedClock {
    fn now(&self) -> Instant { *self.current_time.lock().unwrap() }
    fn sleep(&self, _duration: Duration) { /* no-op in simulation */ }
    ...
}

/// Component using injected clock
pub struct ConcurrencyManager<C: Clock> {
    clock: Arc<C>,  // Injected!
    ...
}
```

**Benefits**:
- Zero runtime overhead in production
- Full control in tests
- Clean separation of concerns

---

## Data Models

### Object Layout with Compression

```
┌─────────────────────────────────────────────────────────────┐
│                    ObjectHeader (56 bytes)                   │
├─────────────────────────────────────────────────────────────┤
│ type_tag: u16                                                │
│ id: [16]u8                                                   │
│ version: u32                                                 │
│ size: u32                                                    │
│ flags: u8  (deleted, compressed, pinned)                    │
│ compression: u8  (none=0, lz4=1, zstd=2)                    │
│ checksum: u32                                                │
│ lsn: u64                                                     │
├─────────────────────────────────────────────────────────────┤
│                    Fixed Fields                              │
│  (uncompressed, cache-aligned)                              │
├─────────────────────────────────────────────────────────────┤
│                    Variable Data                             │
│  (compressed if size > 1KB)                                 │
├─────────────────────────────────────────────────────────────┤
│                    References (GlobalObjRef[])               │
│  (128 bits each)                                            │
└─────────────────────────────────────────────────────────────┘
```

### WAL Entry Format

```
┌─────────────────────────────────────────────────────────────┐
│                    WAL Entry Header (32 bytes)               │
├─────────────────────────────────────────────────────────────┤
│ lsn: u64                                                     │
│ entry_type: u8  (insert, update, delete, checkpoint)        │
│ shard_id: u32                                                │
│ data_size: u32                                               │
│ objref: GlobalObjRef (128 bits)                             │
│ checksum: u32                                                │
├─────────────────────────────────────────────────────────────┤
│                    Entry Data                                │
│  (serialized object or operation)                           │
└─────────────────────────────────────────────────────────────┘
```

## Error Handling

### Error Types

```rust
pub enum OwlError {
    // Storage errors
    OutOfMemory,
    OutOfSpace,
    CorruptedData { expected_checksum: u32, actual: u32 },
    
    // Distributed errors
    NetworkPartition,
    QuorumLost,
    ReplicationTimeout,
    ShardUnavailable { shard_id: u32 },
    
    // Concurrency errors
    WriteConflict { txn_id: u64 },
    Deadlock { cycle: Vec<u64> },
    TransactionTimeout,
    
    // Simulation errors (test-only)
    InjectedFailure { failure_type: String },
}
```

### Recovery Strategies

1. **Transient Errors**: Retry with exponential backoff
2. **Network Partitions**: Wait for quorum, reject minority writes
3. **Node Crashes**: Automatic failover via Raft
4. **Data Corruption**: Restore from replicas or backup

## Testing Strategy

### Test Pyramid

```
         ┌─────────────────┐
         │  Property Tests │  (1000s of scenarios)
         │   (Simulation)  │
         └─────────────────┘
              ↑
         ┌─────────────────┐
         │ Integration Tests│  (100s of scenarios)
         │   (Simulation)  │
         └─────────────────┘
              ↑
         ┌─────────────────┐
         │   Unit Tests    │  (1000s of tests)
         │   (Fast, Local) │
         └─────────────────┘
```

### Key Test Scenarios

1. **Raft Leader Election**: Network partition during election
2. **Split-Brain Prevention**: Minority partition rejects writes
3. **Crash Recovery**: Node crashes mid-write, recovers from WAL
4. **Concurrent Writes**: Multiple clients write same object
5. **Rebalancing**: Add node, verify data moves correctly
6. **Cascading Failures**: Multiple nodes fail in sequence

## Performance Considerations

### Optimization Strategies

1. **Co-location**: Keep related objects in same page (4KB)
2. **Caching**: LRU cache for hot objects
3. **Batching**: Group commits for throughput
4. **Lock-free**: Avoid mutex contention
5. **SIMD**: Vectorized operations for analytics

### Performance Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| Local navigation | <50ns | Microbenchmark |
| Remote navigation | <1μs | Simulation |
| Write throughput | 10M+ TPS | Benchmark |
| Read throughput | 5M+ QPS | Benchmark |
| Replication lag | <10ms | Monitoring |
| Failover time | <2s | Simulation |

## Migration Path

### Phase 1: Foundation (Months 1-3)
- Implement GlobalObjRef
- WAL-first atomic writes
- Simulated Clock/Scheduler/FS
- Single-node simulation tests

### Phase 2: Distribution (Months 4-9)
- Consistent hashing
- Raft consensus
- Simulated Network
- Distributed simulation tests

### Phase 3: Optimization (Months 10-12)
- Lock-free structures
- Compression
- Columnar cache
- Performance tuning

### Phase 4: Production (Months 13-14)
- Monitoring/observability
- Backup/restore
- Documentation
- Production deployment

## Conclusion

This design preserves Owl's unique O(1) navigation advantage while adding distributed capabilities, strong durability, and world-class testing infrastructure. The simulation framework ensures we can ship with confidence, having tested millions of scenarios before production deployment.
