// Distributed Write-Ahead Log (WAL) with Checkpointing
//
// This module implements WAL-first durability for distributed operations:
// - Enhanced WAL entries with shard_id and distributed transaction support
// - WAL-first write pattern: WAL → Arena → Acknowledge
// - Reservation/commit pattern for atomic cross-shard operations
// - Distributed checkpointing system for cluster-wide consistency

use crate::distributed::consistent_hash::ShardId;
use crate::distributed::global_objref::GlobalObjRef;
use crate::error::{OQLError, OQLResult};
use crate::simulation::filesystem::FileSystem;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Log Sequence Number for ordering WAL entries
pub type LSN = u64;

/// Transaction ID for distributed transactions
pub type TxnId = u64;

/// WAL entry types for different operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WALEntryType {
    /// Insert a new object
    Insert,
    /// Update an existing object
    Update,
    /// Delete an object
    Delete,
    /// Begin a distributed transaction
    TxnBegin,
    /// Commit a distributed transaction
    TxnCommit,
    /// Rollback a distributed transaction
    TxnRollback,
    /// Checkpoint marker
    Checkpoint,
    /// Shard rebalancing operation
    Rebalance,
}

/// WAL entry with distributed transaction support
///
/// Layout matches design document:
/// - lsn: u64 (8 bytes)
/// - entry_type: u8 (1 byte)
/// - shard_id: u32 (4 bytes)
/// - data_size: u32 (4 bytes)
/// - objref: GlobalObjRef (16 bytes)
/// - checksum: u32 (4 bytes)
/// - txn_id: Option<u64> (9 bytes: 1 flag + 8 bytes)
/// - timestamp: u64 (8 bytes)
/// Total header: 54 bytes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WALEntry {
    /// Log sequence number (monotonically increasing)
    pub lsn: LSN,
    /// Type of operation
    pub entry_type: WALEntryType,
    /// Shard that owns this operation
    pub shard_id: ShardId,
    /// Size of the data payload
    pub data_size: u32,
    /// Global object reference
    pub objref: GlobalObjRef,
    /// CRC32 checksum of the data
    pub checksum: u32,
    /// Transaction ID for distributed transactions
    pub txn_id: Option<TxnId>,
    /// Timestamp when entry was created
    pub timestamp: u64,
    /// Serialized object data or operation payload
    pub data: Vec<u8>,
}

impl WALEntry {
    /// Create a new WAL entry
    pub fn new(
        lsn: LSN,
        entry_type: WALEntryType,
        shard_id: ShardId,
        objref: GlobalObjRef,
        data: Vec<u8>,
        txn_id: Option<TxnId>,
    ) -> Self {
        let checksum = crc32fast::hash(&data);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        Self {
            lsn,
            entry_type,
            shard_id,
            data_size: data.len() as u32,
            objref,
            checksum,
            txn_id,
            timestamp,
            data,
        }
    }

    /// Verify the checksum of this entry
    pub fn verify_checksum(&self) -> bool {
        crc32fast::hash(&self.data) == self.checksum
    }

    /// Get the total size of this entry when serialized
    pub fn total_size(&self) -> usize {
        54 + self.data.len() // Header + data
    }

    /// Serialize entry to bytes for storage
    pub fn to_bytes(&self) -> OQLResult<Vec<u8>> {
        bincode::serialize(self).map_err(|e| {
            OQLError::serialization_error(&format!("Failed to serialize WAL entry: {}", e), None)
        })
    }

    /// Deserialize entry from bytes
    pub fn from_bytes(bytes: &[u8]) -> OQLResult<Self> {
        let entry: Self = bincode::deserialize(bytes).map_err(|e| {
            OQLError::serialization_error(&format!("Failed to deserialize WAL entry: {}", e), None)
        })?;

        // Verify checksum
        if !entry.verify_checksum() {
            return Err(OQLError::storage_error(
                &format!(
                    "WAL entry checksum mismatch: expected {}, got {}",
                    entry.checksum,
                    crc32fast::hash(&entry.data)
                ),
                None,
            ));
        }

        Ok(entry)
    }
}

/// Checkpoint metadata for distributed consistency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// Checkpoint ID (unique across cluster)
    pub checkpoint_id: u64,
    /// LSN at which this checkpoint was taken
    pub lsn: LSN,
    /// Timestamp when checkpoint was created
    pub timestamp: u64,
    /// Participating shards and their LSNs
    pub shard_lsns: HashMap<ShardId, LSN>,
    /// Checksum of the checkpoint data
    pub checksum: u32,
    /// Size of checkpoint data in bytes
    pub size: u64,
}

