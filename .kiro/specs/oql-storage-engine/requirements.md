# Requirements Document

## Introduction

The OQL Storage Engine is a persistent object graph database storage layer designed to provide O(1) navigation through object references. Unlike traditional databases that treat queries as operations, OQL treats navigation (e.g., `order.customer.profile.first_name`) as CPU instructions - direct memory dereferences. The system implements a disk-backed heap where objects live, references are physical pointers, and persistence is orthogonal to the application. This storage engine serves as the foundational layer for the OQL database system, replacing traditional key-value stores with a purpose-built object graph storage mechanism.

## Glossary

- **Storage_Engine**: The core system responsible for persisting and retrieving objects from disk with O(1) navigation guarantees
- **ObjRef**: A physical pointer represented as a 64-bit offset into the storage file, enabling direct object access
- **Arena**: A memory-mapped 4KB page used for object allocation with co-location optimization
- **Swizzling**: The process of converting an ObjRef (disk offset) into a memory pointer for direct access
- **Co-Location**: The strategy of storing related objects (e.g., order and customer) in the same arena block for cache efficiency
- **LSM_Segment**: A Log-Structured Merge tree segment (256MB SST file) used for larger deployments
- **WAL**: Write-Ahead Log, an append-only file ensuring durability and crash recovery
- **MVCC**: Multi-Version Concurrency Control, enabling snapshot isolation for transactions
- **Backlink**: A reverse reference index tracking which objects reference a given object, used for compaction

## Requirements

### Requirement 1

**User Story:** As a developer, I want to navigate object references with O(1) performance, so that accessing related data (e.g., order.customer.name) is as fast as a memory dereference.

#### Acceptance Criteria

1. WHEN a developer accesses a reference field (e.g., order.customer), THE Storage_Engine SHALL resolve the reference in O(1) time complexity
2. WHEN navigating through multiple reference levels (e.g., order.customer.profile.first_name), THE Storage_Engine SHALL complete the operation within 50 nanoseconds per reference hop
3. WHEN an ObjRef is swizzled to a memory pointer, THE Storage_Engine SHALL perform at most one cache line load per object access
4. WHEN related objects are stored, THE Storage_Engine SHALL co-locate them in the same 4KB arena block to minimize cache misses
5. IF a reference points to a non-existent object, THEN THE Storage_Engine SHALL return a null value without throwing exceptions

### Requirement 2

**User Story:** As a developer, I want objects to persist automatically without explicit serialization code, so that I can work with objects naturally without database-specific logic.

#### Acceptance Criteria

1. WHEN an object is created in memory, THE Storage_Engine SHALL persist it to disk without requiring explicit save operations
2. WHEN an object is modified, THE Storage_Engine SHALL track changes and persist them through the WAL
3. WHEN the system restarts, THE Storage_Engine SHALL restore all objects to their last committed state
4. WHEN objects reference each other, THE Storage_Engine SHALL maintain reference integrity across persistence boundaries
5. IF the system crashes during a write operation, THEN THE Storage_Engine SHALL recover to the last consistent state using the WAL

### Requirement 3

**User Story:** As a developer, I want references to be physical pointers rather than string IDs, so that navigation is a direct memory operation without lookup overhead.

#### Acceptance Criteria

1. WHEN storing an object reference, THE Storage_Engine SHALL use a 64-bit offset (ObjRef) instead of string identifiers
2. WHEN resolving a reference, THE Storage_Engine SHALL convert the ObjRef to a memory pointer through swizzling
3. WHEN an object is relocated during compaction, THE Storage_Engine SHALL update all ObjRefs pointing to it using the Backlink index
4. WHEN multiple objects reference the same target, THE Storage_Engine SHALL maintain a single physical copy with multiple ObjRefs
5. IF an ObjRef becomes invalid due to object deletion, THEN THE Storage_Engine SHALL mark it as null in the Backlink index

### Requirement 4

**User Story:** As a developer, I want the storage engine to optimize for spatial locality, so that accessing related objects benefits from CPU cache efficiency.

#### Acceptance Criteria

1. WHEN allocating a new object, THE Storage_Engine SHALL attempt to place it in the same Arena as its referenced objects
2. WHEN an order references a customer, THE Storage_Engine SHALL co-locate them within the same 4KB block when possible
3. WHEN scanning a subgraph of related objects, THE Storage_Engine SHALL prefetch adjacent arena blocks using madvise
4. WHEN arena fragmentation exceeds a threshold, THE Storage_Engine SHALL trigger compaction to restore co-location
5. IF co-location is not possible due to space constraints, THEN THE Storage_Engine SHALL allocate in the nearest available arena

