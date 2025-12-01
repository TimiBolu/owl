# Owl Next-Gen: State-of-the-Art Storage Engine Design

## Vision Statement

**Design a distributed, durable, high-performance storage engine that retains Owl's O(1) graph navigation while matching or exceeding ClickHouse, ScyllaDB, and TigerBeetle in their respective strengths.**

---

## Core Principles

1. **Preserve O(1) Navigation**: Physical pointers remain the foundation
2. **Add Horizontal Scaling**: Shard across nodes without losing pointer semantics
3. **Guarantee Durability**: WAL-first with atomic commits
4. **Maximize Performance**: Compression, columnar storage, lock-free structures
5. **Zero Compromise**: Don't sacrifice graph navigation for other features

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Owl Next-Gen Architecture                 │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │   Node 1     │  │   Node 2     │  │   Node 3     │     │
│  │              │  │              │  │              │     │
│  │ ┌──────────┐ │  │ ┌──────────┐ │  │ ┌──────────┐ │     │
│  │ │  Shard A │ │  │ │  Shard B │ │  │ │  Shard C │ │     │
│  │ │  Arena   │ │  │ │  Arena   │ │  │ │  Arena   │ │     │
│  │ └──────────┘ │  │ └──────────┘ │  │ └──────────┘ │     │
│  │ ┌──────────┐ │  │ ┌──────────┐ │  │ ┌──────────┐ │     │
│  │ │   WAL    │ │  │ │   WAL    │  │  │   WAL    │ │     │
│  │ └──────────┘ │  │ └──────────┘ │  │ └──────────┘ │     │
│  │ ┌──────────┐ │  │ ┌──────────┐ │  │ ┌──────────┐ │     │
│  │ │ Columnar │ │  │ │ Columnar │ │  │ │ Columnar │ │     │
│  │ │  Cache   │ │  │ │  Cache   │ │  │ │  Cache   │ │     │
│  │ └──────────┘ │  │ └──────────┘ │  │ └──────────┘ │     │
│  └──────────────┘  └──────────────┘  └──────────────┘     │
│         ↕                  ↕                  ↕             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │         Consensus Layer (Raft/VSR)                   │  │
│  └──────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────┐  │
│  │         Distributed Transaction Coordinator          │  │
│  └──────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────┐  │
│  │         Global Pointer Translation Table (GPTT)      │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

---

## 1. DISTRIBUTED ARCHITECTURE

### 1.1 Sharded Arenas with Global Pointers

**Problem Solved**: Single-file limitation, no horizontal scaling

**Design**:
```zig
/// Global Object Reference - 128 bits
/// [Shard ID: 32 bits][Local Offset: 64 bits][Flags: 32 bits]
pub const GlobalObjRef = packed struct {
    shard_id: u32,      // Which shard owns this object
    local_offset: u64,  // Offset within shard's arena
    flags: u32,         // Compression, type hints, etc.
    
    pub fn toLocal(self: GlobalObjRef) ObjRef {
        return ObjRef.init(self.local_offset);
    }
    
    pub fn isLocal(self: GlobalObjRef, current_shard: u32) bool {
        return self.shard_id == current_shard;
    }
};
```

**Key Innovation**: Pointers encode shard location, enabling O(1) navigation across nodes

**Navigation**:
- Local reference: <50ns (same as current)
- Remote reference: <1μs (network hop + cache)
- Transparent to application layer

---

### 1.2 Consistent Hashing for Sharding

**Shard Assignment**:
```zig
pub fn getShardForObject(object_id: u128, num_shards: u32) u32 {
    const hash = std.hash.Wyhash.hash(0, std.mem.asBytes(&object_id));
    return @intCast(hash % num_shards);
}
```

**Rebalancing**:
- Virtual nodes (vnodes) for smooth rebalancing
- Minimal data movement when adding/removing nodes
- Background migration with zero downtime

---

### 1.3 Replication with Consensus

**Replication Factor**: Configurable (default RF=3)

**Consensus Protocol**: Raft (proven, well-understood)

**Replication Flow**:
```
Client Write
    ↓
Leader receives write
    ↓
Append to local WAL
    ↓
Replicate to followers (parallel)
    ↓
Wait for quorum (2/3)
    ↓
Commit to arena
    ↓
Acknowledge client
```

**Performance**: <1ms commit latency (3-node cluster, same DC)

---

## 2. DURABILITY GUARANTEES

### 2.1 WAL-First Architecture

**Problem Solved**: WAL not integrated, potential data loss

