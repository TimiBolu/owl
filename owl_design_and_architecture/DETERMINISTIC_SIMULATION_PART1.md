# Deterministic Simulation Testing for Owl Storage Engine

## Part 1: Foundations & Current State Analysis

### What is Deterministic Simulation Testing?

**Deterministic Simulation Testing (DST)** is a technique pioneered by FoundationDB where:
1. **All non-determinism is controlled** (time, randomness, I/O, network, threads)
2. **Tests are reproducible** from a single seed value
3. **Failures can be replayed** exactly
4. **Massive state space exploration** (millions of scenarios in minutes)

**Key Insight**: If you control all sources of non-determinism, you can test distributed systems as thoroughly as single-threaded code.

---

## Current Owl Architecture: Sources of Non-Determinism

Let me analyze the current codebase for non-deterministic sources:

### 1. **Time-Based Non-Determinism** ⚠️

**Found in**:
```rust
// src/concurrency.rs
pub struct LockInfo {
    pub acquired_at: Instant,  // ❌ Real time!
}

pub struct WaitingTransaction {
    pub requested_at: Instant,  // ❌ Real time!
}

// src/document.rs
pub struct Document {
    pub created_at: chrono::DateTime<chrono::Utc>,  // ❌ Real time!
    pub updated_at: chrono::DateTime<chrono::Utc>,  // ❌ Real time!
}
```

**Problem**: Tests depend on wall-clock time, making them non-reproducible.

**Solution**: Inject a `SimulatedClock` that advances deterministically.

---

### 2. **Thread Scheduling Non-Determinism** ⚠️

**Found in**:
```rust
// src/concurrency.rs
thread::sleep(self.deadlock_check_interval);  // ❌ Real threads!

// src/execution/thread_pool.rs
// Uses real OS threads with non-deterministic scheduling
```

**Problem**: Thread interleaving is non-deterministic, making race conditions hard to reproduce.

**Solution**: Inject a `SimulatedScheduler` that controls thread execution order.

---

### 3. **Randomness Non-Determinism** ⚠️

**Found in**:
```rust
// src/transaction.rs
pub fn new(isolation_level: IsolationLevel) -> Self {
    Self {
        id: Uuid::new_v4().to_string(),  // ❌ Random UUID!
    }
}
```

**Problem**: Random IDs make tests non-reproducible.

**Solution**: Inject a `SeededRng` that produces deterministic "random" values.

---

### 4. **I/O Non-Determinism** ⚠️

**Found in**:
```rust
// src/storage.rs (Zig adapter)
let arena = Arena::create(path, size)  // ❌ Real file I/O!

// owl_storage_zig/src/arena.zig
const file = try std.fs.cwd().createFile(path, .{...});  // ❌ Real disk!
const mmap = try std.posix.mmap(...);  // ❌ Real mmap!
```

**Problem**: Disk I/O is slow and non-deterministic (failures, latency).

**Solution**: Inject a `SimulatedFileSystem` that operates in-memory.

---

### 5. **Network Non-Determinism** (Future)

**Not yet implemented**, but Next-Gen Owl will have:
- Network communication between nodes
- Message delays and reordering
- Network partitions

**Solution**: Inject a `SimulatedNetwork` that controls message delivery.

---

## Reasoning: Why DST is Critical for Owl

### Problem 1: Distributed Consensus is Hard to Test

Next-Gen Owl will use **Raft consensus** for replication. Traditional testing approaches:

❌ **Unit tests**: Can't test distributed scenarios  
❌ **Integration tests**: Slow, flaky, limited coverage  
❌ **Chaos engineering**: Non-reproducible failures  

✅ **DST**: Test millions of scenarios (network partitions, crashes, message reordering) in minutes, all reproducible.

### Problem 2: Concurrency Bugs are Rare but Catastrophic

Current Owl has:
- Lock-based concurrency (`RwLock`, `Mutex`)
- Deadlock detection
- Optimistic locking

**Traditional testing**: Might miss 1-in-10,000 race conditions  
**DST**: Systematically explores all thread interleavings

### Problem 3: Data Corruption is Unacceptable

Owl stores user data. A single bug that corrupts data = loss of trust.

**Traditional testing**: Limited scenarios  
**DST**: Test crash recovery, partial writes, checksum failures systematically

---

## Design Principles for Owl DST

### Principle 1: Dependency Injection

**All non-deterministic operations must go through injected interfaces**:

```rust
pub trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

pub trait Scheduler {
    fn spawn<F>(&self, f: F) where F: FnOnce() + Send + 'static;
    fn yield_now(&self);
}

pub trait FileSystem {
    fn create_file(&self, path: &str) -> Result<File>;
    fn mmap(&self, file: &File, size: usize) -> Result<&[u8]>;
}

pub trait Network {
    fn send(&self, node: NodeId, msg: Message) -> Result<()>;
    fn recv(&self) -> Result<Message>;
}
```

### Principle 2: Deterministic Execution

**All operations must be deterministic given the same seed**:

```rust
pub struct SimulationSeed(u64);

pub struct Simulation {
    seed: SimulationSeed,
    clock: SimulatedClock,
    scheduler: SimulatedScheduler,
    fs: SimulatedFileSystem,
    network: SimulatedNetwork,
    rng: SeededRng,
}

impl Simulation {
    pub fn new(seed: u64) -> Self {
        // All components initialized from seed
        // Same seed = same execution
    }
    
    pub fn run<F>(&mut self, test: F) -> TestResult
    where F: FnOnce(&mut Self) -> TestResult
    {
        test(self)
    }
}
```

### Principle 3: Reproducible Failures

**When a test fails, we must be able to replay it exactly**:

```rust
#[test]
fn test_concurrent_writes() {
    let seed = 12345;  // Failed with this seed
    let mut sim = Simulation::new(seed);
    
    sim.run(|sim| {
        // Test code here
        // Will execute identically every time
    });
}
```

---

## Next Steps

See **DETERMINISTIC_SIMULATION_PART2.md** for:
- Detailed implementation design
- Integration with current Owl code
- Example test scenarios
- Performance considerations
