# Design Document

## Overview

The OQL Storage Engine is a persistent object graph database that treats navigation as a CPU instruction rather than a query operation. The core innovation is that `order.customer.profile.first_name` executes as 1-2 memory loads, not a database query. This is achieved through physical references (64-bit offsets), co-located object storage, and swizzling (converting disk offsets to memory pointers).

The design follows a hybrid architecture: memory-mapped arenas for embedded deployments (<100 GB) and custom LSM segments for server deployments (>100 GB). All operations maintain ACID guarantees through Write-Ahead Logging (WAL) and Multi-Version Concurrency Control (MVCC).

**Core Design Principles:**
- O(1) navigation through physical pointers
- Orthogonal persistence (objects don't know they're persisted)
- Co-location of related objects for cache efficiency
- No JOINs - navigation is not a query
- Rust safety without sacrificing performance

## Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        OQL API (Rust)                        │
│  order.customer.name  →  O(1) deref                         │
└───────────────────────▲───────────────────────▲─────────────┘
                        │                       │
        ┌───────────────┴───────────┐   ┌───────┴─────────────┐
        │   Query Engine             │   │   Transaction Mgr   │
        │  (OQL → Path Plan)         │   │  (WAL, MVCC, GC)    │
        └───────────────▲───────────┘   └───────▲─────────────┘
                        │                       │
        ┌───────────────┴───────────┐   ┌───────┴─────────────┐
        │   Object Cache             │   │   WAL & Recovery    │
        │  (Per-Txn + Global)        │   │  (Append-Only)      │
        └───────────────▲───────────┘   └───────▲─────────────┘
                        │                       │
        ┌───────────────┴───────────────────────┴─────────────┐
        │             Storage Engine                           │
        │  ┌─────────────┐  ┌─────────────────────┐           │
        │  │   Arenas    │  │   LSM Segments      │           │
        │  │ (mmap, 4KB) │  │ (SST, 256MB)        │           │
        │  └─────▲───────┘  └─────────▲───────────┘           │
        │        │                    │                        │
        └────────┴────────────────────┴────────────────────────┘
                 │                    │
        ┌────────▼────────┐  ┌────────▼────────┐
        │  Disk (mmap)    │  │  Disk (SST)     │
        └─────────────────┘  └─────────────────┘
```

### Component Interaction Flow

**Write Path:**
```
1. Application creates/modifies object
2. Transaction Manager allocates ObjRef
3. Arena allocates space (co-located with references)
4. Object written to arena
5. WAL records operation
6. Index updated with ObjRef
7. Transaction commits (fsync WAL)
```

**Read Path (O(1) Navigation):**
```
1. Application accesses order.customer.name
2. Swizzle order ObjRef → memory pointer
3. Load customer ObjRef from order
4. Swizzle customer ObjRef → memory pointer
5. Load name field from customer
Total: 2-3 cache line loads
```

## Components and Interfaces

### 1. Object and ObjRef (Core Data Structures)

**Purpose**: Define the fundamental object layout and physical reference mechanism.

**Object Layout:**
```rust
#[repr(C)]
pub struct ObjectHeader {
    type_tag: u16,        // Schema type ID (0=User, 1=Order, etc.)
    id: Uuid,             // 16 bytes - logical identifier
    version: u32,         // MVCC version number
    size: u32,            // Total object size in bytes
    flags: u8,            // Deleted, Overflow, Pinned, etc.
    checksum: u32,        // CRC32 for integrity
    lsn: u64,             // Log Sequence Number
}

pub struct Object {
    header: ObjectHeader,
    fixed: [u8],          // Fixed-size fields (aligned)
    var: VarData,         // CBOR-encoded nested/variable data
    refs: [ObjRef],       // Array of references to other objects
}
```

**ObjRef (Physical Pointer):**
```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct ObjRef {
    offset: u64,  // File-global offset (8 bytes)
}

impl ObjRef {
    pub fn null() -> Self {
        ObjRef { offset: 0 }
    }
    
    pub fn is_null(&self) -> bool {
        self.offset == 0
    }
    
    // Swizzle: Convert disk offset to memory pointer
    pub fn swizzle(&self, base: *mut u8) -> *mut Object {
        unsafe { base.add(self.offset as usize) as *mut Object }
    }
    
    pub fn page_id(&self) -> u32 {
        (self.offset / 4096) as u32
    }
    
