// WAL-First Storage for Atomic OQL Operations
//
// This module implements WAL-first durability for OQL operations:
// - WAL-first write pattern: WAL → Arena → Acknowledge
// - Reservation/commit pattern for atomic cross-shard operations
// - Crash recovery with WAL replay for OQL operations
// - Preserves OQL semantics while ensuring durability

use crate::distributed::consistent_hash::ShardId;
use crate::distributed::global_objref::GlobalObjRef;
use crate::distributed::wal::{DistributedWAL, TxnId, WALEntryType, LSN};
use crate::document::{Document, DocumentId, ObjRef};
use crate::error::{OQLError, OQLResult};
use crate::simulation::filesystem::FileSystem;
use crate::storage::DocumentStore;
use owl_storage_zig::{Arena, ObjRef as ZigObjRef};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

/// Adapter between simulation::FileSystem and owl_storage_zig::FileSystem
struct ArenaFileSystemAdapter {
    inner: Arc<dyn crate::simulation::filesystem::FileSystem>,
}

impl ArenaFileSystemAdapter {
    fn new(inner: Arc<dyn crate::simulation::filesystem::FileSystem>) -> Self {
        Self { inner }
    }
}

impl owl_storage_zig::FileSystem for ArenaFileSystemAdapter {
    fn create(&self, path: &Path) -> std::io::Result<Box<dyn owl_storage_zig::FileHandle>> {
        let handle = self.inner.create(path)?;
        Ok(Box::new(ArenaFileHandleAdapter { inner: handle }))
    }

    fn open(&self, path: &Path) -> std::io::Result<Box<dyn owl_storage_zig::FileHandle>> {
        let handle = self.inner.open(path)?;
        Ok(Box::new(ArenaFileHandleAdapter { inner: handle }))
    }

    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        self.inner.remove(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn file_size(&self, path: &Path) -> std::io::Result<u64> {
        self.inner.file_size(path)
    }

    fn mmap(&self, path: &Path, size: usize) -> std::io::Result<(*mut u8, usize)> {
        let mmap = self.inner.mmap(path, size)?;
        let slice = mmap.as_slice();
        Ok((slice.as_ptr() as *mut u8, slice.len()))
    }

    fn munmap(&self, _ptr: *mut u8, _size: usize) -> std::io::Result<()> {
        Ok(())
    }

    fn sync(&self, _path: &Path) -> std::io::Result<()> {
        self.inner.sync_all()
    }
}

struct ArenaFileHandleAdapter {
    inner: Box<dyn crate::simulation::filesystem::FileHandle>,
}

impl owl_storage_zig::FileHandle for ArenaFileHandleAdapter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.inner.write(data)
    }

    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buffer)
    }

    fn seek(&mut self, pos: u64) -> std::io::Result<u64> {
        self.inner.seek(std::io::SeekFrom::Start(pos))
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.inner.sync()
    }

    fn size(&self) -> std::io::Result<u64> {
        self.inner.size()
    }
}

/// Reservation for atomic operations
#[derive(Debug, Clone)]
pub struct Reservation {
    /// Reserved object reference
    pub objref: GlobalObjRef,
    /// Size of the reserved space
    pub size: u32,
    /// Reservation ID for tracking
    pub reservation_id: u64,
    /// Shard where space is reserved
    pub shard_id: ShardId,
}

/// WAL-first storage wrapper for OQL operations
#[allow(dead_code)]
pub struct WALFirstStorage {
    /// Current shard ID
    shard_id: ShardId,
    /// Underlying arena for object storage
    arena: Arc<Arena>,
    /// Distributed WAL for durability
    wal: Arc<DistributedWAL>,
    /// Document index: (schema, id) -> GlobalObjRef
    document_index: Arc<RwLock<HashMap<(String, DocumentId), GlobalObjRef>>>,
    /// Active reservations
    reservations: Arc<Mutex<HashMap<u64, Reservation>>>,
    /// Next reservation ID
    next_reservation_id: Arc<Mutex<u64>>,
    /// FileSystem for dependency injection
    filesystem: Arc<dyn FileSystem>,
}

