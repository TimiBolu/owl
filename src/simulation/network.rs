use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Node identifier in the network
pub type NodeId = u32;

/// Network message
#[derive(Debug, Clone)]
pub struct Message {
    pub from: NodeId,
    pub to: NodeId,
    pub data: Vec<u8>,
    pub message_type: String,
}

/// Network trait for message passing with dependency injection
pub trait Network: Send + Sync {
    /// Send a message to another node
    fn send(&self, from: NodeId, to: NodeId, data: Vec<u8>) -> Result<(), NetworkError>;

    /// Send a typed message
    fn send_message(&self, message: Message) -> Result<(), NetworkError>;

    /// Receive messages for a node (non-blocking)
    fn receive(&self, node_id: NodeId) -> Vec<Message>;

    /// Check if two nodes can communicate
    fn can_communicate(&self, from: NodeId, to: NodeId) -> bool;

    /// Get network latency between nodes
    fn latency(&self, from: NodeId, to: NodeId) -> Duration;
}

/// Network errors
#[derive(Debug, Clone)]
pub enum NetworkError {
    NodeNotFound(NodeId),
    NetworkPartition,
    MessageDropped,
    MessageReordered,
    Timeout,
    ConnectionRefused,
    NetworkCongestion,
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::NodeNotFound(id) => write!(f, "Node {} not found", id),
            NetworkError::NetworkPartition => write!(f, "Network partition"),
            NetworkError::MessageDropped => write!(f, "Message dropped"),
            NetworkError::MessageReordered => write!(f, "Message reordered"),
            NetworkError::Timeout => write!(f, "Network timeout"),
            NetworkError::ConnectionRefused => write!(f, "Connection refused"),
            NetworkError::NetworkCongestion => write!(f, "Network congestion"),
        }
    }
}

impl std::error::Error for NetworkError {}

/// Pending message with delivery time and ordering info
#[derive(Debug)]
#[allow(dead_code)]
struct PendingMessage {
    message: Message,
    delivery_time: std::time::Instant,
    original_order: u64, // For reordering simulation
}

/// Network partition configuration
#[derive(Debug, Clone)]
struct NetworkPartition {
    /// Nodes in partition group A
    group_a: HashSet<NodeId>,
    /// Nodes in partition group B  
    group_b: HashSet<NodeId>,
    /// Whether this is a complete partition (no communication) or partial
    complete: bool,
}

/// Network failure configuration
#[derive(Debug, Clone)]
pub struct NetworkFailureConfig {
    /// Packet loss rate (0.0 to 1.0)
    pub packet_loss_rate: f64,
    /// Message reordering rate (0.0 to 1.0)
    pub reorder_rate: f64,
    /// Maximum reorder delay
    pub max_reorder_delay: Duration,
    /// Network congestion probability
    pub congestion_rate: f64,
    /// Additional latency during congestion
    pub congestion_latency: Duration,
}

/// Simulated network for distributed testing
pub struct SimulatedNetwork {
    /// Message queues for each node
    message_queues: Arc<Mutex<HashMap<NodeId, VecDeque<Message>>>>,
    /// Pending messages (for simulating delays)
    pending_messages: Arc<Mutex<VecDeque<PendingMessage>>>,
    /// Active network partitions
    partitions: Arc<Mutex<Vec<NetworkPartition>>>,
    /// Per-node pair latency configuration
    latency_matrix: Arc<Mutex<HashMap<(NodeId, NodeId), Duration>>>,
    /// Base latency between nodes
    base_latency: Duration,
    /// Network failure configuration
    failure_config: Arc<Mutex<NetworkFailureConfig>>,
    /// Random number generator for deterministic behavior
    rng: Arc<Mutex<StdRng>>,
    /// Current simulated time
    current_time: Arc<Mutex<std::time::Instant>>,
    /// Message ordering counter for reordering simulation
    message_counter: Arc<Mutex<u64>>,
    /// Registered nodes in the network
    nodes: Arc<Mutex<HashSet<NodeId>>>,
}

impl Default for NetworkFailureConfig {
    fn default() -> Self {
        Self {
            packet_loss_rate: 0.0,
            reorder_rate: 0.0,
            max_reorder_delay: Duration::from_millis(100),
            congestion_rate: 0.0,
            congestion_latency: Duration::from_millis(500),
        }
    }
}

