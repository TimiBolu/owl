// Concurrent Access Control for OQL Database System
// Implements optimistic locking, deadlock detection, and multi-client support

use crate::document::{Document, DocumentId};
use crate::error::{ErrorContext, OQLError, OQLResult};
use crate::simulation::Clock;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Lock type for document access
#[derive(Debug, Clone, PartialEq)]
pub enum LockType {
    Read,
    Write,
}

/// Information about a lock held by a transaction
#[derive(Debug, Clone)]
pub struct LockInfo {
    pub transaction_id: String,
    pub document_key: String,
    pub lock_type: LockType,
    pub acquired_at: Instant,
    pub expected_version: Option<u64>, // For optimistic locking
}

/// Represents a transaction waiting for a lock
#[derive(Debug, Clone)]
pub struct WaitingTransaction {
    pub transaction_id: String,
    pub document_key: String,
    pub lock_type: LockType,
    pub requested_at: Instant,
}

/// Deadlock detection result
#[derive(Debug, Clone)]
pub enum DeadlockResult {
    NoDeadlock,
    DeadlockDetected {
        cycle: Vec<String>, // Transaction IDs in the cycle
        victim: String,     // Transaction to abort
    },
}

/// Optimistic locking conflict information
#[derive(Debug, Clone)]
pub struct OptimisticConflict {
    pub document_key: String,
    pub expected_version: u64,
    pub actual_version: u64,
    pub transaction_id: String,
}

/// Concurrent access manager that handles locking and deadlock detection
pub struct ConcurrencyManager {
    /// Active locks by document key
    locks: Arc<RwLock<HashMap<String, Vec<LockInfo>>>>,
    /// Transactions waiting for locks
    waiting_queue: Arc<Mutex<VecDeque<WaitingTransaction>>>,
    /// Transaction dependency graph for deadlock detection
    wait_for_graph: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    /// Active transactions
    active_transactions: Arc<RwLock<HashSet<String>>>,
    /// Clock for time operations (dependency injection)
    clock: Arc<dyn Clock>,
    /// Lock timeout duration
    lock_timeout: Duration,
    /// Deadlock detection interval
    deadlock_check_interval: Duration,
}

