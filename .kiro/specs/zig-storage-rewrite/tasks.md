# Implementation Plan: Zig Storage Engine Rewrite

## Phase 1: Core Types & Memory Management (Weeks 1-3)

- [x] 1. Core Types & Memory Management
  - [x] 1.1 ObjRef Implementation
    - Implement 64-bit physical pointer type for object references
    - `ObjRef` is exactly 8 bytes (`extern struct`)
    - Supports null references (offset = 0)
    - Can extract page_id and slot_id from offset
    - Swizzling (offset → pointer) in <10ns (inline function)
    - Serialization/deserialization works correctly (simple u64)
    - All unit tests pass (15 tests)
    - _Files: `owl_storage_zig/src/objref.zig`_
    - _Requirements: Core memory management, C ABI compatibility_
    - _Status: ✅ COMPLETE (2025-11-15)_

  - [x] 1.2 ObjectHeader Implementation
    - Implement fixed-size object header with metadata
    - Header fits in 64-byte cache line (48 bytes actual)
    - `extern struct` with predictable layout
    - Supports type_tag, version, size, flags, checksum, LSN
    - Bit flags for deleted/pinned/overflow
    - CRC32 checksum calculation
    - All unit tests pass (18 tests)
    - _Files: `owl_storage_zig/src/object_header.zig`_
    - _Requirements: Cache-aligned headers, metadata tracking_
    - _Status: ✅ COMPLETE (2025-11-15)_

  - [x] 1.3 Object Structure
    - Implement complete object with header, data, and references
    - Object contains header + fixed fields + var_data + refs
    - Explicit allocator usage (no hidden allocations)
    - Serialization to bytes for storage
    - Deserialization from bytes
    - Reference array management
    - All unit tests pass (15 tests)
    - _Files: `owl_storage_zig/src/object.zig`_
    - _Requirements: Complete object model, serialization_
    - _Status: ✅ COMPLETE (2025-11-15)_

  - [x] 1.4 Arena Allocator
    - Implement arena-based memory allocator for objects
    - Page-based allocation (4KB pages)
    - Slot-based object storage within pages
    - Free list management for deleted objects
    - Memory compaction support
    - All unit tests pass (12 tests)
    - _Files: `owl_storage_zig/src/arena.zig`_
    - _Requirements: Efficient memory allocation, fragmentation management_
    - _Status: ✅ COMPLETE (2025-11-15)_

  - [x] 1.5 Rust FFI Bindings
    - Create C-compatible FFI layer for Rust integration
    - Export arena operations (create, allocate, read, write, delete)
    - Export ObjRef operations
    - Handle error propagation across FFI boundary
    - Memory safety guarantees
    - All integration tests pass
    - _Files: `owl_storage_zig/rust_bindings/src/ffi.rs`, `owl_storage_zig/rust_bindings/src/arena.rs`_
    - _Requirements: Rust-Zig interop, safe FFI_
    - _Status: ✅ COMPLETE (2025-11-15)_

## Phase 2: Storage Layer (Weeks 4-5)

- [x] 2. Storage Layer
  - [x] 2.1 Page Management
    - Implement page-based storage with 4KB pages
    - Page header with metadata (page_id, free_space, slot_count)
    - Slot directory for object locations within page
    - Free space tracking and compaction
    - Page serialization/deserialization
    - All unit tests pass
    - _Files: `owl_storage_zig/src/page.zig`_
    - _Requirements: Page-based storage, efficient space management_
    - _Status: ✅ COMPLETE (2025-11-15)_

  - [x] 2.2 Buffer Pool Manager
    - Implement LRU buffer pool for page caching
    - Pin/unpin mechanism for page access
    - Dirty page tracking
    - Page eviction policy (LRU)
    - Thread-safe page access
    - All unit tests pass
    - _Files: `owl_storage_zig/src/buffer_pool.zig`_
    - _Requirements: Page caching, memory efficiency_
    - _Status: ✅ COMPLETE (2025-11-15)_

  - [x] 2.3 Rust Storage Integration
    - Create Rust wrapper for Zig storage engine
    - Implement DocumentStore trait using Zig backend
    - Handle Document ↔ Object conversion
    - Error handling and propagation
    - All integration tests pass
    - _Files: `owl_storage_engine/src/zig_store.rs`, `owl_storage_engine/src/document_bridge.rs`_
    - _Requirements: Rust integration, DocumentStore implementation_
    - _Status: ✅ COMPLETE (2025-11-15)_