impl SimulatedNetwork {
    /// Create a new simulated network
    pub fn new(seed: u64) -> Self {
        Self {
            message_queues: Arc::new(Mutex::new(HashMap::new())),
            pending_messages: Arc::new(Mutex::new(VecDeque::new())),
            partitions: Arc::new(Mutex::new(Vec::new())),
            latency_matrix: Arc::new(Mutex::new(HashMap::new())),
            base_latency: Duration::from_millis(10),
            failure_config: Arc::new(Mutex::new(NetworkFailureConfig::default())),
            rng: Arc::new(Mutex::new(StdRng::seed_from_u64(seed))),
            current_time: Arc::new(Mutex::new(std::time::Instant::now())),
            message_counter: Arc::new(Mutex::new(0)),
            nodes: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Add a node to the network
    pub fn add_node(&self, node_id: NodeId) {
        let mut queues = self.message_queues.lock().unwrap();
        queues.entry(node_id).or_insert_with(VecDeque::new);

        let mut nodes = self.nodes.lock().unwrap();
        nodes.insert(node_id);
    }

    /// Remove a node from the network
    pub fn remove_node(&self, node_id: NodeId) {
        let mut queues = self.message_queues.lock().unwrap();
        queues.remove(&node_id);

        let mut nodes = self.nodes.lock().unwrap();
        nodes.remove(&node_id);

        // Remove any pending messages to/from this node
        let mut pending = self.pending_messages.lock().unwrap();
        pending.retain(|msg| msg.message.from != node_id && msg.message.to != node_id);
    }

    /// Get all registered nodes
    pub fn get_nodes(&self) -> Vec<NodeId> {
        self.nodes.lock().unwrap().iter().cloned().collect()
    }

    /// Create a network partition between two groups of nodes
    pub fn partition(&self, group1: &[NodeId], group2: &[NodeId]) {
        let partition = NetworkPartition {
            group_a: group1.iter().cloned().collect(),
            group_b: group2.iter().cloned().collect(),
            complete: true,
        };

        let mut partitions = self.partitions.lock().unwrap();
        partitions.push(partition);
    }

    /// Create a partial partition with reduced connectivity
    pub fn partial_partition(&self, group1: &[NodeId], group2: &[NodeId]) {
        let partition = NetworkPartition {
            group_a: group1.iter().cloned().collect(),
            group_b: group2.iter().cloned().collect(),
            complete: false,
        };

        let mut partitions = self.partitions.lock().unwrap();
        partitions.push(partition);
    }

    /// Heal all network partitions
    pub fn heal_partition(&self) {
        let mut partitions = self.partitions.lock().unwrap();
        partitions.clear();
    }

    /// Heal specific partition between groups
    pub fn heal_specific_partition(&self, group1: &[NodeId], group2: &[NodeId]) {
        let mut partitions = self.partitions.lock().unwrap();
        let group1_set: HashSet<NodeId> = group1.iter().cloned().collect();
        let group2_set: HashSet<NodeId> = group2.iter().cloned().collect();

        partitions.retain(|partition| {
            !(partition.group_a == group1_set && partition.group_b == group2_set)
                && !(partition.group_a == group2_set && partition.group_b == group1_set)
        });
    }

    /// Set comprehensive failure configuration
    pub fn set_failure_config(&self, config: NetworkFailureConfig) {
        *self.failure_config.lock().unwrap() = config;
    }

    /// Set packet loss rate
    pub fn set_packet_loss_rate(&self, rate: f64) {
        let mut config = self.failure_config.lock().unwrap();
        config.packet_loss_rate = rate.clamp(0.0, 1.0);
    }

    /// Set message reordering rate
    pub fn set_reorder_rate(&self, rate: f64) {
        let mut config = self.failure_config.lock().unwrap();
        config.reorder_rate = rate.clamp(0.0, 1.0);
    }

    /// Set network congestion rate
    pub fn set_congestion_rate(&self, rate: f64) {
        let mut config = self.failure_config.lock().unwrap();
        config.congestion_rate = rate.clamp(0.0, 1.0);
    }

    /// Set base network latency
    pub fn set_base_latency(&self, latency: Duration) {
        // Store as default for all node pairs
        let nodes = self.get_nodes();
        let mut matrix = self.latency_matrix.lock().unwrap();

        for &from in &nodes {
            for &to in &nodes {
                if from != to {
                    matrix.insert((from, to), latency);
                }
            }
        }
    }

    /// Set latency between specific nodes
    pub fn set_latency(&self, from: NodeId, to: NodeId, latency: Duration) {
        let mut matrix = self.latency_matrix.lock().unwrap();
        matrix.insert((from, to), latency);
        matrix.insert((to, from), latency); // Symmetric by default
    }

    /// Set asymmetric latency between nodes
    pub fn set_asymmetric_latency(&self, from: NodeId, to: NodeId, latency: Duration) {
        let mut matrix = self.latency_matrix.lock().unwrap();
        matrix.insert((from, to), latency);
    }

    /// Advance network time and deliver pending messages
    pub fn advance_time(&self, duration: Duration) {
        let mut current_time = self.current_time.lock().unwrap();
        *current_time += duration;
        let target_time = *current_time;
        drop(current_time);

        // Deliver messages that are ready
        let mut pending = self.pending_messages.lock().unwrap();
        let mut queues = self.message_queues.lock().unwrap();

        let mut delivered = Vec::new();

        for (i, pending_msg) in pending.iter().enumerate() {
            if pending_msg.delivery_time <= target_time {
                // Deliver message
                if let Some(queue) = queues.get_mut(&pending_msg.message.to) {
                    queue.push_back(pending_msg.message.clone());
                }
                delivered.push(i);
            }
        }

        // Remove delivered messages (in reverse order to maintain indices)
        for &i in delivered.iter().rev() {
            pending.remove(i);
        }
    }

    /// Get number of pending messages
    pub fn pending_message_count(&self) -> usize {
        self.pending_messages.lock().unwrap().len()
    }

    /// Get number of queued messages for a node
    pub fn queued_message_count(&self, node_id: NodeId) -> usize {
        self.message_queues
            .lock()
            .unwrap()
            .get(&node_id)
            .map(|queue| queue.len())
            .unwrap_or(0)
    }

    /// Clear all messages (for testing)
    pub fn clear_all_messages(&self) {
        self.pending_messages.lock().unwrap().clear();
        let mut queues = self.message_queues.lock().unwrap();
        for queue in queues.values_mut() {
            queue.clear();
        }
    }

    /// Check if message should be dropped due to packet loss
    fn should_drop_message(&self) -> bool {
        let config = self.failure_config.lock().unwrap();
        if config.packet_loss_rate <= 0.0 {
            return false;
        }

        let mut rng = self.rng.lock().unwrap();
        rng.gen::<f64>() < config.packet_loss_rate
    }

    /// Check if message should be reordered
    fn should_reorder_message(&self) -> bool {
        let config = self.failure_config.lock().unwrap();
        if config.reorder_rate <= 0.0 {
            return false;
        }

        let mut rng = self.rng.lock().unwrap();
        rng.gen::<f64>() < config.reorder_rate
    }

    /// Check if network is congested
    fn is_congested(&self) -> bool {
        let config = self.failure_config.lock().unwrap();
        if config.congestion_rate <= 0.0 {
            return false;
        }

        let mut rng = self.rng.lock().unwrap();
        rng.gen::<f64>() < config.congestion_rate
    }

    /// Check if nodes are partitioned
    fn are_partitioned(&self, from: NodeId, to: NodeId) -> bool {
        let partitions = self.partitions.lock().unwrap();

        for partition in partitions.iter() {
            let from_in_a = partition.group_a.contains(&from);
            let from_in_b = partition.group_b.contains(&from);
            let to_in_a = partition.group_a.contains(&to);
            let to_in_b = partition.group_b.contains(&to);

            // Check if nodes are in different partition groups
            if (from_in_a && to_in_b) || (from_in_b && to_in_a) {
                if partition.complete {
                    return true; // Complete partition blocks all communication
                } else {
                    // Partial partition - use random chance based on seed
                    let mut rng = self.rng.lock().unwrap();
                    return rng.gen::<f64>() < 0.8; // 80% chance of blocking in partial partition
                }
            }
        }

        false
    }

    /// Get latency between two nodes
    fn get_latency(&self, from: NodeId, to: NodeId) -> Duration {
        let matrix = self.latency_matrix.lock().unwrap();
        matrix
            .get(&(from, to))
            .cloned()
            .unwrap_or(self.base_latency)
    }
}

impl Network for SimulatedNetwork {
    fn send(&self, from: NodeId, to: NodeId, data: Vec<u8>) -> Result<(), NetworkError> {
        let message = Message {
            from,
            to,
            data,
            message_type: "generic".to_string(),
        };
        self.send_message(message)
    }

    fn send_message(&self, message: Message) -> Result<(), NetworkError> {
        // Check if nodes exist
        let queues = self.message_queues.lock().unwrap();
        if !queues.contains_key(&message.from) {
            return Err(NetworkError::NodeNotFound(message.from));
        }
        if !queues.contains_key(&message.to) {
            return Err(NetworkError::NodeNotFound(message.to));
        }
        drop(queues);

        // Check for network partition
        if self.are_partitioned(message.from, message.to) {
            return Err(NetworkError::NetworkPartition);
        }

        // Check for packet loss
        if self.should_drop_message() {
            return Err(NetworkError::MessageDropped);
        }

        // Calculate delivery time with various failure modes
        let current_time = *self.current_time.lock().unwrap();
        let mut rng = self.rng.lock().unwrap();

        // Get base latency for this node pair
        let base_latency = self.get_latency(message.from, message.to);

        // Add jitter (±50%)
        let jitter_factor = rng.gen_range(0.5..1.5);
        let mut latency =
            Duration::from_nanos((base_latency.as_nanos() as f64 * jitter_factor) as u64);

        // Check for network congestion
        if self.is_congested() {
            let config = self.failure_config.lock().unwrap();
            latency += config.congestion_latency;
        }

        // Check for message reordering
        let mut delivery_time = current_time + latency;
        if self.should_reorder_message() {
            let config = self.failure_config.lock().unwrap();
            let reorder_delay =
                Duration::from_nanos(rng.gen_range(0..config.max_reorder_delay.as_nanos()) as u64);
            delivery_time += reorder_delay;
        }

        // Get message order for reordering simulation
        let message_order = {
            let mut counter = self.message_counter.lock().unwrap();
            *counter += 1;
            *counter
        };

        drop(rng);

        // Queue message for delivery
        let pending_msg = PendingMessage {
            message,
            delivery_time,
            original_order: message_order,
        };

        self.pending_messages.lock().unwrap().push_back(pending_msg);

        Ok(())
    }

    fn receive(&self, node_id: NodeId) -> Vec<Message> {
        let mut queues = self.message_queues.lock().unwrap();
        if let Some(queue) = queues.get_mut(&node_id) {
            let mut messages = Vec::new();
            while let Some(message) = queue.pop_front() {
                messages.push(message);
            }
            messages
        } else {
            Vec::new()
        }
    }

    fn can_communicate(&self, from: NodeId, to: NodeId) -> bool {
        !self.are_partitioned(from, to)
    }

    fn latency(&self, from: NodeId, to: NodeId) -> Duration {
        self.get_latency(from, to)
    }
}

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::thread;

/// Connection pool entry
#[derive(Debug)]
struct Connection {
    stream: TcpStream,
    last_used: std::time::Instant,
}

/// Circuit breaker states
#[derive(Debug, Clone, PartialEq)]
enum CircuitBreakerState {
    Closed,   // Normal operation
    Open,     // Failing, reject requests
    HalfOpen, // Testing if service recovered
}

/// Circuit breaker for connection management
#[derive(Debug)]
struct CircuitBreaker {
    state: CircuitBreakerState,
    failure_count: u32,
    failure_threshold: u32,
    timeout: Duration,
    last_failure_time: Option<std::time::Instant>,
}

impl CircuitBreaker {
    fn new(failure_threshold: u32, timeout: Duration) -> Self {
        Self {
            state: CircuitBreakerState::Closed,
            failure_count: 0,
            failure_threshold,
            timeout,
            last_failure_time: None,
        }
    }

    fn can_execute(&mut self) -> bool {
        match self.state {
            CircuitBreakerState::Closed => true,
            CircuitBreakerState::Open => {
                if let Some(last_failure) = self.last_failure_time {
                    if last_failure.elapsed() >= self.timeout {
                        self.state = CircuitBreakerState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitBreakerState::HalfOpen => true,
        }
    }

    fn on_success(&mut self) {
        self.failure_count = 0;
        self.state = CircuitBreakerState::Closed;
        self.last_failure_time = None;
    }

    fn on_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure_time = Some(std::time::Instant::now());

        if self.failure_count >= self.failure_threshold {
            self.state = CircuitBreakerState::Open;
        }
    }
}

/// Real network for production deployment
#[allow(dead_code)]
pub struct RealNetwork {
    /// Optional name for debugging
    name: Option<String>,
    /// Connection pool for reusing TCP connections
    connection_pool: Arc<Mutex<HashMap<NodeId, Connection>>>,
    /// Circuit breakers per node
    circuit_breakers: Arc<Mutex<HashMap<NodeId, CircuitBreaker>>>,
    /// Node address mapping
    node_addresses: Arc<Mutex<HashMap<NodeId, SocketAddr>>>,
    /// Connection timeout
    connection_timeout: Duration,
    /// Read timeout
    read_timeout: Duration,
    /// Write timeout  
    write_timeout: Duration,
    /// Maximum connections per node
    max_connections_per_node: usize,
}

impl RealNetwork {
    pub fn new() -> Self {
        Self {
            name: None,
            connection_pool: Arc::new(Mutex::new(HashMap::new())),
            circuit_breakers: Arc::new(Mutex::new(HashMap::new())),
            node_addresses: Arc::new(Mutex::new(HashMap::new())),
            connection_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            max_connections_per_node: 10,
        }
    }

    pub fn with_name(name: String) -> Self {
        Self {
            name: Some(name),
            connection_pool: Arc::new(Mutex::new(HashMap::new())),
            circuit_breakers: Arc::new(Mutex::new(HashMap::new())),
            node_addresses: Arc::new(Mutex::new(HashMap::new())),
            connection_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            max_connections_per_node: 10,
        }
    }

    /// Register a node with its network address
    pub fn register_node(&self, node_id: NodeId, address: SocketAddr) {
        let mut addresses = self.node_addresses.lock().unwrap();
        addresses.insert(node_id, address);

        // Initialize circuit breaker for this node
        let mut breakers = self.circuit_breakers.lock().unwrap();
        breakers.insert(node_id, CircuitBreaker::new(5, Duration::from_secs(60)));
    }

    /// Set connection timeout
    pub fn set_connection_timeout(&mut self, timeout: Duration) {
        self.connection_timeout = timeout;
    }

    /// Set read/write timeouts
    pub fn set_io_timeouts(&mut self, read_timeout: Duration, write_timeout: Duration) {
        self.read_timeout = read_timeout;
        self.write_timeout = write_timeout;
    }

    /// Get or create a connection to a node
    fn get_connection(&self, node_id: NodeId) -> Result<TcpStream, NetworkError> {
        // Check circuit breaker
        {
            let mut breakers = self.circuit_breakers.lock().unwrap();
            if let Some(breaker) = breakers.get_mut(&node_id) {
                if !breaker.can_execute() {
                    return Err(NetworkError::ConnectionRefused);
                }
            }
        }

        // Try to get existing connection from pool
        {
            let mut pool = self.connection_pool.lock().unwrap();
            if let Some(conn) = pool.remove(&node_id) {
                // Check if connection is still valid (not too old)
                if conn.last_used.elapsed() < Duration::from_secs(300) {
                    // Try to use existing connection
                    return Ok(conn.stream);
                }
            }
        }

        // Create new connection
        let addresses = self.node_addresses.lock().unwrap();
        let address = addresses
            .get(&node_id)
            .ok_or(NetworkError::NodeNotFound(node_id))?;

        match TcpStream::connect_timeout(address, self.connection_timeout) {
            Ok(stream) => {
                // Set timeouts
                stream
                    .set_read_timeout(Some(self.read_timeout))
                    .map_err(|_| NetworkError::Timeout)?;
                stream
                    .set_write_timeout(Some(self.write_timeout))
                    .map_err(|_| NetworkError::Timeout)?;

                // Mark success in circuit breaker
                let mut breakers = self.circuit_breakers.lock().unwrap();
                if let Some(breaker) = breakers.get_mut(&node_id) {
                    breaker.on_success();
                }

                Ok(stream)
            }
            Err(_) => {
                // Mark failure in circuit breaker
                let mut breakers = self.circuit_breakers.lock().unwrap();
                if let Some(breaker) = breakers.get_mut(&node_id) {
                    breaker.on_failure();
                }

                Err(NetworkError::ConnectionRefused)
            }
        }
    }

    /// Return a connection to the pool
    fn return_connection(&self, node_id: NodeId, stream: TcpStream) {
        let mut pool = self.connection_pool.lock().unwrap();

        // Only keep connection if pool isn't full
        if pool.len() < self.max_connections_per_node {
            let connection = Connection {
                stream,
                last_used: std::time::Instant::now(),
            };
            pool.insert(node_id, connection);
        }
        // Otherwise, connection will be dropped when it goes out of scope
    }

    /// Send data over TCP with automatic reconnection
    fn send_tcp_data(&self, node_id: NodeId, data: &[u8]) -> Result<(), NetworkError> {
        const MAX_RETRIES: u32 = 3;
        let mut retries = 0;

        while retries < MAX_RETRIES {
            match self.get_connection(node_id) {
                Ok(mut stream) => {
                    // Serialize message length + data
                    let message_len = data.len() as u32;
                    let len_bytes = message_len.to_be_bytes();

                    // Send length header first
                    match stream.write_all(&len_bytes) {
                        Ok(_) => {
                            // Send actual data
                            match stream.write_all(data) {
                                Ok(_) => {
                                    // Flush to ensure data is sent
                                    match stream.flush() {
                                        Ok(_) => {
                                            // Return connection to pool for reuse
                                            self.return_connection(node_id, stream);
                                            return Ok(());
                                        }
                                        Err(_) => {
                                            // Don't return failed connection to pool
                                        }
                                    }
                                }
                                Err(_) => {
                                    // Don't return failed connection to pool
                                }
                            }
                        }
                        Err(_) => {
                            // Don't return failed connection to pool
                        }
                    }
                }
                Err(e) => return Err(e),
            }

            retries += 1;

            // Exponential backoff
            if retries < MAX_RETRIES {
                thread::sleep(Duration::from_millis(100 * (1 << retries)));
            }
        }

        // Mark failure after all retries exhausted
        let mut breakers = self.circuit_breakers.lock().unwrap();
        if let Some(breaker) = breakers.get_mut(&node_id) {
            breaker.on_failure();
        }

        Err(NetworkError::Timeout)
    }
}

impl Default for RealNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl Network for RealNetwork {
    fn send(&self, from: NodeId, to: NodeId, data: Vec<u8>) -> Result<(), NetworkError> {
        let message = Message {
            from,
            to,
            data,
            message_type: "generic".to_string(),
        };
        self.send_message(message)
    }

    fn send_message(&self, message: Message) -> Result<(), NetworkError> {
        // Serialize message to bytes
        let serialized = self.serialize_message(&message)?;

        // Send over TCP
        self.send_tcp_data(message.to, &serialized)
    }

    fn receive(&self, _node_id: NodeId) -> Vec<Message> {
        // Real network implementation would have a background receiver thread
        // that populates message queues. For now, return empty.
        // In a full implementation, this would:
        // 1. Have a TcpListener for incoming connections
        // 2. Background threads to handle incoming messages
        // 3. Message queues per node
        Vec::new()
    }

    fn can_communicate(&self, _from: NodeId, to: NodeId) -> bool {
        // Check if we have address for target node and circuit breaker allows
        let addresses = self.node_addresses.lock().unwrap();
        if !addresses.contains_key(&to) {
            return false;
        }

        let mut breakers = self.circuit_breakers.lock().unwrap();
        if let Some(breaker) = breakers.get_mut(&to) {
            breaker.can_execute()
        } else {
            true
        }
    }

    fn latency(&self, _from: NodeId, _to: NodeId) -> Duration {
        // In production, this could measure actual network latency
        // For now, return a reasonable default
        Duration::from_millis(50)
    }
}

impl RealNetwork {
    /// Serialize message to bytes for network transmission
    fn serialize_message(&self, message: &Message) -> Result<Vec<u8>, NetworkError> {
        // Simple serialization format:
        // [from: 4 bytes][to: 4 bytes][type_len: 4 bytes][type: variable][data_len: 4 bytes][data: variable]

        let mut buffer = Vec::new();

        // From node ID
        buffer.extend_from_slice(&message.from.to_be_bytes());

        // To node ID
        buffer.extend_from_slice(&message.to.to_be_bytes());

        // Message type length and data
        let type_bytes = message.message_type.as_bytes();
        buffer.extend_from_slice(&(type_bytes.len() as u32).to_be_bytes());
        buffer.extend_from_slice(type_bytes);

        // Message data length and data
        buffer.extend_from_slice(&(message.data.len() as u32).to_be_bytes());
        buffer.extend_from_slice(&message.data);

        Ok(buffer)
    }

    /// Deserialize message from bytes
    #[allow(dead_code)]
    fn deserialize_message(&self, data: &[u8]) -> Result<Message, NetworkError> {
        if data.len() < 16 {
            return Err(NetworkError::MessageDropped);
        }

        let mut offset = 0;

        // Read from node ID
        let from = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        offset += 4;

        // Read to node ID
        let to = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        offset += 4;

        // Read message type length
        let type_len = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
        offset += 4;

        if offset + type_len > data.len() {
            return Err(NetworkError::MessageDropped);
        }

        // Read message type
        let message_type = String::from_utf8_lossy(&data[offset..offset + type_len]).to_string();
        offset += type_len;

        if offset + 4 > data.len() {
            return Err(NetworkError::MessageDropped);
        }

        // Read data length
        let data_len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + data_len > data.len() {
            return Err(NetworkError::MessageDropped);
        }

        // Read message data
        let message_data = data[offset..offset + data_len].to_vec();

        Ok(Message {
            from,
            to,
            data: message_data,
            message_type,
        })
    }

    /// Start a background listener for incoming connections (for full implementation)
    pub fn start_listener(&self, _bind_address: SocketAddr) -> Result<(), NetworkError> {
        // This would start a background thread with TcpListener
        // For now, just return Ok to indicate the interface exists
        Ok(())
    }

    /// Get connection pool statistics
    pub fn connection_stats(&self) -> HashMap<NodeId, usize> {
        let pool = self.connection_pool.lock().unwrap();
        pool.iter().map(|(&node_id, _)| (node_id, 1)).collect()
    }

    /// Get circuit breaker states
    pub fn circuit_breaker_states(&self) -> HashMap<NodeId, String> {
        let breakers = self.circuit_breakers.lock().unwrap();
        breakers
            .iter()
            .map(|(&node_id, breaker)| {
                let state_str = match breaker.state {
                    CircuitBreakerState::Closed => "Closed",
                    CircuitBreakerState::Open => "Open",
                    CircuitBreakerState::HalfOpen => "HalfOpen",
                };
                (node_id, state_str.to_string())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simulated_network_basic() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        network.add_node(1);
        network.add_node(2);

        // Send message
        let result = network.send(1, 2, b"Hello".to_vec());
        assert!(result.is_ok());

        // Message should be pending (not delivered yet)
        let messages = network.receive(2);
        assert_eq!(messages.len(), 0);

        // Advance time to deliver message
        network.advance_time(Duration::from_millis(20));

        // Message should be delivered
        let messages = network.receive(2);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].data, b"Hello");
        assert_eq!(messages[0].from, 1);
        assert_eq!(messages[0].to, 2);
    }

    #[test]
    fn test_network_partition() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        network.add_node(1);
        network.add_node(2);
        network.add_node(3);

        // Create partition: {1} | {2, 3}
        network.partition(&[1], &[2, 3]);

        // Communication within partition should work
        assert!(network.can_communicate(2, 3));
        assert!(network.can_communicate(3, 2));

        // Cross-partition communication should fail
        assert!(!network.can_communicate(1, 2));
        assert!(!network.can_communicate(2, 1));

        let result = network.send(1, 2, b"Hello".to_vec());
        assert!(matches!(result, Err(NetworkError::NetworkPartition)));

        // Heal partition
        network.heal_partition();

        // Communication should work again
        assert!(network.can_communicate(1, 2));
        let result = network.send(1, 2, b"Hello".to_vec());
        assert!(result.is_ok());
    }

    #[test]
    fn test_packet_loss() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        network.add_node(1);
        network.add_node(2);

        // Set 100% packet loss
        network.set_packet_loss_rate(1.0);

        // Messages should be dropped
        let result = network.send(1, 2, b"Hello".to_vec());
        assert!(matches!(result, Err(NetworkError::MessageDropped)));

        // Disable packet loss
        network.set_packet_loss_rate(0.0);

        // Messages should go through
        let result = network.send(1, 2, b"Hello".to_vec());
        assert!(result.is_ok());
    }

    #[test]
    fn test_message_reordering() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        network.add_node(1);
        network.add_node(2);

        // Set high reordering rate
        network.set_reorder_rate(1.0);

        // Send multiple messages
        for i in 0..5 {
            let data = format!("Message {}", i).into_bytes();
            let result = network.send(1, 2, data);
            assert!(result.is_ok());
        }

        // Advance time to deliver all messages
        network.advance_time(Duration::from_secs(1));

        // Messages should be delivered (possibly out of order due to reordering)
        let messages = network.receive(2);
        assert_eq!(messages.len(), 5);
    }

    #[test]
    fn test_network_congestion() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        network.add_node(1);
        network.add_node(2);

        // Set high congestion rate
        network.set_congestion_rate(1.0);

        // Send message
        let result = network.send(1, 2, b"Hello".to_vec());
        assert!(result.is_ok());

        // Should have pending message
        assert_eq!(network.pending_message_count(), 1);

        // Advance small amount of time - message shouldn't be delivered yet due to congestion
        network.advance_time(Duration::from_millis(10));
        let messages = network.receive(2);
        assert_eq!(messages.len(), 0);

        // Advance more time to account for congestion delay
        network.advance_time(Duration::from_secs(1));
        let messages = network.receive(2);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_partial_partition() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        for i in 1..=5 {
            network.add_node(i);
        }

        // Create partial partition: {1, 2} | {3, 4, 5}
        network.partial_partition(&[1, 2], &[3, 4, 5]);

        // Some messages should get through (partial partition)
        let mut success_count = 0;
        let mut failure_count = 0;

        for _ in 0..20 {
            let result = network.send(1, 3, b"Hello".to_vec());
            match result {
                Ok(_) => success_count += 1,
                Err(NetworkError::NetworkPartition) => failure_count += 1,
                _ => {}
            }
        }

        // Should have both successes and failures in partial partition
        assert!(success_count > 0);
        assert!(failure_count > 0);
    }