/// Serialized document for WAL storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedDocument {
    pub schema: String,
    pub id: DocumentId,
    pub document: Document,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeletePayload {
    schema: String,
    id: DocumentId,
}

impl SerializedDocument {
    pub fn new(document: Document) -> Self {
        Self {
            schema: document.schema.clone(),
            id: document.id.clone(),
            document,
            version: 1, // Start with version 1
        }
    }

    pub fn with_version(document: Document, version: u64) -> Self {
        Self {
            schema: document.schema.clone(),
            id: document.id.clone(),
            document,
            version,
        }
    }
}

impl WALFirstStorage {
    const BLOB_HEADER_SIZE: usize = 8; // [u32 payload_len][u32 crc32(payload)]

    /// Create a new WAL-first storage instance
    pub fn new<P: AsRef<Path>>(
        shard_id: ShardId,
        arena_path: P,
        arena_size: usize,
        wal_path: P,
        filesystem: Arc<dyn FileSystem>,
    ) -> OQLResult<Self> {
        // Create arena with filesystem dependency injection.
        let arena_fs = Arc::new(ArenaFileSystemAdapter::new(filesystem.clone()));
        let arena = Arena::create_with_fs(&arena_path, arena_size, arena_fs).map_err(|e| {
            OQLError::storage_error(&format!("Failed to create arena: {:?}", e), None)
        })?;

        // Create distributed WAL
        let wal = DistributedWAL::new(shard_id, &wal_path, filesystem.clone())?;

        let store = Self {
            shard_id,
            arena: Arc::new(arena),
            wal: Arc::new(wal),
            document_index: Arc::new(RwLock::new(HashMap::new())),
            reservations: Arc::new(Mutex::new(HashMap::new())),
            next_reservation_id: Arc::new(Mutex::new(1)),
            filesystem,
        };

        // Startup replay makes WAL-first storage restart-safe by default.
        store.recover_from_wal(1)?;
        Ok(store)
    }

    /// Reserve space for an object (step 1 of atomic write)
    pub fn reserve(&self, size: u32) -> OQLResult<Reservation> {
        // Allocate space in arena but don't write yet
        let zig_objref = self.arena.alloc(size).map_err(|e| {
            OQLError::storage_error(&format!("Failed to allocate space: {:?}", e), None)
        })?;

        // Convert to GlobalObjRef via document::ObjRef
        let local_objref = ObjRef::new(zig_objref.offset());
        let global_objref = GlobalObjRef::from_local(local_objref, self.shard_id);

        // Generate reservation ID
        let reservation_id = {
            let mut next_id = self
                .next_reservation_id
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire reservation ID lock"))?;
            let current_id = *next_id;
            *next_id += 1;
            current_id
        };

        let reservation = Reservation {
            objref: global_objref,
            size,
            reservation_id,
            shard_id: self.shard_id,
        };

        // Track reservation
        {
            let mut reservations = self
                .reservations
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire reservations lock"))?;
            reservations.insert(reservation_id, reservation.clone());
        }

        Ok(reservation)
    }

    /// Write to WAL with fsync (step 2 of atomic write) - DURABLE
    pub fn write_to_wal(
        &self,
        entry_type: WALEntryType,
        objref: GlobalObjRef,
        document: &Document,
        txn_id: Option<TxnId>,
    ) -> OQLResult<LSN> {
        // Serialize document for WAL
        let serialized_doc = SerializedDocument::new(document.clone());
        let wal_data = bincode::serialize(&serialized_doc).map_err(|e| {
            OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None)
        })?;

        // Write to WAL with fsync - this is the durability guarantee point
        let lsn = self
            .wal
            .append_with_sync(entry_type, objref, wal_data, txn_id)?;