impl CheckpointMetadata {
    /// Create new checkpoint metadata
    pub fn new(checkpoint_id: u64, lsn: LSN, shard_lsns: HashMap<ShardId, LSN>) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        // Calculate checksum of shard LSNs for integrity
        let mut shard_data = Vec::new();
        for (&shard_id, &shard_lsn) in &shard_lsns {
            shard_data.extend_from_slice(&shard_id.to_le_bytes());
            shard_data.extend_from_slice(&shard_lsn.to_le_bytes());
        }
        let checksum = crc32fast::hash(&shard_data);

        Self {
            checkpoint_id,
            lsn,
            timestamp,
            shard_lsns,
            checksum,
            size: 0, // Will be set when checkpoint is written
        }
    }

    /// Verify the integrity of this checkpoint metadata
    pub fn verify(&self) -> bool {
        let mut shard_data = Vec::new();
        for (&shard_id, &shard_lsn) in &self.shard_lsns {
            shard_data.extend_from_slice(&shard_id.to_le_bytes());
            shard_data.extend_from_slice(&shard_lsn.to_le_bytes());
        }
        crc32fast::hash(&shard_data) == self.checksum
    }
}

/// Distributed Write-Ahead Log with checkpointing support
pub struct DistributedWAL {
    /// Current shard ID
    shard_id: ShardId,
    /// Next LSN to assign
    next_lsn: Arc<Mutex<LSN>>,
    /// WAL file handle
    wal_file: Arc<Mutex<Box<dyn crate::simulation::filesystem::FileHandle>>>,
    /// FileSystem for I/O operations (dependency injection)
    filesystem: Arc<dyn FileSystem>,
    /// WAL file path
    wal_path: String,
    /// In-memory cache of recent entries
    entry_cache: Arc<RwLock<HashMap<LSN, WALEntry>>>,
    /// Active transactions
    active_transactions: Arc<RwLock<HashMap<TxnId, Vec<LSN>>>>,
    /// Checkpoint metadata
    checkpoints: Arc<RwLock<Vec<CheckpointMetadata>>>,
    /// Maximum cache size (number of entries)
    max_cache_size: usize,
}

impl DistributedWAL {
    /// Create a new distributed WAL
    pub fn new<P: AsRef<Path>>(
        shard_id: ShardId,
        wal_path: P,
        filesystem: Arc<dyn FileSystem>,
    ) -> OQLResult<Self> {
        let wal_path_ref = wal_path.as_ref();
        let wal_path_str = wal_path_ref
            .to_str()
            .ok_or_else(|| OQLError::generic("Invalid WAL path"))?
            .to_string();

        // Open WAL read/write and create when missing.
        let options = crate::simulation::filesystem::FileOptions {
            read: true,
            write: true,
            create: !filesystem.exists(wal_path_ref),
            truncate: false,
            append: false,
        };
        let wal_file = filesystem
            .open_with_options(wal_path_ref, &options)
            .map_err(|e| {
                OQLError::storage_error(&format!("Failed to open WAL file: {}", e), None)
            })?;

        let wal = Self {
            shard_id,
            next_lsn: Arc::new(Mutex::new(1)),
            wal_file: Arc::new(Mutex::new(wal_file)),
            filesystem,
            wal_path: wal_path_str,
            entry_cache: Arc::new(RwLock::new(HashMap::new())),
            active_transactions: Arc::new(RwLock::new(HashMap::new())),
            checkpoints: Arc::new(RwLock::new(Vec::new())),
            max_cache_size: 10000, // Keep last 10k entries in memory
        };
        wal.initialize_from_file()?;
        Ok(wal)
    }

    /// Append an entry to the WAL with fsync for durability
    pub fn append_with_sync(
        &self,
        entry_type: WALEntryType,
        objref: GlobalObjRef,
        data: Vec<u8>,
        txn_id: Option<TxnId>,
    ) -> OQLResult<LSN> {
        // Assign LSN
        let lsn = {
            let mut next_lsn = self
                .next_lsn
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire LSN lock"))?;
            let current_lsn = *next_lsn;
            *next_lsn += 1;
            current_lsn
        };

        // Create WAL entry
        let entry = WALEntry::new(lsn, entry_type, self.shard_id, objref, data, txn_id);

        // Serialize entry
        let entry_bytes = entry.to_bytes()?;

        // Write to WAL file with fsync
        {
            let mut file = self
                .wal_file
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire WAL file lock"))?;
            file.seek(SeekFrom::End(0)).map_err(|e| {
                OQLError::storage_error(&format!("Failed to seek WAL file: {}", e), None)
            })?;

            // Write entry size first (4 bytes)
            let size_bytes = (entry_bytes.len() as u32).to_le_bytes();
            Self::write_all(file.as_mut(), &size_bytes)?;

            // Write entry data
            Self::write_all(file.as_mut(), &entry_bytes)?;

            // Force sync to disk for durability guarantee
            file.sync().map_err(|e| {
                OQLError::storage_error(&format!("Failed to sync WAL: {}", e), None)
            })?;
        }

        // Add to cache
        self.add_to_cache(entry);

        // Track transaction if applicable
        if let Some(txn_id) = txn_id {
            let mut active_txns = self
                .active_transactions
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            active_txns.entry(txn_id).or_insert_with(Vec::new).push(lsn);
        }

        Ok(lsn)
    }

