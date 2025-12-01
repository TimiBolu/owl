// Transaction Support for OQL Database System
// Provides ACID transaction capabilities with natural syntax
//
// ⚠️ DEPRECATED: This module uses RocksDB for transaction management.
// It will be replaced with Zig MVCC transactions in Phase 4 integration.
// See docs/ROCKSDB_DEPRECATION.md for details.
//
// Note: The Zig transaction layer (MVCC, WAL) is already implemented in Phase 4.
// This module needs to be updated to use the Zig transaction primitives instead
// of RocksDB WriteBatch.
//
// TODO: Integrate with Zig transaction layer (Phase 4 integration)

use crate::concurrency::{ConcurrencyManager, LockType};
use crate::document::{Document, DocumentId};
use crate::error::{OQLError, OQLResult};
use crate::storage::DocumentStore;
// use rocksdb::WriteOptions; // TODO: Implement with Zig transactions
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Transaction isolation levels
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IsolationLevel {
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl Default for IsolationLevel {
    fn default() -> Self {
        IsolationLevel::RepeatableRead
    }
}

/// Transaction state
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionState {
    Active,
    Committed,
    RolledBack,
    Failed,
}

/// Transaction context that tracks operations and state
pub struct TransactionContext {
    pub id: String,
    pub state: TransactionState,
    pub isolation_level: IsolationLevel,
    pub operations: Vec<TransactionOperation>,
    pub read_set: HashMap<String, u64>, // key -> version for optimistic locking
    pub write_set: HashMap<String, Document>, // key -> document for write buffering
}

/// Types of operations that can be performed in a transaction
#[derive(Debug, Clone)]
pub enum TransactionOperation {
    Insert { schema: String, document: Document },
    Update { schema: String, id: DocumentId, old_doc: Option<Document>, new_doc: Document },
    Delete { schema: String, id: DocumentId, old_doc: Option<Document> },
}

impl TransactionContext {
    pub fn new(isolation_level: IsolationLevel) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            state: TransactionState::Active,
            isolation_level,
            operations: Vec::new(),
            read_set: HashMap::new(),
            write_set: HashMap::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.state == TransactionState::Active
    }

    pub fn add_operation(&mut self, operation: TransactionOperation) {
        if self.is_active() {
            self.operations.push(operation);
        }
    }

    pub fn mark_read(&mut self, key: String, version: u64) {
        if self.is_active() {
            self.read_set.insert(key, version);
        }
    }

    pub fn add_write(&mut self, key: String, document: Document) {
        if self.is_active() {
            self.write_set.insert(key, document);
        }
    }
}

/// Transaction manager that handles ACID properties
/// TODO: Reimplement using Zig transaction layer
pub struct TransactionManager {
    // db: Arc<rocksdb::DB>,
    active_transactions: Arc<Mutex<HashMap<String, TransactionContext>>>,
    concurrency_manager: Arc<ConcurrencyManager>,
}

impl TransactionManager {
    /// Create a new transaction manager
    /// TODO: Reimplement with Zig transaction layer
    pub fn new(_db_path: &str) -> OQLResult<Self> {
        Ok(Self {
            // db: Arc::new(db),
            active_transactions: Arc::new(Mutex::new(HashMap::new())),
            concurrency_manager: Arc::new(ConcurrencyManager::new_with_real_clock()),
        })
    }

    /// Begin a new transaction
    pub fn begin_transaction(&self, isolation_level: IsolationLevel) -> OQLResult<String> {
        let context = TransactionContext::new(isolation_level);
        let transaction_id = context.id.clone();

        // Register with concurrency manager
        self.concurrency_manager.register_transaction(&transaction_id)?;

        let mut active_txns = self.active_transactions.lock().map_err(|_| {
            let context = crate::error::ErrorContext::new("begin_transaction")
                .with_location("transaction_manager");
            OQLError::transaction_error(
                "Failed to acquire transaction lock",
                Some(context)
            )
        })?;

        active_txns.insert(transaction_id.clone(), context);
        Ok(transaction_id)
    }