### Requirement 5

**User Story:** As a developer, I want ACID transaction guarantees, so that concurrent operations maintain data consistency and durability.

#### Acceptance Criteria

1. WHEN a transaction begins, THE Storage_Engine SHALL create an isolated snapshot using MVCC
2. WHEN multiple transactions modify the same object, THE Storage_Engine SHALL serialize writes to prevent conflicts
3. WHEN a transaction commits, THE Storage_Engine SHALL ensure all changes are written to the WAL before returning
4. WHEN a transaction is aborted, THE Storage_Engine SHALL discard all changes without affecting committed data
5. IF the system crashes before a transaction commits, THEN THE Storage_Engine SHALL discard all uncommitted changes during recovery

### Requirement 6

**User Story:** As a developer, I want the storage engine to handle both small embedded deployments and large server deployments, so that I can use the same system across different scales.

#### Acceptance Criteria

1. WHEN the database size is less than 100 GB, THE Storage_Engine SHALL use memory-mapped arenas for optimal performance
2. WHEN the database size exceeds 100 GB, THE Storage_Engine SHALL transition to LSM_Segment-based storage
3. WHEN operating in embedded mode, THE Storage_Engine SHALL minimize memory footprint to 1.2x the working set size
4. WHEN operating in server mode, THE Storage_Engine SHALL support throughput exceeding 1 million transactions per second
5. IF memory pressure is detected, THEN THE Storage_Engine SHALL evict cold objects from the cache while maintaining hot object access

### Requirement 7

**User Story:** As a developer, I want efficient compaction and garbage collection, so that deleted objects are reclaimed and fragmentation is minimized.

#### Acceptance Criteria

1. WHEN objects are deleted, THE Storage_Engine SHALL mark them for garbage collection without immediate reclamation
2. WHEN compaction runs, THE Storage_Engine SHALL rewrite live objects to new arenas and update all ObjRefs via Backlinks
3. WHEN an object is relocated, THE Storage_Engine SHALL ensure atomic updates to all referencing ObjRefs
4. WHEN compaction completes, THE Storage_Engine SHALL reclaim space from old arenas and update free lists
5. IF compaction is interrupted, THEN THE Storage_Engine SHALL resume from the last checkpoint without data loss

### Requirement 8

**User Story:** As a developer, I want crash recovery to be fast and reliable, so that the system can resume operations quickly after failures.

#### Acceptance Criteria

1. WHEN the system crashes, THE Storage_Engine SHALL replay the WAL from the last checkpoint to restore state
2. WHEN recovery completes, THE Storage_Engine SHALL verify data integrity using checksums on all objects
3. WHEN the WAL grows large, THE Storage_Engine SHALL create periodic checkpoints to bound recovery time
4. WHEN recovery detects corruption, THE Storage_Engine SHALL report the affected objects and continue with valid data
5. IF the WAL is corrupted, THEN THE Storage_Engine SHALL recover to the last valid checkpoint

### Requirement 9

**User Story:** As a developer, I want the storage engine to provide query execution capabilities, so that I can filter and project object graphs efficiently.

#### Acceptance Criteria

1. WHEN executing a query with a WHERE clause, THE Storage_Engine SHALL scan objects and apply filters efficiently
2. WHEN projecting fields from related objects, THE Storage_Engine SHALL batch-swizzle references to minimize overhead
3. WHEN a query accesses indexed fields, THE Storage_Engine SHALL use indexes to avoid full scans
4. WHEN query results are large, THE Storage_Engine SHALL stream results to avoid memory exhaustion
5. IF a query accesses deeply nested references, THEN THE Storage_Engine SHALL prefetch the entire subgraph in a single batch

### Requirement 10

**User Story:** As a developer, I want the storage engine to be implemented safely in Rust, so that memory safety is guaranteed without sacrificing performance.

#### Acceptance Criteria

1. WHEN implementing the storage engine, THE Storage_Engine SHALL use safe Rust for all public APIs
2. WHEN swizzling ObjRefs to pointers, THE Storage_Engine SHALL encapsulate unsafe operations in safe wrapper functions
3. WHEN managing memory-mapped regions, THE Storage_Engine SHALL ensure proper lifetime management to prevent use-after-free
4. WHEN concurrent access occurs, THE Storage_Engine SHALL use Rust's type system to prevent data races
5. IF unsafe code is required for performance, THEN THE Storage_Engine SHALL document invariants and provide safe abstractions