    pub fn slot_id(&self) -> u16 {
        ((self.offset % 4096) / 64) as u16
    }
}
```

**Key Interfaces:**
```rust
pub trait ObjectStore {
    fn alloc(&mut self, size: u32, near: Option<ObjRef>) -> ObjRef;
    fn write(&mut self, objref: ObjRef, data: &[u8]) -> Result<()>;
    fn read(&self, objref: ObjRef) -> Result<&Object>;
    fn delete(&mut self, objref: ObjRef) -> Result<()>;
}
```

### 2. Arena (Memory-Mapped 4KB Pages)

**Purpose**: Provide fast object allocation with co-location optimization for embedded deployments.

**Arena Structure:**
```rust
pub struct Arena {
    file: MmapMut,                    // Memory-mapped file
    free_list: Vec<u32>,              // Available slot IDs
    bump: AtomicU32,                  // Bump allocator pointer
    size: usize,                      // Total arena size
    used: AtomicUsize,                // Bytes used
}

impl Arena {
    pub fn new(path: &Path, size: usize) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        file.set_len(size as u64)?;
        
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        
        // Enable huge pages for performance
        #[cfg(target_os = "linux")]
        unsafe {
            libc::madvise(
                mmap.as_ptr() as *mut _,
                mmap.len(),
                libc::MADV_HUGEPAGE
            );
        }
        
        Ok(Arena {
            file: mmap,
            free_list: Vec::new(),
            bump: AtomicU32::new(0),
            size,
            used: AtomicUsize::new(0),
        })
    }
    
    // Allocate object near another object for co-location
    pub fn alloc_near(&mut self, size: u32, near: Option<ObjRef>) -> Option<ObjRef> {
        if let Some(near_ref) = near {
            let page_id = near_ref.page_id();
            // Try to allocate in same 4KB page
            if let Some(offset) = self.alloc_in_page(page_id, size) {
                return Some(ObjRef { offset });
            }
        }
        
        // Fall back to bump allocation
        self.alloc(size)
    }
    
    pub fn alloc(&mut self, size: u32) -> Option<ObjRef> {
        // Try free list first
        if let Some(slot) = self.free_list.pop() {
            return Some(ObjRef { offset: slot as u64 * 64 });
        }
        
        // Bump allocate
        let offset = self.bump.fetch_add(size, Ordering::SeqCst);
        if offset + size <= self.size as u32 {
            self.used.fetch_add(size as usize, Ordering::SeqCst);
            Some(ObjRef { offset: offset as u64 })
        } else {
            None
        }
    }
}
```

**Key Interfaces:**
```rust
pub trait ArenaAllocator {
    fn alloc(&mut self, size: u32) -> Option<ObjRef>;
    fn alloc_near(&mut self, size: u32, near: Option<ObjRef>) -> Option<ObjRef>;
    fn free(&mut self, objref: ObjRef);
    fn defragment(&mut self) -> Result<()>;
}
```

### 3. LSM Segment (256MB SST Files)

**Purpose**: Provide scalable storage for server deployments with sorted object storage.

**LSM Segment Structure:**
```rust
pub struct Segment {
    file: File,
    index: BTreeMap<Uuid, u64>,      // ID → offset
    bloom: BloomFilter,               // Fast existence checks
    data: Vec<u8>,                    // Serialized objects
    metadata: SegmentMetadata,
}

pub struct SegmentMetadata {
    min_key: Uuid,
    max_key: Uuid,
    object_count: u64,
    size: u64,
    created_at: u64,
    level: u8,                        // LSM level (0-6)
}

impl Segment {
    pub fn write(objects: Vec<Object>, path: &Path) -> Result<Self> {
        let mut index = BTreeMap::new();
        let mut data = Vec::new();
        let mut bloom = BloomFilter::new(objects.len(), 0.01);
        
        // Sort objects by ID for efficient range scans
        let mut sorted = objects;
        sorted.sort_by_key(|obj| obj.header.id);
        
        for obj in sorted {
            let offset = data.len() as u64;
            index.insert(obj.header.id, offset);
            bloom.insert(&obj.header.id);
            
            // Serialize object
            bincode::serialize_into(&mut data, &obj)?;
        }
        
        // Write to disk
        let mut file = File::create(path)?;
        file.write_all(&data)?;
        file.sync_all()?;
        
        Ok(Segment { file, index, bloom, data, metadata })
    }
    
