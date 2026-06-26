# Zig Storage Engine Hot Path Optimizations

## Epic 9: Lock-Free Concurrency - Real Implementation Plan

The current Rust-level optimizations miss the actual hot paths. The real bottlenecks are in the **Zig storage engine** where all the critical operations happen.

## Current Bottlenecks in owl_storage_zig

### 1. Transaction Map (transaction.zig)
```zig
// BOTTLENECK: Mutex-protected HashMap
transactions: std.AutoHashMap(TransactionId, *Transaction),
transactions_mutex: std.Thread.Mutex,
```

**Problem**: Every transaction lookup requires mutex acquisition
**Impact**: Serializes all transaction operations
**Solution**: Lock-free hash table with hazard pointers

### 2. MVCC Version Store (mvcc.zig)
```zig
// BOTTLENECK: Mutex-protected HashMap
version_chains: std.AutoHashMap(u128, VersionChain),
version_mutex: std.Thread.Mutex,
```

**Problem**: Version lookups serialize MVCC operations
**Impact**: Kills concurrent read performance
**Solution**: Lock-free version chains with atomic operations

### 3. Arena Allocation (arena.zig)
```zig
// BOTTLENECK: Atomic fetch-add for every allocation
const offset = self.bump.fetchAdd(size, .seq_cst);
```

**Problem**: Sequential consistency is overkill
**Impact**: Unnecessary memory barriers
**Solution**: Relaxed ordering + per-thread allocation pools

### 4. Object Deserialization (object.zig)
```zig
// BOTTLENECK: Full object deserialization for field access
pub fn read(self: *const Arena, objref: ObjRef, allocator: std.mem.Allocator) !Object
```

**Problem**: Deserializes entire object for single field access
**Impact**: Massive overhead for navigation
**Solution**: Zero-copy field access with memory mapping

## Required Zig Optimizations

### 1. Lock-Free Transaction Map
```zig
// Replace mutex-based HashMap with lock-free implementation
pub const LockFreeTransactionMap = struct {
    buckets: []std.atomic.Value(?*TransactionNode),
    hazard_pointers: HazardPointerManager,
    
    pub fn get(self: *Self, txn_id: TransactionId) ?*Transaction {
        // Lock-free lookup with hazard pointer protection
    }
    
    pub fn insert(self: *Self, txn: *Transaction) !void {
        // Lock-free insertion with CAS operations
    }
};
```

### 2. Lock-Free MVCC Version Chains
```zig
// Replace mutex-based version store
pub const LockFreeVersionStore = struct {
    chains: []std.atomic.Value(?*VersionNode),
    
    pub fn getVisibleVersion(self: *Self, object_id: u128, snapshot_lsn: LSN) ?VersionChain {
        // Lock-free version chain traversal
        // Use atomic loads with acquire ordering
    }
    
    pub fn addVersion(self: *Self, object_id: u128, version: VersionChain) !void {
        // Lock-free version chain insertion with CAS
    }
};
```

### 3. Optimized Arena Allocation
```zig
// Per-thread allocation pools to reduce contention
pub const ThreadLocalArena = struct {
    local_bump: u32,
    local_limit: u32,
    global_arena: *Arena,
    
    pub fn alloc(self: *Self, size: u32) !ObjRef {
        // Try thread-local allocation first
        if (self.local_bump + size <= self.local_limit) {
            const offset = self.local_bump;
            self.local_bump += size;
            return ObjRef.init(offset);
        }
        
        // Fall back to global allocation with relaxed ordering
        return self.global_arena.allocSlow(size);
    }
};
```

### 4. Zero-Copy Object Field Access
```zig
// Direct memory-mapped field access without deserialization
pub fn getFieldDirect(self: *const Arena, objref: ObjRef, field_offset: u32) ![]const u8 {
    const obj_ptr = self.basePtr() + objref.getOffset();
    const header = @as(*const ObjectHeader, @ptrCast(obj_ptr));
    
    // Validate header
    if (header.isDeleted()) return error.ObjectDeleted;
    
    // Direct pointer arithmetic to field data
    const field_ptr = obj_ptr + ObjectHeader.headerSize() + field_offset;
    
    // Return slice without copying
    return field_ptr[0..field_length];
}
```

### 5. Cache-Line Optimized Data Structures
```zig
// Align hot data structures to cache lines
pub const CacheLineAligned = struct {
    data: T,
    _padding: [64 - @sizeOf(T)]u8,
} align(64);

// Use cache-friendly layouts for version chains
pub const VersionNode = struct {
    objref: ObjRef,
    created_lsn: LSN,
    deleted_lsn: LSN,
    next: std.atomic.Value(?*VersionNode),
    // Pack into single cache line (64 bytes)
} align(64);
```

## Performance Targets

### Before Optimization (Current State)
- Transaction lookup: ~500ns (mutex + hash lookup)
- Version chain traversal: ~1μs (mutex + linked list)
- Object field access: ~2μs (full deserialization)
- Arena allocation: ~100ns (atomic with seq_cst)

### After Optimization (Target)
- Transaction lookup: ~50ns (lock-free hash table)
- Version chain traversal: ~100ns (lock-free atomic loads)
- Object field access: ~20ns (direct memory access)
- Arena allocation: ~20ns (thread-local pools)

## Implementation Priority

1. **Lock-Free Transaction Map** - Biggest impact on concurrency
2. **Zero-Copy Field Access** - Biggest impact on navigation performance
3. **Lock-Free MVCC Store** - Critical for read scalability
4. **Optimized Arena Allocation** - Reduces allocation overhead
5. **Cache-Line Optimization** - Final performance tuning

## Why This Matters More Than Rust Optimizations

The Rust-level optimizations I implemented are useful but they're optimizing the **wrong layer**:

- **Rust layer**: Query parsing, result formatting, API overhead
- **Zig layer**: Storage, allocation, concurrency, navigation

The Zig layer is where **every single operation** goes through:
- Every document read/write
- Every transaction operation  
- Every reference navigation
- Every memory allocation

Optimizing the Zig layer gives **10-100x** more impact than Rust-layer optimizations.

## Next Steps

1. Implement lock-free transaction map in `transaction.zig`
2. Add zero-copy field access to `object.zig` 
3. Create lock-free version store in `mvcc.zig`
4. Optimize arena allocation in `arena.zig`
5. Add cache-line alignment throughout

This is where the **real** hot path optimization should happen!