## Phase 3: WAL & Durability (Weeks 6-7)

- [x] 3. WAL & Durability
  - [x] 3.1 WAL Structure
    - Implement Write-Ahead Log structure
    - WAL record types (insert, update, delete, checkpoint)
    - LSN (Log Sequence Number) tracking
    - WAL file format and serialization
    - All unit tests pass
    - _Files: `owl_storage_zig/src/wal.zig`_
    - _Requirements: Durability, crash recovery_
    - _Status: ✅ COMPLETE (2025-11-16)_

  - [x] 3.2 WAL Append & Group Commit
    - Implement WAL append operations
    - Group commit for batching writes
    - fsync optimization
    - WAL buffer management
    - All unit tests pass
    - _Files: `owl_storage_zig/src/wal.zig`_
    - _Requirements: Write performance, durability guarantees_
    - _Status: ✅ COMPLETE (2025-11-16)_

  - [x] 3.3 WAL Recovery
    - Implement crash recovery using WAL
    - Replay WAL records on startup
    - Checkpoint mechanism
    - Truncate WAL after checkpoint
    - All recovery tests pass
    - _Files: `owl_storage_zig/src/wal.zig`_
    - _Requirements: Crash recovery, data consistency_
    - _Status: ✅ COMPLETE (2025-11-16)_

## Phase 4: Transactions & MVCC (Weeks 8-9)

- [x] 4. Transactions & MVCC
  - [x] 4.1 Transaction Structure
    - Implement transaction context
    - Transaction ID generation
    - Transaction state tracking (active, committed, aborted)
    - Transaction isolation levels
    - All unit tests pass
    - _Files: `owl_storage_zig/src/transaction.zig`_
    - _Requirements: ACID transactions, isolation_
    - _Status: ✅ COMPLETE (2025-11-16)_

  - [x] 4.2 MVCC Snapshot Isolation
    - Implement Multi-Version Concurrency Control
    - Snapshot isolation for read consistency
    - Version chain management
    - Garbage collection of old versions
    - All unit tests pass
    - _Files: `owl_storage_zig/src/mvcc.zig`_
    - _Requirements: Concurrent access, read consistency_
    - _Status: ✅ COMPLETE (2025-11-16)_

  - [x] 4.3 Transaction Commit & Abort
    - Implement two-phase commit protocol
    - Transaction commit with WAL logging
    - Transaction abort and rollback
    - Lock management and deadlock detection
    - All transaction tests pass
    - _Files: `owl_storage_zig/src/transaction.zig`_
    - _Requirements: Atomicity, consistency_
    - _Status: ✅ COMPLETE (2025-11-16)_

## Phase 5: Performance & Optimization (Weeks 10-11)