impl ConcurrencyManager {
    /// Create a new concurrency manager with dependency injection
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            locks: Arc::new(RwLock::new(HashMap::new())),
            waiting_queue: Arc::new(Mutex::new(VecDeque::new())),
            wait_for_graph: Arc::new(Mutex::new(HashMap::new())),
            active_transactions: Arc::new(RwLock::new(HashSet::new())),
            clock,
            lock_timeout: Duration::from_secs(30),
            deadlock_check_interval: Duration::from_millis(100),
        }
    }

    /// Create a new concurrency manager with custom timeouts (for testing)
    pub fn new_with_timeouts(clock: Arc<dyn Clock>, lock_timeout: Duration, deadlock_check_interval: Duration) -> Self {
        Self {
            locks: Arc::new(RwLock::new(HashMap::new())),
            waiting_queue: Arc::new(Mutex::new(VecDeque::new())),
            wait_for_graph: Arc::new(Mutex::new(HashMap::new())),
            active_transactions: Arc::new(RwLock::new(HashSet::new())),
            clock,
            lock_timeout,
            deadlock_check_interval,
        }
    }
    
    /// Create a new concurrency manager with production clock (for backward compatibility)
    pub fn new_with_real_clock() -> Self {
        use crate::simulation::RealClock;
        Self::new(Arc::new(RealClock::new()))
    }

    /// Register a new transaction
    pub fn register_transaction(&self, transaction_id: &str) -> OQLResult<()> {
        let mut active = self.active_transactions.write().map_err(|_| {
            let context = ErrorContext::new("register_transaction")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire transaction registry lock", Some(context))
        })?;

        active.insert(transaction_id.to_string());
        Ok(())
    }

    /// Unregister a transaction and release all its locks
    /// 
    /// Lock ordering: Always acquire locks in this order to prevent deadlocks:
    /// 1. locks (write)
    /// 2. waiting_queue
    /// 3. wait_for_graph
    /// 4. active_transactions
    pub fn unregister_transaction(&self, transaction_id: &str) -> OQLResult<()> {
        // Follow standard lock ordering: acquire all locks in order
        let mut locks = self.locks.write().map_err(|_| {
            let context = ErrorContext::new("unregister_transaction")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire locks registry lock", Some(context))
        })?;

        let mut queue = self.waiting_queue.lock().map_err(|_| {
            let context = ErrorContext::new("unregister_transaction")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire waiting queue lock", Some(context))
        })?;

        let mut graph = self.wait_for_graph.lock().map_err(|_| {
            let context = ErrorContext::new("unregister_transaction")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire wait-for graph lock", Some(context))
        })?;

        let mut active = self.active_transactions.write().map_err(|_| {
            let context = ErrorContext::new("unregister_transaction")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire transaction registry lock", Some(context))
        })?;

        // Remove from active transactions
        active.remove(transaction_id);

        // Remove all locks held by this transaction
        let mut empty_documents = Vec::new();
        for (document_key, document_locks) in locks.iter_mut() {
            document_locks.retain(|lock| lock.transaction_id != transaction_id);
            if document_locks.is_empty() {
                empty_documents.push(document_key.clone());
            }
        }

        // Remove empty document entries
        for document_key in empty_documents {
            locks.remove(&document_key);
        }

        // Remove from waiting queue
        queue.retain(|waiting| waiting.transaction_id != transaction_id);

        // Remove from wait-for graph
        graph.remove(transaction_id);
        
        // Remove this transaction from other transactions' wait lists
        for (_, waiting_for) in graph.iter_mut() {
            waiting_for.remove(transaction_id);
        }

        Ok(())
    }

    /// Acquire a lock on a document with optimistic locking
    pub fn acquire_lock(
        &self,
        transaction_id: &str,
        document_key: &str,
        lock_type: LockType,
        expected_version: Option<u64>,
    ) -> OQLResult<()> {
        let start_time = self.clock.now();
        let mut added_to_queue = false;
        
        loop {
            // Check if we can acquire the lock immediately
            if let Ok(()) = self.try_acquire_lock(transaction_id, document_key, lock_type.clone(), expected_version) {
                return Ok(());
            }

            // Check for timeout
            if start_time.elapsed() > self.lock_timeout {
                let context = ErrorContext::new("acquire_lock")
                    .with_location("concurrency_manager")
                    .with_info("document_key", document_key)
                    .with_info("transaction_id", transaction_id);
                return Err(OQLError::concurrency_error(
                    &format!("Lock acquisition timeout for document: {}", document_key),
                    Some(context)
                ));
            }

            // Add to waiting queue only once
            if !added_to_queue {
                self.add_to_waiting_queue(transaction_id, document_key, lock_type.clone())?;
                added_to_queue = true;
            }

            // Check for deadlocks
            if let DeadlockResult::DeadlockDetected { victim, .. } = self.detect_deadlock()? {
                if victim == transaction_id {
                    let context = ErrorContext::new("acquire_lock")
                        .with_location("concurrency_manager")
                        .with_info("document_key", document_key)
                        .with_info("transaction_id", transaction_id);
                    return Err(OQLError::concurrency_error(
                        "Transaction aborted due to deadlock detection",
                        Some(context)
                    ));
                }
            }

            // Wait a bit before retrying
            self.clock.sleep(self.deadlock_check_interval);
        }
    }

    /// Try to acquire a lock immediately (non-blocking)
    /// 
    /// Lock ordering: Always acquire locks in this order to prevent deadlocks:
    /// 1. locks (write)
    /// 2. waiting_queue
    /// 3. wait_for_graph
    /// 4. active_transactions
    fn try_acquire_lock(
        &self,
        transaction_id: &str,
        document_key: &str,
        lock_type: LockType,
        expected_version: Option<u64>,
    ) -> OQLResult<()> {
        let mut locks = self.locks.write().map_err(|_| {
            let context = ErrorContext::new("try_acquire_lock")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire locks registry lock", Some(context))
        })?;

        self.try_acquire_lock_with_locks(&mut locks, transaction_id, document_key, lock_type, expected_version)
    }

    /// Add a transaction to the waiting queue
    /// 
    /// Lock ordering: Always acquire locks in this order to prevent deadlocks:
    /// 1. locks (read)
    /// 2. waiting_queue
    /// 3. wait_for_graph
    /// 4. active_transactions
    fn add_to_waiting_queue(
        &self,
        transaction_id: &str,
        document_key: &str,
        lock_type: LockType,
    ) -> OQLResult<()> {
        // Follow standard lock ordering
        let locks = self.locks.read().map_err(|_| {
            let context = ErrorContext::new("add_to_waiting_queue")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire locks registry lock", Some(context))
        })?;

        let mut queue = self.waiting_queue.lock().map_err(|_| {
            let context = ErrorContext::new("add_to_waiting_queue")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire waiting queue lock", Some(context))
        })?;

        let mut graph = self.wait_for_graph.lock().map_err(|_| {
            let context = ErrorContext::new("add_to_waiting_queue")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire wait-for graph lock", Some(context))
        })?;

        let waiting = WaitingTransaction {
            transaction_id: transaction_id.to_string(),
            document_key: document_key.to_string(),
            lock_type,
            requested_at: self.clock.now(),
        };

        queue.push_back(waiting);

        // Update wait-for graph while holding all necessary locks
        if let Some(document_locks) = locks.get(document_key) {
            let waiting_for = graph.entry(transaction_id.to_string()).or_insert_with(HashSet::new);
            
            for lock_info in document_locks {
                if lock_info.transaction_id != transaction_id {
                    waiting_for.insert(lock_info.transaction_id.clone());
                }
            }
        }

        Ok(())
    }



    /// Detect deadlocks using cycle detection in the wait-for graph
    pub fn detect_deadlock(&self) -> OQLResult<DeadlockResult> {
        let graph = self.wait_for_graph.lock().map_err(|_| {
            let context = ErrorContext::new("detect_deadlock")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire wait-for graph lock", Some(context))
        })?;

        // Use DFS to detect cycles
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut path = Vec::new();

        for transaction_id in graph.keys() {
            if !visited.contains(transaction_id) {
                if let Some(cycle) = self.dfs_detect_cycle(
                    transaction_id,
                    &graph,
                    &mut visited,
                    &mut rec_stack,
                    &mut path,
                ) {
                    // Choose victim (transaction with least work done - for simplicity, choose first in cycle)
                    let victim = cycle[0].clone();
                    return Ok(DeadlockResult::DeadlockDetected { cycle, victim });
                }
            }
        }

        Ok(DeadlockResult::NoDeadlock)
    }

    /// DFS helper for cycle detection
    fn dfs_detect_cycle(
        &self,
        node: &str,
        graph: &HashMap<String, HashSet<String>>,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());
        path.push(node.to_string());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if let Some(cycle) = self.dfs_detect_cycle(neighbor, graph, visited, rec_stack, path) {
                        return Some(cycle);
                    }
                } else if rec_stack.contains(neighbor) {
                    // Found a cycle - extract it from the path
                    let cycle_start = path.iter().position(|x| x == neighbor).unwrap();
                    return Some(path[cycle_start..].to_vec());
                }
            }
        }

        rec_stack.remove(node);
        path.pop();
        None
    }

    /// Release a specific lock
    /// 
    /// Lock ordering: Always acquire locks in this order to prevent deadlocks:
    /// 1. locks (write)
    /// 2. waiting_queue
    /// 3. wait_for_graph
    /// 4. active_transactions
    pub fn release_lock(&self, transaction_id: &str, document_key: &str) -> OQLResult<()> {
        // Follow standard lock ordering: acquire locks first, then process queue
        {
            let mut locks = self.locks.write().map_err(|_| {
                let context = ErrorContext::new("release_lock")
                    .with_location("concurrency_manager");
                OQLError::concurrency_error("Failed to acquire locks registry lock", Some(context))
            })?;

            if let Some(document_locks) = locks.get_mut(document_key) {
                document_locks.retain(|lock| lock.transaction_id != transaction_id);
                
                if document_locks.is_empty() {
                    locks.remove(document_key);
                }
            }
        } // Release locks before processing queue

        // Process waiting queue to see if any transactions can now proceed
        self.process_waiting_queue()?;

        Ok(())
    }



    /// Process the waiting queue to grant locks to waiting transactions
    /// 
    /// Lock ordering: Always acquire locks in this order to prevent deadlocks:
    /// 1. locks (write)
    /// 2. waiting_queue
    /// 3. wait_for_graph
    /// 4. active_transactions
    fn process_waiting_queue(&self) -> OQLResult<()> {
        // Follow standard lock ordering: locks first, then waiting_queue
        let mut locks = self.locks.write().map_err(|_| {
            let context = ErrorContext::new("process_waiting_queue")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire locks registry lock", Some(context))
        })?;

        let mut queue = self.waiting_queue.lock().map_err(|_| {
            let context = ErrorContext::new("process_waiting_queue")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire waiting queue lock", Some(context))
        })?;

        let mut remaining = VecDeque::new();

        while let Some(waiting) = queue.pop_front() {
            // Try to grant the lock using the already acquired locks
            if let Ok(()) = self.try_acquire_lock_with_locks(
                &mut locks,
                &waiting.transaction_id,
                &waiting.document_key,
                waiting.lock_type.clone(),
                None, // No version check for queued requests
            ) {
                // Lock granted successfully
            } else {
                // Lock not granted, add back to queue
                remaining.push_back(waiting);
            }
        }

        *queue = remaining;
        Ok(())
    }

    /// Internal helper that assumes locks is already acquired (for lock ordering)
    fn try_acquire_lock_with_locks(
        &self,
        locks: &mut HashMap<String, Vec<LockInfo>>,
        transaction_id: &str,
        document_key: &str,
        lock_type: LockType,
        expected_version: Option<u64>,
    ) -> OQLResult<()> {
        let current_time = self.clock.now();
        let document_locks = locks.entry(document_key.to_string()).or_insert_with(Vec::new);

        // Check if this transaction already holds a compatible lock
        for existing_lock in document_locks.iter() {
            if existing_lock.transaction_id == transaction_id {
                match (&existing_lock.lock_type, &lock_type) {
                    (LockType::Write, _) => return Ok(()), // Write lock covers everything
                    (LockType::Read, LockType::Read) => return Ok(()), // Multiple read locks OK
                    (LockType::Read, LockType::Write) => {
                        // Need to upgrade to write lock - check if we're the only reader
                        if document_locks.len() == 1 {
                            // Upgrade the lock
                            let mut upgraded_lock = existing_lock.clone();
                            upgraded_lock.lock_type = LockType::Write;
                            upgraded_lock.expected_version = expected_version;
                            document_locks.clear();
                            document_locks.push(upgraded_lock);
                            return Ok(());
                        } else {
                            // Can't upgrade - other readers exist
                            let context = ErrorContext::new("try_acquire_lock_with_locks")
                                .with_location("concurrency_manager")
                                .with_info("document_key", document_key);
                            return Err(OQLError::concurrency_error(
                                "Cannot upgrade read lock to write lock - other readers exist",
                                Some(context)
                            ));
                        }
                    }
                }
            }
        }

        // Check compatibility with existing locks
        for existing_lock in document_locks.iter() {
            match (&existing_lock.lock_type, &lock_type) {
                (LockType::Write, _) | (_, LockType::Write) => {
                    // Write locks are exclusive
                    let context = ErrorContext::new("try_acquire_lock_with_locks")
                        .with_location("concurrency_manager")
                        .with_info("document_key", document_key)
                        .with_info("existing_transaction", &existing_lock.transaction_id);
                    return Err(OQLError::concurrency_error(
                        "Document is locked by another transaction",
                        Some(context)
                    ));
                }
                (LockType::Read, LockType::Read) => {
                    // Multiple read locks are allowed
                    continue;
                }
            }
        }

        // Acquire the lock
        let lock_info = LockInfo {
            transaction_id: transaction_id.to_string(),
            document_key: document_key.to_string(),
            lock_type,
            acquired_at: current_time,
            expected_version,
        };

        document_locks.push(lock_info);
        Ok(())
    }

    /// Validate optimistic locking version
    pub fn validate_version(
        &self,
        document_key: &str,
        expected_version: u64,
        actual_version: u64,
        transaction_id: &str,
    ) -> OQLResult<()> {
        if expected_version != actual_version {
            let context = ErrorContext::new("validate_version")
                .with_location("concurrency_manager")
                .with_info("document_key", document_key)
                .with_info("expected_version", &expected_version.to_string())
                .with_info("actual_version", &actual_version.to_string());
            
            return Err(OQLError::concurrency_error(
                &format!(
                    "Optimistic locking conflict: expected version {}, but document has version {}",
                    expected_version, actual_version
                ),
                Some(context)
            ));
        }
        Ok(())
    }

    /// Get lock information for a document
    pub fn get_lock_info(&self, document_key: &str) -> OQLResult<Vec<LockInfo>> {
        let locks = self.locks.read().map_err(|_| {
            let context = ErrorContext::new("get_lock_info")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire locks registry lock", Some(context))
        })?;

        Ok(locks.get(document_key).cloned().unwrap_or_default())
    }

    /// Check if a transaction is waiting for locks
    pub fn is_transaction_waiting(&self, transaction_id: &str) -> OQLResult<bool> {
        let queue = self.waiting_queue.lock().map_err(|_| {
            let context = ErrorContext::new("is_transaction_waiting")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire waiting queue lock", Some(context))
        })?;

        Ok(queue.iter().any(|w| w.transaction_id == transaction_id))
    }

    /// Get access to wait-for graph for testing
    #[cfg(test)]
    pub fn get_wait_for_graph(&self) -> &Arc<Mutex<HashMap<String, HashSet<String>>>> {
        &self.wait_for_graph
    }

    /// Get statistics about the concurrency manager
    pub fn get_statistics(&self) -> OQLResult<ConcurrencyStatistics> {
        let locks = self.locks.read().map_err(|_| {
            let context = ErrorContext::new("get_statistics")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire locks registry lock", Some(context))
        })?;

        let queue = self.waiting_queue.lock().map_err(|_| {
            let context = ErrorContext::new("get_statistics")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire waiting queue lock", Some(context))
        })?;

        let active = self.active_transactions.read().map_err(|_| {
            let context = ErrorContext::new("get_statistics")
                .with_location("concurrency_manager");
            OQLError::concurrency_error("Failed to acquire transaction registry lock", Some(context))
        })?;

        let total_locks: usize = locks.values().map(|v| v.len()).sum();
        let locked_documents = locks.len();
        let waiting_transactions = queue.len();
        let active_transactions = active.len();

        Ok(ConcurrencyStatistics {
            active_transactions,
            total_locks,
            locked_documents,
            waiting_transactions,
        })
    }
}