        Ok(lsn)
    }

    /// Commit reservation to arena (step 3 of atomic write)
    pub fn commit_reservation(
        &self,
        reservation: &Reservation,
        document: &Document,
    ) -> OQLResult<()> {
        // Rust owns logical document framing, Zig handles allocation/read/write primitives.
        let payload = self.encode_serialized_doc(&SerializedDocument::new(document.clone()))?;

        // Write to arena at reserved location
        let local_objref = reservation.objref.to_local();
        self.arena
            .write_at(local_objref.offset(), &payload)
            .map_err(|e| {
                OQLError::storage_error(&format!("Failed to write to arena: {:?}", e), None)
            })?;

        // Update document index
        {
            let mut index = self
                .document_index
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
            let key = (document.schema.clone(), document.id.clone());
            index.insert(key, reservation.objref);
        }

        // Remove reservation
        {
            let mut reservations = self
                .reservations
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire reservations lock"))?;
            reservations.remove(&reservation.reservation_id);
        }

        Ok(())
    }

    /// Atomic write operation: Reserve → WAL → Commit
    pub fn write_atomic(
        &self,
        document: &Document,
        txn_id: Option<TxnId>,
    ) -> OQLResult<GlobalObjRef> {
        // Step 1: Reserve space (don't write yet)
        let serialized_doc = SerializedDocument::new(document.clone());
        let payload = self.encode_serialized_doc(&serialized_doc)?;
        let total_size = payload.len();
        let reservation = self.reserve(total_size as u32)?;

        // Step 2: Write to WAL with fsync (DURABLE)
        let _lsn = self.write_to_wal(WALEntryType::Insert, reservation.objref, document, txn_id)?;

        // Step 3: Commit to arena (now safe - WAL has it)
        self.commit_reservation(&reservation, document)?;

        Ok(reservation.objref)
    }

    /// Update an existing document atomically
    pub fn update_atomic(
        &self,
        schema: &str,
        id: &DocumentId,
        new_document: &Document,
        txn_id: Option<TxnId>,
    ) -> OQLResult<GlobalObjRef> {
        // Get current document reference
        let current_objref = {
            let index = self
                .document_index
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
            let key = (schema.to_string(), id.clone());
            index
                .get(&key)
                .copied()
                .ok_or_else(|| OQLError::generic("Document not found for update"))?
        };

        // Reserve space for new version
        let serialized_doc = SerializedDocument::with_version(new_document.clone(), 2); // Increment version
        let payload = self.encode_serialized_doc(&serialized_doc)?;
        let total_size = payload.len();
        let reservation = self.reserve(total_size as u32)?;

        // Write to WAL with fsync (DURABLE)
        let _lsn = self.write_to_wal(
            WALEntryType::Update,
            reservation.objref,
            new_document,
            txn_id,
        )?;

        // Commit new version to arena
        self.commit_reservation(&reservation, new_document)?;

        // Mark old version as deleted (best effort)
        let local_objref = current_objref.to_local();
        let zig_objref = ZigObjRef::new(local_objref.offset());
        let _ = self.arena.delete(zig_objref); // Ignore errors for cleanup

        Ok(reservation.objref)
    }

    /// Delete a document atomically
    pub fn delete_atomic(
        &self,
        schema: &str,
        id: &DocumentId,
        txn_id: Option<TxnId>,
    ) -> OQLResult<bool> {
        // Get current document reference
        let current_objref = {
            let mut index = self
                .document_index
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
            let key = (schema.to_string(), id.clone());
            index.remove(&key)
        };

        if let Some(objref) = current_objref {
            // Write delete to WAL with fsync (DURABLE)
            let _lsn = self.wal.append_with_sync(
                WALEntryType::Delete,
                objref,
                bincode::serialize(&DeletePayload {
                    schema: schema.to_string(),
                    id: id.clone(),
                })
                .map_err(|e| {
                    OQLError::serialization_error(
                        &format!("Failed to serialize delete payload: {}", e),
                        None,
                    )
                })?,
                txn_id,
            )?;

            // Mark as deleted in arena
            let local_objref = objref.to_local();
            let zig_objref = ZigObjRef::new(local_objref.offset());
            self.arena.delete(zig_objref).map_err(|e| {
                OQLError::storage_error(&format!("Failed to delete from arena: {:?}", e), None)
            })?;

            Ok(true)
        } else {
            Ok(false) // Document not found
        }
    }

    /// Recover from WAL after crash
    pub fn recover_from_wal(&self, start_lsn: LSN) -> OQLResult<()> {
        let entries = self.wal.entries_from(start_lsn)?;
        let mut committed_txns = HashSet::new();
        let mut rolled_back_txns = HashSet::new();

        for entry in &entries {
            if let Some(txn_id) = entry.txn_id {
                match entry.entry_type {
                    WALEntryType::TxnCommit => {
                        committed_txns.insert(txn_id);
                    }
                    WALEntryType::TxnRollback => {
                        rolled_back_txns.insert(txn_id);
                    }
                    _ => {}
                }
            }
        }

        for entry in entries {
            // Apply only autocommit entries or explicitly committed transactions.
            if let Some(txn_id) = entry.txn_id {
                if rolled_back_txns.contains(&txn_id) || !committed_txns.contains(&txn_id) {
                    continue;
                }
            }

            match entry.entry_type {
                WALEntryType::Insert | WALEntryType::Update => {
                    // Deserialize document from WAL entry
                    let serialized_doc: SerializedDocument = bincode::deserialize(&entry.data)
                        .map_err(|e| {
                            OQLError::serialization_error(
                                &format!("Failed to deserialize WAL entry: {}", e),
                                None,
                            )
                        })?;

                    // Apply to arena (replay the commit)
                    let payload = self.encode_serialized_doc(&serialized_doc)?;
                    let local_objref = entry.objref.to_local();
                    self.arena
                        .write_at(local_objref.offset(), &payload)
                        .map_err(|e| {
                            OQLError::storage_error(
                                &format!("Failed to replay write: {:?}", e),
                                None,
                            )
                        })?;

                    // Update index
                    let mut index = self
                        .document_index
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
                    let key = (serialized_doc.schema, serialized_doc.id);
                    index.insert(key, entry.objref);
                }
                WALEntryType::Delete => {
                    // Remove from index and mark in arena.
                    if !entry.data.is_empty() {
                        let delete_payload: DeletePayload = bincode::deserialize(&entry.data)
                            .map_err(|e| {
                                OQLError::serialization_error(
                                    &format!("Failed to deserialize delete payload: {}", e),
                                    None,
                                )
                            })?;
                        let mut index = self.document_index.write().map_err(|_| {
                            OQLError::generic("Failed to acquire document index lock")
                        })?;
                        index.remove(&(delete_payload.schema, delete_payload.id));
                    }
                    let local_objref = entry.objref.to_local();
                    let zig_objref = ZigObjRef::new(local_objref.offset());
                    let _ = self.arena.delete(zig_objref); // Best effort
                }
                WALEntryType::TxnBegin | WALEntryType::TxnCommit | WALEntryType::TxnRollback => {
                    // Transaction markers - no action needed for recovery
                }
                WALEntryType::Checkpoint => {
                    // Checkpoint marker - no action needed
                }
                WALEntryType::Rebalance => {
                    // Rebalancing operation - would need special handling
                }
            }
        }

        Ok(())
    }

    /// Get a document by reference (for navigation)
    pub fn get_document_by_ref(&self, objref: GlobalObjRef) -> OQLResult<Option<Document>> {
        if !objref.is_local(self.shard_id) {
            return Err(OQLError::generic(
                "Cross-shard navigation not implemented in this layer",
            ));
        }

        let serialized_doc = self.read_serialized_doc(objref.local_offset())?;
        Ok(Some(serialized_doc.document))
    }

    fn encode_serialized_doc(&self, serialized_doc: &SerializedDocument) -> OQLResult<Vec<u8>> {
        let doc_data = bincode::serialize(serialized_doc).map_err(|e| {
            OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None)
        })?;

        let payload_len = doc_data.len() as u32;
        let checksum = crc32fast::hash(&doc_data);
        let mut out = Vec::with_capacity(Self::BLOB_HEADER_SIZE + doc_data.len());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&checksum.to_le_bytes());
        out.extend_from_slice(&doc_data);
        Ok(out)
    }

    fn read_serialized_doc(&self, offset: u64) -> OQLResult<SerializedDocument> {
        let mut header = [0u8; Self::BLOB_HEADER_SIZE];
        let bytes_read = self.arena.read_at(offset, &mut header).map_err(|e| {
            OQLError::storage_error(&format!("Failed to read blob header: {:?}", e), None)
        })?;
        if bytes_read < Self::BLOB_HEADER_SIZE {
            return Err(OQLError::storage_error("Incomplete blob header", None));
        }

        let payload_size =
            u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let expected_checksum = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let total_size = Self::BLOB_HEADER_SIZE + payload_size;

        let mut buffer = vec![0u8; total_size];
        let full_bytes_read = self
            .arena
            .read_at(offset, &mut buffer)
            .map_err(|e| OQLError::storage_error(&format!("Failed to read blob: {:?}", e), None))?;
        if full_bytes_read < total_size {
            return Err(OQLError::storage_error("Incomplete blob payload", None));
        }

        let payload = &buffer[Self::BLOB_HEADER_SIZE..];
        let actual_checksum = crc32fast::hash(payload);
        if actual_checksum != expected_checksum {
            return Err(OQLError::storage_error("Checksum mismatch", None));
        }

        bincode::deserialize(payload).map_err(|e| {
            OQLError::serialization_error(&format!("Failed to deserialize: {}", e), None)
        })
    }

    /// Get document reference by schema and ID
    pub fn get_document_ref(
        &self,
        schema: &str,
        id: &DocumentId,
    ) -> OQLResult<Option<GlobalObjRef>> {
        let index = self
            .document_index
            .read()
            .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
        let key = (schema.to_string(), id.clone());
        Ok(index.get(&key).copied())
    }

    /// Begin a distributed transaction
    pub fn begin_transaction(&self, txn_id: TxnId) -> OQLResult<LSN> {
        self.wal.begin_transaction(txn_id)
    }

    /// Commit a distributed transaction
    pub fn commit_transaction(&self, txn_id: TxnId) -> OQLResult<LSN> {
        self.wal.commit_transaction(txn_id)
    }

    /// Rollback a distributed transaction
    pub fn rollback_transaction(&self, txn_id: TxnId) -> OQLResult<LSN> {
        self.wal.rollback_transaction(txn_id)
    }

    /// Get current WAL LSN
    pub fn current_lsn(&self) -> LSN {
        self.wal.current_lsn()
    }

    /// Force sync WAL
    pub fn sync_wal(&self) -> OQLResult<()> {
        self.wal.sync()
    }
}