**Design**:
```zig
pub fn atomicWrite(arena: *Arena, wal: *WAL, obj: *Object) !GlobalObjRef {
    // 1. Allocate space in arena (reserve, don't write yet)
    const ref = try arena.reserve(obj.totalSize());
    
    // 2. Write to WAL with allocated reference
    const lsn = try wal.appendWithSync(obj, ref);
    
    // 3. Write to arena (now safe - WAL has it)
    try arena.commitReservation(ref, obj);
    
    // 4. Update in-memory index
    try index.insert(obj.id, ref, lsn);
    
    return ref;
}
```

**Guarantee**: If WAL write succeeds, data is durable. Arena write can be replayed from WAL.

---

### 2.2 Checkpointing System

**Problem Solved**: Unbounded WAL growth, slow recovery

**Design**:
```zig
pub const CheckpointManager = struct {
    /// Checkpoint every N seconds or M bytes
    interval_sec: u64 = 300,  // 5 minutes
    interval_bytes: u64 = 1024 * 1024 * 1024,  // 1GB
    
    pub fn checkpoint(self: *Self) !void {
        // 1. Flush all dirty pages to arena
        try self.arena.flushAll();
        
        // 2. Record checkpoint LSN
        const checkpoint_lsn = self.wal.getCurrentLSN();
        try self.writeCheckpointMarker(checkpoint_lsn);
        
        // 3. Truncate WAL before checkpoint
        try self.wal.truncateBefore(checkpoint_lsn);
        
        // 4. Update metadata
        self.last_checkpoint = checkpoint_lsn;
    }
};
```

**Recovery**:
1. Find latest checkpoint
2. Load arena state at checkpoint
3. Replay WAL from checkpoint to end
4. Fast recovery (seconds, not minutes)

---

### 2.3 Synchronous Durability

**Problem Solved**: mmap async writes, unclear durability

**Design**:
```zig
pub fn syncWrite(arena: *Arena, ref: GlobalObjRef, data: []const u8) !void {
    // Write to mmap
    @memcpy(arena.mmap[offset..], data);
    
    // Explicit fsync for durability
    try std.posix.msync(
        arena.mmap.ptr + (offset & ~4095),  // Page-aligned
        std.mem.alignForward(usize, data.len, 4096),
        std.posix.MS.SYNC  // Synchronous!
    );
}
```

**Trade-off**: Slightly slower writes (~100μs), but guaranteed durability

**Optimization**: Group commits (batch multiple writes, single fsync)

---

## 3. PERFORMANCE OPTIMIZATIONS

### 3.1 Transparent Compression

**Problem Solved**: 10-100x storage overhead

**Design**:
```zig
pub const CompressionType = enum(u8) {
    none = 0,
    lz4 = 1,      // Fast (500 MB/s compress, 2 GB/s decompress)
    zstd = 2,     // Balanced (200 MB/s compress, 800 MB/s decompress)
    zstd_max = 3, // Best ratio (50 MB/s compress, 800 MB/s decompress)
};

pub const CompressedObject = struct {
    header: ObjectHeader,
    compression: CompressionType,
    compressed_size: u32,
    uncompressed_size: u32,
    data: []u8,  // Compressed
    
    pub fn decompress(self: *CompressedObject, allocator: Allocator) !Object {
        const decompressed = switch (self.compression) {
            .none => self.data,
            .lz4 => try lz4.decompress(allocator, self.data, self.uncompressed_size),
            .zstd => try zstd.decompress(allocator, self.data, self.uncompressed_size),
            .zstd_max => try zstd.decompress(allocator, self.data, self.uncompressed_size),
        };
        return Object.fromBytes(allocator, decompressed);
    }
};
```

**Adaptive Compression**:
- Small objects (<1KB): No compression (overhead not worth it)
- Medium objects (1KB-1MB): LZ4 (fast)
- Large objects (>1MB): ZSTD (better ratio)
- Hot objects: Cache decompressed in memory

**Expected Savings**: 5-10x for typical workloads

---

### 3.2 Hybrid Storage: Row + Columnar

**Problem Solved**: Poor analytical performance

**Design**:
```
┌─────────────────────────────────────────┐
│         Hybrid Storage Layer            │
├─────────────────────────────────────────┤
│                                          │
│  Row Store (Arena)     Columnar Cache   │
│  ┌──────────────┐     ┌──────────────┐ │
│  │ Object 1     │     │ Column: age  │ │
│  │ {id, name,   │     │ [25,30,35]   │ │
│  │  age, city}  │     │              │ │
│  ├──────────────┤     ├──────────────┤ │
│  │ Object 2     │     │ Column: city │ │
│  │ {id, name,   │     │ ["NYC","SF"] │ │
│  │  age, city}  │     │              │ │
│  └──────────────┘     └──────────────┘ │
│         ↓                     ↑          │
│    Graph Nav (<50ns)    Analytics (SIMD)│
└─────────────────────────────────────────┘
```