- [x] 5. Performance & Optimization
  - [x] 5.1 Cache Management
    - Implement object cache with LRU eviction
    - Cache hit/miss tracking
    - Cache size limits and memory pressure handling
    - Thread-safe cache access
    - All cache tests pass
    - _Files: `owl_storage_zig/src/cache.zig`_
    - _Requirements: Performance optimization, memory efficiency_
    - _Status: ✅ COMPLETE (2025-11-16)_

  - [x] 5.2 Batch Swizzling
    - Implement batch pointer swizzling for bulk operations
    - Prefetch optimization for sequential access
    - Batch reference resolution
    - Performance benchmarks show improvement
    - All batch tests pass
    - _Files: `owl_storage_zig/src/batch_swizzle.zig`_
    - _Requirements: Bulk operation performance_
    - _Status: ✅ COMPLETE (2025-11-16)_

  - [x] 5.3 Memory Profiling & Optimization
    - Profile memory usage patterns
    - Identify and fix memory leaks
    - Optimize allocation patterns
    - Memory usage reduced by target amount
    - All memory tests pass
    - _Files: `owl_storage_zig/benches/memory_bench.zig`_
    - _Requirements: Memory efficiency_
    - _Status: ✅ COMPLETE (2025-11-16)_

  - [x] 5.4 Performance Benchmarking
    - Create comprehensive benchmark suite
    - Benchmark allocation (<20ns per object)
    - Benchmark navigation (<50ns per hop)
    - Benchmark throughput and latency
    - All performance targets met
    - _Files: `owl_storage_zig/benches/bench_all.zig`_
    - _Requirements: Performance validation_
    - _Status: ✅ COMPLETE (2025-11-16)_

## Phase 6: Testing & Validation (Weeks 12-13)

- [ ] 6. Testing & Validation
  - [ ] 6.1 Property-Based Testing
    - [ ] 6.1.1 Reference integrity properties
      - Test that all references point to valid objects
      - Test reference cycles are detected
      - Test dangling references are prevented
      - _Requirements: Data integrity_

    - [ ] 6.1.2 Transaction serializability properties
      - Test concurrent transactions maintain serializability
      - Test isolation levels are respected
      - Test no lost updates or dirty reads
      - _Requirements: Transaction correctness_

    - [ ] 6.1.3 Crash consistency properties
      - Test data consistency after crashes
      - Test WAL recovery correctness
      - Test no data loss for committed transactions
      - _Requirements: Durability guarantees_

    - [ ] 6.1.4 1000+ test cases per property
      - Generate diverse test cases
      - Cover edge cases and corner cases
      - Randomized testing with seeds
      - _Requirements: Comprehensive testing_

    - [ ] 6.1.5 All properties hold
      - All property tests pass
      - No counterexamples found
      - Performance acceptable under testing
      - _Files: `owl_storage_zig/tests/property_tests.zig`_
      - _Requirements: Correctness validation_

  - [ ] 6.2 Crash Recovery Testing
    - [ ] 6.2.1 Crash at various points in transaction
      - Simulate crashes during transaction execution
      - Test recovery from different crash points
      - Verify transaction atomicity
      - _Requirements: Crash recovery_

    - [ ] 6.2.2 Verify all committed data preserved
      - Check all committed transactions are recovered
      - Verify data integrity after recovery
      - Test no data corruption
      - _Requirements: Durability_

    - [ ] 6.2.3 Verify uncommitted data rolled back
      - Check uncommitted transactions are aborted
      - Verify no partial updates visible
      - Test rollback correctness
      - _Requirements: Atomicity_

    - [ ] 6.2.4 Recovery time <10s for 1GB database
      - Benchmark recovery performance
      - Optimize recovery if needed
      - Test with various database sizes
      - _Requirements: Performance_

    - [ ] 6.2.5 All crash tests pass
      - All crash scenarios handled correctly
      - No data loss or corruption
      - Recovery is deterministic
      - _Files: `owl_storage_zig/tests/crash_tests.zig`_
      - _Requirements: Reliability_

  - [ ] 6.3 Concurrency Testing
    - [ ] 6.3.1 Multiple threads reading/writing
      - Test concurrent read operations
      - Test concurrent write operations
      - Test mixed read/write workloads
      - _Requirements: Concurrency_

    - [ ] 6.3.2 No data races (ThreadSanitizer clean)
      - Run tests with ThreadSanitizer
      - Fix any detected data races
      - Verify thread safety
      - _Requirements: Thread safety_

    - [ ] 6.3.3 Serializable isolation maintained
      - Test isolation under concurrent access
      - Verify no anomalies occur
      - Test conflict detection
      - _Requirements: Isolation_

    - [ ] 6.3.4 Deadlock detection
      - Test deadlock detection mechanism
      - Verify deadlock resolution
      - Test timeout handling
      - _Requirements: Liveness_

    - [ ] 6.3.5 All concurrency tests pass
      - All concurrent scenarios work correctly
      - No race conditions or deadlocks
      - Performance acceptable under load
      - _Files: `owl_storage_zig/tests/concurrency_tests.zig`_
      - _Requirements: Concurrent correctness_

  - [ ] 6.4 Integration Testing with Rust
    - [ ] 6.4.1 All existing Rust tests pass
      - Run full Rust test suite
      - Fix any failing tests
      - Verify backward compatibility
      - _Requirements: Integration_

    - [ ] 6.4.2 No performance regressions
      - Benchmark against previous implementation
      - Verify performance improvements
      - Test query execution speed
      - _Requirements: Performance_

    - [ ] 6.4.3 Memory usage improved
      - Measure memory consumption
      - Verify 50% reduction target met
      - Test memory efficiency
      - _Requirements: Memory optimization_

    - [ ] 6.4.4 End-to-end workflows work
      - Test complete application workflows
      - Verify all features functional
      - Test error handling
      - _Requirements: Functionality_

    - [ ] 6.4.5 All integration tests pass
      - All Rust-Zig integration works
      - No FFI issues
      - Stable and reliable
      - _Files: `rust_bindings/tests/full_integration_test.rs`_
      - _Requirements: Integration correctness_

