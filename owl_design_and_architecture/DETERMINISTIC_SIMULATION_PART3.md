# Deterministic Simulation Testing for Owl - Part 3

## Distributed Scenarios & Advanced Topics

### Component 5: Simulated Network

```rust
// src/simulation/network.rs

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub type NodeId = u32;
pub type MessageId = u64;

#[derive(Clone, Debug)]
pub struct Message {
    pub id: MessageId,
    pub from: NodeId,
    pub to: NodeId,
    pub payload: Vec<u8>,
    pub sent_at: Instant,
}

pub trait Network: Send + Sync {
    fn send(&self, from: NodeId, to: NodeId, payload: Vec<u8>) -> Result<(), NetworkError>;
    fn recv(&self, node: NodeId) -> Result<Option<Message>, NetworkError>;
    fn broadcast(&self, from: NodeId, payload: Vec<u8>) -> Result<(), NetworkError>;
}

pub struct SimulatedNetwork {
    /// Message queues per node
    queues: Arc<Mutex<HashMap<NodeId, VecDeque<Message>>>>,
    /// Network partitions (set of disconnected node pairs)
    partitions: Arc<Mutex<HashSet<(NodeId, NodeId)>>>,
    /// Message delays (min, max) in milliseconds
    delay_range: Arc<Mutex<(u64, u64)>>,
    /// Packet loss rate (0.0 to 1.0)
    loss_rate: Arc<Mutex<f64>>,
    /// Message reordering probability
    reorder_rate: Arc<Mutex<f64>>,
    /// Clock for timestamps
    clock: Arc<SimulatedClock>,
    /// RNG for delays and failures
    rng: Arc<Mutex<rand::rngs::StdRng>>,
    /// Next message ID
    next_msg_id: Arc<Mutex<MessageId>>,
}

impl SimulatedNetwork {
    pub fn new(clock: Arc<SimulatedClock>, seed: u64) -> Self {
        use rand::SeedableRng;
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            partitions: Arc::new(Mutex::new(HashSet::new())),
            delay_range: Arc::new(Mutex::new((0, 10))),  // 0-10ms default
            loss_rate: Arc::new(Mutex::new(0.0)),
            reorder_rate: Arc::new(Mutex::new(0.0)),
            clock,
            rng: Arc::new(Mutex::new(rand::rngs::StdRng::seed_from_u64(seed))),
            next_msg_id: Arc::new(Mutex::new(0)),
        }
    }
    
    /// Create a network partition between two nodes
    pub fn partition(&self, node1: NodeId, node2: NodeId) {
        let mut partitions = self.partitions.lock().unwrap();
        partitions.insert((node1, node2));
        partitions.insert((node2, node1));
    }
    
    /// Heal a network partition
    pub fn heal(&self, node1: NodeId, node2: NodeId) {
        let mut partitions = self.partitions.lock().unwrap();
        partitions.remove(&(node1, node2));
        partitions.remove(&(node2, node1));
    }
    
    /// Set packet loss rate (0.0 to 1.0)
    pub fn set_loss_rate(&self, rate: f64) {
        *self.loss_rate.lock().unwrap() = rate;
    }
    
    /// Set message delay range in milliseconds
    pub fn set_delay_range(&self, min_ms: u64, max_ms: u64) {
        *self.delay_range.lock().unwrap() = (min_ms, max_ms);
    }
    
    fn is_partitioned(&self, from: NodeId, to: NodeId) -> bool {
        self.partitions.lock().unwrap().contains(&(from, to))
    }
    
    fn should_drop_message(&self) -> bool {
        use rand::Rng;
        let loss_rate = *self.loss_rate.lock().unwrap();
        let mut rng = self.rng.lock().unwrap();
        rng.gen::<f64>() < loss_rate
    }
    
    fn get_delay(&self) -> Duration {
        use rand::Rng;
        let (min, max) = *self.delay_range.lock().unwrap();
        let mut rng = self.rng.lock().unwrap();
        let delay_ms = rng.gen_range(min..=max);
        Duration::from_millis(delay_ms)
    }
}

impl Network for SimulatedNetwork {
    fn send(&self, from: NodeId, to: NodeId, payload: Vec<u8>) -> Result<(), NetworkError> {
        // Check partition
        if self.is_partitioned(from, to) {
            return Err(NetworkError::Partitioned);
        }
        
        // Check packet loss
        if self.should_drop_message() {
            return Ok(());  // Silently drop
        }
        
        // Create message
        let msg_id = {
            let mut next_id = self.next_msg_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };
        
        let msg = Message {
            id: msg_id,
            from,
            to,
            payload,
            sent_at: self.clock.now(),
        };
        
        // Schedule delivery with delay
        let delay = self.get_delay();
        let queues = self.queues.clone();
        
        self.clock.schedule(delay, Box::new(move || {
            let mut queues = queues.lock().unwrap();
            queues.entry(to).or_insert_with(VecDeque::new).push_back(msg);
        }));
        
        Ok(())
    }
    
    fn recv(&self, node: NodeId) -> Result<Option<Message>, NetworkError> {
        let mut queues = self.queues.lock().unwrap();
        Ok(queues.get_mut(&node).and_then(|q| q.pop_front()))
    }
    
    fn broadcast(&self, from: NodeId, payload: Vec<u8>) -> Result<(), NetworkError> {
        // Get all nodes
        let queues = self.queues.lock().unwrap();
        let nodes: Vec<NodeId> = queues.keys().copied().collect();
        drop(queues);
        
        // Send to each node
        for to in nodes {
            if to != from {
                self.send(from, to, payload.clone())?;
            }
        }
        
        Ok(())
    }
}
```