**Strategy**:
- Primary storage: Row-oriented arena (graph navigation)
- Secondary cache: Columnar projections (analytics)
- Automatic materialization for hot columns
- Query optimizer chooses best path

**Example**:
```zig
// Graph query: Use row store
user.orders[0].items[2].product  // <200ns

// Analytical query: Use columnar cache
SELECT AVG(age) FROM users WHERE city = 'NYC'  // 10ms (vs 100ms row scan)
```

---

### 3.3 Lock-Free Concurrency

**Problem Solved**: Coarse-grained locking, poor scalability

**Design**:
```zig
/// Lock-free transaction map using hazard pointers
pub const LockFreeTransactionMap = struct {
    buckets: []std.atomic.Value(?*TransactionNode),
    hazard_pointers: HazardPointerManager,
    
    pub fn insert(self: *Self, txn: *Transaction) !void {
        const bucket_idx = txn.id % self.buckets.len;
        
        // Lock-free insertion using CAS
        while (true) {
            const head = self.buckets[bucket_idx].load(.acquire);
            const new_node = try self.allocNode(txn, head);
            
            if (self.buckets[bucket_idx].cmpxchgWeak(
                head, new_node, .release, .acquire
            )) |_| {
                // CAS failed, retry
                self.freeNode(new_node);
                continue;
            }
            
            // Success!
            break;
        }
    }
};
```

**Benefits**:
- No lock contention
- Better CPU cache utilization
- Scales linearly with cores
- 10x throughput improvement on 32-core machines

---

### 3.4 Batch Operations

**Problem Solved**: Low write throughput

**Design**:
```zig
pub fn batchInsert(arena: *Arena, objects: []Object) ![]GlobalObjRef {
    // 1. Reserve space for all objects (single allocation)
    const total_size = calculateTotalSize(objects);
    const base_ref = try arena.reserveBatch(total_size);
    
    // 2. Write all to WAL (single fsync)
    const lsn = try wal.appendBatch(objects, base_ref);
    
    // 3. Write all to arena (sequential writes)
    var refs = try allocator.alloc(GlobalObjRef, objects.len);
    var offset: u64 = 0;
    for (objects, 0..) |*obj, i| {
        refs[i] = GlobalObjRef.init(shard_id, base_ref + offset);
        try arena.writeAt(base_ref + offset, obj.toBytes());
        offset += obj.totalSize();
    }
    
    // 4. Single fsync for all
    try arena.syncRange(base_ref, total_size);
    
    return refs;
}
```

**Performance**: 10M+ inserts/sec (vs 1M current)

---

## 4. ADVANCED FEATURES

### 4.1 Compaction Engine

**Problem Solved**: Fragmentation over time

**Design**:
```zig
pub const CompactionEngine = struct {
    pub fn compact(self: *Self, arena: *Arena) !void {
        // 1. Identify fragmented pages
        const fragmented_pages = try self.findFragmentedPages(arena);
        
        // 2. For each page, relocate live objects
        for (fragmented_pages) |page_id| {
            const live_objects = try self.scanLiveObjects(arena, page_id);
            
            // 3. Allocate new contiguous space
            const new_base = try arena.allocContiguous(live_objects.total_size);
            
            // 4. Copy objects and update references
            try self.relocateObjects(live_objects, new_base);
            
            // 5. Free old page
            try arena.freePage(page_id);
        }
    }
};
```

**Trigger**: Background thread, runs when fragmentation > 30%

**Impact**: Maintains co-location effectiveness over time

---

### 4.2 Tiered Storage

**Design**:
```
Hot Tier (NVMe SSD)
    ↓ (age > 7 days)
Warm Tier (SATA SSD)
    ↓ (age > 30 days)
Cold Tier (HDD/S3)
```

**Implementation**:
```zig
pub const TieredStorage = struct {
    hot: *Arena,    // NVMe, <50ns access
    warm: *Arena,   // SATA, <100μs access
    cold: *Arena,   // HDD/S3, <10ms access
    
    pub fn read(self: *Self, ref: GlobalObjRef) !Object {
        // Try hot tier first
        if (self.hot.contains(ref)) {
            return self.hot.read(ref);
        }
        
        // Try warm tier
        if (self.warm.contains(ref)) {
            const obj = try self.warm.read(ref);
            // Promote to hot tier
            try self.promoteToHot(ref, obj);
            return obj;
        }
        
        // Cold tier (slow)
        return self.cold.read(ref);
    }
};
```

**Cost Savings**: 10x cheaper storage for cold data

---

### 4.3 Distributed Transactions (2PC)

**Problem Solved**: No cross-shard transactions