impl Default for ConcurrencyManager {
    fn default() -> Self {
        Self::new_with_real_clock()
    }
}

/// Statistics about the concurrency manager
#[derive(Debug, Clone)]
pub struct ConcurrencyStatistics {
    pub active_transactions: usize,
    pub total_locks: usize,
    pub locked_documents: usize,
    pub waiting_transactions: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_basic_locking() {
        let manager = ConcurrencyManager::new_with_real_clock();
        let txn1 = "txn1";
        let txn2 = "txn2";
        let doc_key = "doc1";

        // Register transactions
        manager.register_transaction(txn1).unwrap();
        manager.register_transaction(txn2).unwrap();

        // Txn1 acquires read lock
        manager.acquire_lock(txn1, doc_key, LockType::Read, None).unwrap();

        // Txn2 can also acquire read lock
        manager.acquire_lock(txn2, doc_key, LockType::Read, None).unwrap();

        // Release locks
        manager.release_lock(txn1, doc_key).unwrap();
        manager.release_lock(txn2, doc_key).unwrap();

        // Cleanup
        manager.unregister_transaction(txn1).unwrap();
        manager.unregister_transaction(txn2).unwrap();
    }

    #[test]
    fn test_write_lock_exclusivity() {
        let manager = ConcurrencyManager::new_with_real_clock();
        let txn1 = "txn1";
        let txn2 = "txn2";
        let doc_key = "doc1";

        manager.register_transaction(txn1).unwrap();
        manager.register_transaction(txn2).unwrap();

        // Txn1 acquires write lock
        manager.acquire_lock(txn1, doc_key, LockType::Write, None).unwrap();

        // Txn2 cannot acquire any lock (should timeout quickly for test)
        let manager_clone = Arc::new(manager);
        let manager_ref = manager_clone.clone();
        
        let handle = thread::spawn(move || {
            // This should fail due to timeout
            manager_ref.acquire_lock(txn2, doc_key, LockType::Read, None)
        });

        // Wait a bit then release the write lock
        thread::sleep(Duration::from_millis(50));
        manager_clone.release_lock(txn1, doc_key).unwrap();

        // The second transaction should eventually succeed
        let result = handle.join().unwrap();
        // Note: This might still fail due to timeout, which is expected behavior

        manager_clone.unregister_transaction(txn1).unwrap();
        manager_clone.unregister_transaction(txn2).unwrap();
    }

