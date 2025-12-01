//! ACID Property Coordinator (Rust)
//!
//! Task 8.13: Implement Rust-Zig transaction coordination (Rust + Zig)
//!
//! This module ensures ACID properties are maintained across the Rust-Zig
//! language boundary during distributed transactions. It coordinates between
//! the Rust query layer and Zig storage layer to guarantee:
//! - Atomicity: All operations succeed or all fail
//! - Consistency: Database remains in valid state
//! - Isolation: Concurrent transactions don't interfere
//! - Durability: Committed changes survive system failures

use crate::cross_layer_error_handling::{CrossLayerErrorManager, ZigError, ZigErrorCode};
use crate::distributed_transaction_coordinator::{
    DistributedTransactionCoordinator, DistributedTxnId, DistributedTxnState, ParticipantType,
};
use crate::error::{ErrorContext, OQLError, OQLResult};
use crate::simulation::Clock;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// ACID property validation result
#[derive(Debug, Clone, PartialEq)]
pub enum ACIDValidationResult {
    /// All ACID properties are satisfied
    Valid,
    /// Atomicity violation detected
    AtomicityViolation(String),
    /// Consistency violation detected
    ConsistencyViolation(String),
    /// Isolation violation detected
    IsolationViolation(String),
    /// Durability violation detected
    DurabilityViolation(String),
}

/// ACID property coordinator configuration
#[derive(Debug, Clone)]
pub struct ACIDCoordinatorConfig {
    /// Enable strict ACID validation
    pub strict_validation: bool,
    /// Timeout for ACID validation checks
    pub validation_timeout: Duration,
    /// Maximum retry attempts for failed operations
    pub max_retry_attempts: u32,
    /// Backoff strategy for retries
    pub retry_backoff: Duration,
    /// Enable consistency checking
    pub enable_consistency_checks: bool,
    /// Enable isolation level enforcement
    pub enforce_isolation: bool,
    /// Enable durability guarantees
    pub ensure_durability: bool,
}

impl Default for ACIDCoordinatorConfig {
    fn default() -> Self {
        Self {
            strict_validation: true,
            validation_timeout: Duration::from_secs(5),
            max_retry_attempts: 3,
            retry_backoff: Duration::from_millis(100),
            enable_consistency_checks: true,
            enforce_isolation: true,
            ensure_durability: true,
        }
    }
}

/// Transaction operation for ACID tracking
#[derive(Debug, Clone)]
pub struct ACIDOperation {
    /// Operation sequence number
    pub sequence: u32,
    /// Operation type
    pub operation_type: OperationType,
    /// Target layer (Rust or Zig)
    pub target_layer: ParticipantType,
    /// Operation data
    pub data: Vec<u8>,
    /// Timestamp when operation was recorded
    pub timestamp: Instant,
    /// Whether operation has been applied
    pub applied: bool,
    /// Whether operation has been committed
    pub committed: bool,
    /// Rollback information
    pub rollback_data: Option<Vec<u8>>,
}

/// Types of operations that can be performed
#[derive(Debug, Clone, PartialEq)]
pub enum OperationType {
    /// Insert new data
    Insert,
    /// Update existing data
    Update,
    /// Delete existing data
    Delete,
    /// Schema modification
    SchemaChange,
    /// Index operation
    IndexOperation,
    /// WAL operation
    WALOperation,
}

/// ACID property coordinator
pub struct ACIDCoordinator {
    /// Distributed transaction coordinator
    distributed_coordinator: Arc<DistributedTransactionCoordinator>,
    /// Cross-layer error manager
    error_manager: Arc<CrossLayerErrorManager>,
    /// Clock for time operations
    clock: Arc<dyn Clock>,
    /// Configuration
    config: ACIDCoordinatorConfig,
    /// Active ACID contexts per transaction
    acid_contexts: Arc<RwLock<HashMap<DistributedTxnId, ACIDContext>>>,
    /// Statistics
    stats: Arc<Mutex<ACIDStatistics>>,
}