/// Implement DocumentStore trait for WAL-first storage
impl DocumentStore for WALFirstStorage {
    fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>> {
        if let Some(objref) = self.get_document_ref(schema, id)? {
            self.get_document_by_ref(objref)
        } else {
            Ok(None)
        }
    }

    fn put_document(&self, document: &Document) -> OQLResult<()> {
        self.write_atomic(document, None)?;
        Ok(())
    }

    fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool> {
        self.delete_atomic(schema, id, None)
    }

    fn scan_documents(&self, schema: &str) -> OQLResult<Vec<Document>> {
        let index = self
            .document_index
            .read()
            .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;

        let mut documents = Vec::new();
        for ((doc_schema, _), &objref) in index.iter() {
            if doc_schema == schema {
                if let Ok(Some(document)) = self.get_document_by_ref(objref) {
                    documents.push(document);
                }
            }
        }

        Ok(documents)
    }

    fn get_schema(&self, _name: &str) -> OQLResult<Option<crate::schema::Schema>> {
        // Schema operations not implemented in this layer
        Ok(None)
    }

    fn put_schema(&self, _schema: &crate::schema::Schema) -> OQLResult<()> {
        // Schema operations not implemented in this layer
        Ok(())
    }

    fn list_schemas(&self) -> OQLResult<Vec<String>> {
        // Schema operations not implemented in this layer
        Ok(Vec::new())
    }

