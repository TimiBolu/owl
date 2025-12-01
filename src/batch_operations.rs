//! Batch Operations Module
//!
//! This module provides batch operation support for OQL, enabling efficient
//! bulk inserts, updates, and deletes with atomic semantics and group commit.
//!
//! Key features:
//! - Batch insert with single WAL fsync
//! - Atomic batch semantics (all or nothing)
//! - Batch validation and error handling
//! - Integration with Zig storage layer for batch allocation

use crate::error::{OQLError, OQLResult};
use crate::parser::{InsertBlock, UpdateBlock};
use crate::storage::Document;
use crate::storage::Value;
use std::collections::HashMap;

/// Batch operation configuration
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum number of operations in a single batch
    pub max_batch_size: usize,
    /// Maximum total size of batch in bytes
    pub max_batch_bytes: usize,
    /// Enable atomic batch semantics (all or nothing)
    pub atomic: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 1000,
            max_batch_bytes: 1024 * 1024, // 1MB
            atomic: true,
        }
    }
}

/// Batch operation result
#[derive(Debug)]
pub struct BatchResult {
    /// Number of successful operations
    pub success_count: usize,
    /// Number of failed operations
    pub failure_count: usize,
    /// Total operations attempted
    pub total_count: usize,
    /// Errors encountered (if not atomic)
    pub errors: Vec<OQLError>,
}

impl BatchResult {
    pub fn new(total: usize) -> Self {
        Self {
            success_count: 0,
            failure_count: 0,
            total_count: total,
            errors: Vec::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        self.failure_count == 0
    }
}

/// Batch operation type
#[derive(Debug, Clone)]
pub enum BatchOperation {
    Insert(InsertBlock),
    Update(UpdateBlock),
    Delete(String), // Document ID
}

/// Batch transaction for atomic operations
pub struct BatchTransaction {
    operations: Vec<BatchOperation>,
    config: BatchConfig,
    collection: String,
}

impl BatchTransaction {
    /// Create a new batch transaction
    pub fn new(collection: String, config: BatchConfig) -> Self {
        Self {
            operations: Vec::new(),
            config,
            collection,
        }
    }

    /// Add an insert operation to the batch
    pub fn add_insert(&mut self, insert: InsertBlock) -> OQLResult<()> {
        if self.operations.len() >= self.config.max_batch_size {
            return Err(OQLError::BatchError {
                message: format!(
                    "Batch size limit exceeded: {} operations",
                    self.config.max_batch_size
                ),
            });
        }

        self.operations.push(BatchOperation::Insert(insert));
        Ok(())
    }

    /// Add an update operation to the batch
    pub fn add_update(&mut self, update: UpdateBlock) -> OQLResult<()> {
        if self.operations.len() >= self.config.max_batch_size {
            return Err(OQLError::BatchError {
                message: format!(
                    "Batch size limit exceeded: {} operations",
                    self.config.max_batch_size
                ),
            });
        }

        self.operations.push(BatchOperation::Update(update));
        Ok(())
    }

    /// Add a delete operation to the batch
    pub fn add_delete(&mut self, doc_id: String) -> OQLResult<()> {
        if self.operations.len() >= self.config.max_batch_size {
            return Err(OQLError::BatchError {
                message: format!(
                    "Batch size limit exceeded: {} operations",
                    self.config.max_batch_size
                ),
            });
        }

        self.operations.push(BatchOperation::Delete(doc_id));
        Ok(())
    }

    /// Get the number of operations in the batch
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Check if the batch is empty
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Get the collection name
    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Get the operations
    pub fn operations(&self) -> &[BatchOperation] {
        &self.operations
    }

    /// Validate all operations in the batch
    pub fn validate(&self) -> OQLResult<()> {
        if self.operations.is_empty() {
            return Err(OQLError::BatchError {
                message: "Batch is empty".to_string(),
            });
        }

        // Additional validation can be added here
        // For example, checking field types, required fields, etc.

        Ok(())
    }

    /// Clear all operations from the batch
    pub fn clear(&mut self) {
        self.operations.clear();
    }
}

/// Batch executor for processing batch operations
pub struct BatchExecutor {
    config: BatchConfig,
}

impl BatchExecutor {
    /// Create a new batch executor
    pub fn new(config: BatchConfig) -> Self {
        Self { config }
    }

    /// Create a new batch executor with default configuration
    pub fn default() -> Self {
        Self {
            config: BatchConfig::default(),
        }
    }

