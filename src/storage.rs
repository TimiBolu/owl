// OQL Storage Layer - Clean Abstraction
//
// This module provides a zero-cost abstraction over storage providers.
// The DocumentStore trait defines the interface, and specific implementations
// (like ZigStore) are adapters that implement this trait.
//
// ## Default Storage: Zig Arena Storage (ZigStore)
//
// As of November 2025, OQL uses a custom Zig-based arena storage engine as the
// default and recommended storage backend. This provides:
//
// - **O(1) Navigation**: Pointer-based navigation in <50ns (10x faster than RocksDB)
// - **Low Memory Overhead**: <1.2x object size (vs 1.5-2x with RocksDB)
// - **Co-location**: Related objects stored in same 4KB page for cache efficiency
// - **Zero-Copy Access**: Memory-mapped arena for direct object access
//
// ## RocksDB Deprecation
//
// RocksDB storage has been deprecated and removed from the codebase. If you have
// existing RocksDB data, please see docs/ROCKSDB_DEPRECATION.md for migration
// instructions and timeline.
//
// The key-value model of RocksDB was fundamentally incompatible with OQL's
// object-graph navigation model, requiring expensive lookups for every reference
// traversal. The Zig storage engine enables true O(1) navigation through physical
// pointers.

use crate::document::{Document, DocumentId, Value};
use crate::distributed::GlobalObjRef;
use crate::error::{OQLError, OQLResult};
use crate::schema::Schema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Document storage interface - zero-cost abstraction
///
/// This trait defines the contract for any storage backend.
/// Implementations should be zero-cost wrappers around their
/// underlying storage engines.
pub trait DocumentStore: Send + Sync {
    /// Get a document by schema and ID
    fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>>;

    /// Store a document
    fn put_document(&self, document: &Document) -> OQLResult<()>;

    /// Delete a document
    fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool>;

    /// Scan all documents in a schema
    fn scan_documents(&self, schema: &str) -> OQLResult<Vec<Document>>;

    /// Get a schema definition
    fn get_schema(&self, name: &str) -> OQLResult<Option<Schema>>;

    /// Store a schema definition
    fn put_schema(&self, schema: &Schema) -> OQLResult<()>;

    /// Delete a schema definition
    fn delete_schema(&self, _name: &str) -> OQLResult<bool> {
        Ok(false)
    }

    /// List all schema names
    fn list_schemas(&self) -> OQLResult<Vec<String>>;

    /// Upsert scalar index metadata for a schema.
    fn upsert_scalar_index(&self, _index: ScalarIndexDefinition) -> OQLResult<()> {
        Err(OQLError::generic(
            "scalar indexes are not supported by this store",
        ))
    }

    /// Delete scalar index metadata by schema + name.
    fn delete_scalar_index(&self, _schema: &str, _name: &str) -> OQLResult<bool> {
        Err(OQLError::generic(
            "scalar indexes are not supported by this store",
        ))
    }

    /// List scalar index metadata, optionally scoped to a schema.
    fn list_scalar_indexes(&self, _schema: Option<&str>) -> OQLResult<Vec<ScalarIndexDefinition>> {
        Ok(Vec::new())
    }

    /// List reverse reference edges pointing at the target document.
    fn list_references_to(
        &self,
        _target_schema: &str,
        _target_id: &DocumentId,
        _source_schema: Option<&str>,
        _field_path: Option<&str>,
    ) -> OQLResult<Vec<ReverseReferenceEntry>> {
        Ok(Vec::new())
    }