## Phase 7: Documentation & Polish (Week 14)

- [ ] 7. Documentation & Polish
  - [ ] 7.1 API Documentation
    - [ ] 7.1.1 All public functions documented
      - Document all public APIs
      - Include parameter descriptions
      - Document return values and errors
      - _Requirements: Documentation_

    - [ ] 7.1.2 Examples for common use cases
      - Create usage examples
      - Document best practices
      - Include code samples
      - _Requirements: Usability_

    - [ ] 7.1.3 C API reference complete
      - Document C FFI interface
      - Include memory management notes
      - Document error codes
      - _Requirements: C integration_

    - [ ] 7.1.4 Rust API reference complete
      - Document Rust wrapper APIs
      - Include trait implementations
      - Document safety guarantees
      - _Requirements: Rust integration_

    - [ ] 7.1.5 Architecture diagrams
      - Create system architecture diagrams
      - Document component interactions
      - Include data flow diagrams
      - _Files: `docs/api_reference.md`, `docs/architecture.md`, `docs/examples.md`_
      - _Requirements: Documentation completeness_

  - [ ] 7.2 Performance Documentation
    - [ ] 7.2.1 Benchmark results documented
      - Document all benchmark results
      - Include performance comparisons
      - Show performance trends
      - _Requirements: Performance transparency_

    - [ ] 7.2.2 Comparison with Rust implementation
      - Compare performance metrics
      - Document improvements
      - Explain performance differences
      - _Requirements: Performance validation_

    - [ ] 7.2.3 Performance tuning guide
      - Document tuning parameters
      - Include optimization tips
      - Provide configuration examples
      - _Requirements: Performance optimization_

    - [ ] 7.2.4 Memory usage analysis
      - Document memory consumption
      - Explain memory layout
      - Include memory optimization tips
      - _Files: `docs/performance.md`, `docs/tuning_guide.md`_
      - _Requirements: Memory transparency_

  - [ ] 7.3 Migration Guide
    - [ ] 7.3.1 Step-by-step migration instructions
      - Create migration checklist
      - Document migration steps
      - Include rollback procedures
      - _Requirements: Migration support_

    - [ ] 7.3.2 Breaking changes documented
      - List all breaking changes
      - Explain impact and workarounds
      - Provide migration examples
      - _Requirements: Change management_

    - [ ] 7.3.3 Compatibility notes
      - Document compatibility requirements
      - List supported platforms
      - Include version compatibility
      - _Requirements: Compatibility_

    - [ ] 7.3.4 Troubleshooting guide
      - Document common issues
      - Provide solutions and workarounds
      - Include debugging tips
      - _Files: `docs/migration_guide.md`_
      - _Requirements: Support_