    /// Validate a batch of insert operations
    pub fn validate_insert_batch(&self, inserts: &[InsertBlock]) -> OQLResult<()> {
        if inserts.is_empty() {
            return Err(OQLError::BatchError {
                message: "Batch is empty".to_string(),
            });
        }

        if inserts.len() > self.config.max_batch_size {
            return Err(OQLError::BatchError {
                message: format!(
                    "Batch size {} exceeds maximum {}",
                    inserts.len(),
                    self.config.max_batch_size
                ),
            });
        }

        // Validate each insert block has fields
        for (i, insert) in inserts.iter().enumerate() {
            if insert.fields.is_empty() {
                return Err(OQLError::BatchError {
                    message: format!("Insert operation {} has no fields", i),
                });
            }
        }

        Ok(())
    }

    /// Estimate the size of a batch in bytes
    pub fn estimate_batch_size(&self, documents: &[Document]) -> usize {
        documents
            .iter()
            .map(|doc| {
                // Rough estimate: collection name + fields
                doc.collection.len()
                    + doc
                        .fields
                        .iter()
                        .map(|(k, v)| k.len() + self.estimate_value_size(v))
                        .sum::<usize>()
            })
            .sum()
    }

    /// Estimate the size of a value in bytes
    fn estimate_value_size(&self, value: &Value) -> usize {
        match value {
            Value::Null => 1,
            Value::Bool(_) => 1,
            Value::Int(_) => 8,
            Value::Float(_) => 8,
            Value::String(s) => s.len(),
            Value::Array(arr) => arr.iter().map(|v| self.estimate_value_size(v)).sum(),
            Value::Object(obj) => obj
                .iter()
                .map(|(k, v)| k.len() + self.estimate_value_size(v))
                .sum(),
            Value::Reference(_) => 8, // ObjRef size
            Value::DateTime(_) => 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Expression, InsertField, Literal};

    #[test]
    fn test_batch_transaction_creation() {
        let config = BatchConfig::default();
        let batch = BatchTransaction::new("users".to_string(), config);

        assert_eq!(batch.collection(), "users");
        assert_eq!(batch.len(), 0);
        assert!(batch.is_empty());
    }

    #[test]
    fn test_batch_transaction_add_insert() {
        let config = BatchConfig::default();
        let mut batch = BatchTransaction::new("users".to_string(), config);

        let insert = InsertBlock {
            fields: vec![InsertField {
                name: "name".to_string(),
                value: Expression::Literal(Literal::String("Alice".to_string())),
            }],
        };

        batch.add_insert(insert).unwrap();
        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
    }

    #[test]
    fn test_batch_transaction_size_limit() {
        let config = BatchConfig {
            max_batch_size: 2,
            ..Default::default()
        };
        let mut batch = BatchTransaction::new("users".to_string(), config);

        let insert = InsertBlock {
            fields: vec![InsertField {
                name: "name".to_string(),
                value: Expression::Literal(Literal::String("Alice".to_string())),
            }],
        };

        // Add two inserts (should succeed)
        batch.add_insert(insert.clone()).unwrap();
        batch.add_insert(insert.clone()).unwrap();

        // Third insert should fail
        let result = batch.add_insert(insert);
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_transaction_validate() {
        let config = BatchConfig::default();
        let mut batch = BatchTransaction::new("users".to_string(), config);

        // Empty batch should fail validation
        assert!(batch.validate().is_err());

        // Add an insert
        let insert = InsertBlock {
            fields: vec![InsertField {
                name: "name".to_string(),
                value: Expression::Literal(Literal::String("Alice".to_string())),
            }],
        };
        batch.add_insert(insert).unwrap();

        // Now validation should pass
        assert!(batch.validate().is_ok());
    }

    #[test]
    fn test_batch_executor_validate() {
        let executor = BatchExecutor::default();

        // Empty batch should fail
        let empty: Vec<InsertBlock> = vec![];
        assert!(executor.validate_insert_batch(&empty).is_err());

        // Valid batch should pass
        let inserts = vec![InsertBlock {
            fields: vec![InsertField {
                name: "name".to_string(),
                value: Expression::Literal(Literal::String("Alice".to_string())),
            }],
        }];
        assert!(executor.validate_insert_batch(&inserts).is_ok());
    }

    #[test]
    fn test_batch_result() {
        let mut result = BatchResult::new(10);
        assert_eq!(result.total_count, 10);
        assert_eq!(result.success_count, 0);
        assert_eq!(result.failure_count, 0);
        assert!(result.is_success());

        result.success_count = 8;
        result.failure_count = 2;
        assert!(!result.is_success());
    }
}
