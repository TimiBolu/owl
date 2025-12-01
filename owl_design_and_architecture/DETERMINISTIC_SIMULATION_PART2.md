# Deterministic Simulation Testing for Owl - Part 2

## Implementation Design

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Simulation Framework                      │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              Simulation Controller                    │  │
│  │  - Seed management                                    │  │
│  │  - Event scheduling                                   │  │
│  │  - Failure injection                                  │  │
│  └──────────────────────────────────────────────────────┘  │
│                          ↓                                   │
│  ┌──────────┬──────────┬──────────┬──────────┬──────────┐  │
│  │Simulated │Simulated │Simulated │Simulated │Simulated │  │
│  │  Clock   │Scheduler │   FS     │ Network  │   RNG    │  │
│  └──────────┴──────────┴──────────┴──────────┴──────────┘  │
│                          ↓                                   │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              Owl Storage Engine                       │  │
│  │  (Modified to use injected dependencies)             │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

---

## Component 1: Simulated Clock

### Interface

```rust
// src/simulation/clock.rs

use std::time::{Duration, Instant};
use std::sync::{Arc, Mutex};
use std::collections::BinaryHeap;

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
    fn schedule(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>);
}

pub struct SimulatedClock {
    current_time: Arc<Mutex<Instant>>,
    scheduled_events: Arc<Mutex<BinaryHeap<ScheduledEvent>>>,
}

struct ScheduledEvent {
    time: Instant,
    callback: Box<dyn FnOnce() + Send>,
}

impl SimulatedClock {
    pub fn new(start_time: Instant) -> Self {
        Self {
            current_time: Arc::new(Mutex::new(start_time)),
            scheduled_events: Arc::new(Mutex::new(BinaryHeap::new())),
        }
    }
    
    pub fn advance(&self, duration: Duration) {
        let mut time = self.current_time.lock().unwrap();
        *time += duration;
        
        // Fire all events up to current time
        let mut events = self.scheduled_events.lock().unwrap();
        while let Some(event) = events.peek() {
            if event.time <= *time {
                let event = events.pop().unwrap();
                (event.callback)();
            } else {
                break;
            }
        }
    }
}

impl Clock for SimulatedClock {
    fn now(&self) -> Instant {
        *self.current_time.lock().unwrap()
    }
    
    fn sleep(&self, duration: Duration) {
        // In simulation, sleep is a no-op
        // Time advances explicitly via advance()
    }
    
    fn schedule(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>) {
        let time = self.now() + delay;
        let event = ScheduledEvent { time, callback };
        self.scheduled_events.lock().unwrap().push(event);
    }
}

// Real clock for production
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
    
    fn schedule(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>) {
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            callback();
        });
    }
}
```

### Integration with Owl

```rust
// src/concurrency.rs - MODIFIED

pub struct ConcurrencyManager<C: Clock> {
    clock: Arc<C>,  // Injected!
    locks: Arc<RwLock<HashMap<String, Vec<LockInfo>>>>,
    // ... rest of fields
}

impl<C: Clock> ConcurrencyManager<C> {
    pub fn new(clock: Arc<C>) -> Self {
        Self {
            clock,
            locks: Arc::new(RwLock::new(HashMap::new())),
            // ...
        }
    }
    
    pub fn acquire_lock(&self, ...) -> OQLResult<()> {
        let start_time = self.clock.now();  // Use injected clock!
        
        loop {
            // ... lock acquisition logic
            
            if self.clock.now() - start_time > self.lock_timeout {
                return Err(OQLError::concurrency_error("Timeout", None));
            }
            
            self.clock.sleep(self.deadlock_check_interval);  // Simulated sleep!
        }
    }
}
```

---

## Component 2: Simulated Scheduler

### Interface

