# Requirements: Zig Storage Engine Rewrite

## 1. Core Requirements

### 1.1 O(1) Navigation Performance
**Priority**: P0 (Critical)
**Description**: Object graph navigation must execute as pointer dereferencing, not queries.

**Acceptance Criteria**:
- `order.customer.name` executes in <50ns (2-3 memory loads)
- Navigation through 3-level reference chain: <150ns
- No query planning overhead for navigation
- Swizzling (ObjRef → pointer) in <10ns

**Rationale**: This is the core innovation of OQL - navigation is a CPU instruction, not a database query.

### 1.2 Physical Reference System
**Priority**: P0 (Critical)
**Description**: ObjRef must be a 64-bit file offset that can be swizzled to memory pointer.

**Acceptance Criteria**:
- ObjRef is exactly 8 bytes (u64)
- Can calculate page_id and slot_id from offset
- Swizzling converts offset to pointer in O(1)
- Null references (offset=0) are supported
- References are stable across restarts

**Rationale**: Physical pointers enable O(1) navigation and orthogonal persistence.

### 1.3 Co-Location Optimization
**Priority**: P0 (Critical)
**Description**: Related objects must be stored in same 4KB page for cache efficiency.

**Acceptance Criteria**:
- `alloc_near()` places objects in same page when possible
- Co-location success rate >70% for related objects
- Page-aligned allocation (4KB pages)
- Minimize cache misses during navigation

**Rationale**: Co-location ensures navigation stays in L1/L2 cache.

### 1.4 Memory-Mapped Arena Storage
**Priority**: P0 (Critical)
**Description**: Use mmap for zero-copy access to persistent objects.

**Acceptance Criteria**:
- Direct mmap() usage (no wrapper overhead)
- Huge page support on Linux
- Bump allocator for fast allocation
- Free list for reclaimed space
- Support for 1MB - 1TB arena sizes

**Rationale**: mmap provides OS-level page caching and zero-copy I/O.

### 1.5 Orthogonal Persistence
**Priority**: P1 (High)
**Description**: Objects don't know they're persisted - no special serialization code.

**Acceptance Criteria**:
- Objects are plain structs with `extern` layout
- No serialize/deserialize methods on objects
- Persistence is transparent to application code
- Direct memory access to object fields

**Rationale**: Simplifies application code and improves performance.

## 2. Durability Requirements

### 2.1 Write-Ahead Logging (WAL)
**Priority**: P0 (Critical)
**Description**: All mutations must be logged before being applied.

**Acceptance Criteria**:
- WAL append in <2µs
- Group commit (1ms or 1000 entries)
- fsync before commit returns
- Recovery replays WAL entries
- Checkpointing bounds recovery time

**Rationale**: WAL ensures durability and enables crash recovery.

### 2.2 Crash Recovery
**Priority**: P0 (Critical)
**Description**: Database must recover to consistent state after crash.

**Acceptance Criteria**:
- Recovery completes in <10s for 1GB database
- All committed transactions are preserved
- Uncommitted transactions are rolled back
- Checksum validation during recovery
- No data loss for committed transactions

**Rationale**: Production databases must survive crashes.

### 2.3 ACID Transactions
**Priority**: P0 (Critical)
**Description**: Support atomic, consistent, isolated, durable transactions.

**Acceptance Criteria**:
- Snapshot isolation (MVCC)
- Serializable conflict detection
- Atomic commit/abort
- Durable commits (fsync)
- Consistent state after recovery

**Rationale**: Applications need transactional guarantees.

## 3. Performance Requirements

### 3.1 Allocation Performance
**Priority**: P0 (Critical)
**Description**: Object allocation must be extremely fast.

**Acceptance Criteria**:
- Bump allocation: <20ns per object
- Co-located allocation: <50ns per object
- Free list allocation: <30ns per object
- Support 1M+ allocations per second

**Rationale**: Fast allocation enables high write throughput.

### 3.2 Write Throughput
**Priority**: P0 (Critical)
**Description**: Support high transaction throughput.

**Acceptance Criteria**:
- 1M+ transactions per second (single-threaded)
- 5M+ TPS with multiple threads
- Batch writes in <100µs
- WAL group commit reduces fsync overhead

**Rationale**: Production workloads need high write throughput.

### 3.3 Read Throughput
**Priority**: P0 (Critical)
**Description**: Support high query throughput.

**Acceptance Criteria**:
- 500k+ queries per second (single-threaded)
- 2M+ QPS with multiple threads
- Batch swizzling for multi-object reads
- LRU cache hit rate >90%

**Rationale**: Read-heavy workloads are common.

### 3.4 Memory Efficiency
**Priority**: P1 (High)
**Description**: Minimize memory overhead.

**Acceptance Criteria**:
- Object overhead <20% (1.2x object size)
- No hidden allocations (Arc, Box, Vec)
- Explicit allocator usage
- Cache memory bounded by configuration

**Rationale**: Memory is expensive, especially for large databases.

### 3.5 Storage Efficiency
**Priority**: P1 (High)
**Description**: Minimize disk space usage.

**Acceptance Criteria**:
- Write amplification <1.5x
- Compaction reclaims deleted space
- No fragmentation after compaction
- Bloom filters reduce unnecessary reads

**Rationale**: Storage costs money, especially at scale.

## 4. Scalability Requirements