    /// Commit a transaction
    pub fn commit_transaction(&self, transaction_id: &str) -> OQLResult<()> {
        let mut active_txns = self.active_transactions.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire transaction lock".to_string(),
        })?;

        let mut context = active_txns.remove(transaction_id).ok_or_else(|| OQLError::Generic {
            message: format!("Transaction {} not found", transaction_id),
        })?;

        if !context.is_active() {
            return Err(OQLError::Generic {
                message: format!("Transaction {} is not active", transaction_id),
            });
        }

        // Validate optimistic locking for all read operations
        for (key, expected_version) in &context.read_set {
            // Get current version from database
            let cf = self.get_cf_handle("documents")?;
            if let Some(data) = self.db.get_cf(cf, key.as_bytes())? {
                let document: Document = serde_json::from_slice(&data)?;
                self.concurrency_manager.validate_version(
                    key,
                    *expected_version,
                    document.version,
                    transaction_id,
                )?;
            }
        }

        // Use WriteBatch for atomicity
        let mut batch = rocksdb::WriteBatch::default();

        // Apply all operations to the batch
        for operation in &context.operations {
            match operation {
                TransactionOperation::Insert { schema, document } => {
                    let cf = self.get_cf_handle("documents")?;
                    let key = format!("{}#{}", schema, document.id);
                    let data = serde_json::to_vec(&document)?;
                    batch.put_cf(cf, key.as_bytes(), data);
                }
                TransactionOperation::Update { schema, id, new_doc, .. } => {
                    let cf = self.get_cf_handle("documents")?;
                    let key = format!("{}#{}", schema, id);
                    let data = serde_json::to_vec(&new_doc)?;
                    batch.put_cf(cf, key.as_bytes(), data);
                }
                TransactionOperation::Delete { schema, id, .. } => {
                    let cf = self.get_cf_handle("documents")?;
                    let key = format!("{}#{}", schema, id);
                    batch.delete_cf(cf, key.as_bytes());
                }
            }
        }

        // Write the batch atomically
        let write_opts = WriteOptions::default();
        self.db.write_opt(batch, &write_opts).map_err(|e| OQLError::Generic {
            message: format!("Failed to commit transaction: {}", e),
        })?;

        context.state = TransactionState::Committed;

        // Release all locks held by this transaction
        self.concurrency_manager.unregister_transaction(transaction_id)?;

        Ok(())
    }

    /// Rollback a transaction
    pub fn rollback_transaction(&self, transaction_id: &str) -> OQLResult<()> {
        let mut active_txns = self.active_transactions.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire transaction lock".to_string(),
        })?;

        let mut context = active_txns.remove(transaction_id).ok_or_else(|| OQLError::Generic {
            message: format!("Transaction {} not found", transaction_id),
        })?;

        context.state = TransactionState::RolledBack;

        // Release all locks held by this transaction
        self.concurrency_manager.unregister_transaction(transaction_id)?;

        // RocksDB transaction is automatically rolled back when dropped
        Ok(())
    }

    /// Add an operation to a transaction
    pub fn add_operation(&self, transaction_id: &str, operation: TransactionOperation) -> OQLResult<()> {
        let mut active_txns = self.active_transactions.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire transaction lock".to_string(),
        })?;

        let context = active_txns.get_mut(transaction_id).ok_or_else(|| OQLError::Generic {
            message: format!("Transaction {} not found", transaction_id),
        })?;

        if !context.is_active() {
            return Err(OQLError::Generic {
                message: format!("Transaction {} is not active", transaction_id),
            });
        }

        // Add to write set for read-your-own-writes semantics
        match &operation {
            TransactionOperation::Insert { schema, document } => {
                let key = format!("{}#{}", schema, document.id);
                context.add_write(key, document.clone());
            }
            TransactionOperation::Update { schema, id, new_doc, .. } => {
                let key = format!("{}#{}", schema, id);
                context.add_write(key, new_doc.clone());
            }
            TransactionOperation::Delete { schema, id, .. } => {
                let key = format!("{}#{}", schema, id);
                // Remove from write set for deletes
                context.write_set.remove(&key);
            }
        }

        context.add_operation(operation);
        Ok(())
    }

    /// Get a document within a transaction context with locking
    pub fn get_document_in_transaction(
        &self,
        transaction_id: &str,
        schema: &str,
        id: &DocumentId,
    ) -> OQLResult<Option<Document>> {
        let key = format!("{}#{}", schema, id);

        // Acquire read lock
        self.concurrency_manager.acquire_lock(
            transaction_id,
            &key,
            LockType::Read,
            None, // Version will be checked after reading
        )?;

        let mut active_txns = self.active_transactions.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire transaction lock".to_string(),
        })?;

        let context = active_txns.get_mut(transaction_id).ok_or_else(|| OQLError::Generic {
            message: format!("Transaction {} not found", transaction_id),
        })?;

        if !context.is_active() {
            return Err(OQLError::Generic {
                message: format!("Transaction {} is not active", transaction_id),
            });
        }

        // Check write set first (read your own writes)
        if let Some(document) = context.write_set.get(&key) {
            return Ok(Some(document.clone()));
        }

        // Read from database
        let cf = self.get_cf_handle("documents")?;
        match self.db.get_cf(cf, key.as_bytes())? {
            Some(data) => {
                let document: Document = serde_json::from_slice(&data)?;
                
                // Record the version for optimistic locking validation at commit time
                context.mark_read(key, document.version);
                
                Ok(Some(document))
            }
            None => Ok(None),
        }
    }

    /// Update a document within a transaction context with optimistic locking
    pub fn update_document_in_transaction(
        &self,
        transaction_id: &str,
        schema: &str,
        id: &DocumentId,
        new_document: Document,
        expected_version: Option<u64>,
    ) -> OQLResult<()> {
        let key = format!("{}#{}", schema, id);

        // Acquire write lock
        self.concurrency_manager.acquire_lock(
            transaction_id,
            &key,
            LockType::Write,
            expected_version,
        )?;

        // If expected version is provided, validate it
        if let Some(expected_ver) = expected_version {
            let cf = self.get_cf_handle("documents")?;
            if let Some(data) = self.db.get_cf(cf, key.as_bytes())? {
                let current_document: Document = serde_json::from_slice(&data)?;
                self.concurrency_manager.validate_version(
                    &key,
                    expected_ver,
                    current_document.version,
                    transaction_id,
                )?;
            }
        }

        // Get the old document for the operation log
        let old_doc = self.get_document_in_transaction(transaction_id, schema, id)?;

        let operation = TransactionOperation::Update {
            schema: schema.to_string(),
            id: id.clone(),
            old_doc,
            new_doc: new_document.clone(),
        };

        self.add_operation(transaction_id, operation)?;
        Ok(())
    }

    /// Insert a document within a transaction context
    pub fn insert_document_in_transaction(
        &self,
        transaction_id: &str,
        document: Document,
    ) -> OQLResult<()> {
        let key = format!("{}#{}", document.schema, document.id);

        // Acquire write lock for insertion
        self.concurrency_manager.acquire_lock(
            transaction_id,
            &key,
            LockType::Write,
            None, // No version check for new documents
        )?;

        let operation = TransactionOperation::Insert {
            schema: document.schema.clone(),
            document,
        };

        self.add_operation(transaction_id, operation)?;
        Ok(())
    }

    /// Delete a document within a transaction context
    pub fn delete_document_in_transaction(
        &self,
        transaction_id: &str,
        schema: &str,
        id: &DocumentId,
        expected_version: Option<u64>,
    ) -> OQLResult<()> {
        let key = format!("{}#{}", schema, id);

        // Acquire write lock for deletion
        self.concurrency_manager.acquire_lock(
            transaction_id,
            &key,
            LockType::Write,
            expected_version,
        )?;

        // If expected version is provided, validate it
        if let Some(expected_ver) = expected_version {
            let cf = self.get_cf_handle("documents")?;
            if let Some(data) = self.db.get_cf(cf, key.as_bytes())? {
                let current_document: Document = serde_json::from_slice(&data)?;
                self.concurrency_manager.validate_version(
                    &key,
                    expected_ver,
                    current_document.version,
                    transaction_id,
                )?;
            }
        }

        // Get the document to delete for the operation log
        let old_doc = self.get_document_in_transaction(transaction_id, schema, id)?;

        let operation = TransactionOperation::Delete {
            schema: schema.to_string(),
            id: id.clone(),
            old_doc,
        };

        self.add_operation(transaction_id, operation)?;
        Ok(())
    }

    /// Check if a transaction exists and is active
    pub fn is_transaction_active(&self, transaction_id: &str) -> bool {
        if let Ok(active_txns) = self.active_transactions.lock() {
            active_txns.get(transaction_id)
                .map(|ctx| ctx.is_active())
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Get column family handle
    /// TODO: Reimplement with Zig storage
    fn get_cf_handle(&self, _name: &str) -> OQLResult<()> {
        // self.db.cf_handle(name).ok_or_else(|| OQLError::Generic {
        //     message: format!("Column family '{}' not found", name),
        // })
        unimplemented!("Transaction support needs Zig transaction layer integration")
    }
}

/// Transactional document store that wraps operations in transactions
pub struct TransactionalDocumentStore {
    transaction_manager: Arc<TransactionManager>,
    current_transaction: Option<String>,
}

impl TransactionalDocumentStore {
    pub fn new(db_path: &str) -> OQLResult<Self> {
        let transaction_manager = Arc::new(TransactionManager::new(db_path)?);
        Ok(Self {
            transaction_manager,
            current_transaction: None,
        })
    }

    /// Begin a new transaction
    pub fn begin_transaction(&mut self, isolation_level: IsolationLevel) -> OQLResult<String> {
        let transaction_id = self.transaction_manager.begin_transaction(isolation_level)?;
        self.current_transaction = Some(transaction_id.clone());
        Ok(transaction_id)
    }

    /// Commit the current transaction
    pub fn commit_transaction(&mut self) -> OQLResult<()> {
        if let Some(transaction_id) = &self.current_transaction {
            self.transaction_manager.commit_transaction(transaction_id)?;
            self.current_transaction = None;
            Ok(())
        } else {
            Err(OQLError::Generic {
                message: "No active transaction to commit".to_string(),
            })
        }
    }

    /// Rollback the current transaction
    pub fn rollback_transaction(&mut self) -> OQLResult<()> {
        if let Some(transaction_id) = &self.current_transaction {
            self.transaction_manager.rollback_transaction(transaction_id)?;
            self.current_transaction = None;
            Ok(())
        } else {
            Err(OQLError::Generic {
                message: "No active transaction to rollback".to_string(),
            })
        }
    }

    /// Execute a closure within a transaction
    pub fn with_transaction<F, R>(&mut self, isolation_level: IsolationLevel, f: F) -> OQLResult<R>
    where
        F: FnOnce(&mut Self) -> OQLResult<R>,
    {
        self.begin_transaction(isolation_level)?;
        
        match f(self) {
            Ok(result) => {
                self.commit_transaction()?;
                Ok(result)
            }
            Err(error) => {
                let _ = self.rollback_transaction(); // Ignore rollback errors
                Err(error)
            }
        }
    }

    /// Check if currently in a transaction
    pub fn in_transaction(&self) -> bool {
        self.current_transaction.is_some()
    }

    /// Get the current transaction ID
    pub fn current_transaction_id(&self) -> Option<&String> {
        self.current_transaction.as_ref()
    }
}

impl DocumentStore for TransactionalDocumentStore {
    fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>> {
        if let Some(transaction_id) = &self.current_transaction {
            self.transaction_manager.get_document_in_transaction(transaction_id, schema, id)
        } else {
            // Direct read from database when not in transaction
            let cf = self.transaction_manager.get_cf_handle("documents")?;
            let key = format!("{}#{}", schema, id);
            
            match self.transaction_manager.db.get_cf(cf, key.as_bytes())? {
                Some(data) => {
                    let document: Document = serde_json::from_slice(&data)?;
                    Ok(Some(document))
                }
                None => Ok(None),
            }
        }
    }

    fn put_document(&self, document: &Document) -> OQLResult<()> {
        if let Some(transaction_id) = &self.current_transaction {
            let operation = TransactionOperation::Insert {
                schema: document.schema.clone(),
                document: document.clone(),
            };
            self.transaction_manager.add_operation(transaction_id, operation)
        } else {
            // Direct write when not in transaction
            let cf = self.transaction_manager.get_cf_handle("documents")?;
            let key = format!("{}#{}", document.schema, document.id);
            let data = serde_json::to_vec(document)?;
            self.transaction_manager.db.put_cf(cf, key.as_bytes(), data)?;
            Ok(())
        }
    }

    fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool> {
        if let Some(transaction_id) = &self.current_transaction {
            // Get the document first to store in operation log
            let old_doc = self.get_document(schema, id)?;
            let operation = TransactionOperation::Delete {
                schema: schema.to_string(),
                id: id.clone(),
                old_doc,
            };
            self.transaction_manager.add_operation(transaction_id, operation)?;
            Ok(true) // Assume success in transaction context
        } else {
            // Direct delete when not in transaction
            let cf = self.transaction_manager.get_cf_handle("documents")?;
            let key = format!("{}#{}", schema, id);
            
            let exists = self.transaction_manager.db.get_cf(cf, key.as_bytes())?.is_some();
            if exists {
                self.transaction_manager.db.delete_cf(cf, key.as_bytes())?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }

    fn scan_documents(&self, schema: &str) -> OQLResult<crate::storage::DocumentIterator> {
        // For now, scanning is not transactional - it reads committed data
        let cf = self.transaction_manager.get_cf_handle("documents")?;
        let prefix = format!("{}#", schema);
        
        let mut documents = Vec::new();
        let iter = self.transaction_manager.db.prefix_iterator_cf(cf, prefix.as_bytes());
        
        for item in iter {
            let (key, value) = item?;
            let key_str = String::from_utf8_lossy(&key);
            
            if let Some((key_schema, _)) = key_str.split_once('#') {
                if key_schema == schema {
                    let document: Document = serde_json::from_slice(&value)?;
                    documents.push(document);
                } else {
                    break;
                }
            }
        }
        
        Ok(crate::storage::DocumentIterator::new(documents))
    }

    fn get_schema(&self, name: &str) -> OQLResult<Option<crate::schema::Schema>> {
        // Schema operations are not transactional for now
        let cf = self.transaction_manager.get_cf_handle("schemas")?;
        
        match self.transaction_manager.db.get_cf(cf, name.as_bytes())? {
            Some(data) => {
                let schema: crate::schema::Schema = serde_json::from_slice(&data)?;
                Ok(Some(schema))
            }
            None => Ok(None),
        }
    }

    fn put_schema(&self, schema: &crate::schema::Schema) -> OQLResult<()> {
        // Schema operations are not transactional for now
        let cf = self.transaction_manager.get_cf_handle("schemas")?;
        let data = serde_json::to_vec(schema)?;
        
        self.transaction_manager.db.put_cf(cf, schema.name.as_bytes(), data)?;
        Ok(())
    }

    fn list_schemas(&self) -> OQLResult<Vec<String>> {
        // Schema operations are not transactional for now
        let cf = self.transaction_manager.get_cf_handle("schemas")?;
        let mut schemas = Vec::new();
        
        let iter = self.transaction_manager.db.iterator_cf(cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (key, _) = item?;
            let schema_name = String::from_utf8_lossy(&key).to_string();
            schemas.push(schema_name);
        }
        
        Ok(schemas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Value;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_document(id: &str, name: &str) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        Document::with_id(id.to_string(), "test_docs".to_string(), fields)
    }

    #[test]
    fn test_transaction_basic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

        // Test transaction commit
        store.with_transaction(IsolationLevel::RepeatableRead, |store| {
            let doc = create_test_document("doc1", "Test Document");
            store.put_document(&doc)?;
            
            let retrieved = store.get_document("test_docs", &"doc1".to_string())?;
            assert!(retrieved.is_some());
            assert_eq!(retrieved.unwrap().id, "doc1");
            
            Ok(())
        }).unwrap();

        // Verify document persisted after commit
        let retrieved = store.get_document("test_docs", &"doc1".to_string()).unwrap();
        assert!(retrieved.is_some());
    }

    #[test]
    fn test_transaction_rollback() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

        // Test transaction rollback
        let result: OQLResult<()> = store.with_transaction(IsolationLevel::RepeatableRead, |store| {
            let doc = create_test_document("doc2", "Test Document 2");
            store.put_document(&doc)?;
            
            // Force an error to trigger rollback
            Err(OQLError::Generic { message: "Forced error".to_string() })
        });

        assert!(result.is_err());

        // Verify document was not persisted after rollback
        let retrieved = store.get_document("test_docs", &"doc2".to_string()).unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_manual_transaction_control() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = TransactionalDocumentStore::new(temp_dir.path().to_str().unwrap()).unwrap();

        // Manual transaction control
        let txn_id = store.begin_transaction(IsolationLevel::RepeatableRead).unwrap();
        assert!(store.in_transaction());
        assert_eq!(store.current_transaction_id(), Some(&txn_id));

        let doc = create_test_document("doc3", "Manual Transaction");
        store.put_document(&doc).unwrap();

        // Document should be visible within transaction
        let retrieved = store.get_document("test_docs", &"doc3".to_string()).unwrap();
        assert!(retrieved.is_some());

        store.commit_transaction().unwrap();
        assert!(!store.in_transaction());

        // Document should still be visible after commit
        let retrieved = store.get_document("test_docs", &"doc3".to_string()).unwrap();
        assert!(retrieved.is_some());
    }
}