## Phase 8: Query Engine Integration (Weeks 15-20)

- [ ] 8. Query Engine Integration
  - [x] 8.0 Schema-Aware Parser Enhancement
    - [x] 8.0.1 Parser has access to SchemaRegistry
      - Create SchemaRegistry struct with thread-safe schema management
      - Add optional schema_registry field to Parser
      - Add Parser::with_schema_registry() constructor
      - Add parse_oql_with_schema() and parse_oql_statement_with_schema() functions
      - _Files: `src/parser.rs`, `src/schema.rs`_
      - _Requirements: Parser can access schema information during parsing_
      - _Status: ✅ COMPLETE (2025-11-16)_

    - [x] 8.0.2 Type checking during parsing
      - Validate field types match expected types in expressions
      - Check comparison operators are valid for field types
      - Validate function arguments match expected types
      - _Requirements: Type errors caught at parse time_

    - [x] 8.0.3 Field existence validation
      - Validate all field references exist in schema
      - Support nested field validation through references
      - Provide helpful error messages for missing fields
      - _Requirements: Field existence validated during parsing_

    - [x] 8.0.4 Reference target validation
      - Validate reference fields point to existing schemas
      - Check reference navigation is valid
      - Validate cascade behavior is respected
      - _Requirements: Reference integrity checked at parse time_

    - [x] 8.0.5 Better error messages with schema context
      - Include schema information in error messages
      - Suggest similar field names for typos
      - Show expected vs actual types in errors
      - _Requirements: Error messages are clear and actionable_

    - [x] 8.0.6 All parser tests pass
      - Update existing parser tests
      - Add schema-aware parser tests
      - Test error message quality
      - _Files: `tests/schema_aware_parser_test.rs`_
      - _Requirements: All tests pass with schema validation_

  - [x] 8.1 ZigStore Adapter
    - Create DocumentStore implementation using Zig storage
    - Implement `DocumentStore` trait for Zig arena
    - Bridge Rust `Document` ↔ Zig `Object`
    - Handle serialization/deserialization
    - Support all CRUD operations
    - All existing storage tests pass
    - _Files: `owl_storage_engine/src/zig_store.rs`, `owl_storage_engine/src/document_bridge.rs`, `owl_storage_engine/tests/zig_store_test.rs`_
    - _Requirements: Zig storage integration, DocumentStore implementation_

  - [x] 8.2 ObjRef-Based References
    - Change Value::Reference from String to ObjRef
    - Update `Value::Reference(String)` to `Value::Reference(ObjRef)`
    - Update all reference handling code
    - Maintain backward compatibility layer
    - Update serialization format
    - All tests pass with new reference type
    - _Files: `src/document.rs`, `src/executor.rs`, `src/reference.rs`, `src/storage.rs`, `tests/reference_migration_test.rs`_
    - _Requirements: ObjRef-based references, backward compatibility_

  - [x] 8.3 O(1) Reference Navigation
    - Implement true O(1) pointer-based navigation
    - Navigation uses swizzling (pointer arithmetic)
    - No database lookups for reference traversal
    - Benchmark shows <50ns per hop
    - Works with nested references
    - All navigation tests pass
    - _Files: `src/executor.rs`, `owl_storage_engine/src/zig_store.rs`, `benches/navigation_bench.rs`_
    - _Requirements: O(1) navigation performance_

  - [x] 8.4 Executor Integration
    - Update QueryExecutor to use ZigStore
    - QueryExecutor works with ZigStore
    - All existing queries execute correctly
    - Performance improved vs RocksDB
    - Memory usage reduced
    - All executor tests pass
    - _Files: `src/executor.rs`, `src/query_engine.rs`, `tests/executor_zig_test.rs`_
    - _Requirements: Query engine integration_

  - [x] 8.5 RocksDB Deprecation
    - Deprecate RocksDB storage and make Zig storage default
    - ZigStore is default storage implementation
    - RocksDB marked as deprecated
    - Migration path documented
    - Feature flag for RocksDB (optional)
    - All tests use ZigStore by default
    - _Files: `src/storage.rs`, `Cargo.toml`, `docs/ROCKSDB_DEPRECATION.md`_
    - _Requirements: Default storage migration_