---

## Distributed Test Scenarios

### Scenario 1: Raft Leader Election with Network Partition

```rust
#[test]
fn test_raft_leader_election_with_partition() {
    let seed = 999;
    let mut sim = Simulation::new(seed);
    
    sim.run(|sim| {
        // Create 5-node cluster
        let nodes = (0..5).map(|id| {
            RaftNode::new(id, sim.clock.clone(), sim.network.clone())
        }).collect::<Vec<_>>();
        
        // Start all nodes
        for node in &nodes {
            sim.scheduler.spawn(move || node.run());
        }
        
        // Let cluster elect a leader
        sim.advance_time(Duration::from_secs(1));
        sim.run_until_idle();
        
        let leader_id = nodes.iter().find(|n| n.is_leader()).unwrap().id();
        
        // Partition leader from rest of cluster
        for i in 0..5 {
            if i != leader_id {
                sim.network.partition(leader_id, i);
            }
        }
        
        // Advance time for election timeout
        sim.advance_time(Duration::from_secs(2));
        sim.run_until_idle();
        
        // Verify new leader elected
        let new_leaders: Vec<_> = nodes.iter().filter(|n| n.is_leader()).collect();
        assert_eq!(new_leaders.len(), 1);
        assert_ne!(new_leaders[0].id(), leader_id);
        
        // Heal partition
        for i in 0..5 {
            if i != leader_id {
                sim.network.heal(leader_id, i);
            }
        }
        
        // Old leader should step down
        sim.advance_time(Duration::from_secs(1));
        sim.run_until_idle();
        
        assert!(!nodes[leader_id as usize].is_leader());
    });
}
```

### Scenario 2: Split-Brain Prevention