    fn navigate_reference(&self, objref: &ObjRef) -> OQLResult<Option<Document>> {
        let global_objref = GlobalObjRef::from_local(*objref, self.shard_id);
        self.get_document_by_ref(global_objref)
    }

    fn navigate_global_reference(&self, objref: &GlobalObjRef) -> OQLResult<Option<Document>> {
        self.get_document_by_ref(*objref)
    }

    fn get_objref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>> {
        if let Some(global_objref) = self.get_document_ref(schema, id)? {
            Ok(Some(global_objref.to_local()))
        } else {
            Ok(None)
        }
    }

    fn get_global_objref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<GlobalObjRef>> {
        self.get_document_ref(schema, id)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Value;
    use crate::simulation::RealFileSystem;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_document(id: &str, name: &str) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        Document::with_id(id.to_string(), "users".to_string(), fields)
    }

    #[test]
    fn test_wal_first_storage_creation() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        assert_eq!(storage.shard_id, 0);
        assert_eq!(storage.current_lsn(), 0);
    }

    #[test]
    fn test_reservation_commit_pattern() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        let document = create_test_document("user1", "Alice");

        // Step 1: Reserve space
        let reservation = storage.reserve(1024).unwrap();
        assert_eq!(reservation.shard_id, 0);
        assert!(!reservation.objref.is_null());

        // Step 2: Write to WAL
        let lsn = storage
            .write_to_wal(WALEntryType::Insert, reservation.objref, &document, None)
            .unwrap();
        assert_eq!(lsn, 1);

        // Step 3: Commit to arena
        storage.commit_reservation(&reservation, &document).unwrap();

        // Verify document can be retrieved
        let retrieved = storage
            .get_document_by_ref(reservation.objref)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.id, document.id);
        assert_eq!(retrieved.schema, document.schema);
    }

    #[test]
    fn test_atomic_write_operation() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        let document = create_test_document("user1", "Alice");

        // Atomic write
        let objref = storage.write_atomic(&document, None).unwrap();
        assert!(!objref.is_null());
        assert_eq!(objref.shard_id(), 0);

        // Verify document is retrievable
        let retrieved = storage
            .get_document("users", &document.id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.id, document.id);
        assert_eq!(retrieved.schema, document.schema);
    }

    #[test]
    fn test_atomic_update_operation() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        let document = create_test_document("user1", "Alice");
        let updated_document = create_test_document("user1", "Alice Updated");

        // Initial write
        storage.write_atomic(&document, None).unwrap();

        // Update
        let new_objref = storage
            .update_atomic("users", &document.id, &updated_document, None)
            .unwrap();

        // Verify updated document
        let retrieved = storage.get_document_by_ref(new_objref).unwrap().unwrap();
        if let Some(Value::String(name)) = retrieved.fields.get("name") {
            assert_eq!(name, "Alice Updated");
        } else {
            panic!("Expected name field");
        }
    }

    #[test]
    fn test_atomic_delete_operation() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        let document = create_test_document("user1", "Alice");

        // Write and delete
        storage.write_atomic(&document, None).unwrap();
        let deleted = storage.delete_atomic("users", &document.id, None).unwrap();
        assert!(deleted);

        // Verify document is gone
        let retrieved = storage.get_document("users", &document.id).unwrap();
        assert!(retrieved.is_none());

        // Delete non-existent document
        let not_deleted = storage
            .delete_atomic("users", &"nonexistent".to_string(), None)
            .unwrap();
        assert!(!not_deleted);
    }

    #[test]
    fn test_transaction_operations() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        let txn_id = 42;
        let document = create_test_document("user1", "Alice");

        // Begin transaction
        let begin_lsn = storage.begin_transaction(txn_id).unwrap();
        assert_eq!(begin_lsn, 1);

        // Write within transaction
        storage.write_atomic(&document, Some(txn_id)).unwrap();

        // Commit transaction
        let commit_lsn = storage.commit_transaction(txn_id).unwrap();
        assert!(commit_lsn > begin_lsn);

        // Verify document exists
        let retrieved = storage
            .get_document("users", &document.id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.id, document.id);
    }

    #[test]
    fn test_recovery_ignores_uncommitted_transaction_entries() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena_uncommitted.owl");
        let wal_path = temp_dir.path().join("wal_uncommitted.log");
        let filesystem = Arc::new(RealFileSystem::new());

        {
            let storage = WALFirstStorage::new(
                0,
                &arena_path,
                10 * 1024 * 1024,
                &wal_path,
                filesystem.clone(),
            )
            .unwrap();

            let txn_id = 99;
            let document = create_test_document("user_txn", "Temp");
            storage.begin_transaction(txn_id).unwrap();
            storage.write_atomic(&document, Some(txn_id)).unwrap();
            // No commit marker written.
        }

        let restarted =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        restarted.recover_from_wal(1).unwrap();
        let recovered = restarted
            .get_document("users", &"user_txn".to_string())
            .unwrap();
        assert!(recovered.is_none());
    }

    #[test]
    fn test_document_store_interface() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        let document = create_test_document("user1", "Alice");

        // Test DocumentStore interface
        storage.put_document(&document).unwrap();

        let retrieved = storage
            .get_document("users", &document.id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.id, document.id);

        let deleted = storage.delete_document("users", &document.id).unwrap();
        assert!(deleted);

        let not_found = storage.get_document("users", &document.id).unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_wal_recovery() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        // Create storage and write some data
        {
            let storage = WALFirstStorage::new(
                0,
                &arena_path,
                10 * 1024 * 1024,
                &wal_path,
                filesystem.clone(),
            )
            .unwrap();

            let document = create_test_document("user1", "Alice");
            storage.write_atomic(&document, None).unwrap();
        }

        // Create new storage instance (simulating restart)
        let storage =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        // Recover from WAL
        storage.recover_from_wal(1).unwrap();

        // Verify data is recovered
        let retrieved = storage.get_document("users", &"user1".to_string()).unwrap();
        assert!(retrieved.is_some());
    }

    #[test]
    fn test_wal_recovery_restores_delete_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena_del.owl");
        let wal_path = temp_dir.path().join("wal_del.log");
        let filesystem = Arc::new(RealFileSystem::new());

        {
            let storage = WALFirstStorage::new(
                0,
                &arena_path,
                10 * 1024 * 1024,
                &wal_path,
                filesystem.clone(),
            )
            .unwrap();

            let document = create_test_document("user_delete", "Alice");
            storage.write_atomic(&document, None).unwrap();
            assert!(storage.delete_atomic("users", &document.id, None).unwrap());
        }

        let restarted =
            WALFirstStorage::new(0, &arena_path, 10 * 1024 * 1024, &wal_path, filesystem).unwrap();

        let retrieved = restarted
            .get_document("users", &"user_delete".to_string())
            .unwrap();
        assert!(retrieved.is_none());
    }
}