## Phase 9: Query Planning & Optimization (Weeks 21-25)

- [x] 9. Query Planning & Optimization
  - [x] 9.1 Logical Planner
    - Implement logical query planning layer
    - Define `LogicalPlan` enum
    - Convert AST to logical plan
    - Type checking during planning
    - Schema validation
    - All planning tests pass
    - _Files: `src/planner/logical.rs`, `src/planner/types.rs`, `tests/logical_planner_test.rs`_
    - _Requirements: Query planning, type checking_

  - [x] 9.2 Physical Planner
    - Implement physical query planning layer
    - Define `PhysicalPlan` enum
    - Convert logical to physical plan
    - Operator selection (SeqScan, IndexScan)
    - Join algorithm selection
    - All physical planning tests pass
    - _Files: `src/planner/physical.rs`, `src/planner/operators.rs`, `tests/physical_planner_test.rs`_
    - _Requirements: Physical planning, operator selection_

  - [x] 9.3 Physical Operators
    - Implement iterator-based physical operators
    - `PhysicalOperator` trait defined
    - Implement SeqScan, Filter, Project
    - Implement NestedLoopJoin, HashJoin
    - Implement Sort, Limit, Aggregate
    - Iterator-based execution
    - All operator tests pass
    - _Files: `src/execution/operator.rs`, `src/execution/scan.rs`, `src/execution/join.rs`, `src/execution/aggregate.rs`, `tests/operator_test.rs`_
    - _Requirements: Physical operators, iterator execution_

  - [x] 9.4 Statistics Collection
    - Implement statistics collection for cost estimation
    - Table size tracking
    - Cardinality estimation
    - Index selectivity tracking
    - Histogram generation
    - Statistics persistence
    - All statistics tests pass
    - _Files: `src/optimizer/statistics.rs`, `src/optimizer/histogram.rs`, `tests/statistics_test.rs`_
    - _Requirements: Cost estimation, statistics_

  - [x] 9.5 Cost-Based Optimizer
    - Implement cost-based query optimization
    - Cost model for operators
    - Predicate pushdown
    - Index selection
    - Join reordering
    - All optimization tests pass
    - _Files: `src/optimizer/cost_model.rs`, `src/optimizer/rules.rs`, `tests/optimizer_test.rs`_
    - _Requirements: Query optimization, cost-based planning_

## Phase 10: Indexes & Advanced Features (Weeks 26-30)