```rust
// src/simulation/scheduler.rs

use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

pub trait Scheduler: Send + Sync {
    fn spawn<F>(&self, f: F) where F: FnOnce() + Send + 'static;
    fn yield_now(&self);
    fn run_until_idle(&self);
}

pub struct SimulatedScheduler {
    tasks: Arc<Mutex<VecDeque<Box<dyn FnOnce() + Send>>>>,
    rng: Arc<Mutex<rand::rngs::StdRng>>,  // For random scheduling
}

impl SimulatedScheduler {
    pub fn new(seed: u64) -> Self {
        use rand::SeedableRng;
        Self {
            tasks: Arc::new(Mutex::new(VecDeque::new())),
            rng: Arc::new(Mutex::new(rand::rngs::StdRng::seed_from_u64(seed))),
        }
    }
    
    pub fn run_one(&self) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(task) = tasks.pop_front() {
            drop(tasks);  // Release lock before running task
            task();
            true
        } else {
            false
        }
    }
    
    pub fn run_random(&self) -> bool {
        use rand::Rng;
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.is_empty() {
            return false;
        }
        
        let mut rng = self.rng.lock().unwrap();
        let index = rng.gen_range(0..tasks.len());
        let task = tasks.remove(index).unwrap();
        drop(tasks);
        drop(rng);
        
        task();
        true
    }
}

impl Scheduler for SimulatedScheduler {
    fn spawn<F>(&self, f: F) where F: FnOnce() + Send + 'static {
        self.tasks.lock().unwrap().push_back(Box::new(f));
    }
    
    fn yield_now(&self) {
        // In simulation, yield is a no-op
        // Scheduler controls execution explicitly
    }
    
    fn run_until_idle(&self) {
        while self.run_one() {}
    }
}
```

---

## Component 3: Simulated File System

### Interface

```rust
// src/simulation/filesystem.rs

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub trait FileSystem: Send + Sync {
    fn create_file(&self, path: &str, size: usize) -> Result<FileHandle, std::io::Error>;
    fn read(&self, handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, std::io::Error>;
    fn write(&self, handle: &FileHandle, offset: u64, data: &[u8]) -> Result<(), std::io::Error>;
    fn sync(&self, handle: &FileHandle) -> Result<(), std::io::Error>;
    fn delete(&self, path: &str) -> Result<(), std::io::Error>;
}

#[derive(Clone, Debug)]
pub struct FileHandle {
    path: String,
    id: u64,
}

pub struct SimulatedFileSystem {
    files: Arc<RwLock<HashMap<String, SimulatedFile>>>,
    next_id: Arc<Mutex<u64>>,
    failure_injector: Arc<Mutex<FailureInjector>>,
}

struct SimulatedFile {
    data: Vec<u8>,
    synced: bool,
}

impl SimulatedFileSystem {
    pub fn new() -> Self {
        Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(0)),
            failure_injector: Arc::new(Mutex::new(FailureInjector::new())),
        }
    }
    
    pub fn inject_failure(&self, failure: FileSystemFailure) {
        self.failure_injector.lock().unwrap().add(failure);
    }
    
    pub fn crash(&self) {
        // Simulate crash: lose all unsynced writes
        let mut files = self.files.write().unwrap();
        for file in files.values_mut() {
            if !file.synced {
                // Corrupt or lose data
                file.data.clear();
            }
        }
    }
}

impl FileSystem for SimulatedFileSystem {
    fn create_file(&self, path: &str, size: usize) -> Result<FileHandle, std::io::Error> {
        // Check for injected failures
        if let Some(err) = self.failure_injector.lock().unwrap().check_create() {
            return Err(err);
        }
        
        let mut files = self.files.write().unwrap();
        let file = SimulatedFile {
            data: vec![0; size],
            synced: false,
        };
        files.insert(path.to_string(), file);
        
        let id = {
            let mut next_id = self.next_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };
        
        Ok(FileHandle {
            path: path.to_string(),
            id,
        })
    }
    
    fn write(&self, handle: &FileHandle, offset: u64, data: &[u8]) -> Result<(), std::io::Error> {
        let mut files = self.files.write().unwrap();
        if let Some(file) = files.get_mut(&handle.path) {
            let start = offset as usize;
            let end = start + data.len();
            file.data[start..end].copy_from_slice(data);
            file.synced = false;  // Mark as dirty
            Ok(())
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"))
        }
    }
    
    fn sync(&self, handle: &FileHandle) -> Result<(), std::io::Error> {
        let mut files = self.files.write().unwrap();
        if let Some(file) = files.get_mut(&handle.path) {
            file.synced = true;  // Mark as durable
            Ok(())
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"))
        }
    }
    
    // ... other methods
}
```