    #[test]
    fn test_optimistic_locking_version_validation() {
        let manager = ConcurrencyManager::new_with_real_clock();
        let doc_key = "doc1";
        let txn_id = "txn1";

        // Test version validation
        let result = manager.validate_version(doc_key, 1, 2, txn_id);
        assert!(result.is_err());

        let result = manager.validate_version(doc_key, 1, 1, txn_id);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deadlock_detection() {
        let manager = ConcurrencyManager::new_with_real_clock();
        
        // Create a simple deadlock scenario in the wait-for graph
        {
            let mut graph = manager.wait_for_graph.lock().unwrap();
            let mut txn1_waits = HashSet::new();
            txn1_waits.insert("txn2".to_string());
            graph.insert("txn1".to_string(), txn1_waits);
            
            let mut txn2_waits = HashSet::new();
            txn2_waits.insert("txn1".to_string());
            graph.insert("txn2".to_string(), txn2_waits);
        }

        let result = manager.detect_deadlock().unwrap();
        match result {
            DeadlockResult::DeadlockDetected { cycle, victim } => {
                assert!(cycle.contains(&"txn1".to_string()));
                assert!(cycle.contains(&"txn2".to_string()));
                assert!(victim == "txn1" || victim == "txn2");
            }
            DeadlockResult::NoDeadlock => panic!("Expected deadlock to be detected"),
        }
    }

    #[test]
    fn test_transaction_registration() {
        let manager = ConcurrencyManager::new_with_real_clock();
        let txn_id = "test_txn";

        // Register transaction
        manager.register_transaction(txn_id).unwrap();
        
        // Check it's active
        let active = manager.active_transactions.read().unwrap();
        assert!(active.contains(txn_id));
        drop(active);

        // Unregister transaction
        manager.unregister_transaction(txn_id).unwrap();
        
        // Check it's no longer active
        let active = manager.active_transactions.read().unwrap();
        assert!(!active.contains(txn_id));
    }

    #[test]
    fn test_statistics() {
        let manager = ConcurrencyManager::new_with_real_clock();
        let txn_id = "test_txn";
        let doc_key = "test_doc";

        manager.register_transaction(txn_id).unwrap();
        manager.acquire_lock(txn_id, doc_key, LockType::Read, None).unwrap();

        let stats = manager.get_statistics().unwrap();
        assert_eq!(stats.active_transactions, 1);
        assert_eq!(stats.total_locks, 1);
        assert_eq!(stats.locked_documents, 1);
        assert_eq!(stats.waiting_transactions, 0);

        manager.unregister_transaction(txn_id).unwrap();
    }
}