- [-] 10. Indexes & Advanced Features
  - [x] 10.1 B-Tree Index in Zig
    - Implement B-tree indexes in Zig
    - B-tree structure with configurable order
    - Insert, delete, search operations
    - Range queries
    - Concurrent access support
    - All B-tree tests pass
    - _Files: `owl_storage_zig/src/btree.zig`, `owl_storage_zig/src/btree_test.zig`, `owl_storage_zig/benches/btree_bench.zig`_
    - _Requirements: Index support, B-tree implementation_

  - [x] 10.2 Index Maintenance
    - Automatic index maintenance on writes
    - Indexes updated on insert/update/delete
    - Transactional index updates
    - Index consistency guaranteed
    - Performance overhead <10%
    - All index maintenance tests pass
    - _Files: `owl_storage_engine/src/index_manager.rs`, `tests/index_maintenance_test.rs`_
    - _Requirements: Index maintenance, consistency_

  - [x] 10.3 Index Scan Operator
    - Implement IndexScan physical operator
    - IndexScan operator implemented
    - Range scan support
    - Integration with query planner
    - Performance better than SeqScan
    - All index scan tests pass
    - _Files: `src/execution/index_scan.rs`, `tests/index_scan_test.rs`, `benches/index_scan_bench.rs`_
    - _Requirements: Index-based query execution_

  - [x] 10.4 Garbage Collection
    - Implement garbage collection for deleted objects
    - Mark-and-sweep GC algorithm
    - Reclaim deleted objects
    - Compact arena (optional)
    - Background GC thread
    - All GC tests pass
    - _Files: `owl_storage_zig/src/gc.zig`, `owl_storage_zig/src/gc_test.zig`_
    - _Requirements: Memory reclamation, space efficiency_

  - [x] 10.5 Parallel Execution
    - Implement parallel query execution for large scans
    - Parallel SeqScan operator
    - Parallel aggregation
    - Thread pool management
    - Work stealing scheduler
    - Performance improvement on multi-core
    - All parallel execution tests pass
    - _Files: `src/execution/parallel_scan.rs`, `src/execution/parallel_aggregate.rs`, `src/execution/thread_pool.rs`, `tests/parallel_execution_test.rs`, `benches/parallel_bench.rs`_
    - _Requirements: Parallel execution, multi-core performance_
    - _Status: Implementation complete. Tests blocked by storage layer ObjectHeader issue (see 10.6)_

  - [x] 10.6 Fix Storage Layer ObjectHeader Integration
    - **Critical Bug**: ZigStore not writing proper ObjectHeader format
    - **Root Cause**: Arena expects 48-byte ObjectHeader at start of each object with:
      - type_tag (u16): Schema type ID
      - id ([16]u8): UUID bytes
      - version (u32): MVCC version
      - size (u32): Total object size
      - flags (u8): Deleted/Overflow/Pinned flags
      - checksum (u32): CRC32 for integrity
      - lsn (u64): Log sequence number
    - **Current Issue**: Raw serialized data written without header, causing Arena to read garbage as header and interpret FLAG_DELETED bit incorrectly
    - **Impact**: All document operations fail with "ObjectDeleted" error
    - **Fix Approach**:
      1. Create ObjectHeader when storing documents in `store_document_internal()`
      2. Serialize header + document data together (header_bytes + serialized_doc)
      3. Allocate space for header + data (48 + data_len bytes)
      4. Write combined buffer to Arena
      5. When reading in `read_document_internal()`:
         - Read first 48 bytes as ObjectHeader
         - Validate header (check deleted flag, verify checksum)
         - Read remaining bytes as document data
         - Deserialize document from data portion
      6. Update `navigate_reference()` to also read and validate header
      7. Ensure `delete_document()` sets FLAG_DELETED in header
    - **Testing**: Once fixed, all parallel execution tests should pass
    - _Files: `src/storage.rs` (ZigStore implementation), `tests/simple_storage_test.rs`, `tests/parallel_execution_test.rs`_
    - _Requirements: Proper Arena integration, data integrity_
    - _Priority: HIGH - Blocks parallel execution testing and other storage operations_

  - [x] 10.7 Schema Evolution Tools
    - Tools for schema evolution and migration
    - Schema versioning system
    - Migration script support
    - Backward compatibility checks
    - Field addition/removal support
    - Type change validation
    - All migration tests pass
    - _Files: `src/schema/evolution.rs`, `src/schema/migration.rs`, `tools/schema_migrate.rs`, `tests/schema_evolution_test.rs`_
    - _Requirements: Schema evolution, migration support_

## Phase 11: Final Integration & Testing (Weeks 31-32)