    /// Append an entry without immediate sync (for batch operations)
    pub fn append(
        &self,
        entry_type: WALEntryType,
        objref: GlobalObjRef,
        data: Vec<u8>,
        txn_id: Option<TxnId>,
    ) -> OQLResult<LSN> {
        // Similar to append_with_sync but without fsync
        let lsn = {
            let mut next_lsn = self
                .next_lsn
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire LSN lock"))?;
            let current_lsn = *next_lsn;
            *next_lsn += 1;
            current_lsn
        };

        let entry = WALEntry::new(lsn, entry_type, self.shard_id, objref, data, txn_id);
        let entry_bytes = entry.to_bytes()?;

        {
            let mut file = self
                .wal_file
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire WAL file lock"))?;
            file.seek(SeekFrom::End(0)).map_err(|e| {
                OQLError::storage_error(&format!("Failed to seek WAL file: {}", e), None)
            })?;

            let size_bytes = (entry_bytes.len() as u32).to_le_bytes();
            Self::write_all(file.as_mut(), &size_bytes)?;
            Self::write_all(file.as_mut(), &entry_bytes)?;
        }

        self.add_to_cache(entry);

        if let Some(txn_id) = txn_id {
            let mut active_txns = self
                .active_transactions
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            active_txns.entry(txn_id).or_insert_with(Vec::new).push(lsn);
        }

        Ok(lsn)
    }

    /// Force sync all pending writes
    pub fn sync(&self) -> OQLResult<()> {
        let mut file = self
            .wal_file
            .lock()
            .map_err(|_| OQLError::generic("Failed to acquire WAL file lock"))?;
        file.sync()
            .map_err(|e| OQLError::storage_error(&format!("Failed to sync WAL: {}", e), None))
    }

    /// Get the current LSN (last assigned)
    pub fn current_lsn(&self) -> LSN {
        let next_lsn = self.next_lsn.lock().unwrap();
        *next_lsn - 1
    }