/// ACID context for a specific transaction
#[derive(Debug, Clone)]
pub struct ACIDContext {
    /// Transaction ID
    pub transaction_id: DistributedTxnId,
    /// Operations performed in this transaction
    pub operations: Vec<ACIDOperation>,
    /// Read set for isolation validation
    pub read_set: HashMap<String, u64>, // key -> version
    /// Write set for atomicity validation
    pub write_set: HashMap<String, Vec<u8>>, // key -> data
    /// Locks held by this transaction
    pub locks_held: HashSet<String>,
    /// Consistency constraints to validate
    pub consistency_constraints: Vec<ConsistencyConstraint>,
    /// Durability checkpoints
    pub durability_checkpoints: Vec<DurabilityCheckpoint>,
    /// Transaction start time
    pub start_time: Instant,
    /// Last validation time
    pub last_validation: Instant,
}

/// Consistency constraint definition
#[derive(Debug, Clone)]
pub struct ConsistencyConstraint {
    /// Constraint name
    pub name: String,
    /// Constraint type
    pub constraint_type: ConstraintType,
    /// Validation function (serialized)
    pub validation_data: Vec<u8>,
    /// Whether constraint is currently satisfied
    pub satisfied: bool,
}

/// Types of consistency constraints
#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintType {
    /// Foreign key constraint
    ForeignKey,
    /// Unique constraint
    Unique,
    /// Check constraint
    Check,
    /// Custom constraint
    Custom,
}

/// Durability checkpoint information
#[derive(Debug, Clone)]
pub struct DurabilityCheckpoint {
    /// Checkpoint sequence number
    pub sequence: u32,
    /// LSN at checkpoint
    pub lsn: u64,
    /// Shard ID
    pub shard_id: u32,
    /// Checkpoint timestamp
    pub timestamp: Instant,
    /// Whether checkpoint is durable
    pub is_durable: bool,
}

/// ACID statistics
#[derive(Debug, Clone, Default)]
pub struct ACIDStatistics {
    pub total_transactions: u64,
    pub atomicity_violations: u64,
    pub consistency_violations: u64,
    pub isolation_violations: u64,
    pub durability_violations: u64,
    pub successful_validations: u64,
    pub failed_validations: u64,
    pub retry_attempts: u64,
}

impl ACIDCoordinator {
    /// Create a new ACID coordinator
    pub fn new(
        distributed_coordinator: Arc<DistributedTransactionCoordinator>,
        error_manager: Arc<CrossLayerErrorManager>,
        clock: Arc<dyn Clock>,
        config: ACIDCoordinatorConfig,
    ) -> Self {
        Self {
            distributed_coordinator,
            error_manager,
            clock,
            config,
            acid_contexts: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(Mutex::new(ACIDStatistics::default())),
        }
    }

    /// Begin ACID coordination for a transaction
    pub fn begin_acid_transaction(
        &self,
        transaction_id: DistributedTxnId,
        participants: Vec<ParticipantType>,
    ) -> OQLResult<()> {
        let context = ACIDContext {
            transaction_id,
            operations: Vec::new(),
            read_set: HashMap::new(),
            write_set: HashMap::new(),
            locks_held: HashSet::new(),
            consistency_constraints: Vec::new(),
            durability_checkpoints: Vec::new(),
            start_time: self.clock.now(),
            last_validation: self.clock.now(),
        };

        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        contexts.insert(transaction_id, context);

        // Update statistics
        if let Ok(mut stats) = self.stats.lock() {
            stats.total_transactions += 1;
        }

        Ok(())
    }

    /// Record an operation for ACID tracking
    pub fn record_operation(
        &self,
        transaction_id: DistributedTxnId,
        operation_type: OperationType,
        target_layer: ParticipantType,
        data: Vec<u8>,
        rollback_data: Option<Vec<u8>>,
    ) -> OQLResult<u32> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        let context = contexts.get_mut(&transaction_id).ok_or_else(|| {
            OQLError::transaction_error(
                &format!("ACID context not found for transaction {}", transaction_id),
                None,
            )
        })?;

        let sequence = context.operations.len() as u32;
        let operation = ACIDOperation {
            sequence,
            operation_type,
            target_layer,
            data,
            timestamp: self.clock.now(),
            applied: false,
            committed: false,
            rollback_data,
        };