    /// Load documents that currently reference the target document.
    fn scan_referencing_documents(
        &self,
        target_schema: &str,
        target_id: &DocumentId,
        source_schema: Option<&str>,
        field_path: Option<&str>,
    ) -> OQLResult<Vec<Document>> {
        let edges = self.list_references_to(target_schema, target_id, source_schema, field_path)?;
        let mut seen = HashSet::new();
        let mut documents = Vec::new();
        for edge in edges {
            let key = (edge.source_schema.clone(), edge.source_id.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(doc) = self.get_document(&key.0, &key.1)? {
                documents.push(doc);
            }
        }
        Ok(documents)
    }

    /// Upsert vector index metadata for a schema.
    fn upsert_vector_index(&self, _index: VectorIndexDefinition) -> OQLResult<()> {
        Err(OQLError::generic(
            "vector indexes are not supported by this store",
        ))
    }

    /// Delete vector index metadata by schema + name.
    fn delete_vector_index(&self, _schema: &str, _name: &str) -> OQLResult<bool> {
        Err(OQLError::generic(
            "vector indexes are not supported by this store",
        ))
    }

    /// List vector index metadata, optionally scoped to a schema.
    fn list_vector_indexes(&self, _schema: Option<&str>) -> OQLResult<Vec<VectorIndexDefinition>> {
        Ok(Vec::new())
    }

    /// Perform ANN search using a storage-native vector index when available.
    ///
    /// Returns `Ok(Some(hits))` when native ANN was used, `Ok(None)` when the
    /// store does not support this query shape and caller should fallback.
    fn vector_ann_search(
        &self,
        _index: &VectorIndexDefinition,
        _documents: &[Document],
        _query: &[f32],
        _k: usize,
        _num_candidates: usize,
    ) -> OQLResult<Option<Vec<VectorAnnHit>>> {
        Ok(None)
    }

    /// Build (or rebuild) a persisted vector index artifact.
    fn build_vector_index(
        &self,
        _schema: &str,
        _name: &str,
        _force_rebuild: bool,
    ) -> OQLResult<VectorIndexBuildReport> {
        Err(OQLError::generic(
            "vector index build is not supported by this store",
        ))
    }

    /// Inspect persisted vector index artifact status.
    fn vector_index_status(
        &self,
        _schema: &str,
        _name: &str,
    ) -> OQLResult<Option<VectorIndexStatus>> {
        Ok(None)
    }

    /// Navigate a reference using O(1) pointer-based navigation
    ///
    /// This is the critical path for OQL's performance advantage:
    /// - Navigation is pointer arithmetic, not a database query
    /// - Executes in <50ns (2-3 memory loads)
    /// - No query planning overhead
    ///
    /// Returns Some(Document) if navigation succeeded, None if reference is null
    /// or points to non-existent document.
    ///
    /// Default implementation returns None (no fast navigation support).
    /// Storage backends that support O(1) navigation should override this.
    fn navigate_reference(&self, _objref: &crate::document::ObjRef) -> OQLResult<Option<Document>> {
        Ok(None) // Default: no fast navigation support
    }

    /// Navigate a shard-aware reference when the store can resolve global references.
    ///
    /// Default implementation only supports local references encoded as shard-local refs.
    fn navigate_global_reference(&self, _objref: &GlobalObjRef) -> OQLResult<Option<Document>> {
        Ok(None)
    }

    /// Get the ObjRef for a document (for creating references)
    ///
    /// This is used to convert document IDs to ObjRefs when inserting reference fields.
    /// Storage backends that support O(1) navigation should implement this.
    fn get_objref(
        &self,
        _schema: &str,
        _id: &crate::document::DocumentId,
    ) -> crate::error::OQLResult<Option<crate::document::ObjRef>> {
        // Default implementation returns None
        Ok(None)
    }

    /// Get the shard-aware object reference for a document when available.
    fn get_global_objref(
        &self,
        _schema: &str,
        _id: &crate::document::DocumentId,
    ) -> crate::error::OQLResult<Option<GlobalObjRef>> {
        Ok(None)
    }

    /// Downcast to concrete type for accessing implementation-specific methods
    ///
    /// This allows tests and advanced use cases to access storage-specific functionality
    /// like getting ObjRefs for creating references between documents.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Type alias for Arc-wrapped DocumentStore
/// This is the standard way to pass storage around the codebase
pub type Storage = Arc<dyn DocumentStore>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScalarIndexDefinition {
    pub name: String,
    pub schema: String,
    pub fields: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReverseReferenceEntry {
    pub source_schema: String,
    pub source_id: DocumentId,
    pub field_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorIndexState {
    Creating,
    Ready,
    Rebuilding,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorIndexType {
    Exact,
    Hnsw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorMetricType {
    Cosine,
    L2,
    Dot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorIndexDefinition {
    pub name: String,
    pub schema: String,
    pub field: String,
    pub dimension: u32,
    pub metric: VectorMetricType,
    pub index_type: VectorIndexType,
    pub quantization: Option<String>,
    pub state: VectorIndexState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorAnnHit {
    pub index: usize,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorIndexBuildReport {
    pub indexed_documents: usize,
    pub skipped_documents: usize,
    pub total_documents: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorIndexStatus {
    pub schema: String,
    pub name: String,
    pub state: VectorIndexState,
    pub file_exists: bool,
    pub file_size_bytes: Option<u64>,
    pub file_mtime_unix_seconds: Option<u64>,
}

/// Create a new storage instance with FileSystem dependency injection
///
/// # Arguments
/// * `path` - Path to the storage file
/// * `size` - Size in bytes (default: 1GB)
/// * `filesystem` - FileSystem implementation for I/O operations
///
/// # Example
/// ```no_run
/// use oql::storage::create_storage_with_fs;
/// use oql::simulation::RealFileSystem;
/// use std::sync::Arc;
///
/// let fs = Arc::new(RealFileSystem::new());
/// let storage = create_storage_with_fs("data.owl", 1024 * 1024 * 1024, fs)?;
/// # Ok::<(), oql::error::OQLError>(())
/// ```
pub fn create_storage_with_fs(
    path: &str,
    size: usize,
    filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
) -> OQLResult<Storage> {
    let store = zig_adapter::ZigStore::new_with_fs(path, size, filesystem)?;
    Ok(Arc::new(store))
}

/// Create a new storage instance with the default backend (Zig) - backward compatibility
///
/// # Arguments
/// * `path` - Path to the storage file
/// * `size` - Size in bytes (default: 1GB)
///
/// # Example
/// ```no_run
/// use oql::storage::create_storage;
///
/// let storage = create_storage("data.owl", 1024 * 1024 * 1024)?;
/// # Ok::<(), oql::error::OQLError>(())
/// ```
pub fn create_storage(path: &str, size: usize) -> OQLResult<Storage> {
    let store = zig_adapter::ZigStore::new(path, size)?;
    Ok(Arc::new(store))
}

/// Zig storage adapter module
///
/// This module contains the adapter that implements DocumentStore
/// using the Zig arena allocator backend.
pub mod zig_adapter {
    use super::*;
    use crate::index_manager::{IndexBackendKind, IndexManager};
    use crate::simulation::RealClock;
    use owl_storage_zig::{
        append_wal_framed, persist_superblock as zig_persist_superblock,
        recover_superblock as zig_recover_superblock, Arena, ObjRef,
    };
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::io::SeekFrom;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, RwLock};

    /// Serialized document with metadata for storage
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct StoredDocument {
        schema: String,
        id: DocumentId,
        document: Document,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum StorageLogEntry {
        TxnBegin {
            txn_id: u64,
        },
        TxnCommit {
            txn_id: u64,
        },
        TxnRollback {
            txn_id: u64,
        },
        Put {
            txn_id: Option<u64>,
            schema: String,
            id: DocumentId,
            objref_offset: u64,
            payload: Vec<u8>,
        },
        Delete {
            txn_id: Option<u64>,
            schema: String,
            id: DocumentId,
        },
        PutSchema {
            txn_id: Option<u64>,
            schema: Schema,
        },
        DeleteSchema {
            txn_id: Option<u64>,
            name: String,
        },
        PutScalarIndex {
            txn_id: Option<u64>,
            index: super::ScalarIndexDefinition,
        },
        DeleteScalarIndex {
            txn_id: Option<u64>,
            schema: String,
            name: String,
        },
        PutVectorIndex {
            txn_id: Option<u64>,
            index: super::VectorIndexDefinition,
        },
        DeleteVectorIndex {
            txn_id: Option<u64>,
            schema: String,
            name: String,
        },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct SuperblockRecord {
        copy: u16,
        sequence: u64,
        arena_size: u64,
        wal_size: u64,
        document_count: u64,
        schema_count: u32,
        parent_crc32: u32,
        record_crc32: u32,
    }

    #[derive(Debug, Clone, Copy)]
    struct SuperblockState {
        sequence: u64,
        last_crc32: u32,
        pending_mutations: u32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SuperblockOpenPolicy {
        Availability,
        Strict,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DurabilityBackend {
        Rust,
        ZigNative,
    }

    /// Adapter between simulation::FileSystem and owl_storage_zig::FileSystem
    struct FileSystemAdapter {
        inner: Arc<dyn crate::simulation::filesystem::FileSystem>,
    }

    impl FileSystemAdapter {
        fn new(inner: Arc<dyn crate::simulation::filesystem::FileSystem>) -> Self {
            Self { inner }
        }
    }

    impl owl_storage_zig::FileSystem for FileSystemAdapter {
        fn create(&self, path: &Path) -> std::io::Result<Box<dyn owl_storage_zig::FileHandle>> {
            let handle = self.inner.create(path)?;
            Ok(Box::new(FileHandleAdapter::new(handle)))
        }

        fn open(&self, path: &Path) -> std::io::Result<Box<dyn owl_storage_zig::FileHandle>> {
            let handle = self.inner.open(path)?;
            Ok(Box::new(FileHandleAdapter::new(handle)))
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
            // Convert Box<dyn MemoryMap> to (*mut u8, usize)
            let mmap = self.inner.mmap(path, size)?;
            let slice = mmap.as_slice();
            let ptr = slice.as_ptr() as *mut u8;
            let size = slice.len();
            // Note: This is a simplified implementation - in production we'd need proper lifetime management
            Ok((ptr, size))
        }

        fn munmap(&self, _ptr: *mut u8, _size: usize) -> std::io::Result<()> {
            // Memory unmapping is handled automatically when MemoryMap is dropped
            Ok(())
        }

        fn sync(&self, _path: &Path) -> std::io::Result<()> {
            // Use sync_all for filesystem-wide sync
            self.inner.sync_all()
        }
    }

    /// Adapter between simulation::FileHandle and owl_storage_zig::FileHandle
    struct FileHandleAdapter {
        inner: Box<dyn crate::simulation::filesystem::FileHandle>,
    }

    impl FileHandleAdapter {
        fn new(inner: Box<dyn crate::simulation::filesystem::FileHandle>) -> Self {
            Self { inner }
        }
    }

    impl owl_storage_zig::FileHandle for FileHandleAdapter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.inner.write(data)
        }

        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buffer)
        }

        fn seek(&mut self, pos: u64) -> std::io::Result<u64> {
            use std::io::SeekFrom;
            self.inner.seek(SeekFrom::Start(pos))
        }

        fn sync(&mut self) -> std::io::Result<()> {
            self.inner.sync()
        }

        fn size(&self) -> std::io::Result<u64> {
            self.inner.size()
        }
    }

    /// Zig Arena storage adapter with WAL-backed metadata and write ordering.
    pub struct ZigStore {
        arena: Arena,
        /// Map from (schema, id) to ObjRef for fast lookups
        document_index: RwLock<HashMap<(String, DocumentId), ObjRef>>,
        /// Reverse reference index keyed by target (schema, id)
        reverse_references:
            RwLock<HashMap<(String, DocumentId), Vec<super::ReverseReferenceEntry>>>,
        /// Schema registry
        schemas: RwLock<HashMap<String, Schema>>,
        /// Scalar index metadata catalog keyed by schema
        scalar_indexes: RwLock<HashMap<String, Vec<super::ScalarIndexDefinition>>>,
        /// Live scalar index manager hydrated from storage-owned metadata
        scalar_index_manager: RwLock<Option<Arc<IndexManager>>>,
        /// Vector index metadata catalog keyed by schema
        vector_indexes: RwLock<HashMap<String, Vec<super::VectorIndexDefinition>>>,
        /// FileSystem for explicit durability boundaries
        filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
        /// WAL file for metadata and recovery
        wal_file: Mutex<Box<dyn crate::simulation::filesystem::FileHandle>>,
        /// Path to WAL sidecar file
        wal_path: PathBuf,
        /// Path to superblock copyset sidecar
        superblock_path: PathBuf,
        /// Directory for persisted vector index files
        vector_index_dir: PathBuf,
        /// Superblock write sequence state
        superblock_state: Mutex<SuperblockState>,
        /// Arena size in bytes
        arena_size_bytes: usize,
        /// Durability backend mode
        durability_backend: DurabilityBackend,
        /// Active local transaction id for thread-safe script transaction context
        active_txn_id: Mutex<Option<u64>>,
        /// Monotonic local transaction ID generator
        next_txn_id: AtomicU64,
    }

    impl ZigStore {
        const BLOB_HEADER_SIZE: usize = 8;
        const WAL_MAGIC: u32 = 0x4c41574f; // "OWAL" little-endian
        const WAL_VERSION: u8 = 1;
        const WAL_FRAME_META_SIZE: usize = 12; // version(1)+flags(1)+reserved(2)+len(4)+crc32(4)
        const WAL_ENTRY_SIZE_MAX: usize = 64 * 1024 * 1024;
        const SUPERBLOCK_MAGIC: u32 = 0x3142534f; // "OSB1"
        const SUPERBLOCK_VERSION: u16 = 1;
        const SUPERBLOCK_COPIES: usize = 4;
        const SUPERBLOCK_RECORD_SIZE: usize = 128;
        const SUPERBLOCK_MUTATION_INTERVAL: u32 = 128;

        /// Create a new ZigStore with FileSystem dependency injection
        pub fn new_with_fs<P: AsRef<Path>>(
            path: P,
            size: usize,
            filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
        ) -> OQLResult<Self> {
            let policy = Self::superblock_policy_from_env();
            Self::new_with_fs_policy(path, size, filesystem, policy, DurabilityBackend::Rust)
        }

        fn new_with_fs_policy<P: AsRef<Path>>(
            path: P,
            size: usize,
            filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
            policy: SuperblockOpenPolicy,
            durability_backend: DurabilityBackend,
        ) -> OQLResult<Self> {
            let wal_path = PathBuf::from(format!("{}.wal", path.as_ref().display()));
            let superblock_path = PathBuf::from(format!("{}.sb", path.as_ref().display()));
            let vector_index_dir = PathBuf::from(format!("{}.vindex", path.as_ref().display()));
            let arena_path = path.as_ref().to_path_buf();
            let recovered_superblock = match durability_backend {
                DurabilityBackend::ZigNative => {
                    zig_recover_superblock(&superblock_path, policy == SuperblockOpenPolicy::Strict)
                        .map_err(|e| {
                            OQLError::storage_error(
                                &format!(
                                    "Failed to recover storage superblock '{}': {}",
                                    superblock_path.display(),
                                    e
                                ),
                                None,
                            )
                        })?
                }
                DurabilityBackend::Rust => {
                    Self::recover_working_superblock(filesystem.clone(), &superblock_path, policy)?
                        .map(|r| owl_storage_zig::SuperblockRecord {
                            sequence: r.sequence,
                            arena_size: r.arena_size,
                            wal_size: r.wal_size,
                            document_count: r.document_count,
                            schema_count: r.schema_count,
                            record_crc32: r.record_crc32,
                        })
                }
            };
            let arena_size_bytes = recovered_superblock
                .map(|sb| sb.arena_size as usize)
                .filter(|s| *s > 0)
                .unwrap_or(size);
            let zig_fs = Arc::new(FileSystemAdapter::new(filesystem.clone()));
            let arena = match Arena::create_with_fs(&arena_path, arena_size_bytes, zig_fs) {
                Ok(arena) => arena,
                Err(open_err) => {
                    // Crash hardening: if arena header is invalid but WAL exists, recreate arena and replay WAL.
                    if filesystem.exists(&arena_path) && filesystem.exists(&wal_path) {
                        let _ = filesystem.remove(&arena_path);
                        let zig_fs_retry = Arc::new(FileSystemAdapter::new(filesystem.clone()));
                        Arena::create_with_fs(&arena_path, arena_size_bytes, zig_fs_retry).map_err(
                            |retry_err| {
                                OQLError::storage_error(
                                    &format!(
                                    "Failed to create arena: {:?}; recovery recreate failed: {:?}",
                                    open_err, retry_err
                                ),
                                    None,
                                )
                            },
                        )?
                    } else {
                        return Err(OQLError::storage_error(
                            &format!("Failed to create arena: {:?}", open_err),
                            None,
                        ));
                    }
                }
            };
            let wal_file = Self::open_wal_for_rw(filesystem.clone(), &wal_path)?;

            let store = Self {
                arena,
                document_index: RwLock::new(HashMap::new()),
                reverse_references: RwLock::new(HashMap::new()),
                schemas: RwLock::new(HashMap::new()),
                scalar_indexes: RwLock::new(HashMap::new()),
                scalar_index_manager: RwLock::new(None),
                vector_indexes: RwLock::new(HashMap::new()),
                filesystem,
                wal_file: Mutex::new(wal_file),
                wal_path,
                superblock_path,
                vector_index_dir,
                superblock_state: Mutex::new(SuperblockState {
                    sequence: recovered_superblock.map_or(0, |s| s.sequence),
                    last_crc32: recovered_superblock.map_or(0, |s| s.record_crc32),
                    pending_mutations: 0,
                }),
                arena_size_bytes,
                durability_backend,
                active_txn_id: Mutex::new(None),
                next_txn_id: AtomicU64::new(1),
            };

            store.recover_from_wal()?;
            store.refresh_runtime_reverse_references()?;
            store.refresh_runtime_scalar_indexes()?;
            store.persist_superblock(true)?;
            Ok(store)
        }

        fn stable_vector_id(doc_id: &str) -> u64 {
            // FNV-1a 64-bit for deterministic ID mapping across process restarts.
            let mut hash: u64 = 0xcbf29ce484222325;
            for b in doc_id.as_bytes() {
                hash ^= *b as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash
        }

        fn sanitize_name(name: &str) -> String {
            let mut out = String::with_capacity(name.len());
            for ch in name.chars() {
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                    out.push(ch);
                } else {
                    out.push('_');
                }
            }
            out
        }

        fn vector_index_file_path(&self, index: &super::VectorIndexDefinition) -> PathBuf {
            let schema = Self::sanitize_name(&index.schema);
            let name = Self::sanitize_name(&index.name);
            self.vector_index_dir
                .join(format!("{}__{}.hnsw", schema, name))
        }

        fn vector_index_file_path_by_name(&self, schema: &str, name: &str) -> PathBuf {
            let schema = Self::sanitize_name(schema);
            let name = Self::sanitize_name(name);
            self.vector_index_dir
                .join(format!("{}__{}.hnsw", schema, name))
        }

        fn value_to_vector(value: &Value) -> Option<Vec<f32>> {
            match value {
                Value::List(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        match item {
                            Value::Float(v) => out.push(*v as f32),
                            Value::Integer(v) => out.push(*v as f32),
                            _ => return None,
                        }
                    }
                    Some(out)
                }
                _ => None,
            }
        }

        fn collect_reference_edges_from_value(
            &self,
            source_schema: &str,
            source_id: &str,
            path: &str,
            value: &Value,
            out: &mut Vec<((String, DocumentId), super::ReverseReferenceEntry)>,
        ) {
            match value {
                Value::Reference(objref) => {
                    if objref.is_null() {
                        return;
                    }
                    let doc_objref = crate::document::ObjRef::new(objref.offset());
                    if let Ok(Some(target)) = self.navigate_reference(&doc_objref) {
                        out.push((
                            (target.schema.clone(), target.id.clone()),
                            super::ReverseReferenceEntry {
                                source_schema: source_schema.to_string(),
                                source_id: source_id.to_string(),
                                field_path: path.to_string(),
                            },
                        ));
                    }
                }
                Value::List(items) => {
                    for item in items {
                        self.collect_reference_edges_from_value(
                            source_schema,
                            source_id,
                            path,
                            item,
                            out,
                        );
                    }
                }
                Value::Object(fields) => {
                    for (child, child_value) in fields {
                        let next_path = if path.is_empty() {
                            child.clone()
                        } else {
                            format!("{}.{}", path, child)
                        };
                        self.collect_reference_edges_from_value(
                            source_schema,
                            source_id,
                            &next_path,
                            child_value,
                            out,
                        );
                    }
                }
                _ => {}
            }
        }

        fn outgoing_reference_edges_for_document(
            &self,
            document: &Document,
        ) -> Vec<((String, DocumentId), super::ReverseReferenceEntry)> {
            let mut edges = Vec::new();
            for (field, value) in &document.fields {
                self.collect_reference_edges_from_value(
                    &document.schema,
                    &document.id,
                    field,
                    value,
                    &mut edges,
                );
            }
            edges
        }

        fn add_reverse_references_for_document(&self, document: &Document) -> OQLResult<()> {
            let edges = self.outgoing_reference_edges_for_document(document);
            if edges.is_empty() {
                return Ok(());
            }
            let mut reverse = self
                .reverse_references
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire reverse reference lock"))?;
            for (target_key, entry) in edges {
                let entries = reverse.entry(target_key).or_default();
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
            Ok(())
        }

        fn remove_reverse_references_for_document(&self, document: &Document) -> OQLResult<()> {
            let edges = self.outgoing_reference_edges_for_document(document);
            if edges.is_empty() {
                return Ok(());
            }
            let mut reverse = self
                .reverse_references
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire reverse reference lock"))?;
            for (target_key, entry) in edges {
                if let Some(entries) = reverse.get_mut(&target_key) {
                    entries.retain(|existing| existing != &entry);
                    if entries.is_empty() {
                        reverse.remove(&target_key);
                    }
                }
            }
            Ok(())
        }

        fn refresh_runtime_reverse_references(&self) -> OQLResult<()> {
            let mut rebuilt: HashMap<(String, DocumentId), Vec<super::ReverseReferenceEntry>> =
                HashMap::new();

            let documents: Vec<Document> = {
                let index = self
                    .document_index
                    .read()
                    .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
                let mut docs = Vec::with_capacity(index.len());
                for ((doc_schema, doc_id), &objref) in index.iter() {
                    if let Ok(mut stored_doc) = self.read_document_internal(objref) {
                        stored_doc.document.id = doc_id.clone();
                        stored_doc.document.schema = doc_schema.clone();
                        docs.push(stored_doc.document);
                    }
                }
                docs
            };

            for document in documents {
                for (target_key, entry) in self.outgoing_reference_edges_for_document(&document) {
                    let entries = rebuilt.entry(target_key).or_default();
                    if !entries.contains(&entry) {
                        entries.push(entry);
                    }
                }
            }

            let mut reverse = self
                .reverse_references
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire reverse reference lock"))?;
            *reverse = rebuilt;
            Ok(())
        }

        fn reference_policy_for_field_path(
            &self,
            source_schema: &str,
            field_path: &str,
        ) -> OQLResult<Option<crate::schema::ReferenceType>> {
            let Some(schema) = self.get_schema(source_schema)? else {
                return Ok(None);
            };

            let Some(first_segment) = field_path.split('.').next() else {
                return Ok(None);
            };

            Ok(schema.get_field_type(first_segment).and_then(|field_type| {
                if field_path.contains('.') && field_type.reference_type().is_none() {
                    None
                } else {
                    field_type.reference_type().cloned()
                }
            }))
        }

        fn remove_reference_value(
            value: &mut Value,
            path: &[&str],
            target_objref: crate::document::ObjRef,
        ) -> bool {
            if path.is_empty() {
                return false;
            }

            if path.len() == 1 {
                match value {
                    Value::Reference(objref) => {
                        if *objref == target_objref {
                            *value = Value::Null;
                            true
                        } else {
                            false
                        }
                    }
                    Value::List(items) => {
                        let before = items.len();
                        items.retain(|item| {
                            !matches!(item, Value::Reference(objref) if *objref == target_objref)
                        });
                        before != items.len()
                    }
                    _ => false,
                }
            } else {
                match value {
                    Value::Object(map) => map
                        .get_mut(path[0])
                        .map(|child| Self::remove_reference_value(child, &path[1..], target_objref))
                        .unwrap_or(false),
                    Value::List(items) => {
                        let mut changed = false;
                        for item in items.iter_mut() {
                            changed |= Self::remove_reference_value(item, path, target_objref);
                        }
                        changed
                    }
                    _ => false,
                }
            }
        }

        fn apply_set_null_to_document(
            &self,
            document: &Document,
            field_paths: &[String],
            target_objref: crate::document::ObjRef,
        ) -> OQLResult<()> {
            let mut updated = document.clone();
            let mut changed = false;

            for field_path in field_paths {
                let segments: Vec<&str> = field_path.split('.').collect();
                if segments.is_empty() {
                    continue;
                }
                if let Some(value) = updated.fields.get_mut(segments[0]) {
                    let changed_here = if segments.len() == 1 {
                        match value {
                            Value::Reference(objref) if *objref == target_objref => {
                                *value = Value::Null;
                                true
                            }
                            Value::List(items) => {
                                let before = items.len();
                                items.retain(|item| {
                                    !matches!(item, Value::Reference(objref) if *objref == target_objref)
                                });
                                before != items.len()
                            }
                            _ => false,
                        }
                    } else {
                        Self::remove_reference_value(value, &segments[1..], target_objref)
                    };
                    if changed_here {
                        changed = true;
                    }
                }
            }

            if changed {
                self.put_document(&updated)?;
            }

            Ok(())
        }

        fn enforce_reference_delete_policies(
            &self,
            target_schema: &str,
            target_id: &DocumentId,
        ) -> OQLResult<()> {
            let Some(target_objref) = self.get_objref(target_schema, target_id)? else {
                return Ok(());
            };
            let incoming = self.list_references_to(target_schema, target_id, None, None)?;
            if incoming.is_empty() {
                return Ok(());
            }

            let mut set_null_paths: HashMap<(String, String), Vec<String>> = HashMap::new();
            let mut cascade_docs: HashSet<(String, String)> = HashSet::new();

            for edge in incoming {
                let Some(policy) =
                    self.reference_policy_for_field_path(&edge.source_schema, &edge.field_path)?
                else {
                    continue;
                };
                match policy.on_delete {
                    crate::schema::CascadeBehavior::Restrict => {
                        return Err(OQLError::ReferenceError {
                            message: format!(
                                "Cannot delete {}.{} because {}.{} still references it via '{}'",
                                target_schema,
                                target_id,
                                edge.source_schema,
                                edge.source_id,
                                edge.field_path
                            ),
                        });
                    }
                    crate::schema::CascadeBehavior::Cascade => {
                        cascade_docs.insert((edge.source_schema.clone(), edge.source_id.clone()));
                    }
                    crate::schema::CascadeBehavior::SetNull => {
                        set_null_paths
                            .entry((edge.source_schema.clone(), edge.source_id.clone()))
                            .or_default()
                            .push(edge.field_path.clone());
                    }
                    crate::schema::CascadeBehavior::NoAction => {}
                }
            }

            for ((schema, id), paths) in set_null_paths {
                if cascade_docs.contains(&(schema.clone(), id.clone())) {
                    continue;
                }
                if let Some(document) = self.get_document(&schema, &id)? {
                    self.apply_set_null_to_document(&document, &paths, target_objref)?;
                }
            }

            for (schema, id) in cascade_docs {
                let _ = self.delete_document(&schema, &id)?;
            }

            Ok(())
        }

        fn find_vector_index_definition(
            &self,
            schema: &str,
            name: &str,
        ) -> OQLResult<Option<super::VectorIndexDefinition>> {
            let catalog = self
                .vector_indexes
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire vector index lock"))?;
            Ok(catalog
                .get(schema)
                .and_then(|entries| entries.iter().find(|e| e.name == name).cloned()))
        }

        fn build_hnsw_for_definition(
            &self,
            index: &super::VectorIndexDefinition,
        ) -> OQLResult<super::VectorIndexBuildReport> {
            if index.index_type != super::VectorIndexType::Hnsw {
                return Err(OQLError::generic(
                    "build only supported for hnsw vector indexes",
                ));
            }

            std::fs::create_dir_all(&self.vector_index_dir).map_err(|e| {
                OQLError::storage_error(
                    &format!(
                        "Failed to create vector index directory '{}': {}",
                        self.vector_index_dir.display(),
                        e
                    ),
                    None,
                )
            })?;

            let docs = self.scan_documents(&index.schema)?;
            let mut hnsw =
                owl_storage_zig::HnswIndex::create(index.dimension, 16, 64).map_err(|e| {
                    OQLError::storage_error(&format!("Failed to create HNSW index: {}", e), None)
                })?;

            let mut indexed_documents = 0usize;
            let mut skipped_documents = 0usize;
            for document in &docs {
                let Some(value) = document.fields.get(&index.field) else {
                    skipped_documents += 1;
                    continue;
                };
                let Some(vec) = Self::value_to_vector(value) else {
                    skipped_documents += 1;
                    continue;
                };
                if vec.len() != index.dimension as usize {
                    skipped_documents += 1;
                    continue;
                }
                hnsw.add_vector(Self::stable_vector_id(&document.id), &vec)
                    .map_err(|e| {
                        OQLError::storage_error(&format!("Failed to build HNSW index: {}", e), None)
                    })?;
                indexed_documents += 1;
            }

            let index_path = self.vector_index_file_path(index);
            hnsw.save(index_path.to_string_lossy().as_ref())
                .map_err(|e| {
                    OQLError::storage_error(&format!("Failed to persist HNSW index: {}", e), None)
                })?;

            Ok(super::VectorIndexBuildReport {
                indexed_documents,
                skipped_documents,
                total_documents: docs.len(),
            })
        }

        #[cfg(test)]
        pub(crate) fn new_with_fs_strict<P: AsRef<Path>>(
            path: P,
            size: usize,
            filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
        ) -> OQLResult<Self> {
            Self::new_with_fs_policy(
                path,
                size,
                filesystem,
                SuperblockOpenPolicy::Strict,
                DurabilityBackend::Rust,
            )
        }

        /// Create a new ZigStore (backward compatibility)
        pub fn new<P: AsRef<Path>>(path: P, size: usize) -> OQLResult<Self> {
            use crate::simulation::RealFileSystem;
            let filesystem = Arc::new(RealFileSystem::new());
            let policy = Self::superblock_policy_from_env();
            Self::new_with_fs_policy(path, size, filesystem, policy, DurabilityBackend::ZigNative)
        }

        fn superblock_policy_from_env() -> SuperblockOpenPolicy {
            match std::env::var("OWL_SUPERBLOCK_POLICY")
                .unwrap_or_else(|_| "availability".to_string())
                .to_ascii_lowercase()
                .as_str()
            {
                "strict" => SuperblockOpenPolicy::Strict,
                _ => SuperblockOpenPolicy::Availability,
            }
        }

        fn open_wal_for_rw(
            filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
            wal_path: &Path,
        ) -> OQLResult<Box<dyn crate::simulation::filesystem::FileHandle>> {
            let options = crate::simulation::filesystem::FileOptions {
                read: true,
                write: true,
                create: !filesystem.exists(wal_path),
                truncate: false,
                append: false,
            };
            filesystem
                .open_with_options(wal_path, &options)
                .map_err(|e| {
                    OQLError::storage_error(
                        &format!("Failed to open storage WAL '{}': {}", wal_path.display(), e),
                        None,
                    )
                })
        }

        fn open_wal_for_read(
            &self,
        ) -> OQLResult<Box<dyn crate::simulation::filesystem::FileHandle>> {
            let options = crate::simulation::filesystem::FileOptions {
                read: true,
                write: false,
                create: false,
                truncate: false,
                append: false,
            };
            self.filesystem
                .open_with_options(&self.wal_path, &options)
                .map_err(|e| {
                    OQLError::storage_error(
                        &format!(
                            "Failed to open storage WAL '{}': {}",
                            self.wal_path.display(),
                            e
                        ),
                        None,
                    )
                })
        }

        fn open_superblock_for_rw(
            filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
            superblock_path: &Path,
        ) -> OQLResult<Box<dyn crate::simulation::filesystem::FileHandle>> {
            let options = crate::simulation::filesystem::FileOptions {
                read: true,
                write: true,
                create: !filesystem.exists(superblock_path),
                truncate: false,
                append: false,
            };
            filesystem
                .open_with_options(superblock_path, &options)
                .map_err(|e| {
                    OQLError::storage_error(
                        &format!(
                            "Failed to open storage superblock '{}': {}",
                            superblock_path.display(),
                            e
                        ),
                        None,
                    )
                })
        }

        fn superblock_record_crc(bytes: &[u8]) -> u32 {
            crc32fast::hash(bytes)
        }

        fn encode_superblock_record(
            record: &SuperblockRecord,
        ) -> [u8; Self::SUPERBLOCK_RECORD_SIZE] {
            let mut out = [0u8; Self::SUPERBLOCK_RECORD_SIZE];
            out[0..4].copy_from_slice(&Self::SUPERBLOCK_MAGIC.to_le_bytes());
            out[4..6].copy_from_slice(&Self::SUPERBLOCK_VERSION.to_le_bytes());
            out[6..8].copy_from_slice(&record.copy.to_le_bytes());
            out[8..16].copy_from_slice(&record.sequence.to_le_bytes());
            out[16..24].copy_from_slice(&record.arena_size.to_le_bytes());
            out[24..32].copy_from_slice(&record.wal_size.to_le_bytes());
            out[32..40].copy_from_slice(&record.document_count.to_le_bytes());
            out[40..44].copy_from_slice(&record.schema_count.to_le_bytes());
            out[44..48].copy_from_slice(&0u32.to_le_bytes()); // reserved
            out[48..52].copy_from_slice(&record.parent_crc32.to_le_bytes());
            let crc = Self::superblock_record_crc(&out[0..52]);
            out[52..56].copy_from_slice(&crc.to_le_bytes());
            out
        }

        fn decode_superblock_record(buf: &[u8]) -> Option<SuperblockRecord> {
            if buf.len() < Self::SUPERBLOCK_RECORD_SIZE {
                return None;
            }
            let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            if magic != Self::SUPERBLOCK_MAGIC {
                return None;
            }
            let version = u16::from_le_bytes([buf[4], buf[5]]);
            if version != Self::SUPERBLOCK_VERSION {
                return None;
            }
            let copy = u16::from_le_bytes([buf[6], buf[7]]);
            if copy as usize >= Self::SUPERBLOCK_COPIES {
                return None;
            }
            let sequence = u64::from_le_bytes([
                buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
            ]);
            let arena_size = u64::from_le_bytes([
                buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
            ]);
            let wal_size = u64::from_le_bytes([
                buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31],
            ]);
            let document_count = u64::from_le_bytes([
                buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39],
            ]);
            let schema_count = u32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]);
            let parent_crc32 = u32::from_le_bytes([buf[48], buf[49], buf[50], buf[51]]);
            let record_crc32 = u32::from_le_bytes([buf[52], buf[53], buf[54], buf[55]]);
            let computed = Self::superblock_record_crc(&buf[0..52]);
            if computed != record_crc32 {
                return None;
            }
            Some(SuperblockRecord {
                copy,
                sequence,
                arena_size,
                wal_size,
                document_count,
                schema_count,
                parent_crc32,
                record_crc32,
            })
        }

        fn recover_working_superblock(
            filesystem: Arc<dyn crate::simulation::filesystem::FileSystem>,
            superblock_path: &Path,
            policy: SuperblockOpenPolicy,
        ) -> OQLResult<Option<SuperblockRecord>> {
            if !filesystem.exists(superblock_path) {
                return Ok(None);
            }
            let options = crate::simulation::filesystem::FileOptions {
                read: true,
                write: false,
                create: false,
                truncate: false,
                append: false,
            };
            let mut file = filesystem
                .open_with_options(superblock_path, &options)
                .map_err(|e| {
                    OQLError::storage_error(
                        &format!(
                            "Failed to open storage superblock '{}': {}",
                            superblock_path.display(),
                            e
                        ),
                        None,
                    )
                })?;
            let file_size = file.size().map_err(|e| {
                OQLError::storage_error(
                    &format!(
                        "Failed to stat storage superblock '{}': {}",
                        superblock_path.display(),
                        e
                    ),
                    None,
                )
            })? as usize;
            if file_size < Self::SUPERBLOCK_RECORD_SIZE {
                return Ok(None);
            }

            let mut records = Vec::new();
            let readable_copies = std::cmp::min(
                Self::SUPERBLOCK_COPIES,
                file_size / Self::SUPERBLOCK_RECORD_SIZE,
            );
            for copy in 0..readable_copies {
                let offset = (copy * Self::SUPERBLOCK_RECORD_SIZE) as u64;
                file.seek(SeekFrom::Start(offset)).map_err(|e| {
                    OQLError::storage_error(
                        &format!(
                            "Failed to seek superblock '{}': {}",
                            superblock_path.display(),
                            e
                        ),
                        None,
                    )
                })?;
                let mut buf = [0u8; Self::SUPERBLOCK_RECORD_SIZE];
                if !Self::read_exact(file.as_mut(), &mut buf)? {
                    break;
                }
                if let Some(rec) = Self::decode_superblock_record(&buf) {
                    records.push(rec);
                }
            }
            if records.is_empty() {
                return Ok(None);
            }

            let mut best_quorum: Option<(SuperblockRecord, usize)> = None;
            for rec in &records {
                let count = records
                    .iter()
                    .filter(|r| r.sequence == rec.sequence && r.record_crc32 == rec.record_crc32)
                    .count();
                if count >= 2 {
                    match best_quorum {
                        None => best_quorum = Some((*rec, count)),
                        Some((best, best_count)) => {
                            if rec.sequence > best.sequence
                                || (rec.sequence == best.sequence && count > best_count)
                            {
                                best_quorum = Some((*rec, count));
                            }
                        }
                    }
                }
            }
            if let Some((winner, _)) = best_quorum {
                if policy == SuperblockOpenPolicy::Strict {
                    if winner.sequence > 1 {
                        let has_parent = records.iter().any(|r| {
                            r.sequence + 1 == winner.sequence
                                && r.record_crc32 == winner.parent_crc32
                        });
                        if !has_parent {
                            return Err(OQLError::storage_error(
                                "Strict superblock validation failed: parent chain not found",
                                None,
                            ));
                        }
                    }
                }
                return Ok(Some(winner));
            }

            if policy == SuperblockOpenPolicy::Strict {
                return Err(OQLError::storage_error(
                    "Strict superblock validation failed: no quorum-valid copyset",
                    None,
                ));
            }

            // Availability-mode fallback: use highest valid singleton to avoid lockout on partial copyset writes.
            let mut winner = records[0];
            for rec in records.into_iter().skip(1) {
                if rec.sequence > winner.sequence {
                    winner = rec;
                }
            }
            Ok(Some(winner))
        }

        fn persist_superblock(&self, force: bool) -> OQLResult<()> {
            let mut state = self
                .superblock_state
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire superblock lock"))?;

            if !force {
                state.pending_mutations = state.pending_mutations.saturating_add(1);
                if state.pending_mutations < Self::SUPERBLOCK_MUTATION_INTERVAL {
                    return Ok(());
                }
            }

            state.pending_mutations = 0;
            state.sequence = state.sequence.saturating_add(1);
            let parent_crc32 = state.last_crc32;
            let sequence = state.sequence;
            drop(state);

            let document_count = self
                .document_index
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?
                .len() as u64;
            let schema_count = self
                .schemas
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire schema lock"))?
                .len() as u32;
            let wal_size = self.filesystem.file_size(&self.wal_path).unwrap_or(0);

            let base = SuperblockRecord {
                copy: 0,
                sequence,
                arena_size: self.arena_size_bytes as u64,
                wal_size,
                document_count,
                schema_count,
                parent_crc32,
                record_crc32: 0,
            };

            if self.durability_backend == DurabilityBackend::ZigNative {
                let initial_full = sequence == 1;
                let record_crc = zig_persist_superblock(
                    &self.superblock_path,
                    sequence,
                    parent_crc32,
                    base.arena_size,
                    base.wal_size,
                    base.document_count,
                    base.schema_count,
                    initial_full,
                )
                .map_err(|e| {
                    OQLError::storage_error(
                        &format!(
                            "Failed to persist storage superblock '{}': {}",
                            self.superblock_path.display(),
                            e
                        ),
                        None,
                    )
                })?;
                let mut state = self
                    .superblock_state
                    .lock()
                    .map_err(|_| OQLError::generic("Failed to acquire superblock lock"))?;
                state.last_crc32 = record_crc;
                return Ok(());
            }

            let mut file =
                Self::open_superblock_for_rw(self.filesystem.clone(), &self.superblock_path)?;
            let copies_to_write: Vec<usize> = if sequence == 1 {
                (0..Self::SUPERBLOCK_COPIES).collect()
            } else {
                let a = (sequence as usize) % Self::SUPERBLOCK_COPIES;
                let b = (a + 1) % Self::SUPERBLOCK_COPIES;
                vec![a, b]
            };
            let mut last_crc = parent_crc32;
            for copy in copies_to_write {
                let mut rec = base;
                rec.copy = copy as u16;
                let encoded = Self::encode_superblock_record(&rec);
                let rec_crc =
                    u32::from_le_bytes([encoded[52], encoded[53], encoded[54], encoded[55]]);
                file.seek(SeekFrom::Start(
                    (copy * Self::SUPERBLOCK_RECORD_SIZE) as u64,
                ))
                .map_err(|e| {
                    OQLError::storage_error(&format!("Failed to seek superblock: {}", e), None)
                })?;
                Self::write_all(file.as_mut(), &encoded)?;
                last_crc = rec_crc;
            }
            file.sync().map_err(|e| {
                OQLError::storage_error(&format!("Failed to sync superblock: {}", e), None)
            })?;

            let mut state = self
                .superblock_state
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire superblock lock"))?;
            state.last_crc32 = last_crc;
            Ok(())
        }

        fn write_all(
            file: &mut dyn crate::simulation::filesystem::FileHandle,
            mut bytes: &[u8],
        ) -> OQLResult<()> {
            while !bytes.is_empty() {
                let n = file.write(bytes).map_err(|e| {
                    OQLError::storage_error(&format!("Failed writing storage WAL: {}", e), None)
                })?;
                if n == 0 {
                    return Err(OQLError::storage_error("Short write to storage WAL", None));
                }
                bytes = &bytes[n..];
            }
            Ok(())
        }

        fn read_exact(
            file: &mut dyn crate::simulation::filesystem::FileHandle,
            mut buf: &mut [u8],
        ) -> OQLResult<bool> {
            if buf.is_empty() {
                return Ok(true);
            }
            let mut read_any = false;
            while !buf.is_empty() {
                let n = file.read(buf).map_err(|e| {
                    OQLError::storage_error(&format!("Failed reading storage WAL: {}", e), None)
                })?;
                if n == 0 {
                    if read_any {
                        return Err(OQLError::storage_error(
                            "Unexpected EOF in storage WAL",
                            None,
                        ));
                    }
                    return Ok(false);
                }
                read_any = true;
                let tmp = buf;
                buf = &mut tmp[n..];
            }
            Ok(true)
        }

        fn append_log_entry(&self, entry: &StorageLogEntry) -> OQLResult<()> {
            let bytes = bincode::serialize(entry).map_err(|e| {
                OQLError::serialization_error(
                    &format!("Failed to serialize storage WAL entry: {}", e),
                    None,
                )
            })?;

            if self.durability_backend == DurabilityBackend::ZigNative {
                return append_wal_framed(&self.wal_path, &bytes).map_err(|e| {
                    OQLError::storage_error(
                        &format!(
                            "Failed to append storage WAL frame '{}': {}",
                            self.wal_path.display(),
                            e
                        ),
                        None,
                    )
                });
            }

            let mut wal = self
                .wal_file
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire storage WAL lock"))?;
            wal.seek(SeekFrom::End(0)).map_err(|e| {
                OQLError::storage_error(&format!("Failed to seek storage WAL: {}", e), None)
            })?;

            let mut frame_meta = [0u8; Self::WAL_FRAME_META_SIZE];
            frame_meta[0] = Self::WAL_VERSION;
            frame_meta[1] = 0; // flags
            frame_meta[2..4].copy_from_slice(&0u16.to_le_bytes()); // reserved
            frame_meta[4..8].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            frame_meta[8..12].copy_from_slice(&crc32fast::hash(&bytes).to_le_bytes());

            Self::write_all(wal.as_mut(), &Self::WAL_MAGIC.to_le_bytes())?;
            Self::write_all(wal.as_mut(), &frame_meta)?;
            Self::write_all(wal.as_mut(), &bytes)?;
            wal.sync().map_err(|e| {
                OQLError::storage_error(&format!("Failed to sync storage WAL: {}", e), None)
            })?;
            Ok(())
        }

        fn current_txn_id(&self) -> OQLResult<Option<u64>> {
            let guard = self
                .active_txn_id
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            Ok(*guard)
        }

        pub fn begin_local_transaction(&self) -> OQLResult<u64> {
            let txn_id = self.next_txn_id.fetch_add(1, Ordering::Relaxed);
            let mut guard = self
                .active_txn_id
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            if guard.is_some() {
                return Err(OQLError::transaction_error(
                    "Nested transactions are not supported",
                    None,
                ));
            }
            self.append_log_entry(&StorageLogEntry::TxnBegin { txn_id })?;
            *guard = Some(txn_id);
            Ok(txn_id)
        }

        pub fn commit_local_transaction(&self, txn_id: u64) -> OQLResult<()> {
            let mut guard = self
                .active_txn_id
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            if *guard != Some(txn_id) {
                return Err(OQLError::transaction_error(
                    "Commit called with non-active transaction id",
                    None,
                ));
            }
            self.append_log_entry(&StorageLogEntry::TxnCommit { txn_id })?;
            *guard = None;
            Ok(())
        }

        pub fn rollback_local_transaction(&self, txn_id: u64) -> OQLResult<()> {
            let mut guard = self
                .active_txn_id
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            if *guard != Some(txn_id) {
                return Err(OQLError::transaction_error(
                    "Rollback called with non-active transaction id",
                    None,
                ));
            }
            self.append_log_entry(&StorageLogEntry::TxnRollback { txn_id })?;
            *guard = None;
            Ok(())
        }

        fn recover_from_wal(&self) -> OQLResult<()> {
            if !self.filesystem.exists(&self.wal_path) {
                return Ok(());
            }

            let mut wal = self.open_wal_for_read()?;
            let mut entries = Vec::new();
            loop {
                let mut prefix = [0u8; 4];
                let has_prefix = match Self::read_exact(wal.as_mut(), &mut prefix) {
                    Ok(v) => v,
                    Err(e) if e.to_string().contains("Unexpected EOF in storage WAL") => {
                        // Forced shutdown can leave a torn tail. Replay only complete entries.
                        break;
                    }
                    Err(e) => return Err(e),
                };
                if !has_prefix {
                    break;
                }

                let payload = if u32::from_le_bytes(prefix) == Self::WAL_MAGIC {
                    let mut meta = [0u8; Self::WAL_FRAME_META_SIZE];
                    let has_meta = match Self::read_exact(wal.as_mut(), &mut meta) {
                        Ok(v) => v,
                        Err(e) if e.to_string().contains("Unexpected EOF in storage WAL") => {
                            break;
                        }
                        Err(e) => return Err(e),
                    };
                    if !has_meta {
                        break;
                    }
                    if meta[0] != Self::WAL_VERSION {
                        break;
                    }
                    let len = u32::from_le_bytes([meta[4], meta[5], meta[6], meta[7]]) as usize;
                    let crc_expected = u32::from_le_bytes([meta[8], meta[9], meta[10], meta[11]]);
                    if len == 0 || len > Self::WAL_ENTRY_SIZE_MAX {
                        break;
                    }
                    let mut payload = vec![0u8; len];
                    let has_payload = match Self::read_exact(wal.as_mut(), &mut payload) {
                        Ok(v) => v,
                        Err(e) if e.to_string().contains("Unexpected EOF in storage WAL") => {
                            break;
                        }
                        Err(e) => return Err(e),
                    };
                    if !has_payload {
                        break;
                    }
                    let crc_actual = crc32fast::hash(&payload);
                    if crc_actual != crc_expected {
                        break;
                    }
                    payload
                } else {
                    // Legacy format compatibility: [len: u32][payload bytes]
                    let len = u32::from_le_bytes(prefix) as usize;
                    if len == 0 || len > Self::WAL_ENTRY_SIZE_MAX {
                        break;
                    }
                    let mut payload = vec![0u8; len];
                    let has_payload = match Self::read_exact(wal.as_mut(), &mut payload) {
                        Ok(v) => v,
                        Err(e) if e.to_string().contains("Unexpected EOF in storage WAL") => {
                            break;
                        }
                        Err(e) => return Err(e),
                    };
                    if !has_payload {
                        break;
                    }
                    payload
                };

                let entry: StorageLogEntry = match bincode::deserialize(&payload) {
                    Ok(entry) => entry,
                    Err(_) => {
                        // Corrupt tail entry. Keep previously applied state.
                        break;
                    }
                };
                entries.push(entry);
            }

            // Recovery rule: apply autocommit entries + entries belonging to explicitly committed txns.
            let mut committed_txns = std::collections::HashSet::new();
            let mut rolled_back_txns = std::collections::HashSet::new();
            for entry in &entries {
                match entry {
                    StorageLogEntry::TxnCommit { txn_id } => {
                        committed_txns.insert(*txn_id);
                    }
                    StorageLogEntry::TxnRollback { txn_id } => {
                        rolled_back_txns.insert(*txn_id);
                    }
                    _ => {}
                }
            }

            for entry in entries {
                let should_apply = match &entry {
                    StorageLogEntry::Put { txn_id, .. }
                    | StorageLogEntry::Delete { txn_id, .. }
                    | StorageLogEntry::PutSchema { txn_id, .. }
                    | StorageLogEntry::DeleteSchema { txn_id, .. }
                    | StorageLogEntry::PutScalarIndex { txn_id, .. }
                    | StorageLogEntry::DeleteScalarIndex { txn_id, .. }
                    | StorageLogEntry::PutVectorIndex { txn_id, .. }
                    | StorageLogEntry::DeleteVectorIndex { txn_id, .. } => match txn_id {
                        None => true,
                        Some(id) => committed_txns.contains(id) && !rolled_back_txns.contains(id),
                    },
                    StorageLogEntry::TxnBegin { .. }
                    | StorageLogEntry::TxnCommit { .. }
                    | StorageLogEntry::TxnRollback { .. } => false,
                };
                if should_apply {
                    self.apply_log_entry(entry)?;
                }
            }

            Ok(())
        }

        fn apply_log_entry(&self, entry: StorageLogEntry) -> OQLResult<()> {
            match entry {
                StorageLogEntry::Put {
                    txn_id: _,
                    schema,
                    id,
                    objref_offset,
                    payload,
                } => {
                    self.arena.write_at(objref_offset, &payload).map_err(|e| {
                        OQLError::storage_error(
                            &format!("Failed to replay arena write: {:?}", e),
                            None,
                        )
                    })?;

                    let mut index = self
                        .document_index
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
                    index.insert((schema, id), ObjRef::new(objref_offset));
                }
                StorageLogEntry::Delete {
                    txn_id: _,
                    schema,
                    id,
                } => {
                    let mut index = self
                        .document_index
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire document index lock"))?;
                    index.remove(&(schema, id));
                }
                StorageLogEntry::PutSchema { txn_id: _, schema } => {
                    let mut schemas = self
                        .schemas
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire schema lock"))?;
                    schemas.insert(schema.name.clone(), schema);
                }
                StorageLogEntry::DeleteSchema { txn_id: _, name } => {
                    let mut schemas = self
                        .schemas
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire schema lock"))?;
                    schemas.remove(&name);
                }
                StorageLogEntry::PutScalarIndex { txn_id: _, index } => {
                    let mut catalog = self
                        .scalar_indexes
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire scalar index lock"))?;
                    let entries = catalog.entry(index.schema.clone()).or_default();
                    if let Some(existing) = entries.iter_mut().find(|e| e.name == index.name) {
                        *existing = index;
                    } else {
                        entries.push(index);
                    }
                }
                StorageLogEntry::DeleteScalarIndex {
                    txn_id: _,
                    schema,
                    name,
                } => {
                    let mut catalog = self
                        .scalar_indexes
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire scalar index lock"))?;
                    if let Some(entries) = catalog.get_mut(&schema) {
                        entries.retain(|entry| entry.name != name);
                    }
                }
                StorageLogEntry::PutVectorIndex { txn_id: _, index } => {
                    let mut catalog = self
                        .vector_indexes
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire vector index lock"))?;
                    let entries = catalog.entry(index.schema.clone()).or_default();
                    if let Some(existing) = entries.iter_mut().find(|e| e.name == index.name) {
                        *existing = index;
                    } else {
                        entries.push(index);
                    }
                }
                StorageLogEntry::DeleteVectorIndex {
                    txn_id: _,
                    schema,
                    name,
                } => {
                    let mut catalog = self
                        .vector_indexes
                        .write()
                        .map_err(|_| OQLError::generic("Failed to acquire vector index lock"))?;
                    if let Some(entries) = catalog.get_mut(&schema) {
                        entries.retain(|entry| entry.name != name);
                    }
                }
                StorageLogEntry::TxnBegin { .. }
                | StorageLogEntry::TxnCommit { .. }
                | StorageLogEntry::TxnRollback { .. } => {}
            }
            Ok(())
        }

        /// Get the ObjRef for a document (for testing and reference creation)
        pub fn get_objref(
            &self,
            schema: &str,
            id: &DocumentId,
        ) -> OQLResult<Option<crate::document::ObjRef>> {
            let index = self
                .document_index
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;

            let key = (schema.to_string(), id.clone());
            Ok(index
                .get(&key)
                .map(|zig_ref| crate::document::ObjRef::new(zig_ref.offset())))
        }

        pub fn scalar_index_manager(&self) -> OQLResult<Option<Arc<IndexManager>>> {
            let guard = self
                .scalar_index_manager
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire scalar index manager lock"))?;
            Ok(guard.clone())
        }

        fn refresh_runtime_scalar_indexes(&self) -> OQLResult<()> {
            let definitions = self.list_scalar_indexes(None)?;
            let manager = if definitions.is_empty() {
                None
            } else {
                let manager = IndexManager::load_scalar_definitions_with_backend(
                    definitions,
                    Arc::new(RealClock::new()),
                    IndexBackendKind::Zig,
                )
                .map_err(|e| OQLError::generic(&format!("Failed to load scalar indexes: {}", e)))?;
                manager.rebuild_from_store(self).map_err(|e| {
                    OQLError::generic(&format!("Failed to rebuild scalar indexes: {}", e))
                })?;
                Some(Arc::new(manager))
            };

            let mut guard = self
                .scalar_index_manager
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire scalar index manager lock"))?;
            *guard = manager;
            Ok(())
        }

        /// Resolve canonical index ID for a payload-carried document ID.
        ///
        /// Historical writes may contain payload IDs that diverge from the
        /// `(schema, id)` index key. This helper scans index entries for a schema
        /// and matches on serialized payload identity, returning the canonical key ID.
        pub fn resolve_index_id_for_payload_id(
            &self,
            schema: &str,
            payload_id: &str,
        ) -> OQLResult<Option<DocumentId>> {
            let index = self
                .document_index
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;

            for ((doc_schema, doc_id), &objref) in index.iter() {
                if doc_schema != schema {
                    continue;
                }
                if let Ok(stored_doc) = self.read_document_internal(objref) {
                    if stored_doc.document.id == payload_id || stored_doc.id == payload_id {
                        return Ok(Some(doc_id.clone()));
                    }
                }
            }
            Ok(None)
        }

        fn encode_stored_doc(&self, stored_doc: &StoredDocument) -> OQLResult<Vec<u8>> {
            let doc_data = bincode::serialize(stored_doc).map_err(|e| {
                OQLError::serialization_error(&format!("Failed to serialize document: {}", e), None)
            })?;

            let payload_len = doc_data.len() as u32;
            let checksum = crc32fast::hash(&doc_data);
            let mut combined = Vec::with_capacity(Self::BLOB_HEADER_SIZE + doc_data.len());
            combined.extend_from_slice(&payload_len.to_le_bytes());
            combined.extend_from_slice(&checksum.to_le_bytes());
            combined.extend_from_slice(&doc_data);
            Ok(combined)
        }

        /// Serialize and store a document in the arena using WAL-first ordering.
        fn store_document_internal(&self, stored_doc: &StoredDocument) -> OQLResult<ObjRef> {
            let payload = self.encode_stored_doc(stored_doc)?;

            // Reserve address first, then WAL-sync, then arena write.
            let objref = self.arena.alloc(payload.len() as u32).map_err(|e| {
                OQLError::storage_error(&format!("Failed to allocate: {:?}", e), None)
            })?;

            self.append_log_entry(&StorageLogEntry::Put {
                txn_id: self.current_txn_id()?,
                schema: stored_doc.schema.clone(),
                id: stored_doc.id.clone(),
                objref_offset: objref.offset(),
                payload: payload.clone(),
            })?;

            self.arena
                .write_at(objref.offset(), &payload)
                .map_err(|e| OQLError::storage_error(&format!("Failed to write: {:?}", e), None))?;

            // Explicit durability boundary for filesystem-backed simulation adapters.
            self.filesystem.sync_all().map_err(|e| {
                OQLError::storage_error(
                    &format!("Failed to sync filesystem after arena write: {}", e),
                    None,
                )
            })?;

            Ok(objref)
        }

        /// Read and deserialize a document from the arena
        fn read_document_internal(&self, objref: ObjRef) -> OQLResult<StoredDocument> {
            let mut header_buffer = [0u8; Self::BLOB_HEADER_SIZE];
            let bytes_read = self
                .arena
                .read_at(objref.offset(), &mut header_buffer)
                .map_err(|e| {
                    OQLError::storage_error(&format!("Failed to read blob header: {:?}", e), None)
                })?;

            if bytes_read < Self::BLOB_HEADER_SIZE {
                return Err(OQLError::storage_error(
                    &format!(
                        "Failed to read blob header: only {} bytes available",
                        bytes_read
                    ),
                    None,
                ));
            }

            let payload_size = u32::from_le_bytes([
                header_buffer[0],
                header_buffer[1],
                header_buffer[2],
                header_buffer[3],
            ]) as usize;
            let expected_checksum = u32::from_le_bytes([
                header_buffer[4],
                header_buffer[5],
                header_buffer[6],
                header_buffer[7],
            ]);
            let total_size = Self::BLOB_HEADER_SIZE + payload_size;

            let mut full_buffer = vec![0u8; total_size];
            let full_bytes_read = self
                .arena
                .read_at(objref.offset(), &mut full_buffer)
                .map_err(|e| {
                    OQLError::storage_error(
                        &format!("Failed to read serialized payload: {:?}", e),
                        None,
                    )
                })?;
            if full_bytes_read < total_size {
                return Err(OQLError::storage_error(
                    &format!(
                        "Failed to read complete payload: got {} bytes, expected {}",
                        full_bytes_read, total_size
                    ),
                    None,
                ));
            }

            let doc_buffer = &full_buffer[Self::BLOB_HEADER_SIZE..];
            let calculated_checksum = crc32fast::hash(doc_buffer);
            if calculated_checksum != expected_checksum {
                return Err(OQLError::storage_error(
                    &format!(
                        "Checksum mismatch: expected 0x{:08x}, got 0x{:08x}",
                        expected_checksum, calculated_checksum
                    ),
                    None,
                ));
            }

            let stored_doc: StoredDocument = bincode::deserialize(doc_buffer).map_err(|e| {
                OQLError::serialization_error(&format!("Failed to deserialize: {}", e), None)
            })?;

            Ok(stored_doc)
        }
    }

    impl DocumentStore for ZigStore {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn get_objref(
            &self,
            schema: &str,
            id: &DocumentId,
        ) -> OQLResult<Option<crate::document::ObjRef>> {
            let index = self
                .document_index
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;

            let key = (schema.to_string(), id.clone());
            Ok(index
                .get(&key)
                .map(|zig_ref| crate::document::ObjRef::new(zig_ref.offset())))
        }

        fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>> {
            let index = self
                .document_index
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;

            let key = (schema.to_string(), id.clone());
            if let Some(&objref) = index.get(&key) {
                let mut stored_doc = self.read_document_internal(objref)?;
                // Canonicalize identity from index key. Historical payloads may carry stale IDs.
                stored_doc.document.id = id.clone();
                stored_doc.document.schema = schema.to_string();
                Ok(Some(stored_doc.document))
            } else {
                Ok(None)
            }
        }

        fn navigate_reference(
            &self,
            objref: &crate::document::ObjRef,
        ) -> OQLResult<Option<Document>> {
            if objref.is_null() {
                return Ok(None);
            }

            let zig_objref = ObjRef::new(objref.offset());
            match self.read_document_internal(zig_objref) {
                Ok(stored_doc) => Ok(Some(stored_doc.document)),
                Err(_) => Ok(None),
            }
        }

        fn put_document(&self, document: &Document) -> OQLResult<()> {
            let key = (document.schema.clone(), document.id.clone());
            let existing = {
                let index = self
                    .document_index
                    .read()
                    .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
                index.get(&key).copied()
            };
            let old_document = existing.and_then(|objref| {
                self.read_document_internal(objref)
                    .ok()
                    .map(|s| (objref, s.document))
            });

            let stored_doc = StoredDocument {
                schema: document.schema.clone(),
                id: document.id.clone(),
                document: document.clone(),
            };
            let new_objref = self.store_document_internal(&stored_doc)?;

            let mut index = self
                .document_index
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            index.insert(key, new_objref);
            drop(index);
            if let Some((_, old_doc)) = old_document.as_ref() {
                self.remove_reverse_references_for_document(old_doc)?;
            }
            self.add_reverse_references_for_document(document)?;
            if let Some(manager) = self.scalar_index_manager()? {
                match old_document {
                    Some((old_objref, old_doc)) => manager
                        .update_document(
                            &old_doc,
                            ObjRef::new(old_objref.offset()),
                            document,
                            ObjRef::new(new_objref.offset()),
                        )
                        .map_err(|e| {
                            OQLError::generic(&format!("Failed updating scalar indexes: {}", e))
                        })?,
                    None => manager
                        .index_document(document, ObjRef::new(new_objref.offset()))
                        .map_err(|e| {
                            OQLError::generic(&format!("Failed indexing document: {}", e))
                        })?,
                }
            }
            let _ = self.persist_superblock(false);
            Ok(())
        }

        fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool> {
            self.enforce_reference_delete_policies(schema, id)?;

            let old_document = {
                let index = self
                    .document_index
                    .read()
                    .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
                let key = (schema.to_string(), id.clone());
                index
                    .get(&key)
                    .copied()
                    .and_then(|objref| self.read_document_internal(objref).ok().map(|s| s.document))
            };
            self.append_log_entry(&StorageLogEntry::Delete {
                txn_id: self.current_txn_id()?,
                schema: schema.to_string(),
                id: id.clone(),
            })?;

            let mut index = self
                .document_index
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            let key = (schema.to_string(), id.clone());
            let removed = index.remove(&key).is_some();
            drop(index);
            if removed {
                if let Some(doc) = old_document.as_ref() {
                    self.remove_reverse_references_for_document(doc)?;
                }
                if let (Some(manager), Some(doc)) =
                    (self.scalar_index_manager()?, old_document.as_ref())
                {
                    manager.remove_document(doc).map_err(|e| {
                        OQLError::generic(&format!("Failed removing scalar index entries: {}", e))
                    })?;
                }
            }
            let _ = self.persist_superblock(false);
            Ok(removed)
        }

        fn scan_documents(&self, schema: &str) -> OQLResult<Vec<Document>> {
            let index = self
                .document_index
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;

            let mut documents = Vec::new();
            for ((doc_schema, doc_id), &objref) in index.iter() {
                if doc_schema == schema {
                    if let Ok(mut stored_doc) = self.read_document_internal(objref) {
                        // Canonicalize identity from index key. Historical payloads may carry stale IDs.
                        stored_doc.document.id = doc_id.clone();
                        stored_doc.document.schema = doc_schema.clone();
                        documents.push(stored_doc.document);
                    }
                }
            }

            Ok(documents)
        }

        fn get_schema(&self, name: &str) -> OQLResult<Option<Schema>> {
            let schemas = self
                .schemas
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
            Ok(schemas.get(name).cloned())
        }

        fn put_schema(&self, schema: &Schema) -> OQLResult<()> {
            self.append_log_entry(&StorageLogEntry::PutSchema {
                txn_id: self.current_txn_id()?,
                schema: schema.clone(),
            })?;

            let mut schemas = self
                .schemas
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            schemas.insert(schema.name.clone(), schema.clone());
            drop(schemas);
            let _ = self.persist_superblock(false);
            Ok(())
        }

        fn delete_schema(&self, name: &str) -> OQLResult<bool> {
            self.append_log_entry(&StorageLogEntry::DeleteSchema {
                txn_id: self.current_txn_id()?,
                name: name.to_string(),
            })?;

            let mut schemas = self
                .schemas
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire write lock"))?;
            let removed = schemas.remove(name).is_some();
            drop(schemas);
            let _ = self.persist_superblock(false);
            Ok(removed)
        }

        fn list_schemas(&self) -> OQLResult<Vec<String>> {
            let schemas = self
                .schemas
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire read lock"))?;
            Ok(schemas.keys().cloned().collect())
        }

        fn upsert_scalar_index(&self, index: super::ScalarIndexDefinition) -> OQLResult<()> {
            self.append_log_entry(&StorageLogEntry::PutScalarIndex {
                txn_id: self.current_txn_id()?,
                index: index.clone(),
            })?;

            let mut catalog = self
                .scalar_indexes
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire scalar index lock"))?;
            let entries = catalog.entry(index.schema.clone()).or_default();
            if let Some(existing) = entries.iter_mut().find(|e| e.name == index.name) {
                *existing = index;
            } else {
                entries.push(index);
            }
            drop(catalog);
            self.refresh_runtime_scalar_indexes()?;
            let _ = self.persist_superblock(false);
            Ok(())
        }

        fn delete_scalar_index(&self, schema: &str, name: &str) -> OQLResult<bool> {
            self.append_log_entry(&StorageLogEntry::DeleteScalarIndex {
                txn_id: self.current_txn_id()?,
                schema: schema.to_string(),
                name: name.to_string(),
            })?;

            let mut catalog = self
                .scalar_indexes
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire scalar index lock"))?;
            let mut removed = false;
            if let Some(entries) = catalog.get_mut(schema) {
                let before = entries.len();
                entries.retain(|entry| entry.name != name);
                removed = before != entries.len();
            }
            drop(catalog);
            self.refresh_runtime_scalar_indexes()?;
            let _ = self.persist_superblock(false);
            Ok(removed)
        }

        fn list_scalar_indexes(
            &self,
            schema: Option<&str>,
        ) -> OQLResult<Vec<super::ScalarIndexDefinition>> {
            let catalog = self
                .scalar_indexes
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire scalar index lock"))?;
            let mut out = Vec::new();
            match schema {
                Some(target) => {
                    if let Some(entries) = catalog.get(target) {
                        out.extend(entries.clone());
                    }
                }
                None => {
                    for entries in catalog.values() {
                        out.extend(entries.clone());
                    }
                }
            }
            out.sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.name.cmp(&b.name)));
            Ok(out)
        }

        fn list_references_to(
            &self,
            target_schema: &str,
            target_id: &DocumentId,
            source_schema: Option<&str>,
            field_path: Option<&str>,
        ) -> OQLResult<Vec<super::ReverseReferenceEntry>> {
            let reverse = self
                .reverse_references
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire reverse reference lock"))?;
            let key = (target_schema.to_string(), target_id.clone());
            let entries = reverse.get(&key).cloned().unwrap_or_default();
            Ok(entries
                .into_iter()
                .filter(|entry| {
                    source_schema
                        .map(|expected| entry.source_schema == expected)
                        .unwrap_or(true)
                        && field_path
                            .map(|expected| entry.field_path == expected)
                            .unwrap_or(true)
                })
                .collect())
        }

        fn upsert_vector_index(&self, index: super::VectorIndexDefinition) -> OQLResult<()> {
            self.append_log_entry(&StorageLogEntry::PutVectorIndex {
                txn_id: self.current_txn_id()?,
                index: index.clone(),
            })?;

            let mut catalog = self
                .vector_indexes
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire vector index lock"))?;
            let entries = catalog.entry(index.schema.clone()).or_default();
            if let Some(existing) = entries.iter_mut().find(|e| e.name == index.name) {
                *existing = index;
            } else {
                entries.push(index);
            }
            drop(catalog);
            let _ = self.persist_superblock(false);
            Ok(())
        }

        fn delete_vector_index(&self, schema: &str, name: &str) -> OQLResult<bool> {
            self.append_log_entry(&StorageLogEntry::DeleteVectorIndex {
                txn_id: self.current_txn_id()?,
                schema: schema.to_string(),
                name: name.to_string(),
            })?;

            let mut catalog = self
                .vector_indexes
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire vector index lock"))?;
            let mut removed = false;
            if let Some(entries) = catalog.get_mut(schema) {
                let before = entries.len();
                entries.retain(|entry| entry.name != name);
                removed = before != entries.len();
            }
            drop(catalog);
            if removed {
                let _ = std::fs::remove_file(self.vector_index_file_path_by_name(schema, name));
            }
            let _ = self.persist_superblock(false);
            Ok(removed)
        }

        fn list_vector_indexes(
            &self,
            schema: Option<&str>,
        ) -> OQLResult<Vec<super::VectorIndexDefinition>> {
            let catalog = self
                .vector_indexes
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire vector index lock"))?;
            let mut out = Vec::new();
            match schema {
                Some(target) => {
                    if let Some(entries) = catalog.get(target) {
                        out.extend(entries.clone());
                    }
                }
                None => {
                    for entries in catalog.values() {
                        out.extend(entries.clone());
                    }
                }
            }
            out.sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.name.cmp(&b.name)));
            Ok(out)
        }

        fn vector_ann_search(
            &self,
            index: &super::VectorIndexDefinition,
            documents: &[Document],
            query: &[f32],
            k: usize,
            num_candidates: usize,
        ) -> OQLResult<Option<Vec<super::VectorAnnHit>>> {
            if documents.is_empty() || k == 0 {
                return Ok(Some(Vec::new()));
            }
            if index.index_type != super::VectorIndexType::Hnsw {
                return Ok(None);
            }
            if index.metric != super::VectorMetricType::Cosine {
                return Ok(None);
            }
            if query.len() != index.dimension as usize {
                return Err(OQLError::TypeError {
                    expected: format!("Vector({})", index.dimension),
                    actual: format!("Vector({})", query.len()),
                });
            }

            // Only use persisted collection-level HNSW when this operation is over
            // the full collection; otherwise caller should fallback to exact rerank.
            let full_docs = self.scan_documents(&index.schema)?;
            if full_docs.len() != documents.len() {
                return Ok(None);
            }
            let full_set: std::collections::HashSet<&str> =
                full_docs.iter().map(|d| d.id.as_str()).collect();
            if documents.iter().any(|d| !full_set.contains(d.id.as_str())) {
                return Ok(None);
            }

            let index_path = self.vector_index_file_path(index);
            let index_path_str = index_path.to_string_lossy().to_string();

            let mut hnsw = if std::path::Path::new(&index_path).exists() {
                owl_storage_zig::HnswIndex::load(&index_path_str).ok()
            } else {
                None
            };

            if hnsw.is_none() {
                let _ = self.build_hnsw_for_definition(index)?;
                hnsw = owl_storage_zig::HnswIndex::load(&index_path_str).ok();
            }

            let Some(hnsw) = hnsw else {
                return Ok(None);
            };
            let ef = num_candidates.max(k);
            let hits = hnsw.search(query, k, ef).map_err(|e| {
                OQLError::storage_error(&format!("HNSW ANN search failed: {}", e), None)
            })?;

            let mut doc_index_by_vector_id =
                std::collections::HashMap::with_capacity(documents.len());
            for (i, d) in documents.iter().enumerate() {
                doc_index_by_vector_id.insert(Self::stable_vector_id(&d.id), i);
            }

            let mut out = Vec::new();
            for hit in hits {
                if let Some(&doc_index) = doc_index_by_vector_id.get(&hit.id) {
                    out.push(super::VectorAnnHit {
                        index: doc_index,
                        score: hit.score,
                    });
                }
            }
            Ok(Some(out))
        }

        fn build_vector_index(
            &self,
            schema: &str,
            name: &str,
            force_rebuild: bool,
        ) -> OQLResult<super::VectorIndexBuildReport> {
            let index = self
                .find_vector_index_definition(schema, name)?
                .ok_or_else(|| {
                    OQLError::generic(&format!(
                        "vector index '{}.{}' does not exist",
                        schema, name
                    ))
                })?;
            if index.index_type != super::VectorIndexType::Hnsw {
                return Err(OQLError::generic(
                    "vector index build currently supports only hnsw indexes",
                ));
            }

            let index_path = self.vector_index_file_path(&index);
            if force_rebuild {
                let _ = std::fs::remove_file(&index_path);
            } else if index_path.exists() {
                let docs = self.scan_documents(&index.schema)?;
                return Ok(super::VectorIndexBuildReport {
                    indexed_documents: docs.len(),
                    skipped_documents: 0,
                    total_documents: docs.len(),
                });
            }
            self.build_hnsw_for_definition(&index)
        }

        fn vector_index_status(
            &self,
            schema: &str,
            name: &str,
        ) -> OQLResult<Option<super::VectorIndexStatus>> {
            let index = match self.find_vector_index_definition(schema, name)? {
                Some(index) => index,
                None => return Ok(None),
            };
            let index_path = self.vector_index_file_path(&index);
            let metadata = std::fs::metadata(&index_path).ok();
            let file_exists = metadata.is_some();
            let file_size_bytes = metadata.as_ref().map(|m| m.len());
            let file_mtime_unix_seconds = metadata
                .and_then(|m| m.modified().ok())
                .and_then(|ts| ts.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            Ok(Some(super::VectorIndexStatus {
                schema: schema.to_string(),
                name: name.to_string(),
                state: index.state,
                file_exists,
                file_size_bytes,
                file_mtime_unix_seconds,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Value;
    use crate::simulation::SimulatedFileSystem;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;

    fn create_test_document(schema: &str, id: &str) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String("Test".to_string()));
        fields.insert("age".to_string(), Value::Integer(25));

        Document::with_id(id.to_string(), schema.to_string(), fields)
    }

    fn cleanup_storage_files(path: &str) {
        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_creation() {
        let storage = create_storage("test_storage.owl", 10 * 1024 * 1024);
        assert!(storage.is_ok());
        std::fs::remove_file("test_storage.owl").ok();
    }

    #[test]
    fn test_put_and_get_document() {
        let storage = create_storage("test_put_get.owl", 10 * 1024 * 1024).unwrap();
        let doc = create_test_document("users", "user1");

        // Put document
        storage.put_document(&doc).unwrap();

        // Get document
        let retrieved = storage.get_document("users", &doc.id).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, doc.id);

        std::fs::remove_file("test_put_get.owl").ok();
    }

    #[test]
    fn test_delete_document() {
        let storage = create_storage("test_delete.owl", 10 * 1024 * 1024).unwrap();
        let doc = create_test_document("users", "user1");

        // Put and delete
        storage.put_document(&doc).unwrap();
        let deleted = storage.delete_document("users", &doc.id).unwrap();
        assert!(deleted);

        // Verify deleted
        let retrieved = storage.get_document("users", &doc.id).unwrap();
        assert!(retrieved.is_none());

        std::fs::remove_file("test_delete.owl").ok();
    }

    #[test]
    fn test_scan_documents() {
        let storage = create_storage("test_scan.owl", 10 * 1024 * 1024).unwrap();

        // Put multiple documents
        for i in 0..5 {
            let doc = create_test_document("users", &format!("user{}", i));
            storage.put_document(&doc).unwrap();
        }

        // Scan
        let docs = storage.scan_documents("users").unwrap();
        assert_eq!(docs.len(), 5);

        std::fs::remove_file("test_scan.owl").ok();
    }

    #[test]
    fn test_schema_operations() {
        let storage = create_storage("test_schema.owl", 10 * 1024 * 1024).unwrap();

        let schema = Schema::new("users".to_string(), HashMap::new());

        // Put schema
        storage.put_schema(&schema).unwrap();

        // Get schema
        let retrieved = storage.get_schema("users").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "users");

        // List schemas
        let schemas = storage.list_schemas().unwrap();
        assert_eq!(schemas.len(), 1);
        assert!(schemas.contains(&"users".to_string()));

        std::fs::remove_file("test_schema.owl").ok();
    }

    #[test]
    fn test_storage_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        assert_send::<Storage>();
        assert_sync::<Storage>();
    }

    #[test]
    fn test_storage_recovers_documents_after_restart() {
        let path = "test_recover_docs.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            let doc = create_test_document("users", "user_restart");
            storage.put_document(&doc).unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let doc = reopened
            .get_document("users", &"user_restart".to_string())
            .unwrap();
        assert!(doc.is_some());
        assert_eq!(doc.unwrap().id, "user_restart");

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_recovers_schemas_after_restart() {
        let path = "test_recover_schema.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            let schema = Schema::new("accounts".to_string(), HashMap::new());
            storage.put_schema(&schema).unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let schema = reopened.get_schema("accounts").unwrap();
        assert!(schema.is_some());
        assert_eq!(schema.unwrap().name, "accounts");

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_delete_persists_after_restart() {
        let path = "test_recover_delete.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            let doc = create_test_document("users", "user_delete");
            storage.put_document(&doc).unwrap();
            assert!(storage
                .delete_document("users", &"user_delete".to_string())
                .unwrap());
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let doc = reopened
            .get_document("users", &"user_delete".to_string())
            .unwrap();
        assert!(doc.is_none());

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_crash_recovery_with_simulated_fs() {
        let fs = Arc::new(SimulatedFileSystem::new(42));
        let path = "sim_recover.owl";

        {
            let storage = create_storage_with_fs(path, 10 * 1024 * 1024, fs.clone()).unwrap();
            let doc = create_test_document("users", "sim_user");
            storage.put_document(&doc).unwrap();
        }

        fs.crash();
        fs.recover();

        let reopened = create_storage_with_fs(path, 10 * 1024 * 1024, fs).unwrap();
        let doc = reopened
            .get_document("users", &"sim_user".to_string())
            .unwrap();
        assert!(doc.is_some());
    }

    #[test]
    fn test_storage_recovers_with_truncated_wal_tail() {
        let path = "test_truncated_tail.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            let doc = create_test_document("users", "tail_user");
            storage.put_document(&doc).unwrap();
        }

        // Simulate power-loss torn write at WAL tail.
        use std::io::Write;
        let mut wal = std::fs::OpenOptions::new()
            .append(true)
            .open(format!("{}.wal", path))
            .unwrap();
        wal.write_all(&[8, 0, 0, 0, 1, 2]).unwrap();
        wal.sync_all().unwrap();

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let doc = reopened
            .get_document("users", &"tail_user".to_string())
            .unwrap();
        assert!(doc.is_some());

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_recovers_legacy_wal_format() {
        let path = "test_legacy_wal.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            storage
                .put_schema(&Schema::new("legacy_schema".to_string(), HashMap::new()))
                .unwrap();
        }

        // Convert a newly-written framed WAL entry into legacy format [len][payload].
        let wal_path = format!("{}.wal", path);
        let wal_bytes = std::fs::read(&wal_path).unwrap();
        assert!(wal_bytes.len() > 16);
        let magic = u32::from_le_bytes([wal_bytes[0], wal_bytes[1], wal_bytes[2], wal_bytes[3]]);
        assert_eq!(magic, 0x4c41574f); // "OWAL"
        let payload_len =
            u32::from_le_bytes([wal_bytes[8], wal_bytes[9], wal_bytes[10], wal_bytes[11]]) as usize;
        assert!(wal_bytes.len() >= 16 + payload_len);
        let payload = &wal_bytes[16..16 + payload_len];

        use std::io::Write;
        let mut legacy_wal = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&wal_path)
            .unwrap();
        legacy_wal
            .write_all(&(payload.len() as u32).to_le_bytes())
            .unwrap();
        legacy_wal.write_all(payload).unwrap();
        legacy_wal.sync_all().unwrap();

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let schema = reopened.get_schema("legacy_schema").unwrap();
        assert!(schema.is_some());

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_recovers_with_partial_superblock_copyset() {
        let path = "test_superblock_partial.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            storage
                .put_schema(&Schema::new("sb_schema".to_string(), HashMap::new()))
                .unwrap();
        }

        let sb_path = format!("{}.sb", path);
        let mut sb_bytes = std::fs::read(&sb_path).unwrap();
        assert!(sb_bytes.len() >= 128);
        // Corrupt copy 0; remaining copies should still allow recovery.
        for b in &mut sb_bytes[0..128] {
            *b = 0;
        }
        std::fs::write(&sb_path, sb_bytes).unwrap();

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let schema = reopened.get_schema("sb_schema").unwrap();
        assert!(schema.is_some());

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
        std::fs::remove_file(sb_path).ok();
    }

    #[test]
    fn test_storage_superblock_strict_policy_rejects_singleton_copy() {
        let path = "test_superblock_strict.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            storage
                .put_schema(&Schema::new("strict_schema".to_string(), HashMap::new()))
                .unwrap();
        }

        let sb_path = format!("{}.sb", path);
        let mut sb_bytes = std::fs::read(&sb_path).unwrap();
        assert!(sb_bytes.len() >= 4 * 128);
        // Corrupt 3/4 copies so only one valid copy remains.
        for b in &mut sb_bytes[0..(3 * 128)] {
            *b = 0;
        }
        std::fs::write(&sb_path, sb_bytes).unwrap();

        use crate::simulation::RealFileSystem;
        let fs = Arc::new(RealFileSystem::new());
        let strict_open = zig_adapter::ZigStore::new_with_fs_strict(path, 10 * 1024 * 1024, fs);
        assert!(strict_open.is_err());
        assert!(strict_open
            .err()
            .unwrap()
            .to_string()
            .contains("Strict superblock validation failed"));

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_recovery_ignores_uncommitted_transaction_entries() {
        let path = "test_txn_uncommitted_recovery.owl";
        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            let tx_id = storage.begin_local_transaction().unwrap();
            let doc = create_test_document("users", "user_txn_pending");
            storage.put_document(&doc).unwrap();
            // Emit explicit rollback marker to guarantee non-commit semantics.
            storage.rollback_local_transaction(tx_id).unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let doc = reopened
            .get_document("users", &"user_txn_pending".to_string())
            .unwrap();
        assert!(doc.is_none(), "rolled back transaction should not recover");

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_recovery_applies_committed_transaction_entries() {
        let path = "test_txn_committed_recovery.owl";
        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            let tx_id = storage.begin_local_transaction().unwrap();
            let doc = create_test_document("users", "user_txn_committed");
            storage.put_document(&doc).unwrap();
            storage.commit_local_transaction(tx_id).unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let doc = reopened
            .get_document("users", &"user_txn_committed".to_string())
            .unwrap();
        assert!(doc.is_some(), "committed transaction should recover");

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_recovery_ignores_open_transaction_without_commit_marker() {
        let path = "test_txn_open_without_commit.owl";
        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            let _tx_id = storage.begin_local_transaction().unwrap();
            let doc = create_test_document("users", "user_txn_open");
            storage.put_document(&doc).unwrap();
            // Simulate crash: no commit/rollback marker.
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let doc = reopened
            .get_document("users", &"user_txn_open".to_string())
            .unwrap();
        assert!(
            doc.is_none(),
            "open transaction without commit marker must not recover"
        );

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_reverse_reference_index_tracks_put_update_and_delete() {
        let path = "test_reverse_refs.owl";
        let storage = create_storage(path, 10 * 1024 * 1024).unwrap();

        let target = create_test_document("users", "user_target");
        let other = create_test_document("users", "user_other");
        storage.put_document(&target).unwrap();
        storage.put_document(&other).unwrap();

        let target_ref = storage
            .get_objref("users", &"user_target".to_string())
            .unwrap()
            .unwrap();
        let other_ref = storage
            .get_objref("users", &"user_other".to_string())
            .unwrap()
            .unwrap();

        let mut order_1_fields = HashMap::new();
        order_1_fields.insert("customer".to_string(), Value::Reference(target_ref));
        let order_1 =
            Document::with_id("order_1".to_string(), "orders".to_string(), order_1_fields);
        storage.put_document(&order_1).unwrap();

        let mut order_2_fields = HashMap::new();
        order_2_fields.insert(
            "audit".to_string(),
            Value::Object(HashMap::from([(
                "approver".to_string(),
                Value::Reference(target_ref),
            )])),
        );
        let order_2 =
            Document::with_id("order_2".to_string(), "orders".to_string(), order_2_fields);
        storage.put_document(&order_2).unwrap();

        let mut order_3_fields = HashMap::new();
        order_3_fields.insert(
            "watchers".to_string(),
            Value::List(vec![Value::Reference(target_ref)]),
        );
        let order_3 =
            Document::with_id("order_3".to_string(), "orders".to_string(), order_3_fields);
        storage.put_document(&order_3).unwrap();

        let mut edges = storage
            .list_references_to("users", &"user_target".to_string(), Some("orders"), None)
            .unwrap();
        edges.sort_by(|a, b| {
            a.field_path
                .cmp(&b.field_path)
                .then_with(|| a.source_id.cmp(&b.source_id))
        });
        assert_eq!(edges.len(), 3);
        assert_eq!(edges[0].field_path, "audit.approver");
        assert_eq!(edges[1].field_path, "customer");
        assert_eq!(edges[2].field_path, "watchers");

        let referencing_docs = storage
            .scan_referencing_documents("users", &"user_target".to_string(), Some("orders"), None)
            .unwrap();
        assert_eq!(referencing_docs.len(), 3);

        let mut updated_order_3_fields = HashMap::new();
        updated_order_3_fields.insert(
            "watchers".to_string(),
            Value::List(vec![Value::Reference(other_ref)]),
        );
        let updated_order_3 = Document::with_id(
            "order_3".to_string(),
            "orders".to_string(),
            updated_order_3_fields,
        );
        storage.put_document(&updated_order_3).unwrap();

        let edges_after_update = storage
            .list_references_to("users", &"user_target".to_string(), Some("orders"), None)
            .unwrap();
        assert_eq!(edges_after_update.len(), 2);
        assert!(edges_after_update
            .iter()
            .all(|edge| edge.field_path != "watchers"));

        assert!(storage
            .delete_document("orders", &"order_1".to_string())
            .unwrap());
        let edges_after_delete = storage
            .list_references_to("users", &"user_target".to_string(), Some("orders"), None)
            .unwrap();
        assert_eq!(edges_after_delete.len(), 1);
        assert_eq!(edges_after_delete[0].field_path, "audit.approver");

        cleanup_storage_files(path);
    }

    #[test]
    fn test_reverse_reference_index_rebuilds_after_restart() {
        let path = "test_reverse_refs_restart.owl";
        {
            let storage = create_storage(path, 10 * 1024 * 1024).unwrap();
            let target = create_test_document("users", "user_target");
            storage.put_document(&target).unwrap();
            let target_ref = storage
                .get_objref("users", &"user_target".to_string())
                .unwrap()
                .unwrap();

            let mut fields = HashMap::new();
            fields.insert("customer".to_string(), Value::Reference(target_ref));
            fields.insert(
                "review".to_string(),
                Value::Object(HashMap::from([(
                    "owner".to_string(),
                    Value::Reference(target_ref),
                )])),
            );
            let order =
                Document::with_id("order_restart".to_string(), "orders".to_string(), fields);
            storage.put_document(&order).unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let mut edges = reopened
            .list_references_to("users", &"user_target".to_string(), Some("orders"), None)
            .unwrap();
        edges.sort_by(|a, b| a.field_path.cmp(&b.field_path));
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].field_path, "customer");
        assert_eq!(edges[1].field_path, "review.owner");

        cleanup_storage_files(path);
    }

    #[test]
    fn test_delete_document_restricts_when_referenced() {
        let path = "test_delete_restrict.owl";
        let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();

        storage
            .put_schema(&Schema::new(
                "users".to_string(),
                HashMap::from([("id".to_string(), crate::schema::FieldType::String)]),
            ))
            .unwrap();
        storage
            .put_schema(&Schema::new(
                "orders".to_string(),
                HashMap::from([(
                    "customer".to_string(),
                    crate::schema::FieldType::Reference(crate::schema::ReferenceType::new(
                        "users".to_string(),
                    )),
                )]),
            ))
            .unwrap();

        let user = create_test_document("users", "user_1");
        storage.put_document(&user).unwrap();
        let user_ref = storage
            .get_objref("users", &"user_1".to_string())
            .unwrap()
            .unwrap();

        let order = Document::with_id(
            "order_1".to_string(),
            "orders".to_string(),
            HashMap::from([("customer".to_string(), Value::Reference(user_ref))]),
        );
        storage.put_document(&order).unwrap();

        let err = storage
            .delete_document("users", &"user_1".to_string())
            .expect_err("expected restrict policy to block delete");
        assert!(err.to_string().contains("still references it"));

        cleanup_storage_files(path);
    }

    #[test]
    fn test_delete_document_sets_optional_reference_to_null() {
        let path = "test_delete_set_null.owl";
        let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();

        storage
            .put_schema(&Schema::new(
                "users".to_string(),
                HashMap::from([("id".to_string(), crate::schema::FieldType::String)]),
            ))
            .unwrap();
        storage
            .put_schema(&Schema::new(
                "sessions".to_string(),
                HashMap::from([(
                    "user".to_string(),
                    crate::schema::FieldType::Reference(crate::schema::ReferenceType::optional(
                        "users".to_string(),
                    )),
                )]),
            ))
            .unwrap();

        let user = create_test_document("users", "user_1");
        storage.put_document(&user).unwrap();
        let user_ref = storage
            .get_objref("users", &"user_1".to_string())
            .unwrap()
            .unwrap();

        let session = Document::with_id(
            "session_1".to_string(),
            "sessions".to_string(),
            HashMap::from([("user".to_string(), Value::Reference(user_ref))]),
        );
        storage.put_document(&session).unwrap();

        assert!(storage
            .delete_document("users", &"user_1".to_string())
            .unwrap());
        let updated_session = storage
            .get_document("sessions", &"session_1".to_string())
            .unwrap()
            .expect("session should remain");
        assert_eq!(updated_session.get_field("user"), Some(&Value::Null));

        cleanup_storage_files(path);
    }

    #[test]
    fn test_delete_document_cascades_to_referencing_documents() {
        let path = "test_delete_cascade.owl";
        let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();

        storage
            .put_schema(&Schema::new(
                "users".to_string(),
                HashMap::from([("id".to_string(), crate::schema::FieldType::String)]),
            ))
            .unwrap();
        storage
            .put_schema(&Schema::new(
                "sessions".to_string(),
                HashMap::from([(
                    "user".to_string(),
                    crate::schema::FieldType::Reference(
                        crate::schema::ReferenceType::with_cascade(
                            "users".to_string(),
                            crate::schema::CascadeBehavior::Cascade,
                        ),
                    ),
                )]),
            ))
            .unwrap();

        let user = create_test_document("users", "user_1");
        storage.put_document(&user).unwrap();
        let user_ref = storage
            .get_objref("users", &"user_1".to_string())
            .unwrap()
            .unwrap();

        let session = Document::with_id(
            "session_1".to_string(),
            "sessions".to_string(),
            HashMap::from([("user".to_string(), Value::Reference(user_ref))]),
        );
        storage.put_document(&session).unwrap();

        assert!(storage
            .delete_document("users", &"user_1".to_string())
            .unwrap());
        assert!(storage
            .get_document("sessions", &"session_1".to_string())
            .unwrap()
            .is_none());

        cleanup_storage_files(path);
    }

    #[test]
    fn test_vector_index_catalog_persists_after_restart() {
        let path = "test_vector_index_restart.owl";
        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            storage
                .upsert_vector_index(super::VectorIndexDefinition {
                    name: "idx_users_embedding".to_string(),
                    schema: "users".to_string(),
                    field: "embedding".to_string(),
                    dimension: 3,
                    metric: super::VectorMetricType::Cosine,
                    index_type: super::VectorIndexType::Exact,
                    quantization: None,
                    state: super::VectorIndexState::Ready,
                })
                .unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let indexes = reopened.list_vector_indexes(Some("users")).unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].name, "idx_users_embedding");

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_scalar_index_catalog_persists_after_restart() {
        let path = "test_scalar_index_restart.owl";
        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            storage
                .upsert_scalar_index(super::ScalarIndexDefinition {
                    name: "idx_users_profile_country".to_string(),
                    schema: "users".to_string(),
                    fields: vec!["profile.country".to_string()],
                    unique: false,
                })
                .unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let indexes = reopened.list_scalar_indexes(Some("users")).unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].name, "idx_users_profile_country");
        assert_eq!(indexes[0].fields, vec!["profile.country".to_string()]);

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_scalar_index_recovery_ignores_uncommitted_transaction_entry() {
        let path = "test_scalar_index_txn_recovery.owl";
        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            let tx_id = storage.begin_local_transaction().unwrap();
            storage
                .upsert_scalar_index(super::ScalarIndexDefinition {
                    name: "idx_users_email".to_string(),
                    schema: "users".to_string(),
                    fields: vec!["email".to_string()],
                    unique: true,
                })
                .unwrap();
            storage.rollback_local_transaction(tx_id).unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let indexes = reopened.list_scalar_indexes(Some("users")).unwrap();
        assert!(
            indexes.is_empty(),
            "rolled back scalar index entry must not recover"
        );

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_runtime_scalar_indexes_track_put_and_delete() {
        let path = "test_runtime_scalar_indexes.owl";
        let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
        storage
            .upsert_scalar_index(super::ScalarIndexDefinition {
                name: "idx_users_profile.country".to_string(),
                schema: "users".to_string(),
                fields: vec!["profile.country".to_string()],
                unique: false,
            })
            .unwrap();

        let mut profile = HashMap::new();
        profile.insert("country".to_string(), Value::String("NG".to_string()));
        let mut fields = HashMap::new();
        fields.insert("profile".to_string(), Value::Object(profile));
        let doc = Document::with_id("u1".to_string(), "users".to_string(), fields);
        storage.put_document(&doc).unwrap();

        let manager = storage
            .scalar_index_manager()
            .unwrap()
            .expect("runtime scalar manager");
        let indexes = manager.get_indexes("users").unwrap();
        let idx = indexes
            .iter()
            .find(|i| i.name == "idx_users_profile.country")
            .expect("nested scalar index");
        let hits = idx
            .range_query(
                wyhash::wyhash("NG".as_bytes(), 0),
                wyhash::wyhash("NG".as_bytes(), 0),
            )
            .unwrap();
        assert_eq!(hits.len(), 1);

        storage.delete_document("users", &"u1".to_string()).unwrap();
        let hits_after = idx
            .range_query(
                wyhash::wyhash("NG".as_bytes(), 0),
                wyhash::wyhash("NG".as_bytes(), 0),
            )
            .unwrap();
        assert!(hits_after.is_empty());

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_vector_index_recovery_ignores_uncommitted_transaction_entry() {
        let path = "test_vector_index_txn_recovery.owl";
        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            let tx_id = storage.begin_local_transaction().unwrap();
            storage
                .upsert_vector_index(super::VectorIndexDefinition {
                    name: "idx_users_embedding".to_string(),
                    schema: "users".to_string(),
                    field: "embedding".to_string(),
                    dimension: 3,
                    metric: super::VectorMetricType::Cosine,
                    index_type: super::VectorIndexType::Exact,
                    quantization: None,
                    state: super::VectorIndexState::Ready,
                })
                .unwrap();
            storage.rollback_local_transaction(tx_id).unwrap();
        }

        let reopened = create_storage(path, 10 * 1024 * 1024).unwrap();
        let indexes = reopened.list_vector_indexes(Some("users")).unwrap();
        assert!(
            indexes.is_empty(),
            "rolled back vector index entry must not recover"
        );

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_hnsw_vector_index_builds_persists_and_recovers() {
        let path = "test_hnsw_vector_index.owl";
        let index = super::VectorIndexDefinition {
            name: "idx_users_embedding_hnsw".to_string(),
            schema: "users".to_string(),
            field: "embedding".to_string(),
            dimension: 3,
            metric: super::VectorMetricType::Cosine,
            index_type: super::VectorIndexType::Hnsw,
            quantization: None,
            state: super::VectorIndexState::Ready,
        };

        {
            let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            storage.upsert_vector_index(index.clone()).unwrap();

            let mut f1 = HashMap::new();
            f1.insert(
                "embedding".to_string(),
                Value::List(vec![
                    Value::Float(1.0),
                    Value::Float(0.0),
                    Value::Float(0.0),
                ]),
            );
            let mut f2 = HashMap::new();
            f2.insert(
                "embedding".to_string(),
                Value::List(vec![
                    Value::Float(0.0),
                    Value::Float(1.0),
                    Value::Float(0.0),
                ]),
            );
            let mut f3 = HashMap::new();
            f3.insert(
                "embedding".to_string(),
                Value::List(vec![
                    Value::Float(0.9),
                    Value::Float(0.1),
                    Value::Float(0.0),
                ]),
            );
            storage
                .put_document(&Document::with_id(
                    "u1".to_string(),
                    "users".to_string(),
                    f1,
                ))
                .unwrap();
            storage
                .put_document(&Document::with_id(
                    "u2".to_string(),
                    "users".to_string(),
                    f2,
                ))
                .unwrap();
            storage
                .put_document(&Document::with_id(
                    "u3".to_string(),
                    "users".to_string(),
                    f3,
                ))
                .unwrap();

            let docs = storage.scan_documents("users").unwrap();
            let hits = storage
                .vector_ann_search(&index, &docs, &[1.0, 0.0, 0.0], 2, 8)
                .unwrap()
                .unwrap();
            assert_eq!(hits.len(), 2);
            let path = format!("{}.vindex/users__idx_users_embedding_hnsw.hnsw", path);
            assert!(std::path::Path::new(&path).exists());
        }

        {
            let reopened = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
            let docs = reopened.scan_documents("users").unwrap();
            let hits = reopened
                .vector_ann_search(&index, &docs, &[1.0, 0.0, 0.0], 2, 8)
                .unwrap()
                .unwrap();
            assert_eq!(hits.len(), 2);
        }

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
        std::fs::remove_file(format!(
            "{}.vindex/users__idx_users_embedding_hnsw.hnsw",
            path
        ))
        .ok();
        std::fs::remove_dir(format!("{}.vindex", path)).ok();
    }

    #[test]
    fn test_hnsw_vector_index_status_reports_file_metadata() {
        let path = "test_hnsw_vector_index_status.owl";
        let index = super::VectorIndexDefinition {
            name: "idx_users_embedding_hnsw".to_string(),
            schema: "users".to_string(),
            field: "embedding".to_string(),
            dimension: 3,
            metric: super::VectorMetricType::Cosine,
            index_type: super::VectorIndexType::Hnsw,
            quantization: None,
            state: super::VectorIndexState::Ready,
        };

        let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
        storage.upsert_vector_index(index.clone()).unwrap();

        let status_before = storage
            .vector_index_status("users", "idx_users_embedding_hnsw")
            .unwrap()
            .unwrap();
        assert!(!status_before.file_exists);

        let mut f = HashMap::new();
        f.insert(
            "embedding".to_string(),
            Value::List(vec![
                Value::Float(1.0),
                Value::Float(0.0),
                Value::Float(0.0),
            ]),
        );
        storage
            .put_document(&Document::with_id("u1".to_string(), "users".to_string(), f))
            .unwrap();
        storage
            .build_vector_index("users", "idx_users_embedding_hnsw", true)
            .unwrap();

        let status_after = storage
            .vector_index_status("users", "idx_users_embedding_hnsw")
            .unwrap()
            .unwrap();
        assert!(status_after.file_exists);
        assert!(status_after.file_size_bytes.unwrap_or(0) > 0);

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
        std::fs::remove_file(format!(
            "{}.vindex/users__idx_users_embedding_hnsw.hnsw",
            path
        ))
        .ok();
        std::fs::remove_dir(format!("{}.vindex", path)).ok();
    }

    #[test]
    fn test_storage_rejects_nested_local_transaction_begin() {
        let path = "test_txn_nested_begin.owl";
        let storage = zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap();
        let tx1 = storage.begin_local_transaction().unwrap();
        let tx2 = storage.begin_local_transaction();
        assert!(tx2.is_err(), "nested local transactions should be rejected");
        storage.rollback_local_transaction(tx1).unwrap();

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_concurrent_begin_allows_single_active_transaction() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use std::time::Duration;

        let path = "test_txn_concurrent_begin.owl";
        let storage = Arc::new(zig_adapter::ZigStore::new(path, 10 * 1024 * 1024).unwrap());
        let barrier = Arc::new(Barrier::new(2));

        let s1 = Arc::clone(&storage);
        let b1 = Arc::clone(&barrier);
        let t1 = thread::spawn(move || {
            b1.wait();
            let tx = s1.begin_local_transaction().expect("thread1 begin");
            thread::sleep(Duration::from_millis(75));
            s1.commit_local_transaction(tx).expect("thread1 commit");
            true
        });

        let s2 = Arc::clone(&storage);
        let b2 = Arc::clone(&barrier);
        let t2 = thread::spawn(move || {
            b2.wait();
            thread::sleep(Duration::from_millis(10));
            s2.begin_local_transaction().is_err()
        });

        let thread1_ok = t1.join().expect("join thread1");
        let thread2_saw_reject = t2.join().expect("join thread2");
        assert!(thread1_ok);
        assert!(
            thread2_saw_reject,
            "concurrent second transaction should be rejected while first is active"
        );

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }

    #[test]
    fn test_storage_performance_guardrail_smoke() {
        let path = "perf_guardrail.owl";
        let storage = create_storage(path, 64 * 1024 * 1024).unwrap();

        let count = 500usize;
        let start_put = Instant::now();
        for i in 0..count {
            let doc = create_test_document("users", &format!("user{}", i));
            storage.put_document(&doc).unwrap();
        }
        let put_elapsed = start_put.elapsed();

        let start_get = Instant::now();
        for i in 0..count {
            let id = format!("user{}", i);
            let doc = storage.get_document("users", &id).unwrap();
            assert!(doc.is_some());
        }
        let get_elapsed = start_get.elapsed();

        let avg_put_us = put_elapsed.as_micros() as f64 / count as f64;
        let avg_get_us = get_elapsed.as_micros() as f64 / count as f64;

        // Loose CI guardrail to catch major regressions, not microbenchmark precision.
        assert!(avg_put_us < 10_000.0, "avg put too slow: {}us", avg_put_us);
        assert!(avg_get_us < 10_000.0, "avg get too slow: {}us", avg_get_us);

        std::fs::remove_file(path).ok();
        std::fs::remove_file(format!("{}.wal", path)).ok();
        std::fs::remove_file(format!("{}.sb", path)).ok();
    }
}