```rust
#[test]
fn test_split_brain_prevention() {
    let seed = 777;
    let mut sim = Simulation::new(seed);
    
    sim.run(|sim| {
        // Create 5-node cluster
        let nodes = create_cluster(sim, 5);
        
        // Partition into two groups: [0,1] and [2,3,4]
        sim.network.partition(0, 2);
        sim.network.partition(0, 3);
        sim.network.partition(0, 4);
        sim.network.partition(1, 2);
        sim.network.partition(1, 3);
        sim.network.partition(1, 4);
        
        // Advance time
        sim.advance_time(Duration::from_secs(5));
        sim.run_until_idle();
        
        // Only the majority partition (2,3,4) should have a leader
        let leaders_in_minority = nodes[0..2].iter().filter(|n| n.is_leader()).count();
        let leaders_in_majority = nodes[2..5].iter().filter(|n| n.is_leader()).count();
        
        assert_eq!(leaders_in_minority, 0);  // No leader in minority
        assert_eq!(leaders_in_majority, 1);  // One leader in majority
    });
}
```

### Scenario 3: Concurrent Writes with Replication

```rust
#[test]
fn test_concurrent_writes_with_replication() {
    let seed = 555;
    let mut sim = Simulation::new(seed);
    
    sim.run(|sim| {
        let cluster = create_owl_cluster(sim, 3);
        
        // Client 1 writes to node 0
        sim.scheduler.spawn(|| {
            cluster[0].put_document(&doc1).unwrap();
        });
        
        // Client 2 writes to node 1 (same document, different value)
        sim.scheduler.spawn(|| {
            cluster[1].put_document(&doc1_v2).unwrap();
        });
        
        // Run until both writes complete
        sim.run_until_idle();
        
        // Advance time for replication
        sim.advance_time(Duration::from_millis(100));
        sim.run_until_idle();
        
        // All nodes should have same value (last-write-wins or conflict resolution)
        let val0 = cluster[0].get_document("users", &doc1.id).unwrap();
        let val1 = cluster[1].get_document("users", &doc1.id).unwrap();
        let val2 = cluster[2].get_document("users", &doc1.id).unwrap();
        
        assert_eq!(val0, val1);
        assert_eq!(val1, val2);
    });
}
```

---

## Failure Injection Strategies

### Strategy 1: Systematic Exploration

```rust
pub struct FailureInjector {
    failures: Vec<InjectedFailure>,
    current_index: usize,
}

pub enum InjectedFailure {
    NetworkPartition { node1: NodeId, node2: NodeId, at: Duration },
    NodeCrash { node: NodeId, at: Duration },
    DiskFull { node: NodeId, at: Duration },
    PacketLoss { rate: f64, duration: Duration },
    SlowDisk { node: NodeId, delay_ms: u64, duration: Duration },
}

impl FailureInjector {
    pub fn generate_scenarios(num_nodes: usize) -> Vec<Vec<InjectedFailure>> {
        let mut scenarios = Vec::new();
        
        // Scenario 1: Single node crash
        for node in 0..num_nodes {
            scenarios.push(vec![
                InjectedFailure::NodeCrash {
                    node: node as NodeId,
                    at: Duration::from_secs(1),
                }
            ]);
        }
        
        // Scenario 2: Network partition (all pairs)
        for i in 0..num_nodes {
            for j in i+1..num_nodes {
                scenarios.push(vec![
                    InjectedFailure::NetworkPartition {
                        node1: i as NodeId,
                        node2: j as NodeId,
                        at: Duration::from_secs(1),
                    }
                ]);
            }
        }
        
        // Scenario 3: Cascading failures
        scenarios.push(vec![
            InjectedFailure::NodeCrash { node: 0, at: Duration::from_secs(1) },
            InjectedFailure::NodeCrash { node: 1, at: Duration::from_secs(2) },
            InjectedFailure::NodeCrash { node: 2, at: Duration::from_secs(3) },
        ]);
        
        // ... more scenarios
        
        scenarios
    }
}
```

### Strategy 2: Random Exploration