    /// Get a WAL entry by LSN
    pub fn get_entry(&self, lsn: LSN) -> OQLResult<Option<WALEntry>> {
        // Check cache first
        {
            let cache = self
                .entry_cache
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire cache lock"))?;
            if let Some(entry) = cache.get(&lsn) {
                return Ok(Some(entry.clone()));
            }
        }

        for entry in self.read_entries_from_file()? {
            if entry.lsn == lsn {
                self.add_to_cache(entry.clone());
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// Replay WAL entries from a given LSN
    pub fn replay_from<F>(&self, start_lsn: LSN, mut apply_fn: F) -> OQLResult<()>
    where
        F: FnMut(&WALEntry) -> OQLResult<()>,
    {
        let entries = self.read_entries_from_file()?;
        for entry in entries {
            if entry.lsn >= start_lsn {
                apply_fn(&entry)?;
            }
        }

        Ok(())
    }

    /// Return WAL entries from `start_lsn` onward in ascending LSN order.
    pub fn entries_from(&self, start_lsn: LSN) -> OQLResult<Vec<WALEntry>> {
        let mut entries = self.read_entries_from_file()?;
        entries.retain(|entry| entry.lsn >= start_lsn);
        entries.sort_by_key(|entry| entry.lsn);
        Ok(entries)
    }

    /// Begin a distributed transaction
    pub fn begin_transaction(&self, txn_id: TxnId) -> OQLResult<LSN> {
        self.append_with_sync(
            WALEntryType::TxnBegin,
            GlobalObjRef::null(),
            Vec::new(),
            Some(txn_id),
        )
    }

    /// Commit a distributed transaction
    pub fn commit_transaction(&self, txn_id: TxnId) -> OQLResult<LSN> {
        let lsn = self.append_with_sync(
            WALEntryType::TxnCommit,
            GlobalObjRef::null(),
            Vec::new(),
            Some(txn_id),
        )?;

        // Remove from active transactions
        let mut active_txns = self
            .active_transactions
            .write()
            .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
        active_txns.remove(&txn_id);

        Ok(lsn)
    }

    /// Rollback a distributed transaction
    pub fn rollback_transaction(&self, txn_id: TxnId) -> OQLResult<LSN> {
        let lsn = self.append_with_sync(
            WALEntryType::TxnRollback,
            GlobalObjRef::null(),
            Vec::new(),
            Some(txn_id),
        )?;

        // Remove from active transactions
        let mut active_txns = self
            .active_transactions
            .write()
            .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
        active_txns.remove(&txn_id);

        Ok(lsn)
    }

    /// Add entry to cache with LRU eviction
    fn add_to_cache(&self, entry: WALEntry) {
        let mut cache = self.entry_cache.write().unwrap();

        // Simple eviction: remove oldest entries if cache is full
        if cache.len() >= self.max_cache_size {
            let oldest_lsn = cache.keys().min().copied();
            if let Some(lsn) = oldest_lsn {
                cache.remove(&lsn);
            }
        }

        cache.insert(entry.lsn, entry);
    }

    fn write_all(
        file: &mut dyn crate::simulation::filesystem::FileHandle,
        mut bytes: &[u8],
    ) -> OQLResult<()> {
        while !bytes.is_empty() {
            let written = file.write(bytes).map_err(|e| {
                OQLError::storage_error(&format!("Failed to write WAL file: {}", e), None)
            })?;
            if written == 0 {
                return Err(OQLError::storage_error("Short write to WAL file", None));
            }
            bytes = &bytes[written..];
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
                OQLError::storage_error(&format!("Failed to read WAL file: {}", e), None)
            })?;
            if n == 0 {
                if read_any {
                    return Err(OQLError::storage_error(
                        "Unexpected EOF while reading WAL file",
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

    fn read_entries_from_file(&self) -> OQLResult<Vec<WALEntry>> {
        let options = crate::simulation::filesystem::FileOptions {
            read: true,
            write: false,
            create: false,
            truncate: false,
            append: false,
        };
        let mut file = self
            .filesystem
            .open_with_options(Path::new(&self.wal_path), &options)
            .map_err(|e| {
                OQLError::storage_error(&format!("Failed to open WAL for read: {}", e), None)
            })?;

        let mut entries = Vec::new();
        loop {
            let mut len_buf = [0u8; 4];
            match Self::read_exact(file.as_mut(), &mut len_buf)? {
                true => {}
                false => break,
            }

            let len = u32::from_le_bytes(len_buf) as usize;
            if len == 0 {
                return Err(OQLError::storage_error(
                    "Corrupt WAL: zero-length entry",
                    None,
                ));
            }

            let mut payload = vec![0u8; len];
            if !Self::read_exact(file.as_mut(), &mut payload)? {
                return Err(OQLError::storage_error(
                    "Corrupt WAL: truncated entry payload",
                    None,
                ));
            }
            entries.push(WALEntry::from_bytes(&payload)?);
        }

        entries.sort_by_key(|entry| entry.lsn);
        Ok(entries)
    }

    fn initialize_from_file(&self) -> OQLResult<()> {
        let entries = self.read_entries_from_file()?;
        let mut highest_lsn = 0;
        let mut active_txns: HashMap<TxnId, Vec<LSN>> = HashMap::new();

        for entry in entries {
            highest_lsn = highest_lsn.max(entry.lsn);
            self.add_to_cache(entry.clone());
            if let Some(txn_id) = entry.txn_id {
                match entry.entry_type {
                    WALEntryType::TxnCommit | WALEntryType::TxnRollback => {
                        active_txns.remove(&txn_id);
                    }
                    _ => {
                        active_txns.entry(txn_id).or_default().push(entry.lsn);
                    }
                }
            }
        }

        {
            let mut next = self
                .next_lsn
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire LSN lock"))?;
            *next = highest_lsn.saturating_add(1).max(1);
        }
        {
            let mut txns = self
                .active_transactions
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            *txns = active_txns;
        }

        Ok(())
    }
}

/// Distributed checkpoint manager coordinating across shards
#[allow(dead_code)]
pub struct CheckpointManager {
    /// Current shard ID
    shard_id: ShardId,
    /// WAL instance
    wal: Arc<DistributedWAL>,
    /// FileSystem for checkpoint files
    filesystem: Arc<dyn FileSystem>,
    /// Checkpoint directory
    checkpoint_dir: String,
    /// Next checkpoint ID
    next_checkpoint_id: Arc<Mutex<u64>>,
    /// Checkpoint interval (in seconds)
    checkpoint_interval: Duration,
}

impl CheckpointManager {
    /// Create a new checkpoint manager
    pub fn new<P: AsRef<Path>>(
        shard_id: ShardId,
        wal: Arc<DistributedWAL>,
        checkpoint_dir: P,
        filesystem: Arc<dyn FileSystem>,
    ) -> OQLResult<Self> {
        let checkpoint_dir_str = checkpoint_dir
            .as_ref()
            .to_str()
            .ok_or_else(|| OQLError::generic("Invalid checkpoint directory path"))?
            .to_string();

        // Create checkpoint directory if it doesn't exist
        if !filesystem.exists(checkpoint_dir.as_ref()) {
            // Note: FileSystem trait doesn't have create_dir, so we assume it exists
            // In a real implementation, we'd extend the trait or handle this differently
        }

        Ok(Self {
            shard_id,
            wal,
            filesystem,
            checkpoint_dir: checkpoint_dir_str,
            next_checkpoint_id: Arc::new(Mutex::new(1)),
            checkpoint_interval: Duration::from_secs(300), // 5 minutes default
        })
    }

    /// Create a checkpoint at the current LSN
    pub fn create_checkpoint(
        &self,
        shard_lsns: HashMap<ShardId, LSN>,
    ) -> OQLResult<CheckpointMetadata> {
        let checkpoint_id = {
            let mut next_id = self
                .next_checkpoint_id
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire checkpoint ID lock"))?;
            let current_id = *next_id;
            *next_id += 1;
            current_id
        };

        let current_lsn = self.wal.current_lsn();
        let mut metadata = CheckpointMetadata::new(checkpoint_id, current_lsn, shard_lsns);

        // Write checkpoint marker to WAL
        let checkpoint_data = bincode::serialize(&metadata).map_err(|e| {
            OQLError::serialization_error(
                &format!("Failed to serialize checkpoint metadata: {}", e),
                None,
            )
        })?;

        self.wal.append_with_sync(
            WALEntryType::Checkpoint,
            GlobalObjRef::null(),
            checkpoint_data.clone(),
            None,
        )?;

        // Write checkpoint metadata to separate file
        let checkpoint_path = format!(
            "{}/checkpoint_{:08}.meta",
            self.checkpoint_dir, checkpoint_id
        );
        let mut checkpoint_file = self
            .filesystem
            .create(Path::new(&checkpoint_path))
            .map_err(|e| {
                OQLError::storage_error(&format!("Failed to create checkpoint file: {}", e), None)
            })?;

        DistributedWAL::write_all(checkpoint_file.as_mut(), &checkpoint_data)?;

        checkpoint_file.sync().map_err(|e| {
            OQLError::storage_error(&format!("Failed to sync checkpoint file: {}", e), None)
        })?;

        metadata.size = checkpoint_data.len() as u64;

        // Add to checkpoint list
        {
            let mut checkpoints = self
                .wal
                .checkpoints
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire checkpoints lock"))?;
            checkpoints.push(metadata.clone());
        }

        Ok(metadata)
    }

    /// Get the latest checkpoint
    pub fn latest_checkpoint(&self) -> OQLResult<Option<CheckpointMetadata>> {
        {
            let checkpoints = self
                .wal
                .checkpoints
                .read()
                .map_err(|_| OQLError::generic("Failed to acquire checkpoints lock"))?;
            if let Some(last) = checkpoints.last() {
                return Ok(Some(last.clone()));
            }
        }

        // Cold start path: rebuild checkpoint metadata from WAL checkpoint markers.
        let mut rebuilt = Vec::new();
        for entry in self.wal.read_entries_from_file()? {
            if entry.entry_type == WALEntryType::Checkpoint && !entry.data.is_empty() {
                let metadata: CheckpointMetadata =
                    bincode::deserialize(&entry.data).map_err(|e| {
                        OQLError::serialization_error(
                            &format!("Failed to deserialize checkpoint metadata from WAL: {}", e),
                            None,
                        )
                    })?;
                rebuilt.push(metadata);
            }
        }
        rebuilt.sort_by_key(|c| c.lsn);

        if !rebuilt.is_empty() {
            let mut checkpoints = self
                .wal
                .checkpoints
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire checkpoints lock"))?;
            *checkpoints = rebuilt.clone();
        }

        Ok(rebuilt.last().cloned())
    }

    /// Truncate WAL after successful checkpoint
    pub fn truncate_wal_after_checkpoint(&self, checkpoint_lsn: LSN) -> OQLResult<()> {
        // Rewrite WAL file with only entries newer than checkpoint_lsn.
        let retained: Vec<WALEntry> = self
            .wal
            .read_entries_from_file()?
            .into_iter()
            .filter(|entry| entry.lsn > checkpoint_lsn)
            .collect();

        let options = crate::simulation::filesystem::FileOptions {
            read: true,
            write: true,
            create: true,
            truncate: true,
            append: false,
        };
        let mut new_file = self
            .filesystem
            .open_with_options(Path::new(&self.wal.wal_path), &options)
            .map_err(|e| {
                OQLError::storage_error(&format!("Failed to rewrite WAL file: {}", e), None)
            })?;

        for entry in &retained {
            let entry_bytes = entry.to_bytes()?;
            let len_bytes = (entry_bytes.len() as u32).to_le_bytes();
            DistributedWAL::write_all(new_file.as_mut(), &len_bytes)?;
            DistributedWAL::write_all(new_file.as_mut(), &entry_bytes)?;
        }
        new_file.sync().map_err(|e| {
            OQLError::storage_error(&format!("Failed to sync truncated WAL: {}", e), None)
        })?;

        {
            let mut wal_file = self
                .wal
                .wal_file
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire WAL file lock"))?;
            *wal_file = new_file;
        }

        {
            let mut cache = self
                .wal
                .entry_cache
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire cache lock"))?;
            cache.clear();
            for entry in &retained {
                cache.insert(entry.lsn, entry.clone());
            }
        }

        {
            let mut txns = self
                .wal
                .active_transactions
                .write()
                .map_err(|_| OQLError::generic("Failed to acquire transaction lock"))?;
            txns.clear();
            for entry in &retained {
                if let Some(txn_id) = entry.txn_id {
                    match entry.entry_type {
                        WALEntryType::TxnCommit | WALEntryType::TxnRollback => {
                            txns.remove(&txn_id);
                        }
                        _ => {
                            txns.entry(txn_id).or_default().push(entry.lsn);
                        }
                    }
                }
            }
        }

        {
            let max_lsn = retained.iter().map(|e| e.lsn).max().unwrap_or(0);
            let mut next = self
                .wal
                .next_lsn
                .lock()
                .map_err(|_| OQLError::generic("Failed to acquire LSN lock"))?;
            *next = max_lsn.saturating_add(1).max(1);
        }

        Ok(())
    }

    /// Recover from the latest checkpoint
    pub fn recover_from_checkpoint(&self) -> OQLResult<Option<LSN>> {
        let latest = self.latest_checkpoint()?;

        if let Some(checkpoint) = latest {
            if !checkpoint.verify() {
                return Err(OQLError::storage_error(
                    "Checkpoint metadata is corrupted",
                    None,
                ));
            }

            // In a real implementation, this would:
            // 1. Load arena state from checkpoint
            // 2. Replay WAL entries from checkpoint LSN to current
            // 3. Rebuild in-memory structures

            Ok(Some(checkpoint.lsn))
        } else {
            Ok(None)
        }
    }

    /// Set checkpoint interval
    pub fn set_checkpoint_interval(&mut self, interval: Duration) {
        self.checkpoint_interval = interval;
    }

    /// Get checkpoint interval
    pub fn checkpoint_interval(&self) -> Duration {
        self.checkpoint_interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::RealFileSystem;
    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use tempfile::TempDir;

    #[test]
    fn test_wal_entry_creation() {
        let objref = GlobalObjRef::new(1, 1024, 0);
        let data = b"test data".to_vec();
        let entry = WALEntry::new(1, WALEntryType::Insert, 0, objref, data.clone(), None);

        assert_eq!(entry.lsn, 1);
        assert_eq!(entry.entry_type, WALEntryType::Insert);
        assert_eq!(entry.shard_id, 0);
        assert_eq!(entry.objref, objref);
        assert_eq!(entry.data, data);
        assert!(entry.verify_checksum());
    }

    #[test]
    fn test_wal_entry_serialization() {
        let objref = GlobalObjRef::new(1, 1024, 0);
        let data = b"test data".to_vec();
        let entry = WALEntry::new(1, WALEntryType::Insert, 0, objref, data, None);

        let bytes = entry.to_bytes().unwrap();
        let deserialized = WALEntry::from_bytes(&bytes).unwrap();

        assert_eq!(entry.lsn, deserialized.lsn);
        assert_eq!(entry.entry_type, deserialized.entry_type);
        assert_eq!(entry.data, deserialized.data);
    }

    #[test]
    fn test_checkpoint_metadata() {
        let mut shard_lsns = HashMap::new();
        shard_lsns.insert(0, 100);
        shard_lsns.insert(1, 150);
        shard_lsns.insert(2, 120);

        let metadata = CheckpointMetadata::new(1, 150, shard_lsns.clone());

        assert_eq!(metadata.checkpoint_id, 1);
        assert_eq!(metadata.lsn, 150);
        assert_eq!(metadata.shard_lsns, shard_lsns);
        assert!(metadata.verify());
    }

    #[test]
    fn test_distributed_wal_creation() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("test.wal");
        let filesystem = Arc::new(RealFileSystem::new());

        let wal = DistributedWAL::new(0, &wal_path, filesystem).unwrap();
        assert_eq!(wal.shard_id, 0);
        assert_eq!(wal.current_lsn(), 0); // No entries yet
    }

    #[test]
    fn test_wal_append_with_sync() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("test.wal");
        let filesystem = Arc::new(RealFileSystem::new());

        let wal = DistributedWAL::new(0, &wal_path, filesystem).unwrap();
        let objref = GlobalObjRef::new(0, 1024, 0);
        let data = b"test data".to_vec();

        let lsn = wal
            .append_with_sync(WALEntryType::Insert, objref, data, None)
            .unwrap();
        assert_eq!(lsn, 1);
        assert_eq!(wal.current_lsn(), 1);

        // Verify entry is in cache
        let entry = wal.get_entry(lsn).unwrap().unwrap();
        assert_eq!(entry.lsn, lsn);
        assert_eq!(entry.entry_type, WALEntryType::Insert);
    }

    #[test]
    fn test_transaction_operations() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("test.wal");
        let filesystem = Arc::new(RealFileSystem::new());

        let wal = DistributedWAL::new(0, &wal_path, filesystem).unwrap();
        let txn_id = 42;

        // Begin transaction
        let begin_lsn = wal.begin_transaction(txn_id).unwrap();
        assert_eq!(begin_lsn, 1);

        // Add some operations
        let objref = GlobalObjRef::new(0, 1024, 0);
        let data = b"txn data".to_vec();
        let op_lsn = wal
            .append_with_sync(WALEntryType::Insert, objref, data, Some(txn_id))
            .unwrap();
        assert_eq!(op_lsn, 2);

        // Commit transaction
        let commit_lsn = wal.commit_transaction(txn_id).unwrap();
        assert_eq!(commit_lsn, 3);

        // Verify transaction is no longer active
        let active_txns = wal.active_transactions.read().unwrap();
        assert!(!active_txns.contains_key(&txn_id));
    }

    #[test]
    fn test_checkpoint_manager() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("test.wal");
        let checkpoint_dir = temp_dir.path().join("checkpoints");
        let filesystem = Arc::new(RealFileSystem::new());

        let wal = Arc::new(DistributedWAL::new(0, &wal_path, filesystem.clone()).unwrap());
        let checkpoint_mgr =
            CheckpointManager::new(0, wal.clone(), &checkpoint_dir, filesystem).unwrap();

        // Add some WAL entries
        let objref = GlobalObjRef::new(0, 1024, 0);
        wal.append_with_sync(WALEntryType::Insert, objref, b"data1".to_vec(), None)
            .unwrap();
        wal.append_with_sync(WALEntryType::Insert, objref, b"data2".to_vec(), None)
            .unwrap();

        // Create checkpoint
        let mut shard_lsns = HashMap::new();
        shard_lsns.insert(0, 2);
        let checkpoint = checkpoint_mgr.create_checkpoint(shard_lsns).unwrap();

        assert_eq!(checkpoint.checkpoint_id, 1);
        assert_eq!(checkpoint.lsn, 3); // WAL entry for checkpoint itself
        assert!(checkpoint.verify());

        // Verify it's the latest checkpoint
        let latest = checkpoint_mgr.latest_checkpoint().unwrap().unwrap();
        assert_eq!(latest.checkpoint_id, checkpoint.checkpoint_id);
    }

    #[test]
    fn test_wal_reopen_rebuilds_lsn_and_entries() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("restart.wal");
        let filesystem = Arc::new(RealFileSystem::new());
        let objref = GlobalObjRef::new(0, 2048, 0);

        {
            let wal = DistributedWAL::new(0, &wal_path, filesystem.clone()).unwrap();
            assert_eq!(
                wal.append_with_sync(WALEntryType::Insert, objref, b"a".to_vec(), None)
                    .unwrap(),
                1
            );
            assert_eq!(
                wal.append_with_sync(WALEntryType::Update, objref, b"b".to_vec(), None)
                    .unwrap(),
                2
            );
            assert_eq!(wal.current_lsn(), 2);
        }

        let reopened = DistributedWAL::new(0, &wal_path, filesystem).unwrap();
        assert_eq!(reopened.current_lsn(), 2);
        assert_eq!(
            reopened.get_entry(1).unwrap().unwrap().entry_type,
            WALEntryType::Insert
        );
        assert_eq!(
            reopened.get_entry(2).unwrap().unwrap().entry_type,
            WALEntryType::Update
        );
        assert_eq!(
            reopened
                .append_with_sync(WALEntryType::Delete, objref, Vec::new(), None)
                .unwrap(),
            3
        );
    }

    #[test]
    fn test_wal_replay_from_file_after_restart() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("replay.wal");
        let filesystem = Arc::new(RealFileSystem::new());
        let objref = GlobalObjRef::new(0, 1024, 0);

        {
            let wal = DistributedWAL::new(0, &wal_path, filesystem.clone()).unwrap();
            wal.append_with_sync(WALEntryType::Insert, objref, b"one".to_vec(), None)
                .unwrap();
            wal.append_with_sync(WALEntryType::Update, objref, b"two".to_vec(), None)
                .unwrap();
            wal.append_with_sync(WALEntryType::Delete, objref, Vec::new(), None)
                .unwrap();
        }

        let reopened = DistributedWAL::new(0, &wal_path, filesystem).unwrap();
        let mut seen = Vec::new();
        reopened
            .replay_from(2, |entry| {
                seen.push(entry.lsn);
                Ok(())
            })
            .unwrap();
        assert_eq!(seen, vec![2, 3]);
    }

    #[test]
    fn test_wal_reopen_fails_on_truncated_entry() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("corrupt.wal");
        let filesystem = Arc::new(RealFileSystem::new());
        let objref = GlobalObjRef::new(0, 4096, 0);

        {
            let wal = DistributedWAL::new(0, &wal_path, filesystem.clone()).unwrap();
            wal.append_with_sync(WALEntryType::Insert, objref, b"payload".to_vec(), None)
                .unwrap();
        }

        let metadata = std::fs::metadata(&wal_path).unwrap();
        let truncated_len = metadata.len().saturating_sub(2);
        let file = OpenOptions::new().write(true).open(&wal_path).unwrap();
        file.set_len(truncated_len).unwrap();

        let reopened = DistributedWAL::new(0, &wal_path, filesystem);
        assert!(reopened.is_err());
    }

    #[test]
    fn test_checkpoint_reload_from_wal_after_restart() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("cp_reload.wal");
        let checkpoint_dir = temp_dir.path().join("checkpoints");
        std::fs::create_dir_all(&checkpoint_dir).unwrap();
        let filesystem = Arc::new(RealFileSystem::new());

        {
            let wal = Arc::new(DistributedWAL::new(0, &wal_path, filesystem.clone()).unwrap());
            let checkpoint_mgr =
                CheckpointManager::new(0, wal.clone(), &checkpoint_dir, filesystem.clone())
                    .unwrap();
            let objref = GlobalObjRef::new(0, 64, 0);
            wal.append_with_sync(WALEntryType::Insert, objref, b"x".to_vec(), None)
                .unwrap();
            let mut shard_lsns = HashMap::new();
            shard_lsns.insert(0, wal.current_lsn());
            let cp = checkpoint_mgr.create_checkpoint(shard_lsns).unwrap();
            assert_eq!(cp.checkpoint_id, 1);
        }

        let reopened = Arc::new(DistributedWAL::new(0, &wal_path, filesystem.clone()).unwrap());
        let reopened_mgr =
            CheckpointManager::new(0, reopened, &checkpoint_dir, filesystem).unwrap();
        let latest = reopened_mgr.latest_checkpoint().unwrap().unwrap();
        assert_eq!(latest.checkpoint_id, 1);
        assert!(latest.verify());
    }