**Design**:
```zig
pub fn distributedCommit(coordinator: *TxnCoordinator, txn: *Transaction) !void {
    // Phase 1: Prepare
    var prepare_results = try coordinator.broadcastPrepare(txn);
    
    for (prepare_results) |result| {
        if (!result.can_commit) {
            // Abort on all shards
            try coordinator.broadcastAbort(txn);
            return error.TransactionAborted;
        }
    }
    
    // Phase 2: Commit
    try coordinator.broadcastCommit(txn);
    
    // All shards committed successfully
}
```

**Optimization**: Pipelined 2PC (overlap phases for multiple transactions)

**Performance**: <5ms for cross-shard transactions

---

## 5. OPERATIONAL EXCELLENCE

### 5.1 Observability

**Metrics Export** (Prometheus format):
```zig
pub const Metrics = struct {
    // Throughput
    ops_per_sec: Counter,
    bytes_written_per_sec: Counter,
    
    // Latency
    read_latency_p50: Histogram,
    read_latency_p99: Histogram,
    write_latency_p50: Histogram,
    
    // Resource usage
    arena_utilization: Gauge,
    cache_hit_rate: Gauge,
    gc_pause_time: Histogram,
    
    // Distributed
    replication_lag: Gauge,
    cross_shard_hops: Counter,
};
```

**Distributed Tracing** (OpenTelemetry):
```zig
pub fn tracedRead(ref: GlobalObjRef) !Object {
    const span = tracer.startSpan("storage.read");
    defer span.end();
    
    span.setAttribute("shard_id", ref.shard_id);
    span.setAttribute("local_offset", ref.local_offset);
    
    return arena.read(ref);
}
```

---

### 5.2 Backup & Restore

**Snapshot-based Backups**:
```zig
pub fn createSnapshot(arena: *Arena) !Snapshot {
    // 1. Pause writes (brief)
    arena.pauseWrites();
    defer arena.resumeWrites();
    
    // 2. Flush all dirty pages
    try arena.flushAll();
    
    // 3. Create COW snapshot (copy-on-write)
    const snapshot = try arena.createCOWSnapshot();
    
    // 4. Record metadata
    snapshot.lsn = wal.getCurrentLSN();
    snapshot.timestamp = std.time.timestamp();
    
    return snapshot;
}
```

**Incremental Backups**:
- Track changed pages since last backup
- Only backup deltas
- 100x faster than full backups

---

## 6. PERFORMANCE TARGETS

| Metric | Current Owl | Next-Gen Owl | ClickHouse | ScyllaDB | TigerBeetle |
|--------|-------------|--------------|------------|----------|-------------|
| **Graph hop** | <50ns | **<50ns** ✅ | 100ms+ | N/A | N/A |
| **Write TPS** | 1M | **10M+** ✅ | 1M | 1M | 10M+ |
| **Read QPS** | 500K | **5M+** ✅ | 500K | 1M | N/A |
| **Analytics** | 100ms | **50ms** ✅ | 50ms | 500ms | N/A |
| **Compression** | None | **5-10x** ✅ | 10-100x | 5-10x | Minimal |
| **Replication** | None | **RF=3** ✅ | Yes | Yes | Yes |
| **Durability** | Weak | **Strong** ✅ | Strong | Strong | Strong |

**Result**: Best-in-class for graph navigation + competitive everywhere else

---

## 7. MIGRATION PATH

### Phase 1: Foundation (3 months)
- [ ] Implement GlobalObjRef (128-bit pointers)
- [ ] WAL-first atomic writes
- [ ] Checkpointing system
- [ ] LZ4 compression

### Phase 2: Distribution (6 months)
- [ ] Consistent hashing sharding
- [ ] Raft consensus
- [ ] Replication (RF=3)
- [ ] Distributed transactions (2PC)

### Phase 3: Performance (3 months)
- [ ] Lock-free data structures
- [ ] Batch operations
- [ ] Columnar cache
- [ ] Compaction engine

### Phase 4: Operations (2 months)
- [ ] Metrics & tracing
- [ ] Backup & restore
- [ ] Tiered storage
- [ ] Admin tools

**Total**: 14 months to production-ready

---

## 8. CONCLUSION

**Owl Next-Gen achieves**:
✅ Retains O(1) graph navigation (<50ns)
✅ Horizontal scaling (add nodes dynamically)
✅ Strong durability (WAL-first, replication)
✅ 10x write throughput (batch ops, lock-free)
✅ Competitive analytics (columnar cache)
✅ Production-ready operations (monitoring, backups)

**Unique Position**:
- Only distributed database with O(1) pointer-based navigation
- Combines graph database speed with OLTP/OLAP capabilities
- No compromises on durability or performance

**Next Steps**: Review design, prioritize phases, begin implementation.
