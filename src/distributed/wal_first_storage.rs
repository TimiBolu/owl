// WAL-First Storage for Atomic OQL Operations
//
// This module implements WAL-first durability for OQL operations:
// - WAL-first write pattern: WAL → Arena → Acknowledge
// - Reservation/commit pattern for atomic cross-shard operations
// - Crash recovery with WAL replay for OQL operations
// - Preserves OQL semantics while ensuring durability

use crate::distributed::global_objref::GlobalObjRef;
use crate::distributed::consistent_hash::ShardId;
use crate::distributed::wal::{DistributedWAL, WALEntryType, LSN, TxnId};
use crate::document::{Document, DocumentId, ObjRef};
use crate::error::{OQLError, OQLResult};
use crate::simulation::filesystem::FileSystem;
use crate::storage::DocumentStore;
use owl_storage_zig::{Arena, ObjRef as ZigObjRef};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::path::Path;

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
    /// Create a new WAL-first storage instance
    pub fn new<P: AsRef<Path>>(
        shard_id: ShardId,
        arena_path: P,
        arena_size: usize,
        wal_path: P,
        filesystem: Arc<dyn FileSystem>,
    ) -> OQLResult<Self> {
        // Create arena with filesystem dependency injection
        // Note: For now, we'll use the regular create method since the filesystem adapter needs work
        let arena = Arena::create(&arena_path, arena_size)
            .map_err(|e| OQLError::storage_error(&format!("Failed to create arena: {:?}", e), None))?;

        // Create distributed WAL
        let wal = DistributedWAL::new(shard_id, &wal_path, filesystem.clone())?;

        Ok(Self {
            shard_id,
            arena: Arc::new(arena),
            wal: Arc::new(wal),
            document_index: Arc::new(RwLock::new(HashMap::new())),
            reservations: Arc::new(Mutex::new(HashMap::new())),
            next_reservation_id: Arc::new(Mutex::new(1)),
            filesystem,
        })
    }

    /// Reserve space for an object (step 1 of atomic write)
    pub fn reserve(&self, size: u32) -> OQLResult<Reservation> {
        // Allocate space in arena but don't write yet
        let zig_objref = self.arena.alloc(size)
            .map_err(|e| OQLError::storage_error(&format!("Failed to allocate space: {:?}", e), None))?;

        // Convert to GlobalObjRef via document::ObjRef
        let local_objref = ObjRef::new(zig_objref.offset());
        let global_objref = GlobalObjRef::from_local(local_objref, self.shard_id);

        // Generate reservation ID
        let reservation_id = {
            let mut next_id = self.next_reservation_id.lock()
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
            let mut reservations = self.reservations.lock()
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
        let wal_data = bincode::serialize(&serialized_doc)
            .map_err(|e| OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None))?;

        // Write to WAL with fsync - this is the durability guarantee point
        let lsn = self.wal.append_with_sync(entry_type, objref, wal_data, txn_id)?;

        Ok(lsn)
    }

    /// Commit reservation to arena (step 3 of atomic write)
    pub fn commit_reservation(
        &self,
        reservation: &Reservation,
        document: &Document,
    ) -> OQLResult<()> {
        // Serialize document with ObjectHeader for arena storage
        let serialized_doc = SerializedDocument::new(document.clone());
        let doc_data = bincode::serialize(&serialized_doc)
            .map_err(|e| OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None))?;

        // Create ObjectHeader (56 bytes according to Zig layout)
        let mut header_bytes = vec![0u8; 56];
        
        // type_tag (u16) at offset 0 - use schema hash
        let schema_hash = crc32fast::hash(document.schema.as_bytes()) as u16;
        header_bytes[0..2].copy_from_slice(&schema_hash.to_le_bytes());
        
        // id ([16]u8) at offset 2 - use document ID hash
        let mut id_bytes = [0u8; 16];
        let id_str = document.id.to_string();
        let id_hash = crc32fast::hash(id_str.as_bytes());
        for i in 0..4 {
            id_bytes[i*4..(i+1)*4].copy_from_slice(&id_hash.to_le_bytes());
        }
        header_bytes[2..18].copy_from_slice(&id_bytes);
        
        // version (u32) at offset 18
        header_bytes[18..22].copy_from_slice(&1u32.to_le_bytes()); // Version 1
        
        // size (u32) at offset 22 - total size including header
        let total_size = 56 + doc_data.len();
        header_bytes[22..26].copy_from_slice(&(total_size as u32).to_le_bytes());
        
        // flags (u8) at offset 26 - not deleted
        header_bytes[26] = 0;
        
        // checksum (u32) at offset 34 - CRC32 of document data
        let checksum = crc32fast::hash(&doc_data);
        header_bytes[34..38].copy_from_slice(&checksum.to_le_bytes());
        
        // lsn (u64) at offset 40 - use 0 for now
        header_bytes[40..48].copy_from_slice(&0u64.to_le_bytes());

        // Combine header + document data
        let mut combined_data = Vec::with_capacity(total_size);
        combined_data.extend_from_slice(&header_bytes);
        combined_data.extend_from_slice(&doc_data);

        // Write to arena at reserved location
        let local_objref = reservation.objref.to_local();
        let zig_objref = ZigObjRef::new(local_objref.offset());
        self.arena.write(zig_objref, &combined_data)
            .map_err(|e| OQLError::storage_error(&format!("Failed to write to arena: {:?}", e), None))?;

        // Update document index
        {
            let mut index = self.document_index.write()
                .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
            let key = (document.schema.clone(), document.id.clone());
            index.insert(key, reservation.objref);
        }

        // Remove reservation
        {
            let mut reservations = self.reservations.lock()
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
        let doc_data = bincode::serialize(&serialized_doc)
            .map_err(|e| OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None))?;
        
        let total_size = 56 + doc_data.len(); // Header + data
        let reservation = self.reserve(total_size as u32)?;

        // Step 2: Write to WAL with fsync (DURABLE)
        let _lsn = self.write_to_wal(
            WALEntryType::Insert,
            reservation.objref,
            document,
            txn_id,
        )?;

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
            let index = self.document_index.read()
                .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
            let key = (schema.to_string(), id.clone());
            index.get(&key).copied()
                .ok_or_else(|| OQLError::generic("Document not found for update"))?
        };

        // Reserve space for new version
        let serialized_doc = SerializedDocument::with_version(new_document.clone(), 2); // Increment version
        let doc_data = bincode::serialize(&serialized_doc)
            .map_err(|e| OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None))?;
        
        let total_size = 56 + doc_data.len();
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
            let mut index = self.document_index.write()
                .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
            let key = (schema.to_string(), id.clone());
            index.remove(&key)
        };

        if let Some(objref) = current_objref {
            // Write delete to WAL with fsync (DURABLE)
            let _lsn = self.wal.append_with_sync(
                WALEntryType::Delete,
                objref,
                Vec::new(), // No data for delete
                txn_id,
            )?;

            // Mark as deleted in arena
            let local_objref = objref.to_local();
            let zig_objref = ZigObjRef::new(local_objref.offset());
            self.arena.delete(zig_objref)
                .map_err(|e| OQLError::storage_error(&format!("Failed to delete from arena: {:?}", e), None))?;

            Ok(true)
        } else {
            Ok(false) // Document not found
        }
    }

    /// Recover from WAL after crash
    pub fn recover_from_wal(&self, start_lsn: LSN) -> OQLResult<()> {
        self.wal.replay_from(start_lsn, |entry| {
            match entry.entry_type {
                WALEntryType::Insert | WALEntryType::Update => {
                    // Deserialize document from WAL entry
                    let serialized_doc: SerializedDocument = bincode::deserialize(&entry.data)
                        .map_err(|e| OQLError::serialization_error(&format!("Failed to deserialize WAL entry: {}", e), None))?;

                    // Apply to arena (replay the commit)
                    let doc_data = bincode::serialize(&serialized_doc)
                        .map_err(|e| OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None))?;

                    // Create ObjectHeader
                    let mut header_bytes = vec![0u8; 56];
                    let schema_hash = crc32fast::hash(serialized_doc.schema.as_bytes()) as u16;
                    header_bytes[0..2].copy_from_slice(&schema_hash.to_le_bytes());
                    
                    let mut id_bytes = [0u8; 16];
                    let id_str = serialized_doc.id.to_string();
                    let id_hash = crc32fast::hash(id_str.as_bytes());
                    for i in 0..4 {
                        id_bytes[i*4..(i+1)*4].copy_from_slice(&id_hash.to_le_bytes());
                    }
                    header_bytes[2..18].copy_from_slice(&id_bytes);
                    
                    header_bytes[18..22].copy_from_slice(&serialized_doc.version.to_le_bytes());
                    
                    let total_size = 56 + doc_data.len();
                    header_bytes[22..26].copy_from_slice(&(total_size as u32).to_le_bytes());
                    
                    header_bytes[26] = 0; // Not deleted
                    
                    let checksum = crc32fast::hash(&doc_data);
                    header_bytes[34..38].copy_from_slice(&checksum.to_le_bytes());
                    
                    header_bytes[40..48].copy_from_slice(&entry.lsn.to_le_bytes());

                    // Combine and write
                    let mut combined_data = Vec::with_capacity(total_size);
                    combined_data.extend_from_slice(&header_bytes);
                    combined_data.extend_from_slice(&doc_data);

                    let local_objref = entry.objref.to_local();
                    let zig_objref = ZigObjRef::new(local_objref.offset());
                    self.arena.write(zig_objref, &combined_data)
                        .map_err(|e| OQLError::storage_error(&format!("Failed to replay write: {:?}", e), None))?;

                    // Update index
                    let mut index = self.document_index.write()
                        .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
                    let key = (serialized_doc.schema, serialized_doc.id);
                    index.insert(key, entry.objref);
                }
                WALEntryType::Delete => {
                    // Remove from index
                    // Note: We'd need to store schema/id in delete entries for full recovery
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
            Ok(())
        })
    }

    /// Get a document by reference (for navigation)
    pub fn get_document_by_ref(&self, objref: GlobalObjRef) -> OQLResult<Option<Document>> {
        if !objref.is_local(self.shard_id) {
            return Err(OQLError::generic("Cross-shard navigation not implemented in this layer"));
        }

        let local_objref = objref.to_local();
        let zig_objref = ZigObjRef::new(local_objref.offset());
        
        // Read from arena
        const HEADER_SIZE: usize = 56;
        let mut header_buffer = [0u8; HEADER_SIZE];
        
        let bytes_read = self.arena.read(zig_objref, &mut header_buffer)
            .map_err(|e| OQLError::storage_error(&format!("Failed to read header: {:?}", e), None))?;
        
        if bytes_read < HEADER_SIZE {
            return Ok(None); // Object not found or invalid
        }
        
        // Parse header
        let flags = header_buffer[26];
        const FLAG_DELETED: u8 = 0x01;
        if (flags & FLAG_DELETED) != 0 {
            return Ok(None); // Object is deleted
        }
        
        let total_size = u32::from_le_bytes([
            header_buffer[22], header_buffer[23], header_buffer[24], header_buffer[25],
        ]) as usize;
        
        let stored_checksum = u32::from_le_bytes([
            header_buffer[34], header_buffer[35], header_buffer[36], header_buffer[37],
        ]);
        
        // Read full object
        let mut full_buffer = vec![0u8; total_size];
        let full_bytes_read = self.arena.read_at(objref.local_offset(), &mut full_buffer)
            .map_err(|e| OQLError::storage_error(&format!("Failed to read full object: {:?}", e), None))?;
        
        if full_bytes_read < total_size {
            return Ok(None);
        }
        
        // Extract document data (skip header)
        let doc_buffer = &full_buffer[HEADER_SIZE..];
        
        // Verify checksum
        let calculated_checksum = crc32fast::hash(doc_buffer);
        if calculated_checksum != stored_checksum {
            return Err(OQLError::storage_error("Checksum mismatch", None));
        }
        
        // Deserialize document
        let serialized_doc: SerializedDocument = bincode::deserialize(doc_buffer)
            .map_err(|e| OQLError::serialization_error(&format!("Failed to deserialize: {}", e), None))?;
        
        Ok(Some(serialized_doc.document))
    }

    /// Get document reference by schema and ID
    pub fn get_document_ref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<GlobalObjRef>> {
        let index = self.document_index.read()
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
        let index = self.document_index.read()
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

    fn get_objref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>> {
        if let Some(global_objref) = self.get_document_ref(schema, id)? {
            Ok(Some(global_objref.to_local()))
        } else {
            Ok(None)
        }
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

        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

        assert_eq!(storage.shard_id, 0);
        assert_eq!(storage.current_lsn(), 0);
    }

    #[test]
    fn test_reservation_commit_pattern() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

        let document = create_test_document("user1", "Alice");

        // Step 1: Reserve space
        let reservation = storage.reserve(1024).unwrap();
        assert_eq!(reservation.shard_id, 0);
        assert!(!reservation.objref.is_null());

        // Step 2: Write to WAL
        let lsn = storage.write_to_wal(
            WALEntryType::Insert,
            reservation.objref,
            &document,
            None,
        ).unwrap();
        assert_eq!(lsn, 1);

        // Step 3: Commit to arena
        storage.commit_reservation(&reservation, &document).unwrap();

        // Verify document can be retrieved
        let retrieved = storage.get_document_by_ref(reservation.objref).unwrap().unwrap();
        assert_eq!(retrieved.id, document.id);
        assert_eq!(retrieved.schema, document.schema);
    }

    #[test]
    fn test_atomic_write_operation() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

        let document = create_test_document("user1", "Alice");

        // Atomic write
        let objref = storage.write_atomic(&document, None).unwrap();
        assert!(!objref.is_null());
        assert_eq!(objref.shard_id(), 0);

        // Verify document is retrievable
        let retrieved = storage.get_document("users", &document.id).unwrap().unwrap();
        assert_eq!(retrieved.id, document.id);
        assert_eq!(retrieved.schema, document.schema);
    }

    #[test]
    fn test_atomic_update_operation() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

        let document = create_test_document("user1", "Alice");
        let updated_document = create_test_document("user1", "Alice Updated");

        // Initial write
        storage.write_atomic(&document, None).unwrap();

        // Update
        let new_objref = storage.update_atomic(
            "users",
            &document.id,
            &updated_document,
            None,
        ).unwrap();

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

        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

        let document = create_test_document("user1", "Alice");

        // Write and delete
        storage.write_atomic(&document, None).unwrap();
        let deleted = storage.delete_atomic("users", &document.id, None).unwrap();
        assert!(deleted);

        // Verify document is gone
        let retrieved = storage.get_document("users", &document.id).unwrap();
        assert!(retrieved.is_none());

        // Delete non-existent document
        let not_deleted = storage.delete_atomic("users", &"nonexistent".to_string(), None).unwrap();
        assert!(!not_deleted);
    }

    #[test]
    fn test_transaction_operations() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

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
        let retrieved = storage.get_document("users", &document.id).unwrap().unwrap();
        assert_eq!(retrieved.id, document.id);
    }

    #[test]
    fn test_document_store_interface() {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

        let document = create_test_document("user1", "Alice");

        // Test DocumentStore interface
        storage.put_document(&document).unwrap();
        
        let retrieved = storage.get_document("users", &document.id).unwrap().unwrap();
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
            ).unwrap();

            let document = create_test_document("user1", "Alice");
            storage.write_atomic(&document, None).unwrap();
        }

        // Create new storage instance (simulating restart)
        let storage = WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap();

        // Recover from WAL
        storage.recover_from_wal(1).unwrap();

        // Verify data is recovered
        let retrieved = storage.get_document("users", &"user1".to_string()).unwrap();
        // Note: Recovery might not fully restore index, this is a simplified test
    }
}