    pub fn get(&self, id: &Uuid) -> Result<Option<Object>> {
        // Fast bloom filter check
        if !self.bloom.contains(id) {
            return Ok(None);
        }
        
        // Index lookup
        if let Some(&offset) = self.index.get(id) {
            let obj = bincode::deserialize_from(&self.data[offset as usize..])?;
            Ok(Some(obj))
        } else {
            Ok(None)
        }
    }
}
```

**Key Interfaces:**
```rust
pub trait LSMStorage {
    fn write_segment(&mut self, objects: Vec<Object>) -> Result<SegmentId>;
    fn read_object(&self, id: &Uuid) -> Result<Option<Object>>;
    fn compact(&mut self, level: u8) -> Result<()>;
    fn scan_range(&self, start: &Uuid, end: &Uuid) -> Result<Vec<Object>>;
}
```

### 4. Swizzling and Object Cache

**Purpose**: Convert ObjRefs to memory pointers and cache frequently accessed objects.

**Swizzling Implementation:**
```rust
pub struct ObjectCache {
    per_txn: HashMap<ObjRef, *mut Object>,  // Transaction-local cache
    global: LruCache<ObjRef, Arc<Object>>,  // Global LRU cache
    arena_base: *mut u8,                     // Base pointer for swizzling
}

impl ObjectCache {
    pub fn swizzle(&mut self, objref: ObjRef) -> Result<&Object> {
        // Check transaction cache first
        if let Some(&ptr) = self.per_txn.get(&objref) {
            return Ok(unsafe { &*ptr });
        }
        
        // Check global cache
        if let Some(obj) = self.global.get(&objref) {
            return Ok(obj.as_ref());
        }
        
        // Load from disk
        let page_id = objref.page_id();
        let block = self.load_page(page_id)?;
        
        // Swizzle to memory pointer
        let ptr = objref.swizzle(self.arena_base);
        let obj = unsafe { &*ptr };
        
        // Cache for future access
        self.per_txn.insert(objref, ptr);
        self.global.put(objref, Arc::new(obj.clone()));
        
        Ok(obj)
    }
    
    pub fn batch_swizzle(&mut self, objrefs: &[ObjRef]) -> Result<Vec<&Object>> {
        // Group by page for efficient loading
        let mut by_page: HashMap<u32, Vec<ObjRef>> = HashMap::new();
        for &objref in objrefs {
            by_page.entry(objref.page_id()).or_default().push(objref);
        }
        
        // Prefetch pages
        for &page_id in by_page.keys() {
            self.prefetch_page(page_id)?;
        }
        
        // Swizzle all refs
        objrefs.iter().map(|&r| self.swizzle(r)).collect()
    }
}
```

**Key Interfaces:**
```rust
pub trait Swizzler {
    fn swizzle(&mut self, objref: ObjRef) -> Result<&Object>;
    fn batch_swizzle(&mut self, objrefs: &[ObjRef]) -> Result<Vec<&Object>>;
    fn prefetch(&mut self, objref: ObjRef) -> Result<()>;
}
```

### 5. Write-Ahead Log (WAL)

**Purpose**: Ensure durability and enable crash recovery.

**WAL Structure:**
```rust
pub struct WAL {
    file: File,
    buffer: Vec<WALEntry>,
    lsn: AtomicU64,                   // Log Sequence Number
    last_sync: AtomicU64,
}

pub enum WALEntry {
    Insert { lsn: u64, objref: ObjRef, data: Vec<u8> },
    Update { lsn: u64, objref: ObjRef, data: Vec<u8> },
    Delete { lsn: u64, objref: ObjRef },
    Commit { lsn: u64, txn_id: u64 },
    Abort { lsn: u64, txn_id: u64 },
    Checkpoint { lsn: u64 },
}

impl WAL {
    pub fn append(&mut self, entry: WALEntry) -> Result<u64> {
        let lsn = self.lsn.fetch_add(1, Ordering::SeqCst);
        self.buffer.push(entry);
        
        // Group commit: flush every 1ms or 1000 entries
        if self.buffer.len() >= 1000 || self.should_sync() {
            self.sync()?;
        }
        
        Ok(lsn)
    }
    