    #[test]
    fn test_truncate_wal_after_checkpoint_rewrites_file() {
        let temp_dir = TempDir::new().unwrap();
        let wal_path = temp_dir.path().join("truncate.wal");
        let checkpoint_dir = temp_dir.path().join("checkpoints");
        std::fs::create_dir_all(&checkpoint_dir).unwrap();
        let filesystem = Arc::new(RealFileSystem::new());

        let wal = Arc::new(DistributedWAL::new(0, &wal_path, filesystem.clone()).unwrap());
        let checkpoint_mgr =
            CheckpointManager::new(0, wal.clone(), &checkpoint_dir, filesystem).unwrap();
        let objref = GlobalObjRef::new(0, 100, 0);

        wal.append_with_sync(WALEntryType::Insert, objref, b"a".to_vec(), None)
            .unwrap(); // lsn 1
        wal.append_with_sync(WALEntryType::Insert, objref, b"b".to_vec(), None)
            .unwrap(); // lsn 2
        wal.append_with_sync(WALEntryType::Insert, objref, b"c".to_vec(), None)
            .unwrap(); // lsn 3

        checkpoint_mgr.truncate_wal_after_checkpoint(2).unwrap();

        assert!(wal.get_entry(1).unwrap().is_none());
        assert!(wal.get_entry(2).unwrap().is_none());
        assert!(wal.get_entry(3).unwrap().is_some());
        assert_eq!(
            wal.append_with_sync(WALEntryType::Delete, objref, Vec::new(), None)
                .unwrap(),
            4
        );
    }
}
