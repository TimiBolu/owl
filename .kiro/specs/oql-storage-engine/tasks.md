# Implementation Plan

- [x] 1. Implement core object model and physical references
  - Create Object and ObjectHeader structs with proper memory layout (#[repr(C)])
  - Implement ObjRef with swizzling methods (offset to pointer conversion)
  - Add page_id() and slot_id() helper methods for ObjRef
  - Implement VarData encoding using CBOR for nested fields
  - Create ObjectStore trait defining core storage operations
  - _Requirements: 1.1, 1.3, 3.1, 3.2, 10.1, 10.2_

- [x] 2. Build arena-based memory-mapped storage
  - [x] 2.1 Create Arena struct with mmap support
    - Implement Arena::new() with file-backed memory mapping using memmap2
    - Add huge page support using madvise(MADV_HUGEPAGE) on Linux
    - Create bump allocator with AtomicU32 for thread-safe allocation
    - Implement free list for reclaiming deleted object slots
    - _Requirements: 1.4, 4.1, 6.1, 10.3_

  - [x] 2.2 Implement co-location allocation strategy
    - Add alloc_near() method to allocate objects in same 4KB page
    - Implement page-level allocation tracking
    - Create heuristics for determining co-location candidates
    - Add metrics for measuring co-location effectiveness
    - _Requirements: 1.2, 1.4, 4.1, 4.2, 4.3_

  - [x] 2.3 Add arena read/write operations
    - Implement write() method to store objects at ObjRef locations
    - Create read() method to load objects from ObjRef
    - Add bounds checking and validation for all operations
    - Implement atomic updates for object modifications
    - _Requirements: 2.1, 2.4, 10.2, 10.3_

  - [ ]* 2.4 Write arena allocation tests
    - Test bump allocation correctness and thread safety
    - Verify co-location places objects in same page
    - Test free list reclamation
    - Benchmark allocation performance
    - _Requirements: 1.2, 4.1_

- [x] 3. Implement Write-Ahead Log (WAL) for durability
  - [x] 3.1 Create WAL structure and entry types
    - Define WALEntry enum (Insert, Update, Delete, Commit, Abort, Checkpoint)
    - Implement WAL struct with file handle and buffer
    - Add LSN (Log Sequence Number) tracking with AtomicU64
    - Create serialization for WAL entries using bincode
    - _Requirements: 2.2, 2.3, 5.3, 5.5_

  - [x] 3.2 Implement WAL append and group commit
    - Add append() method to buffer WAL entries
    - Implement group commit (flush every 1ms or 1000 entries)
    - Add sync() method with fsync for durability
    - Create background thread for periodic syncing
    - _Requirements: 2.2, 5.3, 8.3_

  - [x] 3.3 Build WAL recovery mechanism
    - Implement recover() to replay WAL entries after crash
    - Add checksum validation for WAL entries
    - Create checkpoint mechanism to bound recovery time
    - Handle corrupted WAL entries gracefully
    - _Requirements: 2.3, 2.5, 8.1, 8.2, 8.4, 8.5_

  - [ ]* 3.4 Write WAL durability tests
    - Test crash recovery by killing process mid-transaction
    - Verify all committed transactions survive crashes
    - Test checkpoint creation and recovery from checkpoint
    - Benchmark WAL write throughput
    - _Requirements: 2.5, 5.5, 8.1_

- [x] 4. Create swizzling and object cache system
  - [x] 4.1 Implement ObjectCache with LRU eviction
    - Create per-transaction cache (HashMap<ObjRef, *mut Object>)
    - Add global LRU cache using lru crate
    - Implement cache invalidation on object updates
    - Add cache hit/miss metrics
    - _Requirements: 1.1, 1.3, 6.5_

  - [x] 4.2 Build swizzling mechanism
    - Implement swizzle() to convert ObjRef to memory pointer
    - Add safety checks to validate ObjRef before dereferencing
    - Create batch_swizzle() for efficient multi-object loading
    - Implement prefetch using madvise(MADV_WILLNEED)
    - _Requirements: 1.1, 1.2, 1.3, 3.2, 4.4, 9.5, 10.2, 10.3_

  - [x] 4.3 Add page loading and prefetching
    - Implement load_page() to bring 4KB pages into cache
    - Create prefetch_page() for sequential access optimization
    - Add page grouping for batch_swizzle operations
    - Implement adaptive prefetching based on access patterns
    - _Requirements: 1.2, 4.3, 9.5_

  - [ ]* 4.4 Write swizzling performance tests
    - Benchmark single-level reference navigation latency
    - Test multi-level navigation (order.customer.profile.name)
    - Verify O(1) complexity with increasing object counts
    - Measure cache hit rates under various workloads
    - _Requirements: 1.1, 1.2, 1.3_

- [x] 5. Implement MVCC transaction manager
  - [x] 5.1 Create Transaction structure with snapshot isolation
    - Define Transaction struct with id, snapshot_lsn, write_set, read_set
    - Implement begin() to create transaction with current LSN snapshot
    - Add read_set tracking for serializability validation
    - Create write_set buffering for deferred writes
    - _Requirements: 5.1, 5.2, 5.4_

  - [x] 5.2 Build transaction commit and abort logic
    - Implement commit() with conflict detection
    - Add validation to check for newer versions in read_set
    - Write all changes to WAL before returning from commit
    - Implement abort() to discard write_set changes
    - _Requirements: 5.1, 5.2, 5.3, 5.4_

  - [x] 5.3 Add lock table for write conflicts
    - Create LockTable with row-level locking
    - Implement deadlock detection using wait-for graph
    - Add timeout-based deadlock resolution
    - Support lock escalation for bulk operations
    - _Requirements: 5.2, 5.4_

  - [ ]* 5.4 Write transaction ACID tests
    - Test atomicity by aborting mid-transaction
    - Verify consistency with concurrent transactions
    - Test isolation levels (snapshot isolation)
    - Verify durability with crash recovery
    - Test deadlock detection and resolution
    - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5_

- [x] 6. Build backlink index and compaction system
  - [x] 6.1 Implement BacklinkIndex for reference tracking
    - Create BacklinkIndex with HashMap<ObjRef, Vec<ObjRef>>
    - Add add_reference() to track referrer → target relationships
    - Implement get_referrers() to find all objects pointing to target
    - Create update_reference() to fix references during compaction
    - _Requirements: 3.3, 7.2, 7.3_

  - [x] 6.2 Create mark-and-sweep garbage collector
    - Implement mark_live_objects() using graph traversal
    - Add root set identification (indexed objects, active transactions)
    - Create sweep phase to identify unreachable objects
    - Implement free list updates for reclaimed space
    - _Requirements: 7.1, 7.2_

  - [x] 6.3 Build compaction and defragmentation
    - Implement relocate_objects() to move live objects to new arena
    - Add atomic reference updates using backlink index
    - Create defragment() to consolidate free space
    - Implement incremental compaction to avoid long pauses
    - _Requirements: 4.4, 7.2, 7.3, 7.4, 7.5_

  - [ ]* 6.4 Write compaction correctness tests
    - Test that all references are updated after compaction
    - Verify no objects are lost during compaction
    - Test compaction with concurrent transactions
    - Benchmark compaction performance and pause times
    - _Requirements: 7.2, 7.3, 7.5_

- [x] 7. Implement LSM-based storage for scalability
  - [x] 7.1 Create Segment structure for SST files
    - Define Segment struct with file, index (BTreeMap), bloom filter
    - Implement Segment::write() to create sorted SST files
    - Add SegmentMetadata for min/max keys, level, size
    - Create serialization for segments using bincode
    - _Requirements: 6.2, 6.3_

  - [x] 7.2 Build LSM read path with bloom filters
    - Implement get() with bloom filter fast path
    - Add index lookup for object location
    - Create scan_range() for range queries
    - Implement multi-segment search with level ordering
    - _Requirements: 6.2, 9.1_

  - [x] 7.3 Implement LSM compaction strategy
    - Create leveled compaction (levels 0-6)
    - Add size-tiered compaction for level 0
    - Implement merge operation for overlapping segments
    - Create background compaction thread
    - _Requirements: 6.2, 6.3, 7.2, 7.4_

  - [ ]* 7.4 Write LSM performance tests
    - Benchmark write throughput with LSM
    - Test read latency with multiple levels
    - Verify compaction reduces read amplification
    - Test scalability beyond 100 GB
    - _Requirements: 6.2, 6.3, 6.4_

- [x] 8. Create query execution engine
  - [x] 8.1 Define query plan structures
    - Create QueryPlan enum (IndexScan, FullScan, NavigationScan)
    - Define Path enum for field access and reference navigation
    - Implement Expression for filter conditions
    - Add projection support for field selection
    - _Requirements: 9.1, 9.2_

  - [x] 8.2 Implement query executor with batch operations
    - Create QueryExecutor with cache and index access
    - Implement execute() for different plan types
    - Add batch_swizzle for efficient reference resolution
    - Create project() for field extraction
    - _Requirements: 9.1, 9.2, 9.5_

  - [x] 8.3 Add index support for query optimization
    - Create Index trait for different index types
    - Implement BTreeIndex for range queries
    - Add HashIndex for equality lookups
    - Create automatic index selection in query planner
    - _Requirements: 9.3_

  - [x] 8.4 Implement streaming for large result sets
    - Create ObjectStream for lazy result iteration
    - Add execute_stream() to QueryExecutor
    - Implement cursor-based pagination
    - Add memory limits to prevent exhaustion
    - _Requirements: 9.4_

  - [ ]* 8.5 Write query execution tests
    - Test single-level reference navigation queries
    - Verify multi-level navigation (order.customer.profile.name)
    - Test filtering with WHERE clauses
    - Benchmark query throughput (QPS)
    - Test streaming with large result sets
    - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5_

- [x] 9. Add configuration and deployment features
  - [x] 9.1 Create configuration system
    - Define Config struct with storage mode (embedded/server)
    - Add arena size, LSM level count, cache size parameters
    - Implement environment-specific configuration loading
    - Create validation for configuration values
    - _Requirements: 6.1, 6.2, 6.3_

  - [x] 9.2 Implement storage mode switching
    - Add logic to choose arena vs LSM based on database size
    - Create migration path from arena to LSM storage
    - Implement hybrid mode for transition period
    - Add monitoring for storage mode performance
    - _Requirements: 6.1, 6.2, 6.4_

  - [x] 9.3 Build CLI tool for database management
    - Create CLI with clap for database operations
    - Add commands for backup, restore, compact, stats
    - Implement query execution from command line
    - Add interactive REPL mode
    - _Requirements: 6.1, 6.3_

  - [ ]* 9.4 Write deployment and configuration tests
    - Test configuration loading from files
    - Verify storage mode switching works correctly
    - Test CLI commands for database management
    - Benchmark embedded vs server mode performance
    - _Requirements: 6.1, 6.2, 6.3, 6.4_

- [ ] 10. Comprehensive testing and benchmarking
  - [x] 10.1 Create end-to-end integration tests
    - Test complete workflow: insert, navigate, query, update, delete
    - Verify O(1) navigation with real object graphs
    - Test concurrent access with multiple threads
    - Verify crash recovery with random failures
    - _Requirements: 1.1, 1.5, 2.1, 2.3, 5.1, 8.1_

  - [x] 10.2 Build performance benchmark suite
    - Benchmark navigation latency (target: <50ns per hop)
    - Measure write throughput (target: 1M+ TPS)
    - Test read throughput (target: 500k+ QPS)
    - Verify memory overhead (target: 1.2x object size)
    - Measure storage efficiency (target: 1.5x write amp)
    - _Requirements: 1.1, 1.2, 6.4_

  - [x] 10.3 Implement property-based tests
    - Use proptest for reference integrity validation
    - Test crash consistency with random operation sequences
    - Verify serializability of concurrent transactions
    - Test compaction preserves all reachable objects
    - _Requirements: 2.5, 5.1, 7.2, 8.1_

  - [ ]* 10.4 Create stress tests and chaos engineering
    - Implement long-running stress test with mixed workload
    - Add chaos testing with random process kills
    - Test memory limits and OOM handling
    - Verify performance under sustained load
    - _Requirements: 6.4, 6.5, 8.1_