    pub fn sync(&mut self) -> Result<()> {
        for entry in &self.buffer {
            bincode::serialize_into(&mut self.file, entry)?;
        }
        self.file.sync_all()?;
        self.buffer.clear();
        self.last_sync.store(self.lsn.load(Ordering::SeqCst), Ordering::SeqCst);
        Ok(())
    }
    
    pub fn recover(&mut self) -> Result<Vec<WALEntry>> {
        let mut entries = Vec::new();
        let mut reader = BufReader::new(&self.file);
        
        while let Ok(entry) = bincode::deserialize_from(&mut reader) {
            entries.push(entry);
        }
        
        Ok(entries)
    }
}
```

**Key Interfaces:**
```rust
pub trait WriteAheadLog {
    fn append(&mut self, entry: WALEntry) -> Result<u64>;
    fn sync(&mut self) -> Result<()>;
    fn recover(&mut self) -> Result<Vec<WALEntry>>;
    fn checkpoint(&mut self) -> Result<()>;
}
```

### 6. Transaction Manager (MVCC)

**Purpose**: Provide ACID transactions with snapshot isolation.

**Transaction Structure:**
```rust
pub struct Transaction {
    id: u64,
    snapshot_lsn: u64,                // Snapshot for reads
    write_set: HashMap<ObjRef, Object>,
    read_set: HashSet<ObjRef>,
    state: TxnState,
}

pub enum TxnState {
    Active,
    Preparing,
    Committed,
    Aborted,
}

pub struct TransactionManager {
    active_txns: HashMap<u64, Transaction>,
    next_txn_id: AtomicU64,
    wal: WAL,
    lock_table: LockTable,
}

impl TransactionManager {
    pub fn begin(&mut self) -> Transaction {
        let txn_id = self.next_txn_id.fetch_add(1, Ordering::SeqCst);
        let snapshot_lsn = self.wal.current_lsn();
        
        Transaction {
            id: txn_id,
            snapshot_lsn,
            write_set: HashMap::new(),
            read_set: HashSet::new(),
            state: TxnState::Active,
        }
    }
    
    pub fn commit(&mut self, txn: &mut Transaction) -> Result<()> {
        // Validate no conflicts
        for objref in &txn.read_set {
            if self.has_newer_version(*objref, txn.snapshot_lsn)? {
                return Err(Error::SerializationFailure);
            }
        }
        
        // Write to WAL
        for (objref, obj) in &txn.write_set {
            self.wal.append(WALEntry::Update {
                lsn: self.wal.current_lsn(),
                objref: *objref,
                data: bincode::serialize(obj)?,
            })?;
        }
        
        self.wal.append(WALEntry::Commit {
            lsn: self.wal.current_lsn(),
            txn_id: txn.id,
        })?;
        
        self.wal.sync()?;
        txn.state = TxnState::Committed;
        Ok(())
    }
}
```

**Key Interfaces:**
```rust
pub trait TransactionManager {
    fn begin(&mut self) -> Transaction;
    fn commit(&mut self, txn: &mut Transaction) -> Result<()>;
    fn abort(&mut self, txn: &mut Transaction) -> Result<()>;
    fn read(&self, txn: &Transaction, objref: ObjRef) -> Result<&Object>;
    fn write(&mut self, txn: &mut Transaction, objref: ObjRef, obj: Object) -> Result<()>;
}
```

### 7. Compaction and Garbage Collection

**Purpose**: Reclaim space from deleted objects and maintain co-location.

**Backlink Index:**
```rust
pub struct BacklinkIndex {
    map: HashMap<ObjRef, Vec<ObjRef>>,  // target → [referrers]
}

impl BacklinkIndex {
    pub fn add_reference(&mut self, from: ObjRef, to: ObjRef) {
        self.map.entry(to).or_default().push(from);
    }
    
    pub fn get_referrers(&self, target: ObjRef) -> &[ObjRef] {
        self.map.get(&target).map(|v| v.as_slice()).unwrap_or(&[])
    }
    
    pub fn update_reference(&mut self, old: ObjRef, new: ObjRef) {
        if let Some(referrers) = self.map.remove(&old) {
            for referrer in referrers {
                // Update referrer to point to new location
                self.rewrite_objref(referrer, old, new);
            }
            self.map.insert(new, Vec::new());
        }
    }
}
```

**Compaction Process:**
```rust
pub struct Compactor {
    backlinks: BacklinkIndex,
    arena: Arena,
}