    #[test]
    fn test_custom_latency() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        network.add_node(1);
        network.add_node(2);
        network.add_node(3);

        // Set custom latency between specific nodes
        network.set_latency(1, 2, Duration::from_millis(100));
        network.set_latency(1, 3, Duration::from_millis(500));

        // Check latency values
        assert_eq!(network.latency(1, 2), Duration::from_millis(100));
        assert_eq!(network.latency(1, 3), Duration::from_millis(500));
        assert_eq!(network.latency(2, 3), Duration::from_millis(10)); // Default
    }

    #[test]
    fn test_comprehensive_failure_config() {
        let network = SimulatedNetwork::new(42);

        // Add nodes
        network.add_node(1);
        network.add_node(2);

        // Set comprehensive failure configuration
        let config = NetworkFailureConfig {
            packet_loss_rate: 0.1,
            reorder_rate: 0.2,
            max_reorder_delay: Duration::from_millis(200),
            congestion_rate: 0.15,
            congestion_latency: Duration::from_millis(1000),
        };

        network.set_failure_config(config);

        // Send messages and verify some failures occur
        let mut total_sent = 0;
        let mut dropped = 0;

        for _ in 0..100 {
            total_sent += 1;
            let result = network.send(1, 2, b"Hello".to_vec());
            if matches!(result, Err(NetworkError::MessageDropped)) {
                dropped += 1;
            }
        }

        // Should have some packet loss (approximately 10%)
        assert!(dropped > 0);
        assert!(dropped < total_sent); // Not all should be dropped
    }

    #[test]
    fn test_deterministic_behavior() {
        let network1 = SimulatedNetwork::new(42);
        let network2 = SimulatedNetwork::new(42);

        // Configure both networks identically
        for network in [&network1, &network2] {
            network.add_node(1);
            network.add_node(2);
            network.set_packet_loss_rate(0.5);
        }

        // Send same messages to both networks
        let mut results1 = Vec::new();
        let mut results2 = Vec::new();

        for i in 0..10 {
            let data = format!("Message {}", i).into_bytes();
            results1.push(network1.send(1, 2, data.clone()));
            results2.push(network2.send(1, 2, data));
        }

        // Results should be identical (deterministic based on seed)
        for (r1, r2) in results1.iter().zip(results2.iter()) {
            match (r1, r2) {
                (Ok(_), Ok(_)) => {}
                (Err(e1), Err(e2)) => {
                    // Both should have same error type
                    assert_eq!(std::mem::discriminant(e1), std::mem::discriminant(e2));
                }
                _ => panic!("Results should be identical with same seed"),
            }
        }
    }

    #[test]
    fn test_real_network_basic() {
        let network = RealNetwork::new();

        // Register nodes with addresses
        let addr1: SocketAddr = "127.0.0.1:8001".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:8002".parse().unwrap();

        network.register_node(1, addr1);
        network.register_node(2, addr2);

        // Check if nodes can communicate (should be true if addresses are registered)
        assert!(network.can_communicate(1, 2));
        assert!(network.can_communicate(2, 1));

        // Check latency
        let latency = network.latency(1, 2);
        assert_eq!(latency, Duration::from_millis(50));
    }

    #[test]
    fn test_message_serialization() {
        let network = RealNetwork::new();

        let original_message = Message {
            from: 1,
            to: 2,
            data: b"Hello, World!".to_vec(),
            message_type: "test".to_string(),
        };

        // Serialize and deserialize
        let serialized = network.serialize_message(&original_message).unwrap();
        let deserialized = network.deserialize_message(&serialized).unwrap();

        // Should be identical
        assert_eq!(deserialized.from, original_message.from);
        assert_eq!(deserialized.to, original_message.to);
        assert_eq!(deserialized.data, original_message.data);
        assert_eq!(deserialized.message_type, original_message.message_type);
    }

    #[test]
    fn test_circuit_breaker() {
        let mut breaker = CircuitBreaker::new(3, Duration::from_millis(100));

        // Initially closed
        assert_eq!(breaker.state, CircuitBreakerState::Closed);
        assert!(breaker.can_execute());

        // Fail a few times
        breaker.on_failure();
        breaker.on_failure();
        assert_eq!(breaker.state, CircuitBreakerState::Closed);
        assert!(breaker.can_execute());

        // Third failure should open circuit
        breaker.on_failure();
        assert_eq!(breaker.state, CircuitBreakerState::Open);
        assert!(!breaker.can_execute());

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(150));

        // Should transition to half-open
        assert!(breaker.can_execute());
        assert_eq!(breaker.state, CircuitBreakerState::HalfOpen);

        // Success should close circuit
        breaker.on_success();
        assert_eq!(breaker.state, CircuitBreakerState::Closed);
        assert!(breaker.can_execute());
    }

    #[test]
    fn test_real_network_configuration() {
        let mut network = RealNetwork::with_name("test-network".to_string());

        // Test timeout configuration
        network.set_connection_timeout(Duration::from_secs(10));
        network.set_io_timeouts(Duration::from_secs(60), Duration::from_secs(60));

        // Should not panic and configuration should be stored
        assert_eq!(network.connection_timeout, Duration::from_secs(10));
        assert_eq!(network.read_timeout, Duration::from_secs(60));
        assert_eq!(network.write_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_connection_pool_stats() {
        let network = RealNetwork::new();

        // Initially empty
        let stats = network.connection_stats();
        assert!(stats.is_empty());

        // Circuit breaker states should be empty initially
        let cb_states = network.circuit_breaker_states();
        assert!(cb_states.is_empty());

        // Register a node
        let addr: SocketAddr = "127.0.0.1:8001".parse().unwrap();
        network.register_node(1, addr);

        // Should have circuit breaker for registered node
        let cb_states = network.circuit_breaker_states();
        assert_eq!(cb_states.len(), 1);
        assert_eq!(cb_states.get(&1), Some(&"Closed".to_string()));
    }
}