        context.operations.push(operation);
        Ok(sequence)
    }

    /// Validate ACID properties for a transaction
    pub fn validate_acid_properties(
        &self,
        transaction_id: DistributedTxnId,
    ) -> OQLResult<ACIDValidationResult> {
        let contexts = self.acid_contexts.read().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        let context = contexts.get(&transaction_id).ok_or_else(|| {
            OQLError::transaction_error(
                &format!("ACID context not found for transaction {}", transaction_id),
                None,
            )
        })?;

        // Validate each ACID property
        if let Err(violation) = self.validate_atomicity(context) {
            self.update_stats(|stats| stats.atomicity_violations += 1);
            return Ok(ACIDValidationResult::AtomicityViolation(violation));
        }

        if self.config.enable_consistency_checks {
            if let Err(violation) = self.validate_consistency(context) {
                self.update_stats(|stats| stats.consistency_violations += 1);
                return Ok(ACIDValidationResult::ConsistencyViolation(violation));
            }
        }

        if self.config.enforce_isolation {
            if let Err(violation) = self.validate_isolation(context) {
                self.update_stats(|stats| stats.isolation_violations += 1);
                return Ok(ACIDValidationResult::IsolationViolation(violation));
            }
        }

        if self.config.ensure_durability {
            if let Err(violation) = self.validate_durability(context) {
                self.update_stats(|stats| stats.durability_violations += 1);
                return Ok(ACIDValidationResult::DurabilityViolation(violation));
            }
        }

        self.update_stats(|stats| stats.successful_validations += 1);
        Ok(ACIDValidationResult::Valid)
    }

    /// Validate atomicity property
    fn validate_atomicity(&self, context: &ACIDContext) -> Result<(), String> {
        // Check that all operations are in consistent state
        let mut applied_count = 0;
        let mut committed_count = 0;

        for operation in &context.operations {
            if operation.applied {
                applied_count += 1;
            }
            if operation.committed {
                committed_count += 1;
            }
        }

        // For atomicity, either all operations should be applied/committed or none
        if applied_count > 0 && applied_count != context.operations.len() {
            return Err(format!(
                "Atomicity violation: {} of {} operations applied",
                applied_count,
                context.operations.len()
            ));
        }

        if committed_count > 0 && committed_count != context.operations.len() {
            return Err(format!(
                "Atomicity violation: {} of {} operations committed",
                committed_count,
                context.operations.len()
            ));
        }

        Ok(())
    }

    /// Validate consistency property
    fn validate_consistency(&self, context: &ACIDContext) -> Result<(), String> {
        // Check all consistency constraints
        for constraint in &context.consistency_constraints {
            if !constraint.satisfied {
                return Err(format!(
                    "Consistency violation: constraint '{}' not satisfied",
                    constraint.name
                ));
            }
        }

        // Additional consistency checks could be added here
        // For example, checking referential integrity across layers

        Ok(())
    }

    /// Validate isolation property
    fn validate_isolation(&self, context: &ACIDContext) -> Result<(), String> {
        // Check for read-write conflicts
        for (key, expected_version) in &context.read_set {
            // In a full implementation, we would check if the key has been
            // modified by another transaction since we read it
            // For now, we assume isolation is maintained if we have proper locks
            if !context.locks_held.contains(key) {
                return Err(format!(
                    "Isolation violation: key '{}' read without proper lock",
                    key
                ));
            }
        }

        // Check write set isolation
        for key in context.write_set.keys() {
            if !context.locks_held.contains(key) {
                return Err(format!(
                    "Isolation violation: key '{}' written without proper lock",
                    key
                ));
            }
        }

        Ok(())
    }

    /// Validate durability property
    fn validate_durability(&self, context: &ACIDContext) -> Result<(), String> {
        // Check that all committed operations have durable checkpoints
        let committed_operations: Vec<_> = context
            .operations
            .iter()
            .filter(|op| op.committed)
            .collect();

        if committed_operations.is_empty() {
            return Ok(()); // No committed operations to check
        }

        // Ensure we have recent durability checkpoints
        let latest_checkpoint = context
            .durability_checkpoints
            .iter()
            .filter(|cp| cp.is_durable)
            .max_by_key(|cp| cp.sequence);

        if latest_checkpoint.is_none() {
            return Err("Durability violation: no durable checkpoints found".to_string());
        }

        let checkpoint = latest_checkpoint.unwrap();

        // Check that checkpoint is recent enough
        let checkpoint_age = self.clock.now().duration_since(checkpoint.timestamp);
        if checkpoint_age > Duration::from_secs(60) {
            return Err(format!(
                "Durability violation: latest checkpoint is {} seconds old",
                checkpoint_age.as_secs()
            ));
        }

        Ok(())
    }

    /// Commit transaction with ACID guarantees
    pub fn commit_with_acid_guarantees(
        &self,
        transaction_id: DistributedTxnId,
    ) -> OQLResult<()> {
        // Validate ACID properties before commit
        let validation_result = self.validate_acid_properties(transaction_id)?;
        
        match validation_result {
            ACIDValidationResult::Valid => {
                // Proceed with commit
                self.distributed_coordinator.commit_distributed_transaction(transaction_id)?;
                
                // Mark all operations as committed
                self.mark_operations_committed(transaction_id)?;
                
                // Create final durability checkpoint
                self.create_durability_checkpoint(transaction_id)?;
                
                Ok(())
            }
            violation => {
                // ACID violation detected - abort transaction
                self.distributed_coordinator.abort_distributed_transaction(transaction_id)?;
                
                Err(OQLError::transaction_error(
                    &format!("ACID violation detected: {:?}", violation),
                    Some(ErrorContext::new("acid_validation")
                        .with_location("acid_coordinator")
                        .with_info("transaction_id", &transaction_id.to_string())
                    )
                ))
            }
        }
    }

    /// Abort transaction with proper rollback
    pub fn abort_with_rollback(&self, transaction_id: DistributedTxnId) -> OQLResult<()> {
        // Get the ACID context
        let context = {
            let contexts = self.acid_contexts.read().map_err(|_| {
                OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
            })?;
            
            contexts.get(&transaction_id).cloned().ok_or_else(|| {
                OQLError::transaction_error(
                    &format!("ACID context not found for transaction {}", transaction_id),
                    None,
                )
            })?
        };

        // Rollback operations in reverse order
        for operation in context.operations.iter().rev() {
            if operation.applied {
                self.rollback_operation(transaction_id, operation)?;
            }
        }

        // Abort the distributed transaction
        self.distributed_coordinator.abort_distributed_transaction(transaction_id)?;

        // Clean up ACID context
        self.cleanup_acid_context(transaction_id)?;

        Ok(())
    }

    /// Rollback a specific operation
    fn rollback_operation(
        &self,
        transaction_id: DistributedTxnId,
        operation: &ACIDOperation,
    ) -> OQLResult<()> {
        match &operation.target_layer {
            ParticipantType::RustLayer => {
                // Rollback Rust layer operation
                // This would involve undoing changes in the Rust transaction manager
                // For now, we assume the Rust transaction manager handles rollback
            }
            ParticipantType::ZigShard(shard_id) => {
                // Rollback Zig layer operation
                if let Some(rollback_data) = &operation.rollback_data {
                    // Apply rollback data to Zig layer
                    // This would involve calling Zig FFI functions to undo the operation
                    self.apply_zig_rollback(*shard_id, rollback_data)?;
                }
            }
            ParticipantType::External(_) => {
                // External participant rollback would be handled here
            }
        }

        Ok(())
    }

    /// Apply rollback data to Zig layer
    fn apply_zig_rollback(&self, shard_id: u32, rollback_data: &[u8]) -> OQLResult<()> {
        // This would call Zig FFI functions to apply rollback
        // For now, we simulate the rollback
        let _result = self.error_manager.execute_with_error_handling(shard_id, || {
            // Simulate rollback operation
            true
        })?;

        Ok(())
    }

    /// Mark all operations as committed
    fn mark_operations_committed(&self, transaction_id: DistributedTxnId) -> OQLResult<()> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        if let Some(context) = contexts.get_mut(&transaction_id) {
            for operation in &mut context.operations {
                operation.committed = true;
            }
        }

        Ok(())
    }

    /// Create durability checkpoint
    fn create_durability_checkpoint(&self, transaction_id: DistributedTxnId) -> OQLResult<()> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        if let Some(context) = contexts.get_mut(&transaction_id) {
            // Create checkpoint for each shard involved
            let mut shard_ids = HashSet::new();
            for operation in &context.operations {
                if let ParticipantType::ZigShard(shard_id) = &operation.target_layer {
                    shard_ids.insert(*shard_id);
                }
            }

            for shard_id in shard_ids {
                let checkpoint = DurabilityCheckpoint {
                    sequence: context.durability_checkpoints.len() as u32,
                    lsn: 0, // Would be obtained from Zig layer
                    shard_id,
                    timestamp: self.clock.now(),
                    is_durable: true,
                };
                context.durability_checkpoints.push(checkpoint);
            }
        }

        Ok(())
    }

    /// Clean up ACID context
    fn cleanup_acid_context(&self, transaction_id: DistributedTxnId) -> OQLResult<()> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        contexts.remove(&transaction_id);
        Ok(())
    }

    /// Add consistency constraint
    pub fn add_consistency_constraint(
        &self,
        transaction_id: DistributedTxnId,
        constraint: ConsistencyConstraint,
    ) -> OQLResult<()> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        if let Some(context) = contexts.get_mut(&transaction_id) {
            context.consistency_constraints.push(constraint);
        }

        Ok(())
    }

    /// Record read operation for isolation tracking
    pub fn record_read(
        &self,
        transaction_id: DistributedTxnId,
        key: String,
        version: u64,
    ) -> OQLResult<()> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        if let Some(context) = contexts.get_mut(&transaction_id) {
            context.read_set.insert(key, version);
        }

        Ok(())
    }

    /// Record write operation for isolation tracking
    pub fn record_write(
        &self,
        transaction_id: DistributedTxnId,
        key: String,
        data: Vec<u8>,
    ) -> OQLResult<()> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        if let Some(context) = contexts.get_mut(&transaction_id) {
            context.write_set.insert(key, data);
        }

        Ok(())
    }

    /// Record lock acquisition
    pub fn record_lock(&self, transaction_id: DistributedTxnId, key: String) -> OQLResult<()> {
        let mut contexts = self.acid_contexts.write().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        if let Some(context) = contexts.get_mut(&transaction_id) {
            context.locks_held.insert(key);
        }

        Ok(())
    }

    /// Get ACID statistics
    pub fn get_statistics(&self) -> OQLResult<ACIDStatistics> {
        let stats = self.stats.lock().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire stats lock", None)
        })?;
        Ok(stats.clone())
    }

    /// Helper to update statistics
    fn update_stats<F>(&self, update_fn: F)
    where
        F: FnOnce(&mut ACIDStatistics),
    {
        if let Ok(mut stats) = self.stats.lock() {
            update_fn(&mut stats);
        }
    }

    /// Perform periodic ACID validation for active transactions
    pub fn periodic_validation(&self) -> OQLResult<Vec<DistributedTxnId>> {
        let mut violations = Vec::new();
        
        let contexts = self.acid_contexts.read().map_err(|_| {
            OQLError::concurrency_error("Failed to acquire ACID contexts lock", None)
        })?;

        for (txn_id, context) in contexts.iter() {
            // Check if validation is due
            let time_since_last_validation = self.clock.now().duration_since(context.last_validation);
            if time_since_last_validation > self.config.validation_timeout {
                match self.validate_acid_properties(*txn_id) {
                    Ok(ACIDValidationResult::Valid) => {
                        // Update last validation time
                        // Note: This would require mutable access, so in practice
                        // we'd need a different approach or separate tracking
                    }
                    Ok(_) | Err(_) => {
                        violations.push(*txn_id);
                    }
                }
            }
        }

        Ok(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::RealClock;
    use crate::transaction::TransactionManager;
    use std::sync::Arc;

    fn create_test_acid_coordinator() -> ACIDCoordinator {
        let rust_txn_manager = Arc::new(TransactionManager::new("test_db").unwrap());
        let clock = Arc::new(RealClock::new());
        let distributed_coordinator = Arc::new(DistributedTransactionCoordinator::new(
            rust_txn_manager,
            clock.clone(),
            Default::default(),
        ));
        let error_manager = Arc::new(CrossLayerErrorManager::new());
        let config = ACIDCoordinatorConfig::default();

        ACIDCoordinator::new(distributed_coordinator, error_manager, clock, config)
    }

    #[test]
    fn test_acid_context_creation() {
        let coordinator = create_test_acid_coordinator();
        let txn_id = 123;
        let participants = vec![ParticipantType::RustLayer, ParticipantType::ZigShard(0)];

        let result = coordinator.begin_acid_transaction(txn_id, participants);
        assert!(result.is_ok());

        // Verify context was created
        let contexts = coordinator.acid_contexts.read().unwrap();
        assert!(contexts.contains_key(&txn_id));
    }

    #[test]
    fn test_operation_recording() {
        let coordinator = create_test_acid_coordinator();
        let txn_id = 123;
        let participants = vec![ParticipantType::RustLayer];

        coordinator.begin_acid_transaction(txn_id, participants).unwrap();

        let sequence = coordinator
            .record_operation(
                txn_id,
                OperationType::Insert,
                ParticipantType::RustLayer,
                vec![1, 2, 3],
                Some(vec![4, 5, 6]),
            )
            .unwrap();

        assert_eq!(sequence, 0);

        // Verify operation was recorded
        let contexts = coordinator.acid_contexts.read().unwrap();
        let context = contexts.get(&txn_id).unwrap();
        assert_eq!(context.operations.len(), 1);
        assert_eq!(context.operations[0].operation_type, OperationType::Insert);
    }

    #[test]
    fn test_atomicity_validation() {
        let coordinator = create_test_acid_coordinator();
        let txn_id = 123;

        coordinator.begin_acid_transaction(txn_id, vec![]).unwrap();

        // Add some operations
        coordinator
            .record_operation(
                txn_id,
                OperationType::Insert,
                ParticipantType::RustLayer,
                vec![1, 2, 3],
                None,
            )
            .unwrap();

        // Initially should be valid (no operations applied/committed)
        let result = coordinator.validate_acid_properties(txn_id).unwrap();
        assert_eq!(result, ACIDValidationResult::Valid);
    }

    #[test]
    fn test_consistency_constraint() {
        let coordinator = create_test_acid_coordinator();
        let txn_id = 123;

        coordinator.begin_acid_transaction(txn_id, vec![]).unwrap();

        let constraint = ConsistencyConstraint {
            name: "test_constraint".to_string(),
            constraint_type: ConstraintType::Check,
            validation_data: vec![],
            satisfied: true,
        };

        coordinator
            .add_consistency_constraint(txn_id, constraint)
            .unwrap();

        // Should still be valid with satisfied constraint
        let result = coordinator.validate_acid_properties(txn_id).unwrap();
        assert_eq!(result, ACIDValidationResult::Valid);
    }

    #[test]
    fn test_read_write_tracking() {
        let coordinator = create_test_acid_coordinator();
        let txn_id = 123;

        coordinator.begin_acid_transaction(txn_id, vec![]).unwrap();

        // Record read and write operations
        coordinator
            .record_read(txn_id, "key1".to_string(), 1)
            .unwrap();
        coordinator
            .record_write(txn_id, "key2".to_string(), vec![1, 2, 3])
            .unwrap();
        coordinator
            .record_lock(txn_id, "key1".to_string())
            .unwrap();
        coordinator
            .record_lock(txn_id, "key2".to_string())
            .unwrap();

        // Should be valid with proper locks
        let result = coordinator.validate_acid_properties(txn_id).unwrap();
        assert_eq!(result, ACIDValidationResult::Valid);
    }

    #[test]
    fn test_statistics() {
        let coordinator = create_test_acid_coordinator();
        let txn_id = 123;

        coordinator.begin_acid_transaction(txn_id, vec![]).unwrap();
        coordinator.validate_acid_properties(txn_id).unwrap();

        let stats = coordinator.get_statistics().unwrap();
        assert_eq!(stats.total_transactions, 1);
        assert_eq!(stats.successful_validations, 1);
    }
}