impl Compactor {
    pub fn compact(&mut self) -> Result<()> {
        // 1. Mark phase: identify live objects
        let live_objects = self.mark_live_objects()?;
        
        // 2. Relocate phase: move objects to new arena
        let mut relocations = HashMap::new();
        for objref in live_objects {
            let obj = self.arena.read(objref)?;
            let new_ref = self.arena.alloc(obj.header.size)?;
            self.arena.write(new_ref, obj)?;
            relocations.insert(objref, new_ref);
        }
        
        // 3. Update phase: fix all ObjRefs using backlinks
        for (old_ref, new_ref) in relocations {
            self.backlinks.update_reference(old_ref, new_ref);
        }
        
        // 4. Reclaim phase: free old arena space
        self.arena.defragment()?;
        
        Ok(())
    }
}
```

**Key Interfaces:**
```rust
pub trait Compactor {
    fn compact(&mut self) -> Result<()>;
    fn mark_live_objects(&self) -> Result<Vec<ObjRef>>;
    fn relocate_objects(&mut self, objects: Vec<ObjRef>) -> Result<HashMap<ObjRef, ObjRef>>;
    fn update_references(&mut self, relocations: HashMap<ObjRef, ObjRef>) -> Result<()>;
}
```

### 8. Query Execution Engine

**Purpose**: Execute queries over the object graph with efficient navigation.

**Query Plan:**
```rust
pub enum QueryPlan {
    IndexScan {
        index: String,
        range: Range<Value>,
        projection: Vec<Path>,
    },
    FullScan {
        type_tag: u16,
        filter: Option<Expression>,
        projection: Vec<Path>,
    },
    NavigationScan {
        root: ObjRef,
        path: Vec<String>,
    },
}

pub enum Path {
    Field(String),
    Reference(String, Box<Path>),
    Array(String, Box<Path>),
}
```

**Query Executor:**
```rust
pub struct QueryExecutor {
    cache: ObjectCache,
    indexes: HashMap<String, Index>,
}

impl QueryExecutor {
    pub fn execute(&mut self, plan: QueryPlan) -> Result<Vec<Object>> {
        match plan {
            QueryPlan::IndexScan { index, range, projection } => {
                let objrefs = self.indexes.get(&index)
                    .ok_or(Error::IndexNotFound)?
                    .scan_range(&range)?;
                
                // Batch swizzle for efficiency
                let objects = self.cache.batch_swizzle(&objrefs)?;
                
                // Project fields
                self.project(objects, &projection)
            }
            
            QueryPlan::NavigationScan { root, path } => {
                let mut current = self.cache.swizzle(root)?;
                
                for field in path {
                    let objref = current.get_reference(&field)?;
                    current = self.cache.swizzle(objref)?;
                }
                
                Ok(vec![current.clone()])
            }
            
            _ => unimplemented!(),
        }
    }
}
```

**Key Interfaces:**
```rust
pub trait QueryExecutor {
    fn execute(&mut self, plan: QueryPlan) -> Result<Vec<Object>>;
    fn execute_stream(&mut self, plan: QueryPlan) -> Result<ObjectStream>;
    fn project(&self, objects: Vec<&Object>, projection: &[Path]) -> Result<Vec<Object>>;
}
```

## Data Models

### Object Storage Model

Objects are stored with a fixed header followed by variable-length data:

```
┌─────────────────────────────────────────────────────────┐
│ ObjectHeader (32 bytes)                                 │
├─────────────────────────────────────────────────────────┤
│ Fixed Fields (aligned, type-specific)                   │
├─────────────────────────────────────────────────────────┤
│ VarData (CBOR-encoded nested/variable fields)           │
├─────────────────────────────────────────────────────────┤
│ References (array of ObjRef, 8 bytes each)              │
└─────────────────────────────────────────────────────────┘
```

### Arena Layout (4KB Pages)

```
┌─────────────────────────────────────────────────────────┐
│ Page 0: Metadata                                        │
├─────────────────────────────────────────────────────────┤
│ Page 1-N: Object Storage (co-located subgraphs)        │
│   ┌─────────────────────────────────────────────────┐   │
│   │ Object 1 (Order)                                │   │
│   ├─────────────────────────────────────────────────┤   │
│   │ Object 2 (Customer) ← referenced by Object 1    │   │
│   ├─────────────────────────────────────────────────┤   │
│   │ Object 3 (Profile) ← referenced by Object 2     │   │
│   └─────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