---

## Component 4: Simulation Controller

### Main Interface

```rust
// src/simulation/mod.rs

pub struct Simulation {
    seed: u64,
    clock: Arc<SimulatedClock>,
    scheduler: Arc<SimulatedScheduler>,
    filesystem: Arc<SimulatedFileSystem>,
    rng: Arc<Mutex<rand::rngs::StdRng>>,
}

impl Simulation {
    pub fn new(seed: u64) -> Self {
        use rand::SeedableRng;
        
        let start_time = Instant::now();  // Arbitrary start
        
        Self {
            seed,
            clock: Arc::new(SimulatedClock::new(start_time)),
            scheduler: Arc::new(SimulatedScheduler::new(seed)),
            filesystem: Arc::new(SimulatedFileSystem::new()),
            rng: Arc::new(Mutex::new(rand::rngs::StdRng::seed_from_u64(seed))),
        }
    }
    
    pub fn run<F, R>(&mut self, test: F) -> R
    where F: FnOnce(&mut Self) -> R
    {
        test(self)
    }
    
    pub fn advance_time(&self, duration: Duration) {
        self.clock.advance(duration);
    }
    
    pub fn run_scheduler_step(&self) {
        self.scheduler.run_one();
    }
    
    pub fn run_until_idle(&self) {
        self.scheduler.run_until_idle();
    }
    
    pub fn inject_crash(&self) {
        self.filesystem.crash();
    }
    
    pub fn create_storage(&self, path: &str, size: usize) -> SimulatedStorage {
        SimulatedStorage::new(
            path,
            size,
            self.clock.clone(),
            self.filesystem.clone(),
        )
    }
}
```

---

## Example Test Scenarios

### Scenario 1: Concurrent Lock Acquisition

```rust
#[test]
fn test_concurrent_lock_acquisition_deterministic() {
    let seed = 42;
    let mut sim = Simulation::new(seed);
    
    sim.run(|sim| {
        let manager = ConcurrencyManager::new(sim.clock.clone());
        
        // Spawn two tasks trying to acquire same lock
        sim.scheduler.spawn(|| {
            manager.acquire_lock("txn1", "doc1", LockType::Write, None).unwrap();
        });
        
        sim.scheduler.spawn(|| {
            manager.acquire_lock("txn2", "doc1", LockType::Write, None).unwrap();
        });
        
        // Run until one succeeds
        sim.run_until_idle();
        
        // Verify state
        assert_eq!(manager.get_lock_info("doc1").unwrap().len(), 1);
    });
}
```

### Scenario 2: Crash Recovery

```rust
#[test]
fn test_crash_recovery_deterministic() {
    let seed = 123;
    let mut sim = Simulation::new(seed);
    
    sim.run(|sim| {
        let storage = sim.create_storage("test.owl", 1024 * 1024);
        
        // Write some data
        storage.put_document(&doc1).unwrap();
        storage.put_document(&doc2).unwrap();
        
        // Simulate crash BEFORE sync
        sim.inject_crash();
        
        // Reopen storage
        let storage2 = sim.create_storage("test.owl", 1024 * 1024);
        
        // Verify data loss (no sync happened)
        assert!(storage2.get_document("users", &doc1.id).unwrap().is_none());
    });
}
```

---

See **DETERMINISTIC_SIMULATION_PART3.md** for:
- Distributed scenarios (network simulation)
- Failure injection strategies
- Property-based testing integration
- Performance optimization