### 4.1 Embedded Mode (<100GB)
**Priority**: P0 (Critical)
**Description**: Support embedded deployments with memory-mapped arenas.

**Acceptance Criteria**:
- Single-file database
- No external dependencies
- Works on laptop/mobile
- <100MB memory for 1GB database

**Rationale**: Many applications need embedded databases.

### 4.2 Server Mode (>100GB)
**Priority**: P1 (High)
**Description**: Support server deployments with LSM storage.

**Acceptance Criteria**:
- LSM tree with 7 levels (L0-L6)
- 256MB SST files
- Bloom filters for fast lookups
- Background compaction
- Support multi-TB databases

**Rationale**: Large applications need scalable storage.

### 4.3 Hybrid Mode (Transition)
**Priority**: P1 (High)
**Description**: Support gradual migration from embedded to server mode.

**Acceptance Criteria**:
- Automatic mode switching at 100GB
- Incremental migration (no downtime)
- Both arena and LSM active during migration
- Transparent to application

**Rationale**: Applications grow over time.

## 5. Correctness Requirements

### 5.1 Reference Integrity
**Priority**: P0 (Critical)
**Description**: All ObjRefs must point to valid objects.

**Acceptance Criteria**:
- No dangling references
- Backlink tracking for updates
- Validation during compaction
- Circular reference detection

**Rationale**: Broken references corrupt the object graph.

### 5.2 Memory Safety
**Priority**: P0 (Critical)
**Description**: No memory corruption, leaks, or undefined behavior.

**Acceptance Criteria**:
- All allocations are freed
- No use-after-free
- No buffer overflows
- No data races
- Passes AddressSanitizer
- Passes ThreadSanitizer

**Rationale**: Memory corruption is catastrophic.

### 5.3 Data Integrity
**Priority**: P0 (Critical)
**Description**: Detect and prevent data corruption.

**Acceptance Criteria**:
- CRC32 checksums on all objects
- WAL checksums
- Validation during recovery
- Corruption detection during reads

**Rationale**: Silent corruption is worse than crashes.

## 6. C ABI Requirements

### 6.1 Clean C Interface
**Priority**: P0 (Critical)
**Description**: Provide stable C API for Rust integration.

**Acceptance Criteria**:
- All exports use `extern "C"` calling convention
- Only C-compatible types in API
- No C++ features (templates, exceptions)
- Stable ABI across versions
- Well-documented error codes

**Rationale**: C ABI is the universal FFI standard.

### 6.2 Zero-Copy Integration
**Priority**: P1 (High)
**Description**: Minimize data copying across FFI boundary.

**Acceptance Criteria**:
- Pass pointers, not values
- Rust can borrow Zig-allocated memory
- No serialization at boundary
- Shared memory for large objects

**Rationale**: FFI overhead should be negligible.

### 6.3 Error Handling
**Priority**: P0 (Critical)
**Description**: Proper error propagation across FFI boundary.

**Acceptance Criteria**:
- Error codes for all operations
- Detailed error messages
- No panics/aborts across boundary
- Rust can convert to Result<T, E>

**Rationale**: Errors must be handled gracefully.

## 7. Development Requirements

### 7.1 Fast Compilation
**Priority**: P1 (High)
**Description**: Minimize build times for rapid iteration.

**Acceptance Criteria**:
- Full build in <10 seconds
- Incremental build in <2 seconds
- No long-running macros
- Parallel compilation

**Rationale**: Fast iteration improves productivity.

### 7.2 Good Debugging
**Priority**: P1 (High)
**Description**: Easy to debug and diagnose issues.

**Acceptance Criteria**:
- Debug symbols in builds
- GDB/LLDB support
- Assertions in debug mode
- Logging infrastructure
- Memory profiling tools

**Rationale**: Debugging is a major time sink.

### 7.3 Comprehensive Testing
**Priority**: P0 (Critical)
**Description**: Extensive test coverage.

**Acceptance Criteria**:
- Unit tests for all modules
- Integration tests for workflows
- Property-based tests for invariants
- Crash recovery tests
- Concurrency tests
- >80% code coverage

**Rationale**: Bugs in storage engines are catastrophic.

## 8. Compatibility Requirements

### 8.1 Platform Support
**Priority**: P0 (Critical)
**Description**: Support major platforms.

**Acceptance Criteria**:
- Linux (x86_64, aarch64)
- macOS (x86_64, aarch64)
- Windows (x86_64) - optional

**Rationale**: Developers use different platforms.

### 8.2 Rust Integration
**Priority**: P0 (Critical)
**Description**: Seamless integration with Rust query layer.

**Acceptance Criteria**:
- Safe Rust wrappers
- Idiomatic Rust API
- No unsafe in public API
- RAII for resource management

**Rationale**: Query layer is in Rust.

## Success Metrics

### Performance Metrics
- Navigation: <50ns per hop ✅
- Allocation: <20ns per object ✅
- Write throughput: >1M TPS ✅
- Read throughput: >500k QPS ✅
- Memory overhead: <1.2x ✅

### Quality Metrics
- Zero memory leaks ✅
- Zero data corruption ✅
- >80% test coverage ✅
- <10s build time ✅

### Adoption Metrics
- Passes all existing Rust tests ✅
- Performance improvement >20% ✅
- Memory reduction >30% ✅
- Team comfortable with Zig ✅