### LSM Segment Layout (256MB SST)

```
┌─────────────────────────────────────────────────────────┐
│ Segment Header (metadata, bloom filter)                 │
├─────────────────────────────────────────────────────────┤
│ Index Block (BTreeMap: Uuid → offset)                   │
├─────────────────────────────────────────────────────────┤
│ Data Blocks (sorted objects)                            │
│   Object 1 (min_key)                                    │
│   Object 2                                              │
│   ...                                                   │
│   Object N (max_key)                                    │
└─────────────────────────────────────────────────────────┘
```

## Error Handling

### Error Categories

```rust
pub enum StorageError {
    // I/O errors
    IOError(std::io::Error),
    
    // Allocation errors
    OutOfSpace,
    AllocationFailed,
    
    // Reference errors
    InvalidObjRef(ObjRef),
    DanglingReference(ObjRef),
    CircularReference(Vec<ObjRef>),
    
    // Transaction errors
    SerializationFailure,
    DeadlockDetected,
    TransactionAborted,
    
    // Corruption errors
    ChecksumMismatch { expected: u32, actual: u32 },
    CorruptedWAL,
    CorruptedObject(ObjRef),
    
    // Recovery errors
    RecoveryFailed(String),
}
```

### Error Recovery Strategy

1. **Transient Errors**: Retry with exponential backoff
2. **Corruption Errors**: Isolate corrupted objects, continue with valid data
3. **Transaction Conflicts**: Abort and retry with new snapshot
4. **Out of Space**: Trigger compaction, then retry
5. **Deadlocks**: Abort youngest transaction

## Testing Strategy

### Unit Testing
- **Object Layout**: Verify header alignment, field access
- **ObjRef Operations**: Test swizzling, page/slot calculations
- **Arena Allocation**: Test bump allocation, free list, co-location
- **WAL Operations**: Test append, sync, recovery
- **MVCC**: Test snapshot isolation, conflict detection

### Integration Testing
- **End-to-End Navigation**: Test O(1) reference resolution
- **Transaction ACID**: Verify atomicity, consistency, isolation, durability
- **Crash Recovery**: Kill process mid-transaction, verify recovery
- **Compaction**: Test object relocation, reference updates

### Performance Testing
- **Navigation Latency**: Measure time for multi-level reference access
- **Write Throughput**: Measure transactions per second
- **Read Throughput**: Measure queries per second
- **Memory Efficiency**: Verify 1.2x overhead target
- **Storage Efficiency**: Verify 1.5x write amplification target

### Property Testing
- **Reference Integrity**: All ObjRefs point to valid objects
- **No Memory Leaks**: All allocated objects eventually freed
- **Crash Consistency**: Any crash leaves database in valid state
- **Serializability**: Concurrent transactions produce serializable results

## Performance Considerations

### Target Metrics

| Metric | Target | Strategy |
|--------|--------|----------|
| Navigation | < 50 ns | Co-location + swizzling |
| Insert TPS | 1M+ | Batch WAL + arenas |
| Query QPS | 500k+ | Index + batch swizzle |
| Memory | 1.2x object size | No duplication |
| Storage | 1.5x write amp | LSM + compaction |

### Optimization Techniques

1. **Co-Location**: Store related objects in same 4KB page
2. **Batch Swizzling**: Load multiple objects in single operation
3. **Prefetching**: Use madvise(MADV_WILLNEED) for sequential access
4. **Huge Pages**: Enable transparent huge pages for arena
5. **Group Commit**: Batch WAL writes every 1ms or 1000 entries
6. **Bloom Filters**: Fast negative lookups in LSM segments
7. **LRU Caching**: Cache hot objects in memory

## Security Considerations

### Memory Safety
- Use safe Rust for all public APIs
- Encapsulate unsafe swizzling in safe wrappers
- Validate all ObjRefs before dereferencing
- Use Rust's type system to prevent data races

### Data Integrity
- CRC32 checksums on all objects
- WAL checksums for corruption detection
- Atomic updates via WAL
- Backlink validation during compaction

### Crash Safety
- All writes go through WAL first
- fsync before commit returns
- Periodic checkpoints bound recovery time
- Validate checksums during recovery

This design provides a complete blueprint for implementing the OQL Storage Engine with O(1) navigation, orthogonal persistence, and production-ready ACID guarantees.