```rust
pub struct RandomFailureGenerator {
    rng: rand::rngs::StdRng,
    config: FailureConfig,
}

pub struct FailureConfig {
    pub crash_probability: f64,
    pub partition_probability: f64,
    pub packet_loss_probability: f64,
    pub max_failures_per_test: usize,
}

impl RandomFailureGenerator {
    pub fn generate(&mut self, duration: Duration) -> Vec<InjectedFailure> {
        use rand::Rng;
        
        let mut failures = Vec::new();
        let num_failures = self.rng.gen_range(0..=self.config.max_failures_per_test);
        
        for _ in 0..num_failures {
            let failure_time = Duration::from_millis(
                self.rng.gen_range(0..duration.as_millis() as u64)
            );
            
            let failure_type = self.rng.gen::<f64>();
            
            if failure_type < self.config.crash_probability {
                failures.push(InjectedFailure::NodeCrash {
                    node: self.rng.gen_range(0..5),
                    at: failure_time,
                });
            } else if failure_type < self.config.crash_probability + self.config.partition_probability {
                failures.push(InjectedFailure::NetworkPartition {
                    node1: self.rng.gen_range(0..5),
                    node2: self.rng.gen_range(0..5),
                    at: failure_time,
                });
            }
            // ... more failure types
        }
        
        failures
    }
}
```

---

## Property-Based Testing Integration

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_all_nodes_converge(
        seed in 0u64..10000,
        num_writes in 1usize..100,
        num_nodes in 3usize..7,
    ) {
        let mut sim = Simulation::new(seed);
        
        sim.run(|sim| {
            let cluster = create_owl_cluster(sim, num_nodes);
            
            // Generate random writes
            for i in 0..num_writes {
                let node = i % num_nodes;
                let doc = generate_random_document(seed + i as u64);
                cluster[node].put_document(&doc).unwrap();
            }
            
            // Run until idle
            sim.run_until_idle();
            
            // Advance time for replication
            sim.advance_time(Duration::from_secs(10));
            sim.run_until_idle();
            
            // Property: All nodes should have same data
            for i in 1..num_nodes {
                assert_eq!(
                    cluster[0].scan_documents("users").unwrap(),
                    cluster[i].scan_documents("users").unwrap(),
                    "Nodes 0 and {} diverged", i
                );
            }
        });
    }
}
```

---

## Performance Optimization

### Technique 1: Parallel Simulation

```rust
pub fn run_parallel_simulations(seeds: Vec<u64>) -> Vec<TestResult> {
    use rayon::prelude::*;
    
    seeds.par_iter().map(|&seed| {
        let mut sim = Simulation::new(seed);
        sim.run(|sim| {
            // Test logic
        })
    }).collect()
}
```

### Technique 2: State Space Reduction

```rust
pub struct StateSpaceExplorer {
    visited_states: HashSet<StateHash>,
}

impl StateSpaceExplorer {
    pub fn should_explore(&mut self, state: &SystemState) -> bool {
        let hash = state.hash();
        if self.visited_states.contains(&hash) {
            false  // Already explored this state
        } else {
            self.visited_states.insert(hash);
            true
        }
    }
}
```

---

## Integration Roadmap

### Phase 1: Single-Node Simulation (1 month)
- [ ] Implement SimulatedClock
- [ ] Implement SimulatedScheduler
- [ ] Implement SimulatedFileSystem
- [ ] Modify ConcurrencyManager to use injected Clock
- [ ] Write 10+ single-node test scenarios

### Phase 2: Distributed Simulation (2 months)
- [ ] Implement SimulatedNetwork
- [ ] Implement Raft consensus with simulation support
- [ ] Write 20+ distributed test scenarios
- [ ] Implement failure injection framework

### Phase 3: Advanced Testing (1 month)
- [ ] Property-based testing integration
- [ ] Parallel simulation execution
- [ ] State space exploration
- [ ] Performance benchmarking

---

## Conclusion

**Deterministic Simulation Testing gives Owl**:
1. ✅ **Reproducible failures** - Debug once, fix forever
2. ✅ **Massive coverage** - Test millions of scenarios
3. ✅ **Distributed confidence** - Verify consensus correctness
4. ✅ **Fast iteration** - No waiting for real networks/disks

**This is how we build a production-ready distributed database.**