- [ ] 11. Final Integration & Testing
  - [ ] 11.1 End-to-End Integration
    - Complete end-to-end integration testing
    - All OQL queries work with Zig storage
    - Performance targets met (10x faster navigation)
    - Memory usage reduced (50% less)
    - All existing tests pass
    - No regressions
    - _Files: `tests/e2e_integration_test.rs`, `benches/e2e_bench.rs`_
    - _Requirements: Complete integration, performance validation_

  - [ ] 11.2 Migration Tools
    - Tools for migrating from RocksDB to Zig storage
    - Data migration tool
    - Schema migration tool
    - Validation tool
    - Rollback capability
    - Migration guide complete
    - _Files: `tools/migrate_rocksdb_to_zig.rs`, `docs/MIGRATION_GUIDE.md`_
    - _Requirements: Migration support, data portability_

  - [ ] 11.3 Performance Validation
    - Validate all performance targets are met
    - Navigation: <50ns per hop ✓
    - Allocation: <20ns per object ✓
    - Query latency: 10x faster than RocksDB
    - Throughput: 5x higher than RocksDB
    - Memory: 50% less than RocksDB
    - All benchmarks documented
    - _Files: `benches/final_validation_bench.rs`, `docs/PERFORMANCE_RESULTS.md`_
    - _Requirements: Performance validation, benchmarking_

---

## Summary

**Total Tasks**: 46 (27 original + 19 new)
**Total Estimated Time**: 32 weeks (14 original + 18 new)
**Critical Path**: Phase 1 → Phase 2 → Phase 3 → Phase 4 → Phase 8 → Phase 9 → Phase 10 → Phase 11

**Key Milestones**:
- Week 3: Core types complete ✅
- Week 5: Rust integration working ✅
- Week 7: WAL and durability complete ✅
- Week 9: Transactions complete ✅
- Week 11: Performance optimized ✅
- Week 13: Testing complete (in progress)
- Week 14: Documentation complete (pending)
- Week 20: Query engine integrated (in progress)
- Week 25: Query planning complete (pending)
- Week 30: Indexes, GC, and parallel execution complete (pending)
- Week 32: Production ready (pending)

**Success Criteria**:
- All tests pass (unit, integration, property-based, e2e)
- Performance >10x better than RocksDB for graph traversal
- Memory usage >50% lower than RocksDB
- No memory leaks or data races
- Complete documentation
- O(1) reference navigation working
- Schema-aware parsing
- Cost-based query optimization
- Statistics collection
- B-tree indexes functional
- Parallel execution
- Schema evolution tools
- Seamless migration from RocksDB
- RocksDB deprecated

**Phase Completion Status**:
- Phase 1: ✅ **COMPLETE** (5/5 tasks)
- Phase 2: ✅ **COMPLETE** (3/3 tasks)
- Phase 3: ✅ **COMPLETE** (3/3 tasks) - WAL & Durability
- Phase 4: ✅ **COMPLETE** (3/3 tasks) - Transactions & MVCC
- Phase 5: ✅ **COMPLETE** (4/4 tasks) - Performance & Optimization
- Phase 6: ⏳ **IN PROGRESS** (0/4 tasks) - Testing & Validation
- Phase 7: ⏳ **PENDING** (0/3 tasks) - Documentation & Polish
- Phase 8: ⏳ **IN PROGRESS** (1/6 tasks) - Query Engine Integration
- Phase 9: ⏳ **PENDING** (0/5 tasks) - Query Planning & Optimization
- Phase 10: ⏳ **PENDING** (0/6 tasks) - Indexes & Advanced Features
- Phase 11: ⏳ **PENDING** (0/3 tasks) - Final Integration & Testing

**Next Priority**: Continue Phase 8 (Query Engine Integration) or start Phase 6 (Testing & Validation)
- **Option A**: Complete Phase 6-7 first (testing, documentation)
  - Pros: Solid foundation, production-ready storage
  - Cons: Delays realizing OQL's performance advantage
  
- **Option B**: Continue Phase 8 now (get O(1) navigation working ASAP)
  - Pros: Immediate performance gains, validates core OQL value proposition
  - Cons: Testing and documentation deferred
  
- **Recommendation**: **Option B** - Query engine integration is the critical path to realizing OQL's performance advantage. Testing and documentation can be added incrementally.
