use super::checkpoint::{
    cleanup_typed_locator_spools, decode_segment_file, open_typed_segment_handles,
    write_segment_file, BaseSegmentHandle, CheckpointImage, IndexSorter, RecordSpool, SegmentSpool, CheckpointIndex, CheckpointInfo,
    CheckpointMetadata, DecodedCheckpoint, FileIndexBase, IndexPageDescriptor, Manifest,
    PublishedIndexPages, SegmentIndexReader, SegmentVerification, TypedLocatorLeaf,
    TypedSegmentFileDescriptor, TypedSegmentHandle,
};
use super::column_segment::TypedCell;
use super::object_store::ObjectFileSystem;
use super::cow::CowRecordDirectory;
use super::delta::{RecordDeltaLookup, RecordPageDelta, RecordRootHandle};
use super::index_delta::{
    visit_index_root_file, write_index_root_file, IndexDelta, IndexDeltaMode, IndexRootDelta,
};
use crate::document::{Document, DocumentId, ObjRef, Value};
use crate::error::{OQLError, OQLResult};
use crate::parser::ComparisonOperator;
use crate::schema::{DocumentSystemField, Schema};
use crate::simulation::filesystem::{FileHandle, FileOptions, FileSystem};
use crate::storage::{
    ColumnBatch, DocumentStore, ProjectedRowBatch, ReverseReferenceEntry, ScalarIndexCursor,
    ScalarIndexDefinition, ScalarIndexDirection, ScalarIndexRange, ScalarIndexScanPage,
    ScanPredicate, VectorIndexDefinition,
};
use crate::storage_engine::btree::OrderedIndexBase;
use crate::storage_engine::{
    BTreeIndex, BorrowedRecordValue, CompiledSchema, RecordCodec, RecordView, TypedKey,
    ColumnBuilder, TypedColumn, TypedSegment,
};
use crc32fast::Hasher;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::io::{ErrorKind, SeekFrom};
use std::ops::{Bound, Deref};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use web_time::{Duration, Instant};

const WAL_MAGIC: [u8; 4] = *b"OWLW";
/// Current WAL frame format. The 24-byte header is:
///
/// ```text
/// 0..4   magic "OWLW"
/// 4..6   format version
/// 6..8   CRC-16 over header bytes 0..6 and 8..20 (version 3 and later)
/// 8..16  commit LSN
/// 16..20 length of what follows the header
/// 20..24 CRC-32 over header bytes 0..20 followed by what follows the header
/// ```
///
/// The header check authenticates the length before it is trusted, so a
/// damaged length in a complete frame is reported as corruption instead of
/// being mistaken for a torn tail.
///
/// In version 4 the header is followed by eight bytes, the synced point,
/// and then the batch. The synced point is how many bytes of this file were
/// known to be on disk when the frame was written. It is what lets recovery
/// tell an unsynced tail from damage: see `damage_is_unsynced_tail`.
const WAL_VERSION: u16 = 4;
/// Version 3 frames have no synced point: the batch follows the header.
const WAL_UNMARKED_VERSION: u16 = 3;
/// Version 2 frames checksum the payload only and leave bytes 6..8 zero. They
/// remain readable so existing databases open; new frames are never written in
/// this format.
const WAL_LEGACY_VERSION: u16 = 2;
const WAL_HEADER_LEN: usize = 24;
const WAL_SYNCED_POINT_LEN: usize = 8;
const MAX_WAL_FRAME: usize = 64 * 1024 * 1024;

/// CRC-16/CCITT-FALSE. Small enough to fit the two reserved header bytes
/// while still detecting every single-bit and every burst error up to 16 bits
/// in the 18 authenticated header bytes.
fn wal_header_check(header: &[u8; WAL_HEADER_LEN]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in header[0..6].iter().chain(&header[8..20]) {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// The header and synced point that go in front of `payload` in the file.
fn encode_wal_frame_head(
    lsn: u64,
    synced_len: u64,
    payload: &[u8],
) -> [u8; WAL_HEADER_LEN + WAL_SYNCED_POINT_LEN] {
    let mut head = [0u8; WAL_HEADER_LEN + WAL_SYNCED_POINT_LEN];
    head[0..4].copy_from_slice(&WAL_MAGIC);
    head[4..6].copy_from_slice(&WAL_VERSION.to_le_bytes());
    head[8..16].copy_from_slice(&lsn.to_le_bytes());
    head[16..20]
        .copy_from_slice(&((WAL_SYNCED_POINT_LEN + payload.len()) as u32).to_le_bytes());
    head[WAL_HEADER_LEN..].copy_from_slice(&synced_len.to_le_bytes());
    let header: &[u8; WAL_HEADER_LEN] = head[..WAL_HEADER_LEN].try_into().unwrap();
    let check = wal_header_check(header);
    head[6..8].copy_from_slice(&check.to_le_bytes());
    let mut hasher = Hasher::new();
    hasher.update(&head[0..20]);
    hasher.update(&head[WAL_HEADER_LEN..]);
    hasher.update(payload);
    let checksum = hasher.finalize();
    head[20..24].copy_from_slice(&checksum.to_le_bytes());
    head
}

/// One frame read back from a WAL file.
struct WalFrame {
    lsn: u64,
    /// The encoded batch.
    payload: Vec<u8>,
    /// How much of the file was on disk when the frame was written, for the
    /// versions that say.
    synced_len: Option<u64>,
    /// The bytes the whole frame takes in the file.
    len: u64,
}

/// What the next bytes of a WAL file turned out to be.
enum WalRead {
    Frame(WalFrame),
    /// The file ends inside a frame, or exactly between frames.
    End,
    /// Every byte of a frame is there and it is not a frame Owl wrote.
    Damaged(&'static str),
}

fn read_wal_frame_or_damage(file: &mut dyn FileHandle, path: &Path) -> OQLResult<WalRead> {
    let mut header = [0u8; WAL_HEADER_LEN];
    let header_len =
        read_up_to(file, &mut header).map_err(|e| storage_io("read WAL header", path, e))?;
    if header_len < WAL_HEADER_LEN {
        return Ok(WalRead::End);
    }
    if header[0..4] != WAL_MAGIC {
        return Ok(WalRead::Damaged("invalid WAL frame magic"));
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != WAL_LEGACY_VERSION
        && u16::from_le_bytes([header[6], header[7]]) != wal_header_check(&header)
    {
        return Ok(WalRead::Damaged("WAL frame header checksum mismatch"));
    }
    if !matches!(
        version,
        WAL_VERSION | WAL_UNMARKED_VERSION | WAL_LEGACY_VERSION
    ) {
        // A header that checks out and names a version this build does not
        // know was written by a newer Owl. That is never an unsynced tail.
        return Err(OQLError::storage_error(
            "unsupported WAL format version",
            None,
        ));
    }
    let lsn = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let body_len = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
    let expected_checksum = u32::from_le_bytes(header[20..24].try_into().unwrap());
    let synced_point_len = if version == WAL_VERSION {
        WAL_SYNCED_POINT_LEN
    } else {
        0
    };
    if body_len > MAX_WAL_FRAME + synced_point_len {
        return Ok(WalRead::Damaged("WAL frame is unreasonably large"));
    }
    if body_len < synced_point_len {
        return Ok(WalRead::Damaged("WAL frame is too short for its version"));
    }
    let mut body = vec![0u8; body_len];
    if read_up_to(file, &mut body).map_err(|e| storage_io("read WAL payload", path, e))?
        < body_len
    {
        return Ok(WalRead::End);
    }
    let mut hasher = Hasher::new();
    if version != WAL_LEGACY_VERSION {
        hasher.update(&header[0..20]);
    }
    hasher.update(&body);
    if hasher.finalize() != expected_checksum {
        return Ok(WalRead::Damaged("WAL frame checksum mismatch"));
    }
    let synced_len = (synced_point_len > 0)
        .then(|| u64::from_le_bytes(body[..WAL_SYNCED_POINT_LEN].try_into().unwrap()));
    body.drain(..synced_point_len);
    Ok(WalRead::Frame(WalFrame {
        lsn,
        payload: body,
        synced_len,
        len: (WAL_HEADER_LEN + body_len) as u64,
    }))
}

/// How a WAL file is read when a frame in it is damaged.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WalTail {
    /// Every byte of the file was synced before anything relied on it, so
    /// damage anywhere is corruption.
    Synced,
    /// The file is the one being appended to. Its last frames may never
    /// have been synced, and a crash may have kept some of them and lost
    /// others.
    MayBeUnsynced,
}

/// Read the next WAL frame at `offset`, where the file's cursor stands.
/// `None` is the end of the log: the file ends inside a frame or between
/// frames, or, in a file whose tail may be unsynced, the frame is damaged
/// and was never synced. Any other defect is an error.
fn read_wal_frame(
    file: &mut dyn FileHandle,
    path: &Path,
    offset: u64,
    tail: WalTail,
) -> OQLResult<Option<WalFrame>> {
    match read_wal_frame_or_damage(file, path)? {
        WalRead::Frame(frame) => Ok(Some(frame)),
        WalRead::End => Ok(None),
        WalRead::Damaged(what) => {
            if tail == WalTail::MayBeUnsynced && damage_is_unsynced_tail(file, path, offset)? {
                tracing::warn!(
                    target: "owl_db::storage_engine::recovery",
                    path = %path.display(),
                    offset,
                    damage = what,
                    "the log ends in writes that were never synced; reading them as its end"
                );
                return Ok(None);
            }
            Err(OQLError::storage_error(what, None))
        }
    }
}

/// Whether the damaged frame at `damage` lies in a part of the file that
/// was never synced.
///
/// A commit is acknowledged only once a sync covers its frame, so nothing
/// acknowledged lives past the last synced byte, and a crash may leave
/// anything there: a frame cut short, or a later write kept and an earlier
/// one lost. Each frame says how much of the file was on disk when it was
/// written. The frames after the damage are the witnesses. If one of them
/// was written when the damaged bytes were already on disk, the damage
/// happened to synced data and is corruption. If none was, the damage and
/// all that follows is the unsynced tail, and the log ends before it.
///
/// A frame after the damage that does not say (it was written by an older
/// Owl) cannot rule corruption out, so it counts as corruption.
///
/// What this cannot see: damage to the last frames synced, when nothing
/// was written after that sync. No later frame vouches for them, and they
/// are read as the end of the log.
fn damage_is_unsynced_tail(
    file: &mut dyn FileHandle,
    path: &Path,
    damage: u64,
) -> OQLResult<bool> {
    const CHUNK: usize = 64 * 1024;
    let size = file
        .size()
        .map_err(|e| storage_io("inspect damaged WAL", path, e))?;
    let mut chunk = vec![0u8; CHUNK];
    // Where the next frame could begin. Past a whole frame, it is the byte
    // after that frame.
    let mut from = damage + 1;
    while from + WAL_HEADER_LEN as u64 <= size {
        let wanted = CHUNK.min((size - from) as usize);
        let mut read = 0;
        while read < wanted {
            match file.read_at(&mut chunk[read..wanted], from + read as u64) {
                Ok(0) => break,
                Ok(count) => read += count,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(storage_io("scan damaged WAL", path, error)),
            }
        }
        if read < WAL_MAGIC.len() {
            break;
        }
        let mut next = from + (read - (WAL_MAGIC.len() - 1)) as u64;
        let found = chunk[..read]
            .windows(WAL_MAGIC.len())
            .position(|window| window == WAL_MAGIC);
        if let Some(at) = found {
            let candidate = from + at as u64;
            next = candidate + 1;
            file.seek(SeekFrom::Start(candidate))
                .map_err(|e| storage_io("seek damaged WAL", path, e))?;
            match read_wal_frame_or_damage(file, path) {
                Ok(WalRead::Frame(frame)) => match frame.synced_len {
                    Some(synced) if synced <= damage => next = candidate + frame.len,
                    _ => return Ok(false),
                },
                // Bytes that only look like the start of a frame.
                Ok(WalRead::End | WalRead::Damaged(_)) => {}
                // A frame from a newer Owl, or a read that failed: neither
                // rules corruption out.
                Err(_) => return Ok(false),
            }
        }
        from = next;
    }
    Ok(true)
}
const DEFAULT_AUTO_CHECKPOINT_WAL_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_AUTO_CHECKPOINT_COMMITS: u64 = 10_000;
const MIN_DELTA_COMPACTION_BYTES: u64 = 64 * 1024;
const COMPACTION_RUN_TARGET_BYTES: usize = 16 * 1024 * 1024;
const COMPACTION_RUN_MAGIC: [u8; 4] = *b"OWCR";
const COMPACTION_RUN_VERSION: u16 = 1;
const COMPACTION_RUN_FILE_HEADER_LEN: usize = 8;
const COMPACTION_RUN_BLOCK_TARGET_BYTES: usize = 256 * 1024;
const COMPACTION_RUN_BLOCK_HEADER_LEN: usize = 16;
const COMPACTION_RUN_ENTRY_HEADER_LEN: usize = 4;
const COMPACTION_CHANGE_FIXED_LEN: usize = 29;
const COMPACTION_MAX_MERGE_FAN_IN: usize = 32;
const COMPACTION_MANIFEST_MAGIC: [u8; 4] = *b"OWCM";
const COMPACTION_MANIFEST_VERSION: u16 = 1;
const COMPACTION_MANIFEST_LEN: usize = 20;
const MAX_COMPACTION_RUN_FILES: u64 = 10_000_000;
const GENERATION_ROOT_MAGIC: [u8; 4] = *b"OWGR";
const GENERATION_ROOT_VERSION: u16 = 2;
const GENERATION_ROOT_HEADER_LEN: usize = 48;
const GENERATION_ROOT_ENTRY_LEN: usize = 72;
const LEGACY_GENERATION_ROOT_ENTRY_LEN: usize = 56;
const MAX_FLUSH_GENERATIONS: usize = 32;
const REVERSE_INDEX_SCHEMA: &str = "\0owl";
const REVERSE_INDEX_NAME: &str = "\0reverse_refs";
/// The index a checkpoint keeps of the rows outside its column segments:
/// collection and id to the row. It is what finds such a row by its id and
/// lists a collection in id order without holding every id in memory.
const ID_INDEX_NAME: &str = "\0ids";

/// The key a row goes under in the id index: its collection, then its id,
/// so that one collection's rows lie together in the order of their ids.
fn id_index_key(collection: u16, id: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(2 + id.len());
    key.extend_from_slice(&collection.to_be_bytes());
    key.extend_from_slice(id.as_bytes());
    key
}

/// The range of the id index that holds one collection.
fn id_index_range(collection: u16) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
    let start = Bound::Included(collection.to_be_bytes().to_vec());
    let end = match collection.checked_add(1) {
        Some(next) => Bound::Excluded(next.to_be_bytes().to_vec()),
        None => Bound::Unbounded,
    };
    (start, end)
}

/// Thresholds for automatic checkpoint maintenance.
///
/// A zero value disables that individual threshold; setting both values to
/// zero disables automatic checkpoints. The first threshold reached wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCheckpointPolicy {
    pub wal_bytes: u64,
    pub commits: u64,
}

impl AutoCheckpointPolicy {
    pub const fn disabled() -> Self {
        Self {
            wal_bytes: 0,
            commits: 0,
        }
    }

    fn for_size_limit(max_size_bytes: usize) -> Self {
        let wal_bytes = if max_size_bytes == 0 {
            DEFAULT_AUTO_CHECKPOINT_WAL_BYTES
        } else {
            DEFAULT_AUTO_CHECKPOINT_WAL_BYTES.min((max_size_bytes as u64 / 2).max(1))
        };
        Self {
            wal_bytes,
            commits: DEFAULT_AUTO_CHECKPOINT_COMMITS,
        }
    }

    fn enabled(self) -> bool {
        self.wal_bytes > 0 || self.commits > 0
    }
}

/// Observable state for checkpoint maintenance and operational diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceStatus {
    pub policy: AutoCheckpointPolicy,
    pub wal_bytes: u64,
    pub commits_since_checkpoint: u64,
    pub checkpoint_sequence: u64,
    pub last_checkpoint: Option<CheckpointInfo>,
    pub last_error: Option<String>,
}

/// Result of a complete read-only validation of the authoritative durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    pub checkpoint_sequence: u64,
    pub checkpoint_lsn: u64,
    pub durable_lsn: u64,
    pub live_objects: usize,
    pub scalar_indexes: usize,
    pub files_checked: usize,
}

/// Files made unreachable and removed after an authoritative manifest was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclamationReport {
    pub retained_sequence: u64,
    pub files_removed: usize,
    pub bytes_reclaimed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayoutStats {
    pub committed_lsn: u64,
    pub typed_base_lsn: u64,
    pub flush_generations: usize,
    pub flush_generation_wal_bytes: u64,
    pub flush_generation_record_bytes: u64,
    pub flush_generation_index_bytes: u64,
    pub typed_segments: usize,
    pub typed_loaded_segments: usize,
    pub typed_identity_bytes: usize,
    pub typed_rows: usize,
    pub typed_authoritative_segments: usize,
    pub typed_authoritative_rows: usize,
    pub row_overflow_rows: usize,
    pub external_id_hash_rows: usize,
    pub ordered_id_tree_rows: usize,
    pub objref_locator_rows: usize,
    pub objref_locator_cached_pages: usize,
    pub objref_locator_cached_bytes: usize,
    pub resident_documents: usize,
    pub lazy_typed_rows: usize,
    pub typed_delta_rows: usize,
    pub typed_stale_rows: usize,
    pub live_rows: usize,
    pub typed_scan_available: bool,
    pub typed_scan_current: bool,
}

#[derive(Debug, Clone)]
pub enum WriteOperation {
    PutDocument(Document),
    DeleteDocument { schema: String, id: DocumentId },
    PutSchema(Schema),
    DeleteSchema { name: String },
    PutScalarIndex(ScalarIndexDefinition),
    DeleteScalarIndex { schema: String, name: String },
    PutVectorIndex(VectorIndexDefinition),
    DeleteVectorIndex { schema: String, name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Mutation {
    PutCompiledSchema(CompiledSchema),
    PutDocument {
        record: Vec<u8>,
        object_ref: ObjRef,
    },
    DeleteDocument {
        schema: String,
        id: DocumentId,
        object_ref: ObjRef,
    },
    PutSchema(Schema),
    DeleteSchema(String),
    PutScalarIndex(ScalarIndexDefinition),
    DeleteScalarIndex {
        schema: String,
        name: String,
    },
    PutVectorIndex(VectorIndexDefinition),
    DeleteVectorIndex {
        schema: String,
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalBatch {
    lsn: u64,
    mutations: Vec<Mutation>,
}

#[derive(Default)]
struct DenseTypedRowDirectory {
    /// Zero means absent; every other slot is a packed `(segment, row)` plus
    /// one. Both coordinates are bounded by persisted u32 lengths.
    collections: HashMap<u16, Vec<u64>>,
    base: Option<TypedLocatorLeaf>,
    overrides: HashMap<ObjRef, u64>,
    len: usize,
}

impl DenseTypedRowDirectory {
    fn insert(
        &mut self,
        object_ref: ObjRef,
        location: (usize, usize),
    ) -> OQLResult<Option<(usize, usize)>> {
        if self.base.is_some() {
            let previous = self.get(&object_ref)?;
            self.overrides.insert(object_ref, Self::pack(location));
            if previous.is_none() {
                self.len += 1;
            }
            return Ok(previous);
        }
        let index = object_ref.object_id() as usize;
        let rows = self
            .collections
            .entry(object_ref.collection_id())
            .or_default();
        if rows.len() <= index {
            rows.resize(index + 1, 0);
        }
        let previous = Self::unpack(rows[index]);
        rows[index] = Self::pack(location);
        if rows[index] != 0 && previous.is_none() {
            self.len += 1;
        }
        Ok(previous)
    }

    fn get(&self, object_ref: &ObjRef) -> OQLResult<Option<(usize, usize)>> {
        if let Some(packed) = self.overrides.get(object_ref) {
            return Ok(Self::unpack(*packed));
        }
        if let Some(base) = &self.base {
            return base.get(*object_ref);
        }
        Ok(self
            .collections
            .get(&object_ref.collection_id())
            .and_then(|rows| rows.get(object_ref.object_id() as usize))
            .copied()
            .and_then(Self::unpack))
    }

    fn contains_key(&self, object_ref: &ObjRef) -> OQLResult<bool> {
        Ok(self.get(object_ref)?.is_some())
    }

    /// Whether the row a column segment holds at `location` is still the
    /// row: it has not been written again or deleted since. Over a base,
    /// only what has changed is looked at.
    fn still_at(&self, object_ref: &ObjRef, location: (usize, usize)) -> OQLResult<bool> {
        if self.base.is_some() {
            return Ok(match self.overrides.get(object_ref) {
                Some(packed) => Self::unpack(*packed) == Some(location),
                None => true,
            });
        }
        Ok(self.get(object_ref)? == Some(location))
    }

    fn remove(&mut self, object_ref: &ObjRef) -> OQLResult<Option<(usize, usize)>> {
        if self.base.is_some() {
            let previous = self.get(object_ref)?;
            self.overrides.insert(*object_ref, 0);
            if previous.is_some() {
                self.len -= 1;
            }
            return Ok(previous);
        }
        let Some(slot) = self
            .collections
            .get_mut(&object_ref.collection_id())
            .and_then(|rows| rows.get_mut(object_ref.object_id() as usize))
        else {
            return Ok(None);
        };
        let location = Self::unpack(*slot);
        if location.is_some() {
            *slot = 0;
            self.len -= 1;
        }
        Ok(location)
    }

    fn len(&self) -> usize {
        self.len
    }

    fn cached_page_stats(&self) -> OQLResult<(usize, usize)> {
        self.base
            .as_ref()
            .map(TypedLocatorLeaf::cached_page_stats)
            .transpose()
            .map(|stats| stats.unwrap_or((0, 0)))
    }

    fn reserve(&mut self, _additional: usize) {}

    fn install_base(&mut self, base: TypedLocatorLeaf) {
        self.len = base.len();
        self.collections.clear();
        self.overrides.clear();
        self.base = Some(base);
    }

    fn pack((segment, row): (usize, usize)) -> u64 {
        let segment = u32::try_from(segment).expect("typed segment index exceeds u32");
        let row = u32::try_from(row).expect("typed segment row exceeds u32");
        ((u64::from(segment) << 32) | u64::from(row))
            .checked_add(1)
            .expect("typed row location cannot use the reserved sentinel")
    }

    fn unpack(packed: u64) -> Option<(usize, usize)> {
        let packed = packed.checked_sub(1)?;
        Some(((packed >> 32) as usize, (packed as u32) as usize))
    }
}

/// What is kept, while the generations written since a compaction are
/// applied to it, to check the indexes afterwards against the compaction's
/// own pages instead of entry by entry.
#[derive(Default)]
struct OpenAudit {
    /// How many rows each indexed collection had in the compaction.
    base_counts: HashMap<String, usize>,
    /// The indexes that answered from the compaction's pages alone.
    from_pages: HashSet<(String, String)>,
    /// The indexes a generation made or dropped: their pages are no guide.
    remade: HashSet<(String, String)>,
    /// Every row a generation wrote or deleted, and whether the
    /// compaction held it.
    touched: HashMap<ObjRef, bool>,
}

#[derive(Default)]
struct State {
    /// For each collection, the commit that last wrote one of its rows.
    /// What is built from a collection's rows is good until this moves.
    collection_stamps: HashMap<String, u64>,
    /// The commit that last changed a schema, or replayed the log. What is
    /// built from any collection's rows is good until this moves.
    catalog_stamp: u64,
    committed_lsn: u64,
    /// Immutable scan base from the last fully materialized checkpoint. Newer
    /// WAL mutations remain in `typed_delta_rows` and merge in ID order.
    typed_base_lsn: u64,
    typed_segments: Vec<Arc<TypedSegmentHandle>>,
    typed_id_fences: HashMap<String, Vec<TypedIdFence>>,
    typed_complete_collections: HashSet<u16>,
    /// Live inserted or updated rows since the typed base, kept in document-ID
    /// order alongside deletion tombstones so scans merge only this bounded
    /// overlay with segment order.
    typed_delta_rows: HashMap<String, BTreeMap<DocumentId, Option<ObjRef>>>,
    collection_ids: HashMap<String, u16>,
    next_collection_id: u16,
    next_object_ids: HashMap<u16, u32>,
    /// Stable collection scan order, derived from live document identities.
    /// This is rebuilt during recovery and maintained atomically with writes.
    collection_rows: HashMap<String, BTreeMap<DocumentId, ObjRef>>,
    /// The ids of the rows the checkpoint holds outside its column
    /// segments, read from its file a page at a time. With it,
    /// `collection_rows` holds only rows written since that checkpoint,
    /// and `typed_delta_rows` says which of the checkpoint's rows have
    /// changed or gone. Without it, as in a database written before the
    /// index existed, `collection_rows` holds every such row.
    overflow_ids: Option<Arc<dyn OrderedIndexBase>>,
    /// How many rows of each collection `overflow_ids` holds.
    overflow_id_counts: HashMap<String, usize>,
    object_directory: DecodedRows,
    /// Authoritative checkpoint rows that have not needed a general
    /// `Document` representation. Locations remain valid for the immutable
    /// `typed_segments` base and disappear atomically on update/delete.
    typed_row_locations: DenseTypedRowDirectory,
    /// The schema version of each row written since the checkpoint whose
    /// files the store reads, and of any row those files cannot be read
    /// for. A row of a column segment takes its version from the segment;
    /// a row whose record is in the checkpoint files, from the record.
    object_schema_versions: crate::storage_engine::object_ref::ByObjRef<u32>,
    /// Rows deleted since the checkpoint whose files the store reads. A
    /// row outside the column segments that is not listed in
    /// `object_schema_versions` is live while those files hold its record
    /// and it is not named here.
    deleted_since_checkpoint: HashSet<ObjRef>,
    object_records: CowRecordDirectory,
    schemas: HashMap<String, Schema>,
    compiled_schemas: HashMap<(u16, u32), CompiledSchema>,
    current_schema_versions: HashMap<String, u32>,
    scalar_indexes: HashMap<String, Vec<ScalarIndexDefinition>>,
    scalar_index_data: HashMap<(String, String), Arc<BTreeIndex>>,
    scalar_index_resets: BTreeSet<(String, String)>,
    scalar_index_deletes: BTreeSet<(String, String)>,
    /// Set while a database being opened has its generations applied:
    /// what is needed to check the indexes afterwards.
    open_audit: Option<OpenAudit>,
    vector_indexes: HashMap<String, Vec<VectorIndexDefinition>>,
    /// The vector indexes built so far, by collection and field. Built
    /// from the rows by the first search that wants one, kept in step by
    /// every commit after, and never stored.
    built_vectors: RwLock<HashMap<(String, String), Arc<crate::storage_engine::VectorIndex>>>,
    reverse_reference_index: Arc<BTreeIndex>,
    /// What open transactions need in order to tell, at commit, whether
    /// someone else changed a document they wrote.
    conflicts: ConflictLog,
    /// The most documents held decoded in `object_directory`, or `None` for
    /// no limit. Past it the oldest are let go: a row a column segment
    /// holds is decoded from there again, and any other row from its
    /// record, which is then kept in memory in its place.
    decoded_budget: Option<usize>,
    /// The most memory, roughly, that the store may take for what it
    /// keeps so as not to read it again, or `None` for no limit: half for
    /// the rows held decoded, a quarter for column segments held decoded
    /// and a quarter for columns and row lists kept between queries. Past
    /// their share the oldest rows are let go, as with `decoded_budget`. A database gets one by default, sized from the
    /// machine, so that it cannot grow without limit; while everything
    /// fits within it nothing is let go and nothing is slower.
    decoded_bytes_budget: Option<usize>,
    /// Whether rows have been let go to keep within the bytes budget. From
    /// then on the store takes it that memory is short.
    let_go_for_bytes: bool,
    /// How much the file-backed indexes keep decoded: a sixteenth of the
    /// bytes budget between them whenever there is one, and a count of
    /// pages each once memory is short. Shared with the indexes, so that
    /// a budget set later reaches them.
    index_page_limit: Arc<crate::storage_engine::checkpoint::IndexPageBound>,
    /// Decoded documents in the order they were taken in, kept only while
    /// there is a budget. It may name documents since removed.
    decoded_order: VecDeque<ObjRef>,
    /// The checkpoint files, for reading back a record whose bytes are no
    /// longer held in memory. Absent while a store is being recovered.
    /// Locked only after the state, never before it.
    checkpoint_records: Option<Arc<Mutex<CheckpointReadHandles>>>,
    /// The rows written or deleted since the full checkpoint being built
    /// froze its commit, and that commit. When the checkpoint is installed
    /// these are the rows it does not cover. `None` while none is built.
    written_since_snapshot: Option<(u64, Vec<(String, DocumentId, ObjRef)>)>,
    /// How many rows each collection has, once counted, until a row of it
    /// is next written or deleted. Counting checks each row changed since
    /// the checkpoint against the checkpoint's files.
    collection_counts: Mutex<HashMap<String, usize>>,
    /// Ids looked up in the checkpoint lately, by collection, and the row
    /// the checkpoint holds under each, if any. The checkpoint does not
    /// change, so an answer stays good until another is installed. Kept
    /// to a fixed number of ids.
    ids_found_on_file: RwLock<(HashMap<String, HashMap<DocumentId, Option<ObjRef>>>, usize)>,
}

/// How many ids looked up in the checkpoint are remembered at once.
const IDS_REMEMBERED: usize = 65_536;

/// A row written after the commit a checkpoint froze, as it stands when
/// the checkpoint is installed.
enum WrittenLater {
    Live {
        schema: String,
        id: DocumentId,
        object_ref: ObjRef,
        version: u32,
        document: Option<Arc<Document>>,
    },
    Gone {
        schema: String,
        id: DocumentId,
        object_ref: ObjRef,
    },
}

/// How each row written since the frozen commit stands now, deleted rows
/// first, so that an id deleted and written again ends up written.
fn rows_written_later(
    state: &State,
    written: Vec<(String, DocumentId, ObjRef)>,
) -> Vec<WrittenLater> {
    let mut seen = HashSet::new();
    let mut later = Vec::new();
    // The last time a row was touched names it as it is now.
    for (schema, id, object_ref) in written.into_iter().rev() {
        if !seen.insert(object_ref) {
            continue;
        }
        later.push(match state.object_schema_versions.get(&object_ref) {
            Some(version) => WrittenLater::Live {
                schema,
                id,
                object_ref,
                version: *version,
                document: state.object_directory.get(&object_ref).cloned(),
            },
            None => WrittenLater::Gone { schema, id, object_ref },
        });
    }
    later.sort_by_key(|row| matches!(row, WrittenLater::Live { .. }));
    later
}

/// The column segments held decoded while a database has a budget come to
/// about this much, as encoded, at most.
const SEGMENT_BYTES_UNDER_A_BUDGET: usize = 64 << 20;

/// The memory the rows held decoded may take when a database is opened and
/// told nothing: what `OWL_DECODED_BYTES` says (0 for no limit), or a
/// quarter of the machine's memory. `None` where neither is known.
///
/// A database that fits within it behaves as one with no limit. One that
/// does not lets its oldest rows go, and reads them again when wanted,
/// where it would otherwise grow until the machine had no memory left.
fn default_decoded_bytes() -> Option<usize> {
    static DEFAULT: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *DEFAULT.get_or_init(|| {
        if let Ok(asked) = std::env::var("OWL_DECODED_BYTES") {
            return asked.trim().parse::<usize>().ok().filter(|bytes| *bytes > 0);
        }
        machine_memory().map(|bytes| bytes / 4)
    })
}

/// How much memory the machine has, in bytes, where that can be learned.
fn machine_memory() -> Option<usize> {
    // Linux says so in a file.
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        return text
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))
            .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<usize>().ok())
            .map(|kilobytes| kilobytes.saturating_mul(1024));
    }
    // macOS and the BSDs answer a question.
    std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.trim().parse::<usize>().ok())
}

/// An index keeps this many of its pages decoded while there is a budget.
const INDEX_PAGES_UNDER_A_BUDGET: usize = 64;

impl State {
    /// The bound the file-backed indexes go by, brought in line with the
    /// budget, and handed to an index being made.
    /// Whether there is any limit on the rows held decoded, so that a row
    /// held now may be let go later.
    fn bounded(&self) -> bool {
        self.decoded_budget.is_some() || self.decoded_bytes_budget.is_some()
    }

    /// Whether the checkpoint holds rows that are found through its files
    /// and not through `collection_rows`, so that a change to one must be
    /// noted in `typed_delta_rows`.
    /// A row of the collection was written or deleted: its count is no
    /// longer known.
    fn forget_count(&mut self, schema: &str) {
        match self.collection_counts.get_mut() {
            Ok(counts) => {
                counts.remove(schema);
            }
            Err(poisoned) => poisoned.into_inner().clear(),
        }
    }

    /// What rows are listed where has changed: no count is known.
    fn forget_counts(&mut self) {
        match self.collection_counts.get_mut() {
            Ok(counts) => counts.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
        // The checkpoint read may be another one now.
        *match self.ids_found_on_file.get_mut() {
            Ok(found) => found,
            Err(poisoned) => poisoned.into_inner(),
        } = Default::default();
    }

    fn has_rows_on_file(&self) -> bool {
        !self.typed_segments.is_empty() || self.overflow_ids.is_some()
    }

    /// Whether memory is short: a count of rows was set, which says so, or
    /// the bytes budget has had rows let go. Until then a database with
    /// only the default budget keeps everything it would with none.
    fn tight(&self) -> bool {
        self.decoded_budget.is_some() || (self.decoded_bytes_budget.is_some() && self.let_go_for_bytes)
    }

    /// The budget is one figure for everything the store keeps so as not
    /// to read it again, shared out: a sixteenth for the index pages held
    /// decoded, the rest of a half for the rows held decoded, a quarter
    /// for the column segments held decoded, and a quarter for the
    /// columns and row lists kept between queries.
    fn rows_bytes(&self) -> Option<usize> {
        self.decoded_bytes_budget.map(|bytes| bytes / 2 - bytes / 16)
    }

    /// The share of the budget for index pages held decoded; zero for
    /// no limit.
    fn index_page_bytes(&self) -> usize {
        self.decoded_bytes_budget.map_or(0, |bytes| (bytes / 16).max(1))
    }

    /// The share of the budget for column segments held decoded.
    fn segment_bytes(&self) -> usize {
        self.decoded_bytes_budget
            .map_or(SEGMENT_BYTES_UNDER_A_BUDGET, |bytes| (bytes / 4).max(1))
    }

    /// The share of the budget for what is kept between queries.
    fn kept_bytes(&self) -> usize {
        self.decoded_bytes_budget
            .map_or(BUILT_COLUMNS_LIMIT, |bytes| bytes / 4)
    }

    fn index_pages(&self) -> Arc<crate::storage_engine::checkpoint::IndexPageBound> {
        self.index_page_limit.set(
            if self.tight() { INDEX_PAGES_UNDER_A_BUDGET } else { 0 },
            self.index_page_bytes(),
        );
        // A database short of memory also bounds the column segments this
        // process holds decoded. One that is not leaves that as it finds it.
        if self.tight() {
            crate::storage_engine::checkpoint::segments_loaded().set_limit(self.segment_bytes());
        }
        self.index_page_limit.clone()
    }

    fn note_decoded(&mut self, object_ref: ObjRef) {
        if self.bounded() {
            self.decoded_order.push_back(object_ref);
        }
    }
}

/// Drop the bytes of records a checkpoint file holds.
///
/// With a budget, a row outside the column segments that is not held
/// decoded is decoded from its record, so a record the checkpoint files do
/// not hold is kept.
fn evict_clean_records(state: &mut State) {
    let State {
        object_records,
        object_schema_versions,
        object_directory,
        typed_row_locations,
        decoded_budget,
        decoded_bytes_budget,
        checkpoint_records: state_checkpoint_records,
        ..
    } = state;
    if decoded_budget.is_none() && decoded_bytes_budget.is_none() {
        object_records.evict_clean();
        return;
    }
    // Such a row is read back from the checkpoint files when they hold it,
    // and only then can its bytes go.
    let files = state_checkpoint_records.as_ref().and_then(|files| files.lock().ok());
    object_records.evict_clean_unless(|object_ref| {
        object_schema_versions.contains_key(object_ref)
            && !object_directory.contains_key(object_ref)
            && !typed_row_locations.contains_key(object_ref).unwrap_or(true)
            && !files
                .as_ref()
                .is_some_and(|files| files.holds(*object_ref).unwrap_or(false))
    });
}

/// Let go of decoded documents beyond the budget.
///
/// A document is dropped only when it can be decoded again from memory or
/// from a column segment: a row with no segment behind it keeps its
/// record, encoded afresh if a checkpoint had already taken the bytes.
fn enforce_decoded_budget(state: &mut State) -> OQLResult<()> {
    if !state.bounded() {
        return Ok(());
    }
    let budget = state.decoded_budget.unwrap_or(usize::MAX);
    let bytes = state.rows_bytes().unwrap_or(usize::MAX);
    if state.object_directory.bytes() > bytes && !state.let_go_for_bytes {
        // Memory is short from here on: what is cached is bounded too.
        state.let_go_for_bytes = true;
        state.index_pages();
    }
    let mut refilled = false;
    while state.object_directory.len() > budget || state.object_directory.bytes() > bytes {
        let Some(candidate) = state.decoded_order.pop_front() else {
            // The order lost track, as it does when the budget was set
            // after documents were already held. Start it over, once.
            if refilled {
                break;
            }
            refilled = true;
            state.decoded_order = state.object_directory.keys().copied().collect();
            continue;
        };
        let Some(document) = state.object_directory.get(&candidate) else {
            continue;
        };
        // A row with no column segment behind it is decoded again from its
        // record: the one in memory, or the one in the checkpoint files.
        let on_file = !state.object_records.is_dirty(&candidate)
            && match &state.checkpoint_records {
                Some(files) => files
                    .lock()
                    .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
                    .holds(candidate)?,
                None => false,
            };
        if !state.typed_row_locations.contains_key(&candidate)?
            && state.object_records.get(&candidate).is_none()
            && !on_file
        {
            let compiled = state
                .object_schema_versions
                .get(&candidate)
                .and_then(|version| {
                    state
                        .compiled_schemas
                        .get(&(candidate.collection_id(), *version))
                });
            let Some(compiled) = compiled else {
                // Nothing to decode it from again, so it stays.
                continue;
            };
            let record = RecordCodec::encode(document, candidate, compiled)?;
            state.object_records.insert_clean(candidate, record);
        }
        state.object_directory.remove(&candidate);
    }
    Ok(())
}

/// The documents one committed batch wrote.
#[derive(Debug, Default)]
struct WrittenKeys {
    /// A change to a schema or an index, which bears on every document.
    everything: bool,
    documents: HashSet<(String, DocumentId)>,
}

impl WrittenKeys {
    fn of(operations: &[WriteOperation]) -> Self {
        let mut keys = Self::default();
        for operation in operations {
            match operation {
                WriteOperation::PutDocument(document) => {
                    keys.documents
                        .insert((document.schema.clone(), document.id.clone()));
                }
                WriteOperation::DeleteDocument { schema, id } => {
                    keys.documents.insert((schema.clone(), id.clone()));
                }
                _ => keys.everything = true,
            }
        }
        keys
    }

    /// The first thing both batches wrote, described for an error message.
    fn overlap(&self, other: &Self) -> Option<String> {
        if self.everything || other.everything {
            return Some("a schema or an index".to_string());
        }
        let (small, large) = if self.documents.len() <= other.documents.len() {
            (&self.documents, &other.documents)
        } else {
            (&other.documents, &self.documents)
        };
        small
            .iter()
            .find(|key| large.contains(*key))
            .map(|(schema, id)| format!("'{schema}' document '{id}'"))
    }
}

/// Commits that an open transaction may conflict with.
///
/// A transaction conflicts when a batch committed after it began wrote a
/// document it also writes. Batches are remembered only while a transaction
/// that began before them is still open, so with no transaction open this
/// holds nothing.
#[derive(Debug, Default)]
struct ConflictLog {
    /// Open transactions and the commit each one began after.
    open: BTreeMap<u64, u64>,
    /// Committed batches newer than the oldest open transaction.
    recent: VecDeque<(u64, WrittenKeys)>,
    /// Batches at or below this commit were forgotten to bound memory, so
    /// a transaction that began before it cannot be checked and is refused.
    forgotten_through: u64,
}

impl ConflictLog {
    /// Batches remembered for the sake of one long-open transaction.
    const MOST_BATCHES: usize = 65_536;

    fn refuse(reason: String) -> OQLError {
        OQLError::TransactionConflict { message: reason }
    }

    /// Whether transaction `id` may commit `written`.
    fn check(&self, id: u64, written: &WrittenKeys) -> OQLResult<()> {
        let began_after = *self.open.get(&id).ok_or_else(|| OQLError::TransactionError {
            message: format!("transaction {id} is not open"),
        })?;
        if began_after < self.forgotten_through {
            return Err(Self::refuse(
                "the transaction was open for too long to be checked against later commits"
                    .to_string(),
            ));
        }
        for (lsn, batch) in self.recent.iter().rev() {
            if *lsn <= began_after {
                break;
            }
            if let Some(what) = batch.overlap(written) {
                return Err(Self::refuse(format!(
                    "{what} was changed by another commit after this transaction began"
                )));
            }
        }
        Ok(())
    }

    /// Note a committed batch for the transactions still open.
    fn committed(&mut self, lsn: u64, written: WrittenKeys) {
        if !self.open.is_empty() {
            self.recent.push_back((lsn, written));
            if self.recent.len() > Self::MOST_BATCHES {
                if let Some((forgotten, _)) = self.recent.pop_front() {
                    self.forgotten_through = forgotten;
                }
            }
        }
        self.forget_unneeded();
    }

    fn close(&mut self, id: u64) {
        self.open.remove(&id);
        self.forget_unneeded();
    }

    fn forget_unneeded(&mut self) {
        match self.open.values().min().copied() {
            None => self.recent.clear(),
            Some(oldest) => {
                while self.recent.front().is_some_and(|(lsn, _)| *lsn <= oldest) {
                    self.recent.pop_front();
                }
            }
        }
    }
}

thread_local! {
    /// The transaction this thread's reads and writes belong to, with the
    /// store it belongs to.
    static BOUND_TRANSACTION: std::cell::Cell<Option<(usize, u64)>> =
        const { std::cell::Cell::new(None) };
}

/// Binds a transaction to the current thread until dropped. See
/// [`RustStore::bind_transaction`].
#[must_use = "the transaction is bound only while this guard lives"]
pub struct TransactionBinding {
    previous: Option<(usize, u64)>,
    /// A binding belongs to the thread that made it.
    _thread: std::marker::PhantomData<*const ()>,
}

impl Drop for TransactionBinding {
    fn drop(&mut self) {
        BOUND_TRANSACTION.with(|bound| bound.set(self.previous));
    }
}

#[derive(Debug, Clone)]
struct TypedIdFence {
    first: DocumentId,
    last: DocumentId,
    segment_index: usize,
}

/// See `StoreCore::durability`.
struct Durability {
    /// Every frame up to this one is on disk.
    synced_lsn: u64,
    /// One committer is syncing; the rest wait for it.
    syncing: bool,
    /// A sync failed. What was applied since the last good one may not be
    /// on disk, so nothing more is acknowledged.
    failed: Option<String>,
}

/// The writes of a transaction that has not committed. Only the
/// transaction itself reads them.
#[derive(Debug, Default)]
struct PendingTransaction {
    operations: Vec<WriteOperation>,
    /// References promised to documents this transaction has written, so
    /// that its later writes can point at them before it commits.
    reserved: ReservedRefs,
}

/// The reference each not-yet-committed document will be stored under.
type ReservedRefs = HashMap<(String, DocumentId), ObjRef>;

struct PendingCheckpoint {
    image: CheckpointImage,
    live_objects: usize,
    commits_at_snapshot: u64,
}

struct PublishedCheckpoint {
    info: CheckpointInfo,
    /// Slot of the base segment file, which is independent of the manifest
    /// slot reported in `info`.
    segment_slot: u8,
    typed_locator: TypedLocatorLeaf,
    typed_segments: Vec<TypedSegmentFileDescriptor>,
    /// The pages of the index of rows by id, when the image carried one.
    id_pages: Option<PublishedIndexPages>,
}

#[derive(Debug)]
struct CompactionChange {
    schema: String,
    id: DocumentId,
    object_ref: ObjRef,
    ordinal: u64,
    record: Option<Vec<u8>>,
}

impl CompactionChange {
    fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.schema.len())
            .saturating_add(self.id.len())
            .saturating_add(self.record.as_ref().map_or(0, Vec::len))
    }

    fn identity(&self) -> (&str, &str) {
        (&self.schema, &self.id)
    }
}

#[derive(Default)]
struct DenseOverrideSet {
    collections: HashMap<u16, Vec<u64>>,
}

impl DenseOverrideSet {
    fn insert(&mut self, object_ref: ObjRef) {
        let words = self
            .collections
            .entry(object_ref.collection_id())
            .or_default();
        let object_id = object_ref.object_id() as usize;
        let word = object_id / u64::BITS as usize;
        if words.len() <= word {
            words.resize(word + 1, 0);
        }
        words[word] |= 1u64 << (object_id % u64::BITS as usize);
    }

    fn contains(&self, object_ref: &ObjRef) -> bool {
        let object_id = object_ref.object_id() as usize;
        self.collections
            .get(&object_ref.collection_id())
            .and_then(|words| words.get(object_id / u64::BITS as usize))
            .is_some_and(|word| word & (1u64 << (object_id % u64::BITS as usize)) != 0)
    }

    fn bytes(&self) -> usize {
        self.collections
            .values()
            .map(|words| words.len() * std::mem::size_of::<u64>())
            .sum()
    }
}

struct CompactionOverlay {
    filesystem: Arc<dyn FileSystem>,
    run_prefix: PathBuf,
    manifest_path: Option<PathBuf>,
    target_bytes: usize,
    buffer: BTreeMap<(String, DocumentId), CompactionChange>,
    buffer_bytes: usize,
    peak_buffer_bytes: usize,
    run_bytes: u64,
    peak_run_bytes: u64,
    runs: Vec<PathBuf>,
    cleanup_runs: Vec<PathBuf>,
    next_run_id: usize,
    ordinal: u64,
    overridden: DenseOverrideSet,
    schemas: BTreeSet<String>,
}

impl CompactionOverlay {
    fn new(
        filesystem: Arc<dyn FileSystem>,
        run_prefix: PathBuf,
        manifest_path: PathBuf,
        target_bytes: usize,
    ) -> Self {
        Self {
            filesystem,
            run_prefix,
            manifest_path: Some(manifest_path),
            target_bytes: target_bytes.max(1),
            buffer: BTreeMap::new(),
            buffer_bytes: 0,
            peak_buffer_bytes: 0,
            run_bytes: 0,
            peak_run_bytes: 0,
            runs: Vec::new(),
            cleanup_runs: Vec::new(),
            next_run_id: 0,
            ordinal: 0,
            overridden: DenseOverrideSet::default(),
            schemas: BTreeSet::new(),
        }
    }

    fn put(
        &mut self,
        schema: String,
        id: DocumentId,
        object_ref: ObjRef,
        record: Vec<u8>,
    ) -> OQLResult<()> {
        self.push(schema, id, object_ref, Some(record))
    }

    fn delete(&mut self, schema: String, id: DocumentId, object_ref: ObjRef) -> OQLResult<()> {
        self.push(schema, id, object_ref, None)
    }

    fn push(
        &mut self,
        schema: String,
        id: DocumentId,
        object_ref: ObjRef,
        record: Option<Vec<u8>>,
    ) -> OQLResult<()> {
        self.ordinal = self.ordinal.checked_add(1).ok_or_else(|| {
            OQLError::storage_error("compaction mutation ordinal exhausted", None)
        })?;
        self.overridden.insert(object_ref);
        self.schemas.insert(schema.clone());
        let change = CompactionChange {
            schema: schema.clone(),
            id: id.clone(),
            object_ref,
            ordinal: self.ordinal,
            record,
        };
        let change_bytes = change.estimated_bytes();
        let key = (schema, id);
        if let Some(previous) = self.buffer.insert(key, change) {
            self.buffer_bytes = self.buffer_bytes.saturating_sub(previous.estimated_bytes());
        }
        self.buffer_bytes = self.buffer_bytes.saturating_add(change_bytes);
        self.peak_buffer_bytes = self.peak_buffer_bytes.max(self.buffer_bytes);
        if self.buffer_bytes >= self.target_bytes {
            self.spill()?;
        }
        Ok(())
    }

    fn spill(&mut self) -> OQLResult<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let path = compaction_run_path(&self.run_prefix, self.next_run_id);
        self.next_run_id += 1;
        self.register_run(path.clone())?;
        // Track the path before creation so unwinding also removes a partially
        // written run.
        self.runs.push(path.clone());
        let mut file = self
            .filesystem
            .create(&path)
            .map_err(|error| storage_io("create compaction run", &path, error))?;
        let mut run_header = [0u8; COMPACTION_RUN_FILE_HEADER_LEN];
        run_header[..4].copy_from_slice(&COMPACTION_RUN_MAGIC);
        run_header[4..6].copy_from_slice(&COMPACTION_RUN_VERSION.to_le_bytes());
        write_all(file.as_mut(), &run_header)
            .map_err(|error| storage_io("write compaction run header", &path, error))?;
        let mut writer = CompactionRunWriter::new(file.as_mut(), &path);
        for change in self.buffer.values() {
            writer.push(change)?;
        }
        writer.finish()?;
        file.flush()
            .map_err(|error| storage_io("flush compaction run", &path, error))?;
        let size = file
            .size()
            .map_err(|error| storage_io("inspect compaction run", &path, error))?;
        self.run_bytes = self.run_bytes.saturating_add(size);
        self.peak_run_bytes = self.peak_run_bytes.max(self.run_bytes);
        self.buffer.clear();
        self.buffer_bytes = 0;
        Ok(())
    }

    fn finish(mut self) -> OQLResult<CompactionPlan> {
        self.spill()?;
        self.collapse_runs()?;
        Ok(CompactionPlan {
            filesystem: self.filesystem.clone(),
            manifest_path: self.manifest_path.take().ok_or_else(|| {
                OQLError::storage_error("compaction manifest ownership was lost", None)
            })?,
            runs: std::mem::take(&mut self.runs),
            cleanup_runs: std::mem::take(&mut self.cleanup_runs),
            overridden: std::mem::take(&mut self.overridden),
            schemas: std::mem::take(&mut self.schemas),
            peak_buffer_bytes: self.peak_buffer_bytes,
            run_bytes: self.run_bytes,
            peak_run_bytes: self.peak_run_bytes,
        })
    }

    fn register_run(&mut self, path: PathBuf) -> OQLResult<()> {
        self.cleanup_runs.push(path);
        publish_compaction_run_manifest(
            self.filesystem.as_ref(),
            self.manifest_path.as_deref().ok_or_else(|| {
                OQLError::storage_error("compaction manifest ownership was lost", None)
            })?,
            self.next_run_id,
        )
    }

    fn collapse_runs(&mut self) -> OQLResult<()> {
        while self.runs.len() > COMPACTION_MAX_MERGE_FAN_IN {
            let inputs = std::mem::take(&mut self.runs);
            let mut outputs =
                Vec::with_capacity(inputs.len().div_ceil(COMPACTION_MAX_MERGE_FAN_IN));
            for chunk in inputs.chunks(COMPACTION_MAX_MERGE_FAN_IN) {
                if chunk.len() == 1 {
                    outputs.push(chunk[0].clone());
                    continue;
                }
                let path = compaction_run_path(&self.run_prefix, self.next_run_id);
                self.next_run_id += 1;
                self.register_run(path.clone())?;
                let mut file = self
                    .filesystem
                    .create(&path)
                    .map_err(|error| storage_io("create merged compaction run", &path, error))?;
                let mut run_header = [0u8; COMPACTION_RUN_FILE_HEADER_LEN];
                run_header[..4].copy_from_slice(&COMPACTION_RUN_MAGIC);
                run_header[4..6].copy_from_slice(&COMPACTION_RUN_VERSION.to_le_bytes());
                write_all(file.as_mut(), &run_header).map_err(|error| {
                    storage_io("write merged compaction run header", &path, error)
                })?;
                let mut writer = CompactionRunWriter::new(file.as_mut(), &path);
                let mut cursor = CompactionChangeCursor::new(self.filesystem.as_ref(), chunk)?;
                while let Some(change) = cursor.next()? {
                    writer.push(&change)?;
                }
                writer.finish()?;
                file.flush()
                    .map_err(|error| storage_io("flush merged compaction run", &path, error))?;
                let output_size = file
                    .size()
                    .map_err(|error| storage_io("inspect merged compaction run", &path, error))?;
                self.run_bytes = self.run_bytes.saturating_add(output_size);
                self.peak_run_bytes = self.peak_run_bytes.max(self.run_bytes);
                for input in chunk {
                    let input_size = self.filesystem.file_size(input).unwrap_or(0);
                    if self.filesystem.exists(input) {
                        self.filesystem.remove(input).map_err(|error| {
                            storage_io("remove merged compaction input", input, error)
                        })?;
                    }
                    self.run_bytes = self.run_bytes.saturating_sub(input_size);
                }
                outputs.push(path);
            }
            self.runs = outputs;
        }
        Ok(())
    }
}

impl Drop for CompactionOverlay {
    fn drop(&mut self) {
        for path in &self.cleanup_runs {
            let _ = self.filesystem.remove(path);
        }
        if let Some(manifest_path) = &self.manifest_path {
            let _ = self.filesystem.remove(manifest_path);
            let _ = self
                .filesystem
                .remove(&compaction_manifest_temp_path(manifest_path));
        }
    }
}

struct CompactionPlan {
    filesystem: Arc<dyn FileSystem>,
    manifest_path: PathBuf,
    runs: Vec<PathBuf>,
    cleanup_runs: Vec<PathBuf>,
    overridden: DenseOverrideSet,
    schemas: BTreeSet<String>,
    peak_buffer_bytes: usize,
    run_bytes: u64,
    peak_run_bytes: u64,
}

impl Drop for CompactionPlan {
    fn drop(&mut self) {
        for path in &self.cleanup_runs {
            let _ = self.filesystem.remove(path);
        }
        let _ = self.filesystem.remove(&self.manifest_path);
        let _ = self
            .filesystem
            .remove(&compaction_manifest_temp_path(&self.manifest_path));
    }
}

fn compaction_manifest_temp_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.tmp", path.display()))
}

fn publish_compaction_run_manifest(
    filesystem: &dyn FileSystem,
    path: &Path,
    run_count: usize,
) -> OQLResult<()> {
    let run_count = u64::try_from(run_count)
        .map_err(|_| OQLError::storage_error("too many compaction run files", None))?;
    if run_count > MAX_COMPACTION_RUN_FILES {
        return Err(OQLError::storage_error(
            "compaction run count exceeds the safety limit",
            None,
        ));
    }
    let mut encoded = [0u8; COMPACTION_MANIFEST_LEN];
    encoded[..4].copy_from_slice(&COMPACTION_MANIFEST_MAGIC);
    encoded[4..6].copy_from_slice(&COMPACTION_MANIFEST_VERSION.to_le_bytes());
    encoded[8..16].copy_from_slice(&run_count.to_le_bytes());
    let checksum = crc32fast::hash(&encoded[..16]);
    encoded[16..20].copy_from_slice(&checksum.to_le_bytes());
    let existed = filesystem.exists(path);
    let mut file = if existed {
        filesystem.open_with_options(
            path,
            &FileOptions {
                read: true,
                write: true,
                create: false,
                truncate: false,
                append: false,
            },
        )
    } else {
        filesystem.create(path)
    }
    .map_err(|error| storage_io("open compaction run manifest", path, error))?;
    file.seek(SeekFrom::End(0))
        .map_err(|error| storage_io("seek compaction run manifest", path, error))?;
    write_all(file.as_mut(), &encoded)
        .and_then(|_| file.sync())
        .map_err(|error| storage_io("append compaction run manifest", path, error))?;
    if !existed {
        filesystem
            .sync_all()
            .map_err(|error| storage_io("sync compaction run manifest", path, error))?;
    }
    Ok(())
}

fn cleanup_compaction_runs(
    filesystem: &dyn FileSystem,
    path: &Path,
    run_prefix: &Path,
) -> OQLResult<()> {
    let temporary = compaction_manifest_temp_path(path);
    if filesystem.exists(&temporary) {
        filesystem
            .remove(&temporary)
            .map_err(|error| storage_io("remove stale compaction manifest", &temporary, error))?;
    }
    if !filesystem.exists(path) {
        return Ok(());
    }
    let size = filesystem
        .file_size(path)
        .map_err(|error| storage_io("inspect compaction run manifest", path, error))?;
    if size > MAX_COMPACTION_RUN_FILES.saturating_mul(COMPACTION_MANIFEST_LEN as u64) {
        return Err(OQLError::storage_error(
            "compaction run manifest exceeds the safety limit",
            None,
        ));
    }
    let mut file = filesystem
        .open(path)
        .map_err(|error| storage_io("open compaction run manifest", path, error))?;
    let mut run_count = 0u64;
    loop {
        let mut encoded = [0u8; COMPACTION_MANIFEST_LEN];
        let read = read_up_to(file.as_mut(), &mut encoded)
            .map_err(|error| storage_io("read compaction run manifest", path, error))?;
        if read == 0 {
            break;
        }
        // A record is durable before its corresponding run is created. A
        // torn final record therefore cannot name an untracked run.
        if read < encoded.len() {
            break;
        }
        let checksum = u32::from_le_bytes(encoded[16..20].try_into().unwrap());
        let count = u64::from_le_bytes(encoded[8..16].try_into().unwrap());
        if encoded[..4] != COMPACTION_MANIFEST_MAGIC
            || u16::from_le_bytes(encoded[4..6].try_into().unwrap()) != COMPACTION_MANIFEST_VERSION
            || encoded[6..8] != [0, 0]
            || crc32fast::hash(&encoded[..16]) != checksum
            || count != run_count.saturating_add(1)
            || count > MAX_COMPACTION_RUN_FILES
        {
            return Err(OQLError::storage_error(
                "compaction run manifest is corrupt",
                None,
            ));
        }
        run_count = count;
    }
    for run_id in 0..run_count {
        let run = compaction_run_path(run_prefix, run_id);
        if filesystem.exists(&run) {
            filesystem
                .remove(&run)
                .map_err(|error| storage_io("remove stale compaction run", &run, error))?;
        }
    }
    filesystem
        .remove(path)
        .map_err(|error| storage_io("remove compaction run manifest", path, error))
}

fn compaction_run_path(run_prefix: &Path, run_id: impl std::fmt::Display) -> PathBuf {
    PathBuf::from(format!("{}.run.{run_id}", run_prefix.display()))
}

fn encode_compaction_change(change: &CompactionChange) -> OQLResult<Vec<u8>> {
    let schema_len = u32::try_from(change.schema.len())
        .map_err(|_| OQLError::storage_error("compaction schema name is too large", None))?;
    let id_len = u32::try_from(change.id.len())
        .map_err(|_| OQLError::storage_error("compaction document ID is too large", None))?;
    let record_len = u32::try_from(change.record.as_ref().map_or(0, Vec::len))
        .map_err(|_| OQLError::storage_error("compaction record is too large", None))?;
    let mut payload = Vec::with_capacity(
        COMPACTION_CHANGE_FIXED_LEN
            .saturating_add(schema_len as usize)
            .saturating_add(id_len as usize)
            .saturating_add(record_len as usize),
    );
    payload.push(u8::from(change.record.is_some()));
    payload.extend_from_slice(&schema_len.to_le_bytes());
    payload.extend_from_slice(&id_len.to_le_bytes());
    payload.extend_from_slice(&record_len.to_le_bytes());
    payload.extend_from_slice(&change.object_ref.packed().to_le_bytes());
    payload.extend_from_slice(&change.ordinal.to_le_bytes());
    payload.extend_from_slice(change.schema.as_bytes());
    payload.extend_from_slice(change.id.as_bytes());
    if let Some(record) = &change.record {
        payload.extend_from_slice(record);
    }
    Ok(payload)
}

struct CompactionRunWriter<'a> {
    file: &'a mut dyn FileHandle,
    path: &'a Path,
    block: Vec<u8>,
    entries: u32,
}

impl<'a> CompactionRunWriter<'a> {
    fn new(file: &'a mut dyn FileHandle, path: &'a Path) -> Self {
        Self {
            file,
            path,
            block: Vec::with_capacity(COMPACTION_RUN_BLOCK_TARGET_BYTES),
            entries: 0,
        }
    }

    fn push(&mut self, change: &CompactionChange) -> OQLResult<()> {
        let payload = encode_compaction_change(change)?;
        let payload_len = u32::try_from(payload.len())
            .map_err(|_| OQLError::storage_error("compaction run entry is too large", None))?;
        if !self.block.is_empty()
            && self
                .block
                .len()
                .saturating_add(COMPACTION_RUN_ENTRY_HEADER_LEN)
                .saturating_add(payload.len())
                > COMPACTION_RUN_BLOCK_TARGET_BYTES
        {
            self.flush()?;
        }
        self.block.extend_from_slice(&payload_len.to_le_bytes());
        self.block.extend_from_slice(&payload);
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| OQLError::storage_error("too many compaction block entries", None))?;
        Ok(())
    }

    fn finish(&mut self) -> OQLResult<()> {
        self.flush()
    }

    fn flush(&mut self) -> OQLResult<()> {
        if self.block.is_empty() {
            return Ok(());
        }
        let raw_len = u32::try_from(self.block.len())
            .map_err(|_| OQLError::storage_error("compaction run block is too large", None))?;
        let compressed = lz4_flex::compress(&self.block);
        let stored = if compressed.len() < self.block.len() {
            compressed.as_slice()
        } else {
            self.block.as_slice()
        };
        let stored_len = u32::try_from(stored.len())
            .map_err(|_| OQLError::storage_error("compaction stored block is too large", None))?;
        let mut header = [0u8; COMPACTION_RUN_BLOCK_HEADER_LEN];
        header[..4].copy_from_slice(&raw_len.to_le_bytes());
        header[4..8].copy_from_slice(&stored_len.to_le_bytes());
        header[8..12].copy_from_slice(&crc32fast::hash(&self.block).to_le_bytes());
        header[12..16].copy_from_slice(&self.entries.to_le_bytes());
        write_all(self.file, &header)
            .and_then(|_| write_all(self.file, stored))
            .map_err(|error| storage_io("write compaction run block", self.path, error))?;
        self.block.clear();
        self.entries = 0;
        Ok(())
    }
}

struct CompactionRunReader {
    path: PathBuf,
    file: Box<dyn FileHandle>,
    block: Vec<u8>,
    block_cursor: usize,
    block_entries: u32,
}

impl CompactionRunReader {
    fn open(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<Self> {
        let mut reader = Self {
            path: path.to_path_buf(),
            file: filesystem
                .open(path)
                .map_err(|error| storage_io("open compaction run", path, error))?,
            block: Vec::new(),
            block_cursor: 0,
            block_entries: 0,
        };
        let mut header = [0u8; COMPACTION_RUN_FILE_HEADER_LEN];
        if read_up_to(reader.file.as_mut(), &mut header)
            .map_err(|error| storage_io("read compaction run file header", path, error))?
            != header.len()
        {
            return Err(OQLError::storage_error(
                "truncated compaction run file header",
                None,
            ));
        }
        if header[..4] != COMPACTION_RUN_MAGIC
            || u16::from_le_bytes(header[4..6].try_into().unwrap()) != COMPACTION_RUN_VERSION
            || header[6..] != [0, 0]
        {
            return Err(OQLError::storage_error(
                "invalid compaction run file header",
                None,
            ));
        }
        Ok(reader)
    }

    fn next(&mut self) -> OQLResult<Option<CompactionChange>> {
        if self.block_entries == 0 && !self.load_block()? {
            return Ok(None);
        }
        let length_end = self
            .block_cursor
            .checked_add(COMPACTION_RUN_ENTRY_HEADER_LEN)
            .ok_or_else(|| OQLError::storage_error("compaction entry offset overflow", None))?;
        if length_end > self.block.len() {
            return Err(OQLError::storage_error(
                "truncated compaction run entry header",
                None,
            ));
        }
        let payload_len = u32::from_le_bytes(
            self.block[self.block_cursor..length_end]
                .try_into()
                .unwrap(),
        ) as usize;
        if payload_len > MAX_WAL_FRAME {
            return Err(OQLError::storage_error(
                "compaction run entry is unreasonably large",
                None,
            ));
        }
        let payload_end = length_end
            .checked_add(payload_len)
            .ok_or_else(|| OQLError::storage_error("compaction entry length overflow", None))?;
        if payload_end > self.block.len() {
            return Err(OQLError::storage_error(
                "truncated compaction run entry",
                None,
            ));
        }
        let change = decode_compaction_change(&self.block[length_end..payload_end])?;
        self.block_cursor = payload_end;
        self.block_entries -= 1;
        if self.block_entries == 0 && self.block_cursor != self.block.len() {
            return Err(OQLError::storage_error(
                "compaction block entry count does not consume its payload",
                None,
            ));
        }
        Ok(Some(change))
    }

    fn load_block(&mut self) -> OQLResult<bool> {
        let mut header = [0u8; COMPACTION_RUN_BLOCK_HEADER_LEN];
        let header_len = read_up_to(self.file.as_mut(), &mut header)
            .map_err(|error| storage_io("read compaction run block header", &self.path, error))?;
        if header_len == 0 {
            return Ok(false);
        }
        if header_len != header.len() {
            return Err(OQLError::storage_error(
                "truncated compaction run block header",
                None,
            ));
        }
        let raw_len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let stored_len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let checksum = u32::from_le_bytes(header[8..12].try_into().unwrap());
        let entries = u32::from_le_bytes(header[12..16].try_into().unwrap());
        let maximum_block = MAX_WAL_FRAME.saturating_add(COMPACTION_CHANGE_FIXED_LEN);
        if raw_len == 0
            || stored_len == 0
            || stored_len > raw_len
            || raw_len > maximum_block
            || entries == 0
        {
            return Err(OQLError::storage_error(
                "invalid compaction run block dimensions",
                None,
            ));
        }
        let mut stored = vec![0u8; stored_len];
        if read_up_to(self.file.as_mut(), &mut stored)
            .map_err(|error| storage_io("read compaction run block", &self.path, error))?
            != stored_len
        {
            return Err(OQLError::storage_error(
                "truncated compaction run block",
                None,
            ));
        }
        self.block = if stored_len < raw_len {
            lz4_flex::decompress(&stored, raw_len).map_err(|_| {
                OQLError::storage_error("invalid compressed compaction run block", None)
            })?
        } else {
            stored
        };
        if crc32fast::hash(&self.block) != checksum {
            return Err(OQLError::storage_error(
                "compaction run block checksum mismatch",
                None,
            ));
        }
        self.block_cursor = 0;
        self.block_entries = entries;
        Ok(true)
    }
}

fn decode_compaction_change(payload: &[u8]) -> OQLResult<CompactionChange> {
    if payload.len() < COMPACTION_CHANGE_FIXED_LEN {
        return Err(OQLError::storage_error(
            "compaction run entry is shorter than its fixed header",
            None,
        ));
    }
    let has_record = match payload[0] {
        0 => false,
        1 => true,
        _ => {
            return Err(OQLError::storage_error(
                "compaction run entry has invalid flags",
                None,
            ))
        }
    };
    let schema_len = u32::from_le_bytes(payload[1..5].try_into().unwrap()) as usize;
    let id_len = u32::from_le_bytes(payload[5..9].try_into().unwrap()) as usize;
    let record_len = u32::from_le_bytes(payload[9..13].try_into().unwrap()) as usize;
    if !has_record && record_len != 0 {
        return Err(OQLError::storage_error(
            "compaction tombstone contains record bytes",
            None,
        ));
    }
    let object_ref = ObjRef::new(u64::from_le_bytes(payload[13..21].try_into().unwrap()));
    if object_ref.is_null() {
        return Err(OQLError::storage_error(
            "compaction run entry has a null object reference",
            None,
        ));
    }
    let ordinal = u64::from_le_bytes(payload[21..29].try_into().unwrap());
    let schema_end = COMPACTION_CHANGE_FIXED_LEN
        .checked_add(schema_len)
        .ok_or_else(|| OQLError::storage_error("compaction schema length overflow", None))?;
    let id_end = schema_end
        .checked_add(id_len)
        .ok_or_else(|| OQLError::storage_error("compaction ID length overflow", None))?;
    let record_end = id_end
        .checked_add(record_len)
        .ok_or_else(|| OQLError::storage_error("compaction record length overflow", None))?;
    if record_end != payload.len() {
        return Err(OQLError::storage_error(
            "compaction run entry lengths do not match its frame",
            None,
        ));
    }
    let schema = std::str::from_utf8(&payload[COMPACTION_CHANGE_FIXED_LEN..schema_end])
        .map_err(|_| OQLError::storage_error("compaction schema is not UTF-8", None))?
        .to_owned();
    let id = std::str::from_utf8(&payload[schema_end..id_end])
        .map_err(|_| OQLError::storage_error("compaction document ID is not UTF-8", None))?
        .to_owned();
    Ok(CompactionChange {
        schema,
        id,
        object_ref,
        ordinal,
        record: has_record.then(|| payload[id_end..record_end].to_vec()),
    })
}

struct CompactionChangeCursor {
    readers: Vec<CompactionRunReader>,
    heads: Vec<Option<CompactionChange>>,
    heap: BinaryHeap<Reverse<(String, DocumentId, usize)>>,
}

impl CompactionChangeCursor {
    fn new(filesystem: &dyn FileSystem, paths: &[PathBuf]) -> OQLResult<Self> {
        let mut readers = paths
            .iter()
            .map(|path| CompactionRunReader::open(filesystem, path))
            .collect::<OQLResult<Vec<_>>>()?;
        let heads = readers
            .iter_mut()
            .map(CompactionRunReader::next)
            .collect::<OQLResult<Vec<_>>>()?;
        let heap = heads
            .iter()
            .enumerate()
            .filter_map(|(index, change)| {
                change
                    .as_ref()
                    .map(|change| Reverse((change.schema.clone(), change.id.clone(), index)))
            })
            .collect();
        Ok(Self {
            readers,
            heads,
            heap,
        })
    }

    fn next(&mut self) -> OQLResult<Option<CompactionChange>> {
        let Some(Reverse((schema, id, minimum))) = self.heap.pop() else {
            return Ok(None);
        };
        let mut latest = self.take_and_advance(minimum, &schema, &id)?;
        while self
            .heap
            .peek()
            .is_some_and(|Reverse((next_schema, next_id, _))| {
                next_schema == &schema && next_id == &id
            })
        {
            let Reverse((_, _, index)) = self.heap.pop().unwrap();
            let candidate = self.take_and_advance(index, &schema, &id)?;
            if candidate.ordinal > latest.ordinal {
                latest = candidate;
            }
        }
        Ok(Some(latest))
    }

    fn take_and_advance(
        &mut self,
        index: usize,
        schema: &str,
        id: &str,
    ) -> OQLResult<CompactionChange> {
        let change = self.heads[index].take().ok_or_else(|| {
            OQLError::storage_error("compaction merge heap references an empty run", None)
        })?;
        if change.identity() != (schema, id) {
            return Err(OQLError::storage_error(
                "compaction merge heap is inconsistent with its run",
                None,
            ));
        }
        self.heads[index] = self.readers[index].next()?;
        if let Some(next) = &self.heads[index] {
            self.heap
                .push(Reverse((next.schema.clone(), next.id.clone(), index)));
        }
        Ok(change)
    }
}

struct PendingIncrementalCheckpoint {
    manifest: Manifest,
    root: GenerationRoot,
    generation: FlushGeneration,
    wal: Vec<u8>,
    records: Vec<u8>,
    indexes: Vec<u8>,
    commits_at_snapshot: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlushGeneration {
    sequence: u64,
    start_lsn: u64,
    end_lsn: u64,
    len: u64,
    checksum: u32,
    record_len: u64,
    record_checksum: u32,
    index_len: u64,
    index_checksum: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GenerationRoot {
    sequence: u64,
    checkpoint_lsn: u64,
    base_sequence: u64,
    base_lsn: u64,
    generations: Vec<FlushGeneration>,
}

impl GenerationRoot {
    fn encode(&self) -> OQLResult<Vec<u8>> {
        if self.generations.is_empty() || self.generations.len() > MAX_FLUSH_GENERATIONS {
            return Err(OQLError::storage_error(
                "flush generation root has an invalid generation count",
                None,
            ));
        }
        self.validate()?;
        let mut bytes = vec![
            0u8;
            GENERATION_ROOT_HEADER_LEN
                + self.generations.len() * GENERATION_ROOT_ENTRY_LEN
        ];
        bytes[..4].copy_from_slice(&GENERATION_ROOT_MAGIC);
        bytes[4..6].copy_from_slice(&GENERATION_ROOT_VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&(GENERATION_ROOT_HEADER_LEN as u16).to_le_bytes());
        bytes[8..16].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.checkpoint_lsn.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.base_sequence.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.base_lsn.to_le_bytes());
        bytes[40..44].copy_from_slice(&(self.generations.len() as u32).to_le_bytes());
        for (index, generation) in self.generations.iter().enumerate() {
            let offset = GENERATION_ROOT_HEADER_LEN + index * GENERATION_ROOT_ENTRY_LEN;
            bytes[offset..offset + 8].copy_from_slice(&generation.sequence.to_le_bytes());
            bytes[offset + 8..offset + 16].copy_from_slice(&generation.start_lsn.to_le_bytes());
            bytes[offset + 16..offset + 24].copy_from_slice(&generation.end_lsn.to_le_bytes());
            bytes[offset + 24..offset + 32].copy_from_slice(&generation.len.to_le_bytes());
            bytes[offset + 32..offset + 36].copy_from_slice(&generation.checksum.to_le_bytes());
            bytes[offset + 40..offset + 48].copy_from_slice(&generation.record_len.to_le_bytes());
            bytes[offset + 48..offset + 52]
                .copy_from_slice(&generation.record_checksum.to_le_bytes());
            bytes[offset + 56..offset + 64].copy_from_slice(&generation.index_len.to_le_bytes());
            bytes[offset + 64..offset + 68]
                .copy_from_slice(&generation.index_checksum.to_le_bytes());
        }
        let checksum = crc32fast::hash(&bytes);
        bytes[44..48].copy_from_slice(&checksum.to_le_bytes());
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> OQLResult<Self> {
        if bytes.len() < GENERATION_ROOT_HEADER_LEN || bytes[..4] != GENERATION_ROOT_MAGIC {
            return Err(OQLError::storage_error(
                "invalid flush generation root header",
                None,
            ));
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if !matches!(version, 1 | GENERATION_ROOT_VERSION)
            || u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as usize
                != GENERATION_ROOT_HEADER_LEN
        {
            return Err(OQLError::storage_error(
                "invalid flush generation root header",
                None,
            ));
        }
        let count = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        let entry_len = if version == 1 {
            LEGACY_GENERATION_ROOT_ENTRY_LEN
        } else {
            GENERATION_ROOT_ENTRY_LEN
        };
        if count == 0
            || count > MAX_FLUSH_GENERATIONS
            || bytes.len() != GENERATION_ROOT_HEADER_LEN + count * entry_len
        {
            return Err(OQLError::storage_error(
                "flush generation root has an invalid length",
                None,
            ));
        }
        let stored = u32::from_le_bytes(bytes[44..48].try_into().unwrap());
        let mut authenticated = bytes.to_vec();
        authenticated[44..48].fill(0);
        if crc32fast::hash(&authenticated) != stored {
            return Err(OQLError::storage_error(
                "flush generation root checksum mismatch",
                None,
            ));
        }
        let mut generations = Vec::with_capacity(count);
        for index in 0..count {
            let offset = GENERATION_ROOT_HEADER_LEN + index * entry_len;
            generations.push(FlushGeneration {
                sequence: u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()),
                start_lsn: u64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().unwrap()),
                end_lsn: u64::from_le_bytes(bytes[offset + 16..offset + 24].try_into().unwrap()),
                len: u64::from_le_bytes(bytes[offset + 24..offset + 32].try_into().unwrap()),
                checksum: u32::from_le_bytes(bytes[offset + 32..offset + 36].try_into().unwrap()),
                record_len: u64::from_le_bytes(bytes[offset + 40..offset + 48].try_into().unwrap()),
                record_checksum: u32::from_le_bytes(
                    bytes[offset + 48..offset + 52].try_into().unwrap(),
                ),
                index_len: if version == 1 {
                    0
                } else {
                    u64::from_le_bytes(bytes[offset + 56..offset + 64].try_into().unwrap())
                },
                index_checksum: if version == 1 {
                    0
                } else {
                    u32::from_le_bytes(bytes[offset + 64..offset + 68].try_into().unwrap())
                },
            });
        }
        let root = Self {
            sequence: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            checkpoint_lsn: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            base_sequence: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            base_lsn: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            generations,
        };
        root.validate()?;
        Ok(root)
    }

    fn validate(&self) -> OQLResult<()> {
        let mut previous_lsn = self.base_lsn;
        let mut previous_sequence = self.base_sequence;
        for generation in &self.generations {
            if generation.sequence <= previous_sequence
                || generation.start_lsn != previous_lsn.saturating_add(1)
                || generation.end_lsn < generation.start_lsn
                || generation.len == 0
                || generation.record_len == 0
            {
                return Err(OQLError::storage_error(
                    "flush generations are not contiguous and ordered",
                    None,
                ));
            }
            previous_sequence = generation.sequence;
            previous_lsn = generation.end_lsn;
        }
        if previous_sequence != self.sequence || previous_lsn != self.checkpoint_lsn {
            return Err(OQLError::storage_error(
                "flush generation root does not reach its checkpoint",
                None,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IndexReplayMode {
    Maintain,
    DeferToRoot,
}

#[derive(Default)]
struct CheckpointReadHandles {
    record_root: Option<RecordRootHandle>,
    generation_record_roots: Vec<RecordRootHandle>,
    base_segment: Option<BaseSegmentHandle>,
}

impl CheckpointReadHandles {
    /// The record the checkpoint files hold for a live object, newest file
    /// first. `None` when they hold none.
    fn record(&self, object_ref: ObjRef) -> OQLResult<Option<Arc<[u8]>>> {
        for root in &self.generation_record_roots {
            match root.lookup(object_ref)? {
                RecordDeltaLookup::Put(record) => return Ok(Some(record)),
                RecordDeltaLookup::Delete => {
                    return Err(OQLError::storage_error(
                        "live object is deleted by an immutable generation",
                        None,
                    ));
                }
                RecordDeltaLookup::Missing => {}
            }
        }
        match self
            .record_root
            .as_ref()
            .map(|root| root.lookup(object_ref))
            .transpose()?
        {
            Some(RecordDeltaLookup::Put(record)) => return Ok(Some(record)),
            Some(RecordDeltaLookup::Delete) => {
                return Err(OQLError::storage_error(
                    "live object is deleted by its checkpoint record root",
                    None,
                ));
            }
            Some(RecordDeltaLookup::Missing) | None => {}
        }
        self.base_segment
            .as_ref()
            .map(|base| base.lookup(object_ref))
            .transpose()
            .map(Option::flatten)
    }

    /// The record the checkpoint files hold for an object that may or may
    /// not be live in them. `None` when they hold none, or say it was
    /// deleted.
    fn record_if_held(&self, object_ref: ObjRef) -> OQLResult<Option<Arc<[u8]>>> {
        for root in self.generation_record_roots.iter().chain(&self.record_root) {
            match root.lookup(object_ref)? {
                RecordDeltaLookup::Put(record) => return Ok(Some(record)),
                RecordDeltaLookup::Delete => return Ok(None),
                RecordDeltaLookup::Missing => {}
            }
        }
        self.base_segment
            .as_ref()
            .map(|base| base.lookup(object_ref))
            .transpose()
            .map(Option::flatten)
    }

    /// Whether [`Self::record`] would find a record for this object. The
    /// base segment answers from its directory, without reading the record.
    fn holds(&self, object_ref: ObjRef) -> OQLResult<bool> {
        for root in self.generation_record_roots.iter().chain(&self.record_root) {
            match root.lookup(object_ref)? {
                RecordDeltaLookup::Put(_) => return Ok(true),
                RecordDeltaLookup::Delete => return Ok(false),
                RecordDeltaLookup::Missing => {}
            }
        }
        self.base_segment
            .as_ref()
            .map_or(Ok(false), |base| base.contains(object_ref))
    }
}

/// The initial, entirely Rust-owned Owl storage engine.
///
/// Between checkpoints the WAL is the durable mutation stream. Opening a store
/// loads the newest published snapshot segment, rebuilds its in-memory access
/// structures, and replays only WAL batches newer than the checkpoint LSN.
enum MaintenanceMessage {
    Checkpoint,
    Drain(mpsc::Sender<()>),
    Shutdown,
}

#[doc(hidden)]
/// The rows held decoded, by reference, with a running count of roughly
/// how much memory they take.
#[derive(Default)]
struct DecodedRows {
    rows: crate::storage_engine::object_ref::ByObjRef<Arc<Document>>,
    bytes: usize,
}

impl DecodedRows {
    /// Roughly what a decoded row takes: itself, its id and collection
    /// name, and each value with what it holds.
    fn weight(document: &Document) -> usize {
        fn of_value(value: &Value) -> usize {
            std::mem::size_of::<Value>()
                + match value {
                    Value::String(text) => text.len(),
                    Value::List(items) => items.iter().map(of_value).sum(),
                    Value::Object(fields) => 40 + fields.values().map(of_value).sum::<usize>(),
                    _ => 0,
                }
        }
        std::mem::size_of::<Document>()
            + 48
            + document.id.len()
            + document.schema.len()
            + document.fields.values().map(of_value).sum::<usize>()
    }

    fn get(&self, object_ref: &ObjRef) -> Option<&Arc<Document>> {
        self.rows.get(object_ref)
    }

    fn contains_key(&self, object_ref: &ObjRef) -> bool {
        self.rows.contains_key(object_ref)
    }

    fn insert(&mut self, object_ref: ObjRef, document: Arc<Document>) -> Option<Arc<Document>> {
        self.bytes += Self::weight(&document);
        let replaced = self.rows.insert(object_ref, document);
        if let Some(replaced) = &replaced {
            self.bytes = self.bytes.saturating_sub(Self::weight(replaced));
        }
        replaced
    }

    fn remove(&mut self, object_ref: &ObjRef) -> Option<Arc<Document>> {
        let removed = self.rows.remove(object_ref);
        if let Some(removed) = &removed {
            self.bytes = self.bytes.saturating_sub(Self::weight(removed));
        }
        removed
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    fn capacity(&self) -> usize {
        self.rows.capacity()
    }

    fn keys(&self) -> impl Iterator<Item = &ObjRef> + '_ {
        self.rows.keys()
    }

    /// Roughly how many bytes the rows held take.
    fn bytes(&self) -> usize {
        self.bytes
    }
}

impl<'a> IntoIterator for &'a DecodedRows {
    type Item = (&'a ObjRef, &'a Arc<Document>);
    type IntoIter = std::collections::hash_map::Iter<'a, ObjRef, Arc<Document>>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

/// See [`StoreCore::built_columns`].
#[derive(Default)]
struct BuiltColumns {
    /// What the whole cache was built against: the last schema change, and
    /// the last time rows were moved into column segments. Either moving
    /// empties it.
    epoch: (u64, u64),
    collections: HashMap<String, BuiltCollection>,
    /// The most bytes to keep: the store's share of its budget for this,
    /// as it was when the cache was last asked for a collection.
    limit: usize,
}

/// What is kept of one collection's rows between queries.
#[derive(Default)]
struct BuiltCollection {
    /// The commit that last wrote the collection when this was built.
    written: u64,
    /// Unlike any other build's: two batches with the same stamp are of
    /// the same rows.
    stamp: u64,
    /// The collection's rows, in the order its columns are in. Listing them
    /// is itself a pass over the collection.
    rows: Option<Arc<Vec<Arc<Document>>>>,
    /// The same list, held so that it keeps no row in memory: used while
    /// there is any limit on decoded rows, when a row listed may be let go.
    /// It is good for as long as every row in it is still held somewhere.
    weak_rows: Option<Arc<Vec<std::sync::Weak<Document>>>>,
    /// Which row each position in the columns is, when the rows themselves
    /// are not kept: under a budget on decoded rows, a row is decoded when
    /// it is wanted and only then.
    refs: Option<Arc<Vec<ObjRef>>>,
    columns: HashMap<String, Arc<TypedColumn>>,
    /// What the executor made from these columns and asked to have kept
    /// with them.
    kept: HashMap<String, Arc<dyn std::any::Any + Send + Sync>>,
    /// Roughly what the rows list and columns take.
    bytes: usize,
    /// How many statements have asked for the collection's rows or
    /// columns since it was last written. Nothing is built for the first:
    /// a collection written between every read would build its columns
    /// only to drop them.
    asked: u32,
    /// The statement that asked last.
    asked_by: u64,
    /// What a set of fields' columns took, with all that goes with them,
    /// when they were last made and found to be more than may be kept.
    /// Making columns is a pass over the collection, so a set known not
    /// to fit is not made again only to be let go.
    too_big: HashMap<Vec<String>, usize>,
}

impl BuiltCollection {
    /// Note that the running statement asks; `true` once enough have.
    fn asks(&mut self) -> bool {
        let statement = crate::storage::statement_now();
        if self.asked_by != statement {
            self.asked_by = statement;
            self.asked = self.asked.saturating_add(1);
        }
        self.asked >= 2
    }
}

/// The cache of built columns holds about this much at most where the
/// store has no budget; with one, it holds its share of that. Past it,
/// other collections' columns are dropped, and if that is not enough the
/// query reads rows instead.
const BUILT_COLUMNS_LIMIT: usize = 256 << 20;

impl BuiltColumns {
    /// What is kept of `schema`, emptied first if anything it was built
    /// from has since been written.
    fn of<'a>(&'a mut self, state: &State, schema: &str) -> &'a mut BuiltCollection {
        static STAMPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        self.limit = state.kept_bytes();
        let epoch = (state.catalog_stamp, state.typed_base_lsn);
        if self.epoch != epoch {
            self.epoch = epoch;
            self.collections.clear();
        }
        let written = state.collection_stamps.get(schema).copied().unwrap_or(0);
        let kept = self.collections.entry(schema.to_string()).or_default();
        if kept.stamp == 0 || kept.written != written {
            *kept = BuiltCollection {
                written,
                stamp: STAMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                ..BuiltCollection::default()
            };
        }
        kept
    }

    /// Whether `more` bytes for `schema` fit, after dropping what is kept
    /// of other collections if need be.
    fn makes_room(&mut self, schema: &str, more: usize) -> bool {
        let held = |collections: &HashMap<String, BuiltCollection>| -> usize {
            collections.values().map(|kept| kept.bytes).sum()
        };
        if held(&self.collections) + more > self.limit {
            // What the running statement has itself asked for stays: a
            // query that reads two collections would otherwise drop the
            // first one's columns to keep the second's, and make them
            // again the next time it ran, a pass over the collection each
            // time.
            let statement = crate::storage::statement_now();
            self.collections
                .retain(|name, kept| name == schema || kept.asked_by == statement);
        }
        held(&self.collections) + more <= self.limit
    }
}

pub struct StoreCore {
    filesystem: Arc<dyn FileSystem>,
    wal_path: PathBuf,
    sealed_wal_path: PathBuf,
    manifest_paths: [PathBuf; 2],
    segment_paths: [PathBuf; 2],
    generation_root_paths: [PathBuf; 2],
    generation_prefix: PathBuf,
    record_delta_paths: [PathBuf; 2],
    checkpoint_read_handles: Arc<Mutex<CheckpointReadHandles>>,
    index_delta_paths: [PathBuf; 2],
    wal_delta_paths: [PathBuf; 2],
    max_size_bytes: usize,
    wal: Mutex<Option<Box<dyn FileHandle>>>,
    /// Readers share this; only a commit, a checkpoint or caching a decoded
    /// row takes it alone.
    state: RwLock<State>,
    commit_lock: Mutex<()>,
    checkpoint_lock: Mutex<()>,
    pending_checkpoint: Mutex<Option<PendingCheckpoint>>,
    pending_incremental_checkpoint: Mutex<Option<PendingIncrementalCheckpoint>>,
    /// Fields of whole collections as columns, built from the held rows
    /// when first asked for and kept until the next commit.
    built_columns: Mutex<BuiltColumns>,
    transactions: Mutex<HashMap<u64, PendingTransaction>>,
    /// How far the WAL is known to be on disk. A commit writes its frame
    /// and applies it, then waits here until a sync covers it; one sync
    /// covers every frame written before it began, so commits that arrive
    /// together share one.
    durability: Mutex<Durability>,
    durable: Condvar,
    /// The last frame written to the WAL, synced or not.
    written_lsn: AtomicU64,
    /// How many bytes of the active WAL file are known to be on disk. Each
    /// frame carries the value at the time it is written. It may run behind
    /// the truth, never ahead, and is only changed while holding the WAL.
    wal_synced_len: AtomicU64,
    /// Commits that have started and not yet written their frame.
    commits_in_flight: AtomicUsize,
    next_transaction_id: AtomicU64,
    checkpoint_sequence: AtomicU64,
    /// The file system to publish through after each checkpoint, when the
    /// store has been asked to keep readers up to date.
    publisher: Mutex<Option<Arc<ObjectFileSystem>>>,
    /// The checkpoint and the last commit of the last such publication.
    published_at: Mutex<Option<(u64, u64)>>,
    record_delta_base_lsn: AtomicU64,
    auto_checkpoint_policy: Mutex<AutoCheckpointPolicy>,
    commits_since_checkpoint: AtomicU64,
    last_checkpoint: Mutex<Option<CheckpointInfo>>,
    last_maintenance_error: Mutex<Option<String>>,
    poisoned: AtomicBool,
    /// Set for a copy that is there to be read, such as a reader's copy
    /// of a publication: every commit is refused.
    read_only: AtomicBool,
}

pub struct RustStore {
    core: Arc<StoreCore>,
    maintenance_sender: SyncSender<MaintenanceMessage>,
    maintenance_worker: Mutex<Option<JoinHandle<()>>>,
}

impl Deref for RustStore {
    type Target = StoreCore;

    fn deref(&self) -> &Self::Target {
        self.core.as_ref()
    }
}

impl RustStore {
    pub fn storage_layout_stats(&self, schema: &str) -> OQLResult<StorageLayoutStats> {
        let flush_generations = self.published_flush_generations()?;
        let flush_generation_wal_bytes = flush_generations
            .iter()
            .map(|generation| generation.len)
            .sum();
        let flush_generation_record_bytes = flush_generations
            .iter()
            .map(|generation| generation.record_len)
            .sum();
        let flush_generation_index_bytes = flush_generations
            .iter()
            .map(|generation| generation.index_len)
            .sum();
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let collection_id = state.collection_ids.get(schema).copied();
        let schema_version = state.current_schema_versions.get(schema).copied();
        let segments = state
            .typed_segments
            .iter()
            .filter(|segment| {
                Some(segment.collection_id()) == collection_id
                    && Some(segment.schema_version()) == schema_version
            })
            .collect::<Vec<_>>();
        let typed_rows = segments.iter().map(|segment| segment.row_count()).sum();
        let typed_loaded_segments = segments
            .iter()
            .map(|segment| segment.is_loaded())
            .collect::<OQLResult<Vec<_>>>()?
            .into_iter()
            .filter(|loaded| *loaded)
            .count();
        let typed_identity_bytes = segments
            .iter()
            .map(|segment| segment.identity_bytes())
            .sum();
        let typed_authoritative_segments = segments
            .iter()
            .filter(|segment| segment.authoritative())
            .count();
        let typed_authoritative_rows = segments
            .iter()
            .filter(|segment| segment.authoritative())
            .map(|segment| segment.row_count())
            .sum();
        let typed_stale_rows = state.typed_delta_rows.get(schema).map_or(0, BTreeMap::len);
        let typed_delta_rows = state
            .typed_delta_rows
            .get(schema)
            .map_or(0, |rows| rows.values().filter(|row| row.is_some()).count());
        let live_rows = logical_collection_count(&state, schema);
        let resident_typed_rows =
            state
                .object_directory
                .keys()
                .try_fold(0usize, |count, object_ref| {
                    state
                        .typed_row_locations
                        .contains_key(object_ref)
                        .map(|present| count + usize::from(present))
                })?;
        let (objref_locator_cached_pages, objref_locator_cached_bytes) =
            state.typed_row_locations.cached_page_stats()?;
        Ok(StorageLayoutStats {
            committed_lsn: state.committed_lsn,
            typed_base_lsn: state.typed_base_lsn,
            flush_generations: flush_generations.len(),
            flush_generation_wal_bytes,
            flush_generation_record_bytes,
            flush_generation_index_bytes,
            typed_segments: segments.len(),
            typed_loaded_segments,
            typed_identity_bytes,
            typed_rows,
            typed_authoritative_segments,
            typed_authoritative_rows,
            row_overflow_rows: live_rows.saturating_sub(typed_authoritative_rows),
            external_id_hash_rows: 0,
            ordered_id_tree_rows: state.collection_rows.values().map(BTreeMap::len).sum(),
            objref_locator_rows: state.typed_row_locations.len(),
            objref_locator_cached_pages,
            objref_locator_cached_bytes,
            resident_documents: state.object_directory.len(),
            lazy_typed_rows: state
                .typed_row_locations
                .len()
                .saturating_sub(resident_typed_rows),
            typed_delta_rows,
            typed_stale_rows,
            live_rows,
            typed_scan_available: !segments.is_empty()
                && collection_id.is_some_and(|id| state.typed_complete_collections.contains(&id)),
            typed_scan_current: state.committed_lsn == state.typed_base_lsn
                && !segments.is_empty()
                && typed_rows == live_rows,
        })
    }

    /// The identity under which this store's transactions are bound to a
    /// thread.
    /// Keep documents a reader decoded, so the next reader finds them
    /// ready. Reading shares the state and so cannot add to it; this takes
    /// it alone, briefly, and only when something was decoded. A row that
    /// was rewritten or deleted since it was read is not kept.
    fn keep_decoded(&self, decoded: Vec<Decoded>) -> OQLResult<()> {
        if decoded.is_empty() {
            return Ok(());
        }
        let mut state = self
            .state
            .write()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        for Decoded {
            object_ref,
            document,
            record,
            read_at,
        } in decoded
        {
            if state.object_directory.contains_key(&object_ref) {
                continue;
            }
            // Still the row that was read: a segment row keeps its place
            // until it is rewritten, and a record is replaced, not changed.
            let unchanged = match &record {
                None => state.typed_row_locations.get(&object_ref)?.is_some(),
                Some(read) => match state.object_records.get(&object_ref) {
                    Some(current) => Arc::ptr_eq(current, read),
                    // The record was read from a checkpoint file and is not
                    // held in memory. It is still the row's as long as the
                    // row is live and nothing has been committed since it
                    // was read. Without this a row whose record is on file
                    // is decoded again by every statement that wants it.
                    None => {
                        state.committed_lsn == read_at
                            && record_row_is_live(&state, object_ref)?
                    }
                },
            };
            if unchanged {
                state.object_directory.insert(object_ref, document);
                state.note_decoded(object_ref);
            }
        }
        enforce_decoded_budget(&mut state)
    }

    fn binding_key(&self) -> usize {
        Arc::as_ptr(&self.core) as usize
    }

    /// The transaction the current thread is working in, if any.
    fn current_transaction(&self) -> Option<u64> {
        let key = self.binding_key();
        BOUND_TRANSACTION
            .with(std::cell::Cell::get)
            .filter(|(store, _)| *store == key)
            .map(|(_, id)| id)
    }

    /// The reference of a document the open transaction has written and not
    /// yet committed, promised now so its later writes can point at it.
    ///
    /// The object id is taken from the collection's counter at once. If the
    /// transaction is abandoned the id is never used, which is harmless: the
    /// counter only ever needs to stay ahead of what is stored.
    fn reserve_pending_ref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>> {
        let Some(transaction) = self.current_transaction() else {
            return Ok(None);
        };
        let key = (schema.to_string(), id.clone());
        {
            let active = self
                .transactions
                .lock()
                .map_err(|_| OQLError::generic("transaction lock poisoned"))?;
            let Some(pending) = active.get(&transaction) else {
                return Ok(None);
            };
            if let Some(promised) = pending.reserved.get(&key) {
                return Ok(Some(*promised));
            }
            // The last thing the transaction did to the document decides.
            let written = pending.operations.iter().rev().find_map(|operation| match operation {
                WriteOperation::PutDocument(document)
                    if document.schema == schema && document.id == *id =>
                {
                    Some(true)
                }
                WriteOperation::DeleteDocument { schema: deleted, id: deleted_id }
                    if deleted == schema && deleted_id == id =>
                {
                    Some(false)
                }
                _ => None,
            });
            if written != Some(true) {
                return Ok(None);
            }
        }
        let promised = {
            let mut state = self
                .state
                .write()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            // Another commit may have stored the same id meanwhile; then
            // that is the document a reference means.
            if let Some(stored) = lookup_object_ref(&state, schema, id)? {
                return Ok(Some(stored));
            }
            let Some(collection_id) = state.collection_ids.get(schema).copied() else {
                return Ok(None);
            };
            let next = state.next_object_ids.entry(collection_id).or_insert(1);
            let object_id = *next;
            *next = next.checked_add(1).ok_or_else(|| {
                OQLError::storage_error("object identifier space exhausted", None)
            })?;
            ObjRef::from_parts(collection_id, object_id, 1)
        };
        if let Some(pending) = self
            .transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?
            .get_mut(&transaction)
        {
            pending.reserved.insert(key, promised);
        }
        Ok(Some(promised))
    }

    fn not_open(id: u64) -> OQLError {
        OQLError::TransactionError {
            message: format!("transaction {id} is not open"),
        }
    }

    /// What the current thread's transaction has written and not committed.
    /// Outside a transaction there is nothing: other transactions' writes
    /// are theirs alone.
    fn pending_operations(&self) -> OQLResult<Vec<WriteOperation>> {
        let Some(id) = self.current_transaction() else {
            return Ok(Vec::new());
        };
        self.transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?
            .get(&id)
            .map(|transaction| transaction.operations.clone())
            .ok_or_else(|| Self::not_open(id))
    }

    pub fn new<P: AsRef<Path>>(path: P, max_size_bytes: usize) -> OQLResult<Self> {
        Self::new_with_fs(
            path,
            max_size_bytes,
            Arc::new(crate::simulation::filesystem::RealFileSystem::new()),
        )
    }

    pub fn new_with_fs<P: AsRef<Path>>(
        path: P,
        max_size_bytes: usize,
        filesystem: Arc<dyn FileSystem>,
    ) -> OQLResult<Self> {
        Self::open(path, max_size_bytes, filesystem, None)
    }

    /// Open a database that holds at most `decoded_documents` documents
    /// decoded in memory at a time.
    ///
    /// Without a budget every row outside the column segments (one with a
    /// list, an object or a vector in it) stays decoded for as long as the
    /// database is open, which is fastest and costs several times the
    /// row's size on disk. With one, the least recently taken in are let
    /// go and decoded again when read, from their records, which are a
    /// fraction of the size. Answers are the same either way.
    pub fn with_decoded_budget<P: AsRef<Path>>(
        path: P,
        max_size_bytes: usize,
        decoded_documents: usize,
    ) -> OQLResult<Self> {
        Self::open(
            path,
            max_size_bytes,
            Arc::new(crate::simulation::filesystem::RealFileSystem::new()),
            Some(decoded_documents),
        )
    }

    /// Change the budget of an open database. `None` removes it.
    pub fn set_decoded_budget(&self, decoded_documents: Option<usize>) -> OQLResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        // What was kept while there was no budget may hold rows a budget
        // would let go.
        if let Ok(mut built) = self.built_columns.lock() {
            built.collections.clear();
        }
        state.decoded_budget = decoded_documents;
        state.index_pages();
        state.decoded_order = match decoded_documents {
            Some(_) => state.object_directory.keys().copied().collect(),
            None => VecDeque::new(),
        };
        enforce_decoded_budget(&mut state)
    }

    /// How many bytes of column segments, as encoded, this process holds
    /// decoded at most. Those decoded longest ago are let go past it and
    /// read from their files again when wanted. `None` for no limit. The
    /// limit is the process's, across every database it has open; setting
    /// a budget on decoded rows sets it to 64 MB.
    pub fn set_column_segment_budget(bytes: Option<usize>) {
        crate::storage_engine::checkpoint::segments_loaded().set_limit(bytes.unwrap_or(0));
    }

    /// Change the budget, in bytes, on the memory the rows held decoded
    /// may take. `None` removes it. A database opened without one takes a
    /// quarter of the machine's memory, or what `OWL_DECODED_BYTES` says
    /// (0 for none).
    pub fn set_decoded_bytes_budget(&self, bytes: Option<usize>) -> OQLResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        if let Ok(mut built) = self.built_columns.lock() {
            built.collections.clear();
        }
        state.decoded_bytes_budget = bytes;
        state.let_go_for_bytes = false;
        state.index_pages();
        if state.decoded_order.is_empty() && state.bounded() {
            state.decoded_order = state.object_directory.keys().copied().collect();
        }
        enforce_decoded_budget(&mut state)
    }

    /// Roughly how many bytes the rows held decoded take at this moment.
    pub fn decoded_bytes(&self) -> usize {
        self.state.read().map_or(0, |state| state.object_directory.bytes())
    }

    /// How many documents are held decoded in memory at this moment.
    pub fn decoded_documents(&self) -> usize {
        self.state
            .read()
            .map_or(0, |state| state.object_directory.len())
    }

    fn open<P: AsRef<Path>>(
        path: P,
        max_size_bytes: usize,
        filesystem: Arc<dyn FileSystem>,
        decoded_budget: Option<usize>,
    ) -> OQLResult<Self> {
        // A database opened without a budget takes one from the
        // environment when it is set there: the way to bound the memory of
        // a program that does not choose one itself.
        let decoded_budget = decoded_budget.or_else(|| {
            std::env::var("OWL_DECODED_DOCUMENTS")
                .ok()
                .and_then(|value| value.trim().parse().ok())
        });
        // A checkpoint kept as a root over part files reads as one file
        // from here on.
        let filesystem = crate::storage_engine::parts::PartedFileSystem::over(filesystem);
        let wal_path = path.as_ref().to_path_buf();
        let sealed_wal_path = sidecar_path(path.as_ref(), "wal.sealed");
        let manifest_paths = [
            sidecar_path(path.as_ref(), "manifest.0"),
            sidecar_path(path.as_ref(), "manifest.1"),
        ];
        let segment_paths = [
            sidecar_path(path.as_ref(), "segment.0"),
            sidecar_path(path.as_ref(), "segment.1"),
        ];
        let generation_root_paths = [
            sidecar_path(path.as_ref(), "generations.0"),
            sidecar_path(path.as_ref(), "generations.1"),
        ];
        let generation_prefix = sidecar_path(path.as_ref(), "generation");
        for segment_path in &segment_paths {
            cleanup_typed_locator_spools(filesystem.as_ref(), segment_path)?;
        }
        let record_delta_paths = [
            sidecar_path(path.as_ref(), "record-delta.0"),
            sidecar_path(path.as_ref(), "record-delta.1"),
        ];
        let wal_delta_paths = [
            sidecar_path(path.as_ref(), "wal-delta.0"),
            sidecar_path(path.as_ref(), "wal-delta.1"),
        ];
        let index_delta_paths = [
            sidecar_path(path.as_ref(), "index-delta.0"),
            sidecar_path(path.as_ref(), "index-delta.1"),
        ];
        let compaction_manifest_path = sidecar_path(path.as_ref(), "compaction-runs");
        let compaction_run_prefix = sidecar_path(path.as_ref(), "compaction");
        cleanup_compaction_runs(
            filesystem.as_ref(),
            &compaction_manifest_path,
            &compaction_run_prefix,
        )?;
        cleanup_unpublished_flush_generation(
            filesystem.as_ref(),
            &manifest_paths,
            &generation_root_paths,
            &generation_prefix,
        )?;
        // A part a crash left behind, written for a checkpoint whose root
        // never was, is listed by nothing.
        crate::storage_engine::parts::remove_unlisted_parts(filesystem.as_ref(), &segment_paths)
            .map_err(|error| storage_io("remove unlisted checkpoint parts", &wal_path, error))?;
        let mut wal = if filesystem.exists(&wal_path) {
            filesystem
                .open_with_options(
                    &wal_path,
                    &FileOptions {
                        read: true,
                        write: true,
                        create: false,
                        truncate: false,
                        append: false,
                    },
                )
                .map_err(|e| storage_io("open WAL", &wal_path, e))?
        } else {
            let mut file = filesystem
                .create(&wal_path)
                .map_err(|e| storage_io("create WAL", &wal_path, e))?;
            file.sync()
                .map_err(|e| storage_io("sync new WAL", &wal_path, e))?;
            file
        };

        let (checkpoint_state, checkpoint_sequence) = Self::recover_checkpoint(
            filesystem.clone(),
            &manifest_paths,
            &segment_paths,
            &generation_root_paths,
            &generation_prefix,
            &wal_delta_paths,
            &record_delta_paths,
            &index_delta_paths,
            SegmentVerification::Open,
            decoded_budget,
        )?;
        let record_delta_base_lsn = checkpoint_state.committed_lsn;
        let sealed_wal_existed = filesystem.exists(&sealed_wal_path);
        let (checkpoint_state, valid_sealed_wal_len) =
            // The log was synced whole before it was sealed.
            Self::recover(
                filesystem.as_ref(),
                &sealed_wal_path,
                checkpoint_state,
                WalTail::Synced,
            )?;
        if sealed_wal_existed {
            truncate_recovered_wal(filesystem.as_ref(), &sealed_wal_path, valid_sealed_wal_len)?;
        }
        let (state, valid_wal_len) = Self::recover(
            filesystem.as_ref(),
            &wal_path,
            checkpoint_state,
            WalTail::MayBeUnsynced,
        )?;
        let wal_len = wal
            .size()
            .map_err(|e| storage_io("inspect WAL after recovery", &wal_path, e))?;
        if wal_len != valid_wal_len {
            wal.set_len(valid_wal_len)
                .map_err(|e| storage_io("truncate torn WAL tail", &wal_path, e))?;
        }
        // What was read back may have come from memory that a dead process
        // left behind and no sync ever covered. The frames written from
        // here on say the log is on disk up to this point, so make it so.
        wal.sync()
            .map_err(|e| storage_io("sync recovered WAL", &wal_path, e))?;
        wal.seek(SeekFrom::Start(valid_wal_len))
            .map_err(|e| storage_io("seek WAL", &wal_path, e))?;

        let recovered_commits = state.committed_lsn.saturating_sub(record_delta_base_lsn);
        let recovered_lsn = state.committed_lsn;
        // The state reads evicted records back through the same handles the
        // store keeps current.
        // Recovery may have left it a handle on the base segment already;
        // the store keeps that one, and brings it up to date below.
        let mut state = state;
        let checkpoint_read_handles = state
            .checkpoint_records
            .take()
            .unwrap_or_else(|| Arc::new(Mutex::new(CheckpointReadHandles::default())));
        state.checkpoint_records = Some(checkpoint_read_handles.clone());
        let core = Arc::new(StoreCore {
            filesystem,
            wal_path,
            sealed_wal_path,
            manifest_paths,
            segment_paths,
            generation_root_paths,
            generation_prefix,
            record_delta_paths,
            checkpoint_read_handles,
            index_delta_paths,
            wal_delta_paths,
            max_size_bytes,
            wal: Mutex::new(Some(wal)),
            state: RwLock::new(state),
            commit_lock: Mutex::new(()),
            checkpoint_lock: Mutex::new(()),
            pending_checkpoint: Mutex::new(None),
            pending_incremental_checkpoint: Mutex::new(None),
            built_columns: Mutex::new(BuiltColumns::default()),
            transactions: Mutex::new(HashMap::new()),
            // Everything recovered was read back from disk.
            durability: Mutex::new(Durability {
                synced_lsn: recovered_lsn,
                syncing: false,
                failed: None,
            }),
            durable: Condvar::new(),
            written_lsn: AtomicU64::new(recovered_lsn),
            wal_synced_len: AtomicU64::new(valid_wal_len),
            commits_in_flight: AtomicUsize::new(0),
            next_transaction_id: AtomicU64::new(1),
            checkpoint_sequence: AtomicU64::new(checkpoint_sequence),
            publisher: Mutex::new(None),
            published_at: Mutex::new(None),
            record_delta_base_lsn: AtomicU64::new(record_delta_base_lsn),
            auto_checkpoint_policy: Mutex::new(AutoCheckpointPolicy::for_size_limit(
                max_size_bytes,
            )),
            commits_since_checkpoint: AtomicU64::new(recovered_commits),
            last_checkpoint: Mutex::new(None),
            last_maintenance_error: Mutex::new(None),
            poisoned: AtomicBool::new(false),
            read_only: AtomicBool::new(false),
        });
        if sealed_wal_existed {
            core.refresh_record_root_handle()?;
            let checkpoint = core.publish_recovered_state()?;
            core.record_checkpoint_success(&checkpoint);
            core.record_delta_base_lsn
                .store(checkpoint.checkpoint_lsn, Ordering::Release);
            let mut state = core
                .state
                .write()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            state.object_records.mark_clean();
            mark_indexes_clean(&mut state)?;
        }
        core.refresh_record_root_handle()?;
        core.evict_clean_record_bytes()?;
        let (maintenance_sender, maintenance_receiver) = mpsc::sync_channel(1);
        let worker_core = core.clone();
        // A browser has no threads to give. There the store runs with no
        // maintenance worker: nothing checkpoints of its own accord, and a
        // checkpoint is made when it is asked for. The receiving end is
        // kept so that asking for maintenance finds somewhere to ask.
        #[cfg(target_arch = "wasm32")]
        let maintenance_worker = {
            let _ = worker_core;
            std::mem::forget(maintenance_receiver);
            None
        };
        #[cfg(not(target_arch = "wasm32"))]
        let maintenance_worker = Some(
            thread::Builder::new()
                .name("owl-checkpoint-maintenance".to_string())
                .spawn(move || maintenance_worker(worker_core, maintenance_receiver))
                .map_err(|error| {
                    OQLError::generic(&format!(
                        "cannot start checkpoint maintenance worker: {error}"
                    ))
                })?,
        );
        let store = Self {
            core,
            maintenance_sender,
            maintenance_worker: Mutex::new(maintenance_worker),
        };
        // Recovery must preserve checkpoint debt. Otherwise a large WAL that
        // survived a restart can remain below the commit threshold forever
        // and be replayed on every subsequent open.
        if store.auto_checkpoint_due()? {
            store.schedule_maintenance();
        }
        Ok(store)
    }

    /// Validate the authoritative checkpoint and WAL chain without opening the
    /// database for writes.
    pub fn check_integrity<P: AsRef<Path>>(path: P) -> OQLResult<IntegrityReport> {
        Self::check_integrity_with_fs(
            path,
            Arc::new(crate::simulation::filesystem::RealFileSystem::new()),
        )
    }

    #[doc(hidden)]
    pub fn check_integrity_with_fs<P: AsRef<Path>>(
        path: P,
        filesystem: Arc<dyn FileSystem>,
    ) -> OQLResult<IntegrityReport> {
        inspect_integrity(path.as_ref(), filesystem).map(|(report, _)| report)
    }

    /// Validate durable files and require them to reproduce the current live state.
    pub fn verify_integrity(&self) -> OQLResult<IntegrityReport> {
        let _checkpoint = self
            .checkpoint_lock
            .lock()
            .map_err(|_| OQLError::generic("checkpoint lock poisoned"))?;
        let _commit = self
            .commit_lock
            .lock()
            .map_err(|_| OQLError::generic("storage commit lock poisoned"))?;
        let inspected = inspect_integrity(&self.wal_path, self.filesystem.clone());
        // Some deterministic filesystems model position on the file rather
        // than on each handle. Restore the append position before releasing
        // the commit boundary even when validation fails.
        {
            let mut wal = self
                .wal
                .lock()
                .map_err(|_| OQLError::generic("WAL lock poisoned"))?;
            let wal = wal
                .as_mut()
                .ok_or_else(|| OQLError::storage_error("active WAL handle is unavailable", None))?;
            let end = wal.size().map_err(|error| {
                storage_io("inspect WAL after integrity check", &self.wal_path, error)
            })?;
            wal.seek(SeekFrom::Start(end)).map_err(|error| {
                storage_io("restore WAL after integrity check", &self.wal_path, error)
            })?;
        }
        let (report, recovered) = inspected?;
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        validate_state_integrity(&state)?;
        if state.committed_lsn != recovered.committed_lsn
            || !same_identities(&state, &recovered)?
            || logical_document_map(&state)? != logical_document_map(&recovered)?
            || state.scalar_indexes != recovered.scalar_indexes
        {
            return Err(OQLError::storage_error(
                "durable storage does not reproduce the current live state",
                None,
            ));
        }
        Ok(report)
    }

    /// What the store holds in memory, part by part: a name, how many
    /// entries, and roughly how many bytes. The bytes are estimates from
    /// the sizes of what is held, not a count of allocations.
    pub fn memory_breakdown(&self) -> OQLResult<Vec<(String, usize, usize)>> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let mut parts = Self::breakdown_of(&state);
        drop(state);
        if let Ok(built) = self.built_columns.lock() {
            let bytes: usize = built.collections.values().map(|kept| kept.bytes).sum();
            parts.push(("columns and row lists kept between queries".to_string(), built.collections.len(), bytes));
        }
        Ok(parts)
    }

    /// What one state holds in memory, part by part.
    fn breakdown_of(state: &State) -> Vec<(String, usize, usize)> {
        let mut parts = Vec::new();
        parts.push(("rows held decoded".to_string(), state.object_directory.len(), state.object_directory.bytes()));
        let (segments, segment_bytes) = crate::storage_engine::checkpoint::segments_loaded().held();
        parts.push(("column segments held decoded (as encoded)".to_string(), segments, segment_bytes));
        let (records, record_bytes) = state.object_records.held_in_memory();
        parts.push(("records held as bytes".to_string(), records, record_bytes));
        let ids: usize = state.collection_rows.values().map(BTreeMap::len).sum();
        let id_bytes: usize = state
            .collection_rows
            .values()
            .flat_map(BTreeMap::keys)
            .map(|id| id.len() + 16 + 24 + 8 + 16)
            .sum();
        parts.push(("ids of every row, by collection".to_string(), ids, id_bytes));
        parts.push((
            "schema version of rows written since the checkpoint".to_string(),
            state.object_schema_versions.len(),
            state.object_schema_versions.capacity() * 17,
        ));
        parts.push((
            "the map from a reference to its decoded row".to_string(),
            state.object_directory.len(),
            state.object_directory.capacity() * 17,
        ));
        let located: usize = state.typed_row_locations.collections.values().map(Vec::len).sum();
        parts.push((
            "where each row is in the column segments".to_string(),
            state.typed_row_locations.len,
            located * 8 + state.typed_row_locations.overrides.capacity() * 17,
        ));
        let (reverse, reverse_bytes) = state.reverse_reference_index.held_in_memory();
        parts.push(("relationship index, held over its file".to_string(), reverse, reverse_bytes));
        let mut indexed = 0;
        let mut indexed_bytes = 0;
        for index in state.scalar_index_data.values() {
            let (entries, bytes) = index.held_in_memory();
            indexed += entries;
            indexed_bytes += bytes;
        }
        parts.push(("declared indexes, held over their files".to_string(), indexed, indexed_bytes));
        let fenced: usize = state.typed_id_fences.values().map(Vec::len).sum();
        parts.push(("id fences of the column segments".to_string(), fenced, fenced * 64));
        let delta: usize = state.typed_delta_rows.values().map(BTreeMap::len).sum();
        parts.push(("rows changed since the column segments".to_string(), delta, delta * 72));
        parts.push(("order rows were decoded in".to_string(), state.decoded_order.len(), state.decoded_order.capacity() * 8));
        parts.push(("recent commits kept for open transactions".to_string(), state.conflicts.recent.len(), 0));
        parts
    }

    /// Remove files unreachable from the newest durable manifest.
    pub fn reclaim_obsolete_files(&self) -> OQLResult<ReclamationReport> {
        let _checkpoint = self
            .checkpoint_lock
            .lock()
            .map_err(|_| OQLError::generic("checkpoint lock poisoned"))?;
        self.reclaim_after_audit()
    }

    /// [`Self::reclaim_obsolete_files`], for a caller that holds the
    /// checkpoint lock.
    fn reclaim_after_audit(&self) -> OQLResult<ReclamationReport> {
        let _commit = self
            .commit_lock
            .lock()
            .map_err(|_| OQLError::generic("storage commit lock poisoned"))?;
        // Never discard the older generation unless the retained checkpoint
        // and all of its referenced roots can first be reconstructed.
        RustStore::recover_checkpoint(
            self.filesystem.clone(),
            &self.manifest_paths,
            &self.segment_paths,
            &self.generation_root_paths,
            &self.generation_prefix,
            &self.wal_delta_paths,
            &self.record_delta_paths,
            &self.index_delta_paths,
            SegmentVerification::Audit,
            None,
        )?;
        self.core.reclaim_obsolete_files_locked()
    }

    pub fn write_batch(&self, operations: Vec<WriteOperation>) -> OQLResult<u64> {
        self.write_batch_for(operations, None, &ReservedRefs::new())
    }

    /// Commit a batch, on behalf of a transaction when one is given. A
    /// transaction's batch is first checked against what was committed
    /// since the transaction began, under the same locks that make the
    /// commit, so nothing can slip in between the check and the write.
    fn write_batch_for(
        &self,
        operations: Vec<WriteOperation>,
        transaction: Option<u64>,
        reserved: &ReservedRefs,
    ) -> OQLResult<u64> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(OQLError::storage_error(
                "storage writer is poisoned after a failed WAL append; reopen to recover",
                None,
            ));
        }
        if operations.is_empty() {
            return Ok(self.committed_lsn());
        }
        // Tell whoever syncs next that another frame is on its way, so it
        // can wait a moment and cover this one too.
        self.commits_in_flight.fetch_add(1, Ordering::AcqRel);
        let written = self.write_and_apply(operations, transaction, reserved);
        self.commits_in_flight.fetch_sub(1, Ordering::AcqRel);
        let lsn = written?;
        // The commit is applied and visible; it is answered only once it
        // is on disk.
        self.sync_through(lsn)?;

        self.commits_since_checkpoint.fetch_add(1, Ordering::AcqRel);
        // The commit is already durable and visible. Maintenance failure is
        // therefore reported through status and must not turn this write into
        // a false negative that callers might retry.
        self.schedule_maintenance();
        // With a memory budget, what has been written since the last
        // checkpoint is held in memory until the next one. A writer that
        // outruns the checkpoints waits here for them to catch up, so that
        // what is held stays bounded.
        if self.checkpoints_are_behind().unwrap_or(false) {
            let _ = self.wait_for_maintenance();
        }
        Ok(lsn)
    }

    /// Whether a memory budget is set and the log has grown to several
    /// times the size at which a checkpoint is due.
    fn checkpoints_are_behind(&self) -> OQLResult<bool> {
        let (bounded, budget) = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            // A store told how many rows it may hold was told to be
            // frugal: it keeps to the policy's size.
            let bytes = match state.decoded_budget {
                Some(_) => None,
                None => state.decoded_bytes_budget,
            };
            (state.tight(), bytes)
        };
        if !bounded {
            return Ok(false);
        }
        let policy = *self
            .auto_checkpoint_policy
            .lock()
            .map_err(|_| OQLError::generic("checkpoint policy lock poisoned"))?;
        if policy.wal_bytes == 0 {
            return Ok(false);
        }
        let wal_bytes = self
            .filesystem
            .file_size(&self.wal_path)
            .map_err(|error| storage_io("inspect WAL", &self.wal_path, error))?;
        // A checkpoint is due at the policy's size, and one that is a
        // compaction can take a while. The writer goes on meanwhile, up
        // to a sixteenth of the memory budget in log: what that log holds
        // is in memory until the checkpoint is done, and comes to a few
        // times its size there. A small budget leaves the policy's size
        // as the bound, as it was.
        let allowed = policy
            .wal_bytes
            .max(budget.map_or(0, |budget| (budget / 16) as u64));
        Ok(wal_bytes >= allowed)
    }

    /// Check, plan, log and apply one batch, without waiting for the log
    /// to reach disk.
    ///
    /// The WAL is taken before the state, and a sync in progress holds the
    /// WAL: so a commit that has to wait for one waits holding nothing a
    /// reader needs.
    fn write_and_apply(
        &self,
        operations: Vec<WriteOperation>,
        transaction: Option<u64>,
        reserved: &ReservedRefs,
    ) -> OQLResult<u64> {
        if self.read_only.load(Ordering::Acquire) {
            return Err(OQLError::storage_error(
                "this database is a read-only copy and takes no writes",
                None,
            ));
        }
        let _commit = self
            .commit_lock
            .lock()
            .map_err(|_| OQLError::generic("storage commit lock poisoned"))?;
        let mut wal = self
            .wal
            .lock()
            .map_err(|_| OQLError::generic("WAL lock poisoned"))?;
        let mut state = self
            .state
            .write()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let written = WrittenKeys::of(&operations);
        if let Some(id) = transaction {
            state.conflicts.check(id, &written)?;
        }
        let lsn = state.committed_lsn.checked_add(1).ok_or_else(|| {
            OQLError::storage_error("commit sequence number space exhausted", None)
        })?;
        let mutations = Self::plan_mutations(&state, operations, reserved)?;
        let batch = WalBatch { lsn, mutations };
        Self::validate_batch(&state, &batch)?;
        let payload = bincode::serialize(&batch).map_err(|e| {
            OQLError::serialization_error(&format!("cannot encode storage batch: {e}"), None)
        })?;
        if payload.len() > MAX_WAL_FRAME {
            return Err(OQLError::storage_error(
                "storage batch exceeds WAL frame limit",
                None,
            ));
        }
        self.append_frame(&mut wal, lsn, &payload)?;
        self.written_lsn.store(lsn, Ordering::Release);
        Self::apply_batch(&mut state, batch)
            .expect("a validated logged batch must remain applicable");
        if let Some(id) = transaction {
            state.conflicts.open.remove(&id);
        }
        state.conflicts.committed(lsn, written);
        Ok(lsn)
    }

    /// Return once frame `lsn` is on disk.
    ///
    /// The first committer to arrive syncs; the others wait for it, and are
    /// done if its sync covered their frame. Before syncing it gives
    /// commits already under way a moment to write their frames, which is
    /// what lets many commits share one sync. A commit with no company
    /// syncs at once.
    fn sync_through(&self, lsn: u64) -> OQLResult<()> {
        /// The longest a sync is put off for commits already under way.
        const GATHER: Duration = Duration::from_micros(1_500);

        let lost = || OQLError::generic("durability lock poisoned");
        let mut durability = self.durability.lock().map_err(|_| lost())?;
        loop {
            if let Some(reason) = &durability.failed {
                return Err(OQLError::storage_error(
                    &format!("the commit could not be made durable: {reason}; reopen to recover"),
                    None,
                ));
            }
            if durability.synced_lsn >= lsn {
                return Ok(());
            }
            if durability.syncing {
                durability = self.durable.wait(durability).map_err(|_| lost())?;
                continue;
            }
            durability.syncing = true;
            drop(durability);

            let gathering = Instant::now();
            while self.commits_in_flight.load(Ordering::Acquire) > 0
                && gathering.elapsed() < GATHER
            {
                thread::yield_now();
            }
            let synced = self
                .wal
                .lock()
                .map_err(|_| "WAL lock poisoned".to_string())
                .and_then(|mut wal| {
                    // Every frame written so far is in the file by now, and
                    // none can be added while this holds the WAL.
                    let through = self.written_lsn.load(Ordering::Acquire);
                    let wal = wal
                        .as_mut()
                        .ok_or_else(|| "active WAL handle is unavailable".to_string())?;
                    wal.sync().map_err(|error| error.to_string())?;
                    if let Ok(len) = wal.size() {
                        self.wal_synced_len.fetch_max(len, Ordering::AcqRel);
                    }
                    Ok(through)
                });

            durability = self.durability.lock().map_err(|_| lost())?;
            durability.syncing = false;
            match synced {
                Ok(through) => durability.synced_lsn = durability.synced_lsn.max(through),
                Err(reason) => {
                    self.poisoned.store(true, Ordering::Release);
                    durability.failed = Some(reason);
                }
            }
            self.durable.notify_all();
        }
    }

    pub fn committed_lsn(&self) -> u64 {
        self.state.read().map(|s| s.committed_lsn).unwrap_or(0)
    }

    /// Replace the automatic checkpoint thresholds for subsequent commits.
    pub fn configure_auto_checkpoint(&self, policy: AutoCheckpointPolicy) -> OQLResult<()> {
        *self
            .auto_checkpoint_policy
            .lock()
            .map_err(|_| OQLError::generic("checkpoint policy lock poisoned"))? = policy;
        Ok(())
    }

    pub fn maintenance_status(&self) -> OQLResult<MaintenanceStatus> {
        let mut wal_bytes = self
            .filesystem
            .file_size(&self.wal_path)
            .map_err(|error| storage_io("inspect WAL", &self.wal_path, error))?;
        if self.filesystem.exists(&self.sealed_wal_path) {
            wal_bytes = wal_bytes.saturating_add(
                self.filesystem
                    .file_size(&self.sealed_wal_path)
                    .map_err(|error| {
                        storage_io("inspect sealed WAL", &self.sealed_wal_path, error)
                    })?,
            );
        }
        Ok(MaintenanceStatus {
            policy: *self
                .auto_checkpoint_policy
                .lock()
                .map_err(|_| OQLError::generic("checkpoint policy lock poisoned"))?,
            wal_bytes,
            commits_since_checkpoint: self.commits_since_checkpoint.load(Ordering::Acquire),
            checkpoint_sequence: self.checkpoint_sequence.load(Ordering::Acquire),
            last_checkpoint: self
                .last_checkpoint
                .lock()
                .map_err(|_| OQLError::generic("checkpoint status lock poisoned"))?
                .clone(),
            last_error: self
                .last_maintenance_error
                .lock()
                .map_err(|_| OQLError::generic("checkpoint status lock poisoned"))?
                .clone(),
        })
    }

    /// Wait until all maintenance requested by earlier commits has run.
    ///
    /// Normal writes never wait for this barrier; it is intended for orderly
    /// shutdown, administration, and deterministic tests.
    pub fn wait_for_maintenance(&self) -> OQLResult<()> {
        let (reply, response) = mpsc::channel();
        self.maintenance_sender
            .send(MaintenanceMessage::Drain(reply))
            .map_err(|_| OQLError::generic("checkpoint maintenance worker is unavailable"))?;
        response
            .recv()
            .map_err(|_| OQLError::generic("checkpoint maintenance worker stopped unexpectedly"))
    }

    fn schedule_maintenance(&self) {
        match self
            .maintenance_sender
            .try_send(MaintenanceMessage::Checkpoint)
        {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => self
                .core
                .record_maintenance_error("checkpoint maintenance worker is unavailable".into()),
        }
    }

    /// Decode only selected top-level fields from the compiled record.
    ///
    /// The general `DocumentStore` contract still returns full documents. The
    /// physical query pipeline can use this method while projection is moved
    /// into the engine-neutral storage API.
    pub fn get_document_projection(
        &self,
        schema: &str,
        id: &DocumentId,
        fields: &[String],
    ) -> OQLResult<Option<Document>> {
        let object_ref = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let Some(object_ref) = lookup_object_ref(&state, schema, id)? else {
                return Ok(None);
            };
            object_ref
        };
        let record = self.read_compiled_record(object_ref)?;
        let header = RecordCodec::header(&record)?;
        if header.object_ref != object_ref {
            return Err(OQLError::storage_error(
                "checkpoint record identity conflicts with its directory",
                None,
            ));
        }
        let compiled = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .compiled_schemas
            .get(&(object_ref.collection_id(), header.schema_version))
            .cloned()
            .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
        let projection = fields.iter().cloned().collect::<HashSet<_>>();
        decode_record_projection(&record, &compiled, &projection).map(Some)
    }

    /// Resolve an exact composite key through the native scalar index.
    pub fn lookup_scalar_index(
        &self,
        schema: &str,
        index_name: &str,
        values: &[Value],
    ) -> OQLResult<Vec<ObjRef>> {
        let key = TypedKey::encode(values)?;
        let index = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .scalar_index_data
            .get(&(schema.to_string(), index_name.to_string()))
            .cloned()
            .ok_or_else(|| OQLError::generic("scalar index not found"))?;
        index
            .search_key(&key)
            .map_err(|error| OQLError::storage_error(&error, None))
    }

    /// Resolve an inclusive range of complete composite keys.
    pub fn range_scalar_index(
        &self,
        schema: &str,
        index_name: &str,
        start: &[Value],
        end: &[Value],
    ) -> OQLResult<Vec<ObjRef>> {
        let start = TypedKey::encode(start)?;
        let end = TypedKey::encode(end)?;
        let index = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .scalar_index_data
            .get(&(schema.to_string(), index_name.to_string()))
            .cloned()
            .ok_or_else(|| OQLError::generic("scalar index not found"))?;
        index
            .range_keys(&start, &end)
            .map_err(|error| OQLError::storage_error(&error, None))
    }

    /// Persist a complete, checksummed checkpoint and bound WAL recovery.
    ///
    /// Segment and manifest copies alternate. The active WAL is first rotated
    /// to a sealed generation, which is reclaimed only after the new manifest
    /// has been published durably.
    pub fn checkpoint(&self) -> OQLResult<CheckpointInfo> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(OQLError::storage_error(
                "storage writer is poisoned; reopen before checkpointing",
                None,
            ));
        }
        let info = {
            let _checkpoint = self
                .checkpoint_lock
                .lock()
                .map_err(|_| OQLError::generic("checkpoint lock poisoned"))?;
            self.core.checkpoint_concurrent()?
        };
        self.core.publish_if_asked();
        Ok(info)
    }

    /// Refuse every commit from now on, or stop refusing them.
    ///
    /// For a copy that is there to be read: a reader's copy of a
    /// publication is its own, and a write to it would be kept only until
    /// the publication is fetched again.
    pub fn set_read_only(&self, read_only: bool) {
        self.read_only.store(read_only, Ordering::Release);
    }

    /// Publish for readers after every checkpoint and compaction from now
    /// on, through `filesystem`, which must be the one this store was
    /// opened with. Readers then see the database as of its last
    /// checkpoint without anyone calling [`StoreCore::publish`]. Being
    /// asked publishes nothing; the next checkpoint does, whether or not
    /// it finds anything new.
    ///
    /// A publication that fails does not fail the checkpoint it follows.
    /// It is kept as the last maintenance error, and the next checkpoint
    /// tries again.
    pub fn publish_after_checkpoints(&self, filesystem: Arc<ObjectFileSystem>) -> OQLResult<()> {
        *self
            .publisher
            .lock()
            .map_err(|_| OQLError::generic("publisher lock poisoned"))? = Some(filesystem);
        Ok(())
    }

    /// Force a bounded full compaction into a new immutable base generation.
    ///
    /// Ordinary [`Self::checkpoint`] calls publish changes incrementally until
    /// the configured internal byte threshold requires this work.
    pub fn compact(&self) -> OQLResult<CheckpointInfo> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(OQLError::storage_error(
                "storage writer is poisoned; reopen before compacting",
                None,
            ));
        }
        let info = {
            let _checkpoint = self
                .checkpoint_lock
                .lock()
                .map_err(|_| OQLError::generic("checkpoint lock poisoned"))?;
            self.core.compact_concurrent()?
        };
        self.core.publish_if_asked();
        Ok(info)
    }

    /// [`Self::compact`], then remove the copy it replaced.
    ///
    /// A compaction rewrites the whole database into the other of two
    /// slots. The slot it came from then holds a full, older copy that
    /// nothing reads, kept as something to fall back to should the new one
    /// be damaged; the database takes about twice its size on disk for it.
    /// This removes that copy, after everything the new checkpoint refers
    /// to has been read back and checked, so what is discarded is never the
    /// only good copy at that moment. What is given up is the fallback
    /// afterwards: use it where backups or replicas are the protection.
    pub fn compact_and_reclaim(&self) -> OQLResult<(CheckpointInfo, ReclamationReport)> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(OQLError::storage_error(
                "storage writer is poisoned; reopen before compacting",
                None,
            ));
        }
        let (info, reclaimed) = {
            let _checkpoint = self
                .checkpoint_lock
                .lock()
                .map_err(|_| OQLError::generic("checkpoint lock poisoned"))?;
            let info = self.core.compact_concurrent()?;
            (info, self.reclaim_after_audit()?)
        };
        self.core.publish_if_asked();
        Ok((info, reclaimed))
    }
}

impl StoreCore {
    /// Run `work` while the database's files stand still: no commit, no
    /// checkpoint and no reclamation can change them, and everything
    /// committed is in them. Readers carry on. `work` is given the database
    /// path and the last committed sequence number.
    pub(super) fn with_files_frozen<T>(
        &self,
        work: impl FnOnce(&Path, u64) -> OQLResult<T>,
    ) -> OQLResult<T> {
        let _checkpoint = self
            .checkpoint_lock
            .lock()
            .map_err(|_| OQLError::generic("checkpoint lock poisoned"))?;
        let _commit = self
            .commit_lock
            .lock()
            .map_err(|_| OQLError::generic("storage commit lock poisoned"))?;
        {
            let mut wal = self
                .wal
                .lock()
                .map_err(|_| OQLError::generic("WAL lock poisoned"))?;
            wal.as_mut()
                .ok_or_else(|| OQLError::storage_error("active WAL handle is unavailable", None))?
                .sync()
                .map_err(|error| storage_io("sync WAL", &self.wal_path, error))?;
        }
        let committed = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .committed_lsn;
        work(&self.wal_path, committed)
    }

    /// Publish for readers if this store was asked to after checkpoints,
    /// unless what it last published is the database as it stands: the
    /// same checkpoint and the same last commit. A checkpoint that found
    /// nothing new still publishes if this store has not published yet.
    /// A failure is kept as the last maintenance error: the checkpoint it
    /// follows has been made, and failing that would say otherwise.
    ///
    /// Not to be called with the checkpoint lock held: publishing takes it.
    fn publish_if_asked(&self) {
        let Some(filesystem) = self.publisher.lock().ok().and_then(|publisher| publisher.clone()) else {
            return;
        };
        let committed = match self.state.read() {
            Ok(state) => state.committed_lsn,
            Err(_) => return,
        };
        let stands = (self.checkpoint_sequence.load(Ordering::Acquire), committed);
        if self.published_at.lock().is_ok_and(|published| *published == Some(stands)) {
            return;
        }
        match self.publish(&filesystem) {
            Ok(published) => {
                if let Ok(mut at) = self.published_at.lock() {
                    *at = Some((stands.0, published.committed_lsn));
                }
            }
            Err(error) => self.record_maintenance_error(error.to_string()),
        }
    }

    fn prepare_checkpoint(&self) -> OQLResult<PendingCheckpoint> {
        let (sequence, checkpoint_lsn, commits_at_snapshot) = {
            let _commit = self
                .commit_lock
                .lock()
                .map_err(|_| OQLError::generic("storage commit lock poisoned"))?;
            let sequence = self
                .checkpoint_sequence
                .load(Ordering::Acquire)
                .checked_add(1)
                .ok_or_else(|| OQLError::storage_error("checkpoint sequence exhausted", None))?;
            // Rotate before draining the dirty sets: a rotation that fails
            // cleanly must leave them intact, or a later incremental
            // checkpoint would omit those records. The commit lock freezes
            // the committed LSN across both steps.
            self.rotate_wal_locked()?;
            let drained = (|| {
                let mut state = self
                    .state
                    .write()
                    .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
                let checkpoint_lsn = state.committed_lsn;
                state.written_since_snapshot = Some((checkpoint_lsn, Vec::new()));
                let commits_at_snapshot = self.commits_since_checkpoint.load(Ordering::Acquire);
                state.object_records.take_delta(
                    self.record_delta_base_lsn.load(Ordering::Acquire),
                    checkpoint_lsn,
                );
                take_index_delta(
                    &mut state,
                    self.record_delta_base_lsn.load(Ordering::Acquire),
                    checkpoint_lsn,
                )?;
                state
                    .reverse_reference_index
                    .take_delta()
                    .map_err(|error| OQLError::storage_error(&error, None))?;
                Ok::<_, OQLError>((checkpoint_lsn, commits_at_snapshot))
            })();
            let (checkpoint_lsn, commits_at_snapshot) = match drained {
                Ok(drained) => drained,
                Err(error) => {
                    // Same hazard as a failed reconstruction below: the sealed
                    // WAL is unpublished and no pending snapshot covers it.
                    self.poisoned.store(true, Ordering::Release);
                    return Err(error);
                }
            };
            (sequence, checkpoint_lsn, commits_at_snapshot)
        };

        // The previous published checkpoint plus the sealed WAL are immutable
        // durable inputs. The compaction overlay retains encoded records and
        // tombstones only; documents are decoded later in bounded row groups.
        let live_is_tight = self.state.read().map(|state| state.tight()).unwrap_or(false);
        let reconstructed = (|| {
            let recovery_started = Instant::now();
            let (mut base, _) = RustStore::recover_checkpoint_auditing(
                self.filesystem.clone(),
                &self.manifest_paths,
                &self.segment_paths,
                &self.generation_root_paths,
                &self.generation_prefix,
                &self.wal_delta_paths,
                &self.record_delta_paths,
                &self.index_delta_paths,
                SegmentVerification::Open,
                // The base holds no rows decoded: each is read once, as
                // it is compacted.
                Some(0),
                false,
            )?;
            base.decoded_budget = None;
            // The base is the published checkpoint, which the store's own
            // read handles are open on. With them it reads a record from
            // its file when the record is wanted, as the live state does,
            // and need not hold every record's bytes for the length of the
            // compaction.
            base.checkpoint_records = Some(self.checkpoint_read_handles.clone());
            // If the database is short of memory, so is its compaction.
            base.let_go_for_bytes |= live_is_tight;
            evict_clean_records(&mut base);
            let run_prefix = sidecar_path(&self.wal_path, "compaction");
            let run_manifest = sidecar_path(&self.wal_path, "compaction-runs");
            let (plan, valid_length) = replay_wal_for_compaction(
                self.filesystem.clone(),
                &self.sealed_wal_path,
                &mut base,
                run_prefix,
                run_manifest,
            )?;
            tracing::debug!(
                target: "owl_db::storage_engine::checkpoint",
                compaction_runs = plan.runs.len(),
                compaction_run_bytes = plan.run_bytes,
                peak_compaction_run_bytes = plan.peak_run_bytes,
                peak_compaction_buffer_bytes = plan.peak_buffer_bytes,
                override_bitset_bytes = plan.overridden.bytes(),
                "prepared spillable compaction plan"
            );
            let sealed_length = self
                .filesystem
                .file_size(&self.sealed_wal_path)
                .map_err(|error| storage_io("inspect sealed WAL", &self.sealed_wal_path, error))?;
            if valid_length != sealed_length || base.committed_lsn != checkpoint_lsn {
                return Err(OQLError::storage_error(
                    "sealed WAL does not reconstruct the frozen checkpoint LSN",
                    None,
                ));
            }
            tracing::debug!(
                target: "owl_db::storage_engine::checkpoint",
                elapsed_ms = recovery_started.elapsed().as_secs_f64() * 1_000.0,
                "reconstructed frozen checkpoint state"
            );
            let image_started = Instant::now();
            let image = checkpoint_image_from_compaction(&base, &plan, sequence)?;
            let live_objects = image
                .records
                .len()
                .saturating_add(image.record_spool.as_ref().map_or(0, |spool| spool.len()))
                .saturating_add(image.segment_spool.as_ref().map_or(0, |spool| spool.rows()))
                .saturating_add(
                image
                    .typed_segments
                    .iter()
                    .map(|bytes| {
                        TypedSegment::descriptor(bytes).map(|descriptor| descriptor.row_count)
                    })
                    .collect::<OQLResult<Vec<_>>>()?
                    .into_iter()
                    .sum::<usize>(),
            );
            tracing::debug!(
                target: "owl_db::storage_engine::checkpoint",
                elapsed_ms = image_started.elapsed().as_secs_f64() * 1_000.0,
                live_objects,
                "built checkpoint image"
            );
            Ok((image, live_objects))
        })();
        let (image, live_objects) = match reconstructed {
            Ok(reconstructed) => reconstructed,
            Err(error) => {
                // Both WAL generations remain recoverable, but continuing to
                // rotate without a pending snapshot could discard the sealed
                // generation. Reopen performs the safe consolidation path.
                self.poisoned.store(true, Ordering::Release);
                return Err(error);
            }
        };
        Ok(PendingCheckpoint {
            image,
            live_objects,
            commits_at_snapshot,
        })
    }

    fn checkpoint_concurrent(&self) -> OQLResult<CheckpointInfo> {
        // Taken in a statement of its own, so the lock is let go before
        // the checkpoint is finished: finishing puts the checkpoint back
        // under the same lock when it fails again.
        let unfinished = self
            .pending_incremental_checkpoint
            .lock()
            .map_err(|_| OQLError::generic("pending incremental checkpoint lock poisoned"))?
            .take();
        if let Some(pending) = unfinished {
            return self.finish_incremental_checkpoint(pending);
        }
        let pending = self
            .pending_checkpoint
            .lock()
            .map_err(|_| OQLError::generic("pending checkpoint lock poisoned"))?
            .take();
        let pending = match pending {
            Some(pending) => pending,
            None => {
                if let Some(base) = latest_manifest(self.filesystem.as_ref(), &self.manifest_paths)?
                {
                    let committed_lsn = self
                        .state
                        .read()
                        .map_err(|_| OQLError::generic("storage state lock poisoned"))?
                        .committed_lsn;
                    if committed_lsn == base.checkpoint_lsn {
                        return Ok(CheckpointInfo {
                            sequence: base.sequence,
                            checkpoint_lsn: base.checkpoint_lsn,
                            live_objects: base.live_objects as usize,
                            segment_bytes: 0,
                            slot: base.slot,
                        });
                    }
                    if !self.checkpoint_requires_full_compaction(base)? {
                        let pending = self.prepare_incremental_checkpoint(base)?;
                        return self.finish_incremental_checkpoint(pending);
                    }
                }
                self.prepare_checkpoint()?
            }
        };
        self.finish_full_checkpoint(pending)
    }

    fn compact_concurrent(&self) -> OQLResult<CheckpointInfo> {
        let unfinished = self
            .pending_incremental_checkpoint
            .lock()
            .map_err(|_| OQLError::generic("pending incremental checkpoint lock poisoned"))?
            .take();
        if let Some(pending) = unfinished {
            self.finish_incremental_checkpoint(pending)?;
        }
        let pending = self
            .pending_checkpoint
            .lock()
            .map_err(|_| OQLError::generic("pending checkpoint lock poisoned"))?
            .take()
            .map(Ok)
            .unwrap_or_else(|| self.prepare_checkpoint())?;
        self.finish_full_checkpoint(pending)
    }

    fn finish_full_checkpoint(&self, pending: PendingCheckpoint) -> OQLResult<CheckpointInfo> {
        let publish_started = Instant::now();
        match self.publish_pending_checkpoint(&pending) {
            Ok(published) => {
                let checkpoint = &published.info;
                tracing::debug!(
                    target: "owl_db::storage_engine::checkpoint",
                    elapsed_ms = publish_started.elapsed().as_secs_f64() * 1_000.0,
                    segment_bytes = checkpoint.segment_bytes,
                    "published checkpoint image"
                );
                let included = pending.commits_at_snapshot;
                let _ = self.commits_since_checkpoint.fetch_update(
                    Ordering::AcqRel,
                    Ordering::Acquire,
                    |current| Some(current.saturating_sub(included)),
                );
                self.record_checkpoint_success_without_counter_reset(&checkpoint);
                self.record_delta_base_lsn
                    .store(checkpoint.checkpoint_lsn, Ordering::Release);
                let install_started = Instant::now();
                self.install_live_typed_checkpoint(
                    checkpoint.checkpoint_lsn,
                    published.segment_slot,
                    published.typed_locator,
                    published.typed_segments,
                    published.id_pages,
                )?;
                self.evict_clean_record_bytes()?;
                tracing::debug!(
                    target: "owl_db::storage_engine::checkpoint",
                    elapsed_ms = install_started.elapsed().as_secs_f64() * 1_000.0,
                    "installed live typed checkpoint"
                );
                Ok(published.info)
            }
            Err(error) => {
                if self.checkpoint_sequence.load(Ordering::Acquire) < pending.image.sequence {
                    *self
                        .pending_checkpoint
                        .lock()
                        .map_err(|_| OQLError::generic("pending checkpoint lock poisoned"))? =
                        Some(pending);
                } else {
                    self.record_delta_base_lsn
                        .store(pending.image.checkpoint_lsn, Ordering::Release);
                }
                Err(error)
            }
        }
    }

    fn install_live_typed_checkpoint(
        &self,
        checkpoint_lsn: u64,
        segment_slot: u8,
        typed_locator: TypedLocatorLeaf,
        descriptors: Vec<TypedSegmentFileDescriptor>,
        id_pages: Option<PublishedIndexPages>,
    ) -> OQLResult<()> {
        let open_started = Instant::now();
        let segment_path = self
            .segment_paths
            .get(segment_slot as usize)
            .ok_or_else(|| OQLError::storage_error("checkpoint slot is invalid", None))?;
        let typed_segments = open_typed_segment_handles(
            self.filesystem.as_ref(),
            segment_path,
            &descriptors,
            false,
        )?;
        tracing::debug!(
            target: "owl_db::storage_engine::checkpoint",
            elapsed_ms = open_started.elapsed().as_secs_f64() * 1_000.0,
            segments = typed_segments.len(),
            "opened file-backed live typed checkpoint"
        );
        let mut state = self
            .state
            .write()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        // Publication is off-lock. A newer commit must keep using the old
        // base-plus-delta view; reopen can install this checkpoint and replay
        // those newer WAL records exactly.
        // Publication is off-lock, so rows may have been written since
        // the commit the checkpoint froze. The files the store reads are
        // already the new checkpoint's, so the new base is installed all
        // the same, and those rows are put back over it afterwards.
        let written_later = match state.written_since_snapshot.take() {
            Some((since, written)) if since == checkpoint_lsn => {
                rows_written_later(&state, written)
            }
            _ if state.committed_lsn == checkpoint_lsn => Vec::new(),
            _ => {
                // Nothing says which rows are newer than the checkpoint.
                // Keep the view held, and take into memory what the id
                // index of the checkpoint just replaced lists, whose file
                // may now be removed.
                list_overflow_ids_in_memory(&mut state)?;
                return Ok(());
            }
        };
        // The image builder validated logical rows and the publisher
        // authenticated the immutable bytes. Installing the just-published
        // base only needs to validate its small schema catalog; a complete
        // row-by-row audit remains available through integrity inspection.
        validate_typed_segment_catalog(&state, &typed_segments)?;
        state.typed_complete_collections = complete_typed_collections(&state, &typed_segments);
        state.typed_base_lsn = checkpoint_lsn;
        state.typed_segments = typed_segments;
        if !install_typed_id_fences(&mut state, &typed_locator)? {
            rebuild_typed_id_fences(&mut state)?;
        }
        let retire_started = Instant::now();
        // A fully typed database can exchange its complete resident overlay in
        // constant structural work. Keep destruction outside the state lock;
        // mixed/overflow collections retain the precise delta-only fallback.
        let all_collections_complete =
            state.typed_complete_collections.len() == state.collection_ids.len();
        let (retired_resident_rows, retired_directories) = if all_collections_complete {
            let retired = state.object_directory.len();
            state.decoded_order.clear();
            let directories = (
                std::mem::take(&mut state.object_directory),
                std::mem::take(&mut state.collection_rows),
                std::mem::take(&mut state.object_schema_versions),
            );
            (retired, Some(directories))
        } else {
            // Only resident delta rows can need retirement. Walking the
            // published base here made installation O(total rows), even when
            // the durable change was tiny.
            // Every row outside the segments is a candidate, whether or not
            // it is held decoded at the moment.
            // A row whose record was in the files of the checkpoint just
            // replaced is listed nowhere, but may be held decoded.
            let retired = state
                .object_schema_versions
                .keys()
                .chain(
                    state
                        .object_directory
                        .keys()
                        .filter(|object_ref| !state.object_schema_versions.contains_key(object_ref)),
                )
                .copied()
                .map(|object_ref| {
                    typed_locator
                        .get(object_ref)
                        .map(|location| location.map(|location| (object_ref, location)))
                })
                .collect::<OQLResult<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            for (object_ref, (segment_index, row)) in &retired {
                // The segment the row moved to names it. Its record may be
                // nowhere else by now: gone from memory, and left out of the
                // checkpoint's records because the segment holds the row.
                let segment = state.typed_segments.get(*segment_index).ok_or_else(|| {
                    OQLError::storage_error("a row was retired to a missing segment", None)
                })?;
                let schema = state
                    .compiled_schemas
                    .get(&(segment.collection_id(), segment.schema_version()))
                    .map(|compiled| compiled.name.clone())
                    .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
                let id = segment.document_id(*row)?;
                state.object_directory.remove(object_ref);
                if let Some(rows) = state.collection_rows.get_mut(&schema) {
                    rows.remove(&id);
                    if rows.is_empty() {
                        state.collection_rows.remove(&schema);
                    }
                }
                state.object_schema_versions.remove(object_ref);
            }
            // The rows that moved into segments leave these maps mostly
            // empty; give the room back.
            if state.object_schema_versions.capacity() > 4 * state.object_schema_versions.len().max(64) {
                state.object_schema_versions.shrink_to_fit();
            }
            (retired.len(), None)
        };
        state.typed_row_locations.install_base(typed_locator);
        state.typed_delta_rows.clear();
        // The files read from here on are the new checkpoint's, which
        // holds no row deleted before it, and a record for every row
        // outside its column segments: those need no listing in memory.
        state.deleted_since_checkpoint = HashSet::new();
        let records_are_read = match &state.checkpoint_records {
            Some(files) => files
                .lock()
                .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
                .base_segment
                .is_some(),
            None => false,
        };
        if id_pages.is_some() && records_are_read {
            state.object_schema_versions = Default::default();
        }
        // Every row is now in a column segment or listed in the id index
        // of the checkpoint just published, so none need be listed here.
        match id_pages {
            Some(index) => {
                let ids: Arc<dyn OrderedIndexBase> = Arc::new(FileIndexBase::new(
                    self.filesystem.clone(),
                    index.schema,
                    index.name,
                    true,
                    index.pages,
                    state.index_pages(),
                ));
                state.overflow_id_counts = count_overflow_ids(&state, ids.as_ref())?;
                state.overflow_ids = Some(ids);
                state.collection_rows.clear();
            }
            None => list_overflow_ids_in_memory(&mut state)?,
        }
        let rows_on_file = state.has_rows_on_file();
        for row in written_later {
            let (schema, id, object_ref, now) = match row {
                WrittenLater::Gone { schema, id, object_ref } => {
                    state.deleted_since_checkpoint.insert(object_ref);
                    (schema, id, object_ref, None)
                }
                WrittenLater::Live { schema, id, object_ref, version, document } => {
                    state
                        .collection_rows
                        .entry(schema.clone())
                        .or_default()
                        .insert(id.clone(), object_ref);
                    state.object_schema_versions.insert(object_ref, version);
                    if let Some(document) = document {
                        if state.object_directory.insert(object_ref, document).is_none() {
                            state.note_decoded(object_ref);
                        }
                    }
                    (schema, id, object_ref, Some(object_ref))
                }
            };
            // The checkpoint may hold an older version of the row in a
            // column segment; it is no longer the row.
            state.typed_row_locations.remove(&object_ref)?;
            if rows_on_file {
                state.typed_delta_rows.entry(schema).or_default().insert(id, now);
            }
        }
        state.forget_counts();
        drop(state);
        tracing::debug!(
            target: "owl_db::storage_engine::checkpoint",
            elapsed_ms = retire_started.elapsed().as_secs_f64() * 1_000.0,
            retired_resident_rows,
            "retired resident delta rows covered by typed checkpoint"
        );
        drop(retired_directories);
        Ok(())
    }

    fn publish_pending_checkpoint(
        &self,
        pending: &PendingCheckpoint,
    ) -> OQLResult<PublishedCheckpoint> {
        let obsolete_generations = self.published_flush_generations()?;
        let checkpoint = self.publish_image(&pending.image, pending.live_objects)?;
        if self.filesystem.exists(&self.sealed_wal_path) {
            self.filesystem
                .remove(&self.sealed_wal_path)
                .and_then(|_| self.filesystem.sync_all())
                .map_err(|error| storage_io("reclaim sealed WAL", &self.sealed_wal_path, error))?;
        }
        for path in &self.wal_delta_paths {
            if self.filesystem.exists(path) {
                self.filesystem
                    .remove(path)
                    .map_err(|error| storage_io("reclaim compacted WAL delta", path, error))?;
            }
        }
        for generation in obsolete_generations {
            let path = flush_generation_path(&self.generation_prefix, generation.sequence);
            if self.filesystem.exists(&path) {
                self.filesystem
                    .remove(&path)
                    .map_err(|error| storage_io("reclaim compacted generation", &path, error))?;
            }
            let record_path =
                flush_generation_record_path(&self.generation_prefix, generation.sequence);
            if self.filesystem.exists(&record_path) {
                self.filesystem.remove(&record_path).map_err(|error| {
                    storage_io("reclaim compacted generation records", &record_path, error)
                })?;
            }
            let index_path =
                flush_generation_index_path(&self.generation_prefix, generation.sequence);
            if self.filesystem.exists(&index_path) {
                self.filesystem.remove(&index_path).map_err(|error| {
                    storage_io("reclaim compacted generation indexes", &index_path, error)
                })?;
            }
        }
        for path in &self.generation_root_paths {
            if self.filesystem.exists(path) {
                self.filesystem.remove(path).map_err(|error| {
                    storage_io("reclaim compacted generation root", path, error)
                })?;
            }
        }
        self.filesystem.sync_all().map_err(|error| {
            storage_io("sync full checkpoint reclamation", &self.wal_path, error)
        })?;
        Ok(checkpoint)
    }

    fn published_flush_generations(&self) -> OQLResult<Vec<FlushGeneration>> {
        let Some(manifest) = latest_manifest(self.filesystem.as_ref(), &self.manifest_paths)?
        else {
            return Ok(Vec::new());
        };
        Ok(generation_root_for_manifest(
            self.filesystem.as_ref(),
            &self.generation_root_paths,
            manifest,
        )?
        .map(|root| root.generations)
        .unwrap_or_default())
    }

    fn prepare_incremental_checkpoint(
        &self,
        base: Manifest,
    ) -> OQLResult<PendingIncrementalCheckpoint> {
        let rotated = AtomicBool::new(false);
        let prepared = self.prepare_incremental_checkpoint_inner(base, &rotated);
        if prepared.is_err() && rotated.load(Ordering::Acquire) {
            // The sealed WAL now holds acknowledged commits that no manifest
            // publishes, and the dirty record/index sets describing them may
            // already be drained. Nothing in memory can retry this snapshot,
            // and the next rotation would reclaim the sealed WAL as if it had
            // been published. Both WAL generations remain recoverable, so stop
            // writing and let reopen consolidate them.
            self.poisoned.store(true, Ordering::Release);
        }
        prepared
    }

    fn prepare_incremental_checkpoint_inner(
        &self,
        base: Manifest,
        rotated: &AtomicBool,
    ) -> OQLResult<PendingIncrementalCheckpoint> {
        let (
            sequence,
            checkpoint_lsn,
            live_objects,
            commits_at_snapshot,
            record_delta,
            index_delta,
        ) = {
            let _commit = self
                .commit_lock
                .lock()
                .map_err(|_| OQLError::generic("storage commit lock poisoned"))?;
            let sequence = self
                .checkpoint_sequence
                .load(Ordering::Acquire)
                .checked_add(1)
                .ok_or_else(|| OQLError::storage_error("checkpoint sequence exhausted", None))?;
            // Rotate before draining the dirty sets. The commit lock freezes
            // the committed LSN across both steps, and a rotation that fails
            // cleanly then leaves the dirty tracking intact for a later
            // attempt.
            self.rotate_wal_locked()?;
            rotated.store(true, Ordering::Release);
            let mut state = self
                .state
                .write()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let checkpoint_lsn = state.committed_lsn;
            let live_objects = logical_live_count(&state);
            let commits_at_snapshot = self.commits_since_checkpoint.load(Ordering::Acquire);
            // WAL generations are authoritative. Clearing these dirty sets
            // bounds the live overlay after publication; no cumulative root
            // is constructed or copied here.
            let delta_base_lsn = self.record_delta_base_lsn.load(Ordering::Acquire);
            let record_delta = state
                .object_records
                .take_delta(delta_base_lsn, checkpoint_lsn);
            let index_delta = take_index_delta(&mut state, delta_base_lsn, checkpoint_lsn)?;
            drop(state);
            (
                sequence,
                checkpoint_lsn,
                live_objects,
                commits_at_snapshot,
                record_delta,
                index_delta,
            )
        };
        let wal = read_exact_file(self.filesystem.as_ref(), &self.sealed_wal_path)?;
        let records = record_delta.encode()?;
        RecordPageDelta::decode(&records)?;
        let indexes = index_delta.encode()?;
        if wal.is_empty() || checkpoint_lsn <= base.checkpoint_lsn {
            return Err(OQLError::storage_error(
                "incremental checkpoint has no new committed WAL generation",
                None,
            ));
        }
        let mut root = generation_root_for_manifest(
            self.filesystem.as_ref(),
            &self.generation_root_paths,
            base,
        )?
        .unwrap_or(GenerationRoot {
            sequence: base.sequence,
            checkpoint_lsn: base.checkpoint_lsn,
            base_sequence: base.sequence,
            base_lsn: base.checkpoint_lsn,
            generations: Vec::new(),
        });
        let generation = FlushGeneration {
            sequence,
            start_lsn: base.checkpoint_lsn + 1,
            end_lsn: checkpoint_lsn,
            len: wal.len() as u64,
            checksum: crc32fast::hash(&wal),
            record_len: records.len() as u64,
            record_checksum: RecordPageDelta::authority_checksum(&records)?,
            index_len: indexes.len() as u64,
            index_checksum: crc32fast::hash(&indexes),
        };
        root.sequence = sequence;
        root.checkpoint_lsn = checkpoint_lsn;
        root.generations.push(generation);
        root.validate()?;
        let slot = (sequence & 1) as u8;
        Ok(PendingIncrementalCheckpoint {
            manifest: Manifest {
                sequence,
                checkpoint_lsn,
                segment_len: base.segment_len,
                segment_checksum: base.segment_checksum,
                live_objects: live_objects as u64,
                slot,
                segment_slot: base.segment_slot,
                delta_len: 0,
                delta_checksum: 0,
                has_record_root: false,
                has_index_root: false,
            },
            root,
            generation,
            wal,
            records,
            indexes,
            commits_at_snapshot,
        })
    }

    fn finish_incremental_checkpoint(
        &self,
        pending: PendingIncrementalCheckpoint,
    ) -> OQLResult<CheckpointInfo> {
        let result = self.publish_incremental_checkpoint(&pending);
        match result {
            Ok(checkpoint) => {
                let _ = self.commits_since_checkpoint.fetch_update(
                    Ordering::AcqRel,
                    Ordering::Acquire,
                    |current| Some(current.saturating_sub(pending.commits_at_snapshot)),
                );
                self.record_delta_base_lsn
                    .store(checkpoint.checkpoint_lsn, Ordering::Release);
                self.record_checkpoint_success_without_counter_reset(&checkpoint);
                self.evict_clean_record_bytes()?;
                Ok(checkpoint)
            }
            Err(error) => {
                if self.checkpoint_sequence.load(Ordering::Acquire) < pending.manifest.sequence {
                    *self.pending_incremental_checkpoint.lock().map_err(|_| {
                        OQLError::generic("pending incremental checkpoint lock poisoned")
                    })? = Some(pending);
                }
                Err(error)
            }
        }
    }

    fn publish_incremental_checkpoint(
        &self,
        pending: &PendingIncrementalCheckpoint,
    ) -> OQLResult<CheckpointInfo> {
        let slot = pending.manifest.slot as usize;
        let generation_path =
            flush_generation_path(&self.generation_prefix, pending.generation.sequence);
        write_synced_file(self.filesystem.as_ref(), &generation_path, &pending.wal)?;
        let record_path =
            flush_generation_record_path(&self.generation_prefix, pending.generation.sequence);
        write_synced_file(self.filesystem.as_ref(), &record_path, &pending.records)?;
        let index_path =
            flush_generation_index_path(&self.generation_prefix, pending.generation.sequence);
        write_synced_file(self.filesystem.as_ref(), &index_path, &pending.indexes)?;
        write_synced_file(
            self.filesystem.as_ref(),
            &self.generation_root_paths[slot],
            &pending.root.encode()?,
        )?;
        write_synced_file(
            self.filesystem.as_ref(),
            &self.manifest_paths[slot],
            &pending.manifest.encode(),
        )?;
        self.install_record_root_handle(pending.manifest)?;
        self.checkpoint_sequence
            .store(pending.manifest.sequence, Ordering::Release);
        if self.filesystem.exists(&self.sealed_wal_path) {
            self.filesystem
                .remove(&self.sealed_wal_path)
                .map_err(|error| storage_io("reclaim sealed WAL", &self.sealed_wal_path, error))?;
        }
        self.filesystem
            .sync_all()
            .map_err(|error| storage_io("sync incremental checkpoint", &self.wal_path, error))?;
        Ok(CheckpointInfo {
            sequence: pending.manifest.sequence,
            checkpoint_lsn: pending.manifest.checkpoint_lsn,
            live_objects: pending.manifest.live_objects as usize,
            segment_bytes: pending
                .generation
                .len
                .saturating_add(pending.generation.record_len)
                .saturating_add(pending.generation.index_len),
            slot: pending.manifest.slot,
        })
    }

    fn publish_image(
        &self,
        image: &CheckpointImage,
        live_objects: usize,
    ) -> OQLResult<PublishedCheckpoint> {
        let sequence = image.sequence;
        let checkpoint_lsn = image.checkpoint_lsn;
        let slot = (sequence & 1) as usize;
        // Manifest slots alternate with the sequence, but the base segment
        // must not: an incremental checkpoint publishes a new manifest that
        // keeps referencing the previous base, so after one incremental the
        // sequence parity points back at the live base. Writing the segment
        // truncates its file, so always target the slot the durable manifest
        // does not reference. A crash mid-write then leaves the published
        // checkpoint untouched.
        let segment_slot = match latest_manifest(self.filesystem.as_ref(), &self.manifest_paths)? {
            Some(published) => 1 - published.segment_slot as usize,
            None => slot,
        };
        let segment = write_segment_file(
            self.filesystem.as_ref(),
            &self.segment_paths[segment_slot],
            image,
            self.max_size_bytes,
        )?;
        // The root just written took the place of an older one, whose
        // parts nothing lists any more. Failing to remove them costs
        // space until the next attempt, and nothing else.
        let _ = crate::storage_engine::parts::remove_unlisted_parts(
            self.filesystem.as_ref(),
            &self.segment_paths,
        );
        let record_root = RecordPageDelta {
            base_lsn: checkpoint_lsn,
            target_lsn: checkpoint_lsn,
            puts: Vec::new(),
            deletes: Vec::new(),
        }
        .encode()?;
        write_synced_file(
            self.filesystem.as_ref(),
            &self.record_delta_paths[slot],
            &record_root,
        )?;
        let index_root = IndexRootDelta {
            base_lsn: checkpoint_lsn,
            target_lsn: checkpoint_lsn,
            indexes: Vec::new(),
        };
        write_index_root_file(
            self.filesystem.as_ref(),
            &self.index_delta_paths[slot],
            &index_root,
        )?;
        let manifest = Manifest {
            sequence,
            checkpoint_lsn,
            segment_len: segment.len,
            segment_checksum: segment.checksum,
            live_objects: live_objects as u64,
            slot: slot as u8,
            segment_slot: segment_slot as u8,
            delta_len: 0,
            delta_checksum: 0,
            has_record_root: true,
            has_index_root: true,
        };
        if let Err(error) = write_synced_file(
            self.filesystem.as_ref(),
            &self.manifest_paths[slot],
            &manifest.encode(),
        ) {
            // The manifest may be on disk though writing it failed, and
            // nothing here can tell. Publishing again would choose a base
            // segment to write over by reading the manifests, and could
            // choose the one a manifest that did reach the disk depends
            // on. Both logs are still whole, so stop writing and let the
            // next open settle which checkpoint stands.
            self.poisoned.store(true, Ordering::Release);
            return Err(error);
        }
        self.install_record_root_handle(manifest)?;
        let mut published_indexes = segment.indexes;
        let id_pages = published_indexes
            .iter()
            .position(|index| index.schema == REVERSE_INDEX_SCHEMA && index.name == ID_INDEX_NAME)
            .map(|at| published_indexes.remove(at));
        self.install_index_bases(published_indexes)?;
        self.checkpoint_sequence.store(sequence, Ordering::Release);

        Ok(PublishedCheckpoint {
            info: CheckpointInfo {
                sequence,
                checkpoint_lsn,
                live_objects,
                segment_bytes: segment.len,
                slot: slot as u8,
            },
            segment_slot: segment_slot as u8,
            typed_locator: segment.typed_locator,
            typed_segments: segment.typed_segments,
            id_pages,
        })
    }

    fn refresh_record_root_handle(&self) -> OQLResult<()> {
        let manifest = latest_manifest(self.filesystem.as_ref(), &self.manifest_paths)?;
        let next = manifest
            .map(|manifest| self.open_checkpoint_read_handles(manifest))
            .transpose()?
            .unwrap_or_default();
        *self
            .checkpoint_read_handles
            .lock()
            .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))? = next;
        Ok(())
    }

    fn install_record_root_handle(&self, manifest: Manifest) -> OQLResult<()> {
        let next = self.open_checkpoint_read_handles(manifest)?;
        *self
            .checkpoint_read_handles
            .lock()
            .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))? = next;
        Ok(())
    }

    fn install_index_bases(&self, published: Vec<PublishedIndexPages>) -> OQLResult<()> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let limit = state.index_pages();
        for index in published {
            let identity = (index.schema.clone(), index.name.clone());
            if identity.0 == REVERSE_INDEX_SCHEMA && identity.1 == REVERSE_INDEX_NAME {
                state
                    .reverse_reference_index
                    .replace_base_preserving_dirty(Arc::new(FileIndexBase::new(
                        self.filesystem.clone(),
                        index.schema,
                        index.name,
                        false,
                        index.pages,
                        state.index_pages(),
                    )))
                    .map_err(|error| OQLError::storage_error(&error, None))?;
                continue;
            }
            let definition_matches = state
                .scalar_indexes
                .get(&identity.0)
                .into_iter()
                .flatten()
                .any(|definition| {
                    definition.name == identity.1 && definition.unique == index.unique
                });
            let Some(runtime) = state
                .scalar_index_data
                .get(&identity)
                .filter(|_| definition_matches)
            else {
                // The index was deleted or replaced by a commit newer than the
                // checkpoint snapshot. Its live runtime state is authoritative.
                continue;
            };
            runtime
                .replace_base_preserving_dirty(Arc::new(FileIndexBase::new(
                    self.filesystem.clone(),
                    index.schema,
                    index.name,
                    index.unique,
                    index.pages,
                    limit.clone(),
                )))
                .map_err(|error| OQLError::storage_error(&error, None))?;
        }
        Ok(())
    }

    fn open_checkpoint_read_handles(&self, manifest: Manifest) -> OQLResult<CheckpointReadHandles> {
        let record_root = manifest.has_record_root.then(|| {
            RecordRootHandle::open(
                self.filesystem.clone(),
                &self.record_delta_paths[manifest.slot as usize],
            )
            .ok()
        });
        let base_segment = BaseSegmentHandle::open(
            self.filesystem.clone(),
            &self.segment_paths[manifest.segment_slot as usize],
            manifest,
        )
        .ok();
        let generation_record_roots = generation_root_for_manifest(
            self.filesystem.as_ref(),
            &self.generation_root_paths,
            manifest,
        )?
        .map(|root| {
            root.generations
                .into_iter()
                .rev()
                .map(|generation| {
                    RecordRootHandle::open(
                        self.filesystem.clone(),
                        &flush_generation_record_path(&self.generation_prefix, generation.sequence),
                    )
                })
                .collect::<OQLResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
        Ok(CheckpointReadHandles {
            record_root: record_root.flatten(),
            generation_record_roots,
            base_segment,
        })
    }

    fn read_compiled_record(&self, object_ref: ObjRef) -> OQLResult<Arc<[u8]>> {
        let (dirty, materialized) = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            (
                state.object_records.is_dirty(&object_ref),
                state.object_records.get(&object_ref).cloned(),
            )
        };
        if let Some(materialized) = materialized {
            return Ok(materialized);
        }
        if dirty {
            return Err(OQLError::storage_error(
                "dirty live object has no compiled record",
                None,
            ));
        }
        {
            let handles = self
                .checkpoint_read_handles
                .lock()
                .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?;
            for root in &handles.generation_record_roots {
                match root.lookup(object_ref)? {
                    RecordDeltaLookup::Put(record) => return Ok(record),
                    RecordDeltaLookup::Delete => {
                        return Err(OQLError::storage_error(
                            "live object is deleted by an immutable generation",
                            None,
                        ));
                    }
                    RecordDeltaLookup::Missing => {}
                }
            }
            match handles
                .record_root
                .as_ref()
                .map(|root| root.lookup(object_ref))
                .transpose()?
            {
                Some(RecordDeltaLookup::Put(record)) => return Ok(record),
                Some(RecordDeltaLookup::Delete) => {
                    return Err(OQLError::storage_error(
                        "live object is deleted by its checkpoint record root",
                        None,
                    ));
                }
                Some(RecordDeltaLookup::Missing) | None => {}
            }
            if let Some(record) = handles
                .base_segment
                .as_ref()
                .map(|base| base.lookup(object_ref))
                .transpose()?
                .flatten()
            {
                return Ok(record);
            }
        }
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        encode_authoritative_row(&state, object_ref)
    }

    /// Resolve a scan-sized record batch with one state lock and one checkpoint
    /// handle lock. The returned vector preserves the caller's row order.
    fn read_compiled_records(&self, object_refs: &[ObjRef]) -> OQLResult<Vec<Arc<[u8]>>> {
        let mut records = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            object_refs
                .iter()
                .map(|object_ref| {
                    let record = state.object_records.get(object_ref).cloned();
                    if record.is_none() && state.object_records.is_dirty(object_ref) {
                        return Err(OQLError::storage_error(
                            "dirty live object has no compiled record",
                            None,
                        ));
                    }
                    Ok(record)
                })
                .collect::<OQLResult<Vec<_>>>()?
        };
        if records.iter().all(Option::is_some) {
            return Ok(records.into_iter().flatten().collect());
        }

        let handles = self
            .checkpoint_read_handles
            .lock()
            .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?;
        for root in &handles.generation_record_roots {
            for (object_ref, record) in object_refs.iter().zip(&mut records) {
                if record.is_some() {
                    continue;
                }
                match root.lookup(*object_ref)? {
                    RecordDeltaLookup::Put(found) => *record = Some(found),
                    RecordDeltaLookup::Delete => {
                        return Err(OQLError::storage_error(
                            "live object is deleted by an immutable generation",
                            None,
                        ));
                    }
                    RecordDeltaLookup::Missing => {}
                }
            }
        }
        for (object_ref, record) in object_refs.iter().zip(&mut records) {
            if record.is_some() {
                continue;
            }
            match handles
                .record_root
                .as_ref()
                .map(|root| root.lookup(*object_ref))
                .transpose()?
            {
                Some(RecordDeltaLookup::Put(found)) => {
                    *record = Some(found);
                    continue;
                }
                Some(RecordDeltaLookup::Delete) => {
                    return Err(OQLError::storage_error(
                        "live object is deleted by its checkpoint record root",
                        None,
                    ));
                }
                Some(RecordDeltaLookup::Missing) | None => {}
            }
        }
        let mut unresolved = records
            .iter()
            .enumerate()
            .filter_map(|(index, record)| record.is_none().then_some(index))
            .collect::<Vec<_>>();
        if !unresolved.is_empty() {
            let base = handles.base_segment.as_ref();
            let requested = unresolved
                .iter()
                .map(|index| object_refs[*index])
                .collect::<Vec<_>>();
            if let Some(base) = base {
                let found = base.lookup_many(&requested)?;
                for (index, record) in unresolved.iter().copied().zip(found) {
                    records[index] = record;
                }
            }
        }
        drop(handles);
        unresolved.retain(|index| records[*index].is_none());
        if !unresolved.is_empty() {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            for index in unresolved {
                records[index] = Some(encode_authoritative_row(&state, object_refs[index])?);
            }
        }
        for record in &records {
            if record.is_none() {
                return Err(OQLError::storage_error(
                    "live object is absent from compiled record storage",
                    None,
                ));
            }
        }
        records
            .into_iter()
            .map(|record| {
                record.ok_or_else(|| {
                    OQLError::storage_error("compiled record batch is incomplete", None)
                })
            })
            .collect()
    }

    fn checkpoint_image_from_state(
        &self,
        state: &State,
        sequence: u64,
    ) -> OQLResult<CheckpointImage> {
        let identities_started = Instant::now();
        // The files are locked for one read at a time: reading a row
        // through the state takes the same lock.
        let on_file = |object_ref: ObjRef| -> OQLResult<Option<Arc<[u8]>>> {
            self.checkpoint_read_handles
                .lock()
                .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
                .record(object_ref)
        };
        let mut object_refs = Vec::with_capacity(logical_live_count(state));
        for schema in state.collection_ids.keys() {
            object_refs.extend(logical_collection_object_refs(state, schema)?);
        }
        object_refs.sort_by_key(|object_ref| object_ref.packed());
        tracing::debug!(
            target: "owl_db::storage_engine::checkpoint",
            elapsed_ms = identities_started.elapsed().as_secs_f64() * 1_000.0,
            objects = object_refs.len(),
            "collected checkpoint object identities"
        );
        let records_started = Instant::now();
        let mut records = Vec::new();
        for object_ref in object_refs {
            if typed_row_remains_checkpoint_authoritative(state, object_ref)? {
                continue;
            }
            // What memory holds is newest. Failing that, the checkpoint
            // files hold it, and the newest of those is a generation
            // before it is the base: a row last written just before an
            // incremental checkpoint has its current record only there.
            let record = if let Some(record) = state.object_records.get(&object_ref) {
                record.clone()
            } else if let Some(record) = on_file(object_ref)? {
                record
            } else {
                encode_authoritative_row(state, object_ref)?
            };
            let header = RecordCodec::header(&record)?;
            if header.object_ref != object_ref {
                return Err(OQLError::storage_error(
                    "checkpoint record identity conflicts with logical object identity",
                    None,
                ));
            }
            records.push((object_ref, record));
        }
        tracing::debug!(
            target: "owl_db::storage_engine::checkpoint",
            elapsed_ms = records_started.elapsed().as_secs_f64() * 1_000.0,
            overflow_records = records.len(),
            "collected checkpoint overflow records"
        );
        checkpoint_image_with_records(state, sequence, records)
    }

    fn evict_clean_record_bytes(&self) -> OQLResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        evict_clean_records(&mut state);
        Ok(())
    }

    fn reclaim_obsolete_files_locked(&self) -> OQLResult<ReclamationReport> {
        cleanup_unpublished_flush_generation(
            self.filesystem.as_ref(),
            &self.manifest_paths,
            &self.generation_root_paths,
            &self.generation_prefix,
        )?;
        let Some(manifest) = latest_manifest(self.filesystem.as_ref(), &self.manifest_paths)?
        else {
            return Ok(ReclamationReport {
                retained_sequence: 0,
                files_removed: 0,
                bytes_reclaimed: 0,
            });
        };
        let mut report = ReclamationReport {
            retained_sequence: manifest.sequence,
            files_removed: 0,
            bytes_reclaimed: 0,
        };

        // Remove obsolete authority first. After this sync, no surviving
        // manifest can make any subsequently removed artifact reachable.
        for (slot, path) in self.manifest_paths.iter().enumerate() {
            if slot != manifest.slot as usize {
                remove_reclaimed_file(self.filesystem.as_ref(), path, &mut report)?;
            }
        }
        self.filesystem.sync_all().map_err(|error| {
            storage_io("sync obsolete manifest reclamation", &self.wal_path, error)
        })?;

        let retained_generation_root = generation_root_for_manifest(
            self.filesystem.as_ref(),
            &self.generation_root_paths,
            manifest,
        )?;
        for (slot, path) in self.generation_root_paths.iter().enumerate() {
            if retained_generation_root.is_none() || slot != manifest.slot as usize {
                remove_reclaimed_file(self.filesystem.as_ref(), path, &mut report)?;
            }
        }

        let retained_segment = manifest.segment_slot as usize;
        for (slot, path) in self.segment_paths.iter().enumerate() {
            if slot != retained_segment {
                remove_reclaimed_file(self.filesystem.as_ref(), path, &mut report)?;
            }
        }
        // The parts only the removed checkpoint listed go with it. Its
        // size, counted above, was theirs.
        let (parts, _) = crate::storage_engine::parts::remove_unlisted_parts(
            self.filesystem.as_ref(),
            &self.segment_paths,
        )
        .map_err(|error| storage_io("remove unlisted checkpoint parts", &self.wal_path, error))?;
        report.files_removed += parts;
        for (slot, path) in self.record_delta_paths.iter().enumerate() {
            if !manifest.has_record_root || slot != manifest.slot as usize {
                remove_reclaimed_file(self.filesystem.as_ref(), path, &mut report)?;
            }
        }
        for (slot, path) in self.index_delta_paths.iter().enumerate() {
            if !manifest.has_index_root || slot != manifest.slot as usize {
                remove_reclaimed_file(self.filesystem.as_ref(), path, &mut report)?;
            }
        }
        for (slot, path) in self.wal_delta_paths.iter().enumerate() {
            if manifest.delta_len == 0 || slot != manifest.slot as usize {
                remove_reclaimed_file(self.filesystem.as_ref(), path, &mut report)?;
            }
        }
        self.filesystem
            .sync_all()
            .map_err(|error| storage_io("sync obsolete file reclamation", &self.wal_path, error))?;
        Ok(report)
    }

    fn rotate_wal_locked(&self) -> OQLResult<()> {
        if self.filesystem.exists(&self.sealed_wal_path) {
            self.filesystem
                .remove(&self.sealed_wal_path)
                .and_then(|_| self.filesystem.sync_all())
                .map_err(|error| {
                    storage_io("reclaim published sealed WAL", &self.sealed_wal_path, error)
                })?;
        }
        let mut wal_guard = self
            .wal
            .lock()
            .map_err(|_| OQLError::generic("WAL lock poisoned"))?;
        let mut old_wal = wal_guard
            .take()
            .ok_or_else(|| OQLError::storage_error("active WAL handle is unavailable", None))?;
        // Frames may be written and not yet synced, their commits still
        // waiting. They are about to be in a file no later sync touches,
        // so put them on disk now. The commit lock keeps new frames out.
        let through = self.written_lsn.load(Ordering::Acquire);
        if let Err(error) = old_wal.sync() {
            *wal_guard = Some(old_wal);
            return Err(storage_io("sync WAL before sealing it", &self.wal_path, error));
        }
        {
            let mut durability = self
                .durability
                .lock()
                .map_err(|_| OQLError::generic("durability lock poisoned"))?;
            durability.synced_lsn = durability.synced_lsn.max(through);
            self.durable.notify_all();
        }
        drop(old_wal);

        if let Err(error) = self
            .filesystem
            .rename(&self.wal_path, &self.sealed_wal_path)
        {
            *wal_guard = Some(open_writable_wal(self.filesystem.as_ref(), &self.wal_path)?);
            return Err(storage_io("seal active WAL", &self.wal_path, error));
        }
        match create_synced_wal(self.filesystem.as_ref(), &self.wal_path) {
            Ok(new_wal) => {
                *wal_guard = Some(new_wal);
                self.wal_synced_len.store(0, Ordering::Release);
                if let Err(error) = self.filesystem.sync_all() {
                    self.poisoned.store(true, Ordering::Release);
                    return Err(storage_io("sync WAL rotation", &self.wal_path, error));
                }
                Ok(())
            }
            Err(error) => {
                if self.filesystem.exists(&self.wal_path) {
                    let _ = self.filesystem.remove(&self.wal_path);
                }
                let rollback = self
                    .filesystem
                    .rename(&self.sealed_wal_path, &self.wal_path)
                    .map_err(|rollback_error| {
                        storage_io("roll back WAL rotation", &self.wal_path, rollback_error)
                    })
                    .and_then(|_| open_writable_wal(self.filesystem.as_ref(), &self.wal_path));
                match rollback {
                    Ok(restored) => *wal_guard = Some(restored),
                    Err(_) => self.poisoned.store(true, Ordering::Release),
                }
                Err(error)
            }
        }
    }

    fn publish_recovered_state(&self) -> OQLResult<CheckpointInfo> {
        let sequence = self
            .checkpoint_sequence
            .load(Ordering::Acquire)
            .checked_add(1)
            .ok_or_else(|| OQLError::storage_error("checkpoint sequence exhausted", None))?;
        let (image, live_objects) = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            (
                self.checkpoint_image_from_state(&state, sequence)?,
                logical_live_count(&state),
            )
        };
        let published = self.publish_image(&image, live_objects)?;
        let checkpoint = published.info.clone();
        {
            let mut wal = self
                .wal
                .lock()
                .map_err(|_| OQLError::generic("WAL lock poisoned"))?;
            let wal = wal
                .as_mut()
                .ok_or_else(|| OQLError::storage_error("active WAL handle is unavailable", None))?;
            self.wal_synced_len.store(0, Ordering::Release);
            wal.set_len(0)
                .and_then(|_| wal.seek(SeekFrom::Start(0)).map(|_| ()))
                .and_then(|_| wal.sync())
                .map_err(|error| {
                    storage_io("truncate recovered active WAL", &self.wal_path, error)
                })?;
        }
        if self.filesystem.exists(&self.sealed_wal_path) {
            self.filesystem
                .remove(&self.sealed_wal_path)
                .and_then(|_| self.filesystem.sync_all())
                .map_err(|error| {
                    storage_io("reclaim recovered sealed WAL", &self.sealed_wal_path, error)
                })?;
        }
        // The checkpoint just published holds the typed rows in its base
        // segment. The open state must hold them the same way, as it does
        // after any other full checkpoint; otherwise it keeps a second
        // copy of each row and no longer matches what is on disk.
        self.install_live_typed_checkpoint(
            checkpoint.checkpoint_lsn,
            published.segment_slot,
            published.typed_locator,
            published.typed_segments,
            published.id_pages,
        )?;
        Ok(checkpoint)
    }

    fn maybe_auto_checkpoint(&self) -> OQLResult<()> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(OQLError::storage_error(
                "storage writer is poisoned; reopen before checkpointing",
                None,
            ));
        }
        if !self.auto_checkpoint_due()? {
            return Ok(());
        }
        let _checkpoint = self
            .checkpoint_lock
            .lock()
            .map_err(|_| OQLError::generic("checkpoint lock poisoned"))?;
        // Another writer may have completed maintenance while this writer was
        // waiting for the serialized checkpoint boundary.
        if !self.auto_checkpoint_due()? {
            return Ok(());
        }
        if self
            .pending_checkpoint
            .lock()
            .map_err(|_| OQLError::generic("pending checkpoint lock poisoned"))?
            .is_some()
            || self
                .pending_incremental_checkpoint
                .lock()
                .map_err(|_| OQLError::generic("pending incremental checkpoint lock poisoned"))?
                .is_some()
        {
            return self.checkpoint_concurrent().map(|_| ());
        }
        self.checkpoint_concurrent().map(|_| ())
    }

    fn checkpoint_requires_full_compaction(&self, base: Manifest) -> OQLResult<bool> {
        if base.delta_len > 0 {
            // A legacy or transitional manifest cannot safely begin a native
            // immutable generation list without first folding its cumulative
            // roots into a new base.
            return Ok(true);
        }
        let root = generation_root_for_manifest(
            self.filesystem.as_ref(),
            &self.generation_root_paths,
            base,
        )?;
        if root
            .as_ref()
            .is_some_and(|root| root.generations.len() >= MAX_FLUSH_GENERATIONS)
        {
            return Ok(true);
        }
        let active_bytes = self
            .filesystem
            .file_size(&self.wal_path)
            .map_err(|error| storage_io("inspect WAL", &self.wal_path, error))?;
        let generation_bytes = root.as_ref().map_or(0, |root| {
            root.generations
                .iter()
                .map(|generation| {
                    generation
                        .len
                        .saturating_add(generation.record_len)
                        .saturating_add(generation.index_len)
                })
                .sum::<u64>()
        });
        let projected_delta = generation_bytes.saturating_add(active_bytes);
        let compaction_threshold = MIN_DELTA_COMPACTION_BYTES.max(base.segment_len / 2);
        Ok(projected_delta > compaction_threshold)
    }

    fn auto_checkpoint_due(&self) -> OQLResult<bool> {
        if self
            .pending_checkpoint
            .lock()
            .map_err(|_| OQLError::generic("pending checkpoint lock poisoned"))?
            .is_some()
        {
            return Ok(true);
        }
        if self
            .pending_incremental_checkpoint
            .lock()
            .map_err(|_| OQLError::generic("pending incremental checkpoint lock poisoned"))?
            .is_some()
        {
            return Ok(true);
        }
        let policy = *self
            .auto_checkpoint_policy
            .lock()
            .map_err(|_| OQLError::generic("checkpoint policy lock poisoned"))?;
        if !policy.enabled() {
            return Ok(false);
        }
        if policy.commits > 0
            && self.commits_since_checkpoint.load(Ordering::Acquire) >= policy.commits
        {
            return Ok(true);
        }
        if policy.wal_bytes == 0 {
            return Ok(false);
        }
        let mut wal_bytes = self
            .filesystem
            .file_size(&self.wal_path)
            .map_err(|error| storage_io("inspect WAL", &self.wal_path, error))?;
        if self.filesystem.exists(&self.sealed_wal_path) {
            wal_bytes = wal_bytes.saturating_add(
                self.filesystem
                    .file_size(&self.sealed_wal_path)
                    .map_err(|error| {
                        storage_io("inspect sealed WAL", &self.sealed_wal_path, error)
                    })?,
            );
        }
        Ok(wal_bytes >= policy.wal_bytes)
    }

    fn record_checkpoint_success(&self, checkpoint: &CheckpointInfo) {
        self.commits_since_checkpoint.store(0, Ordering::Release);
        self.record_checkpoint_success_without_counter_reset(checkpoint);
    }

    fn record_checkpoint_success_without_counter_reset(&self, checkpoint: &CheckpointInfo) {
        if let Ok(mut last_checkpoint) = self.last_checkpoint.lock() {
            *last_checkpoint = Some(checkpoint.clone());
        }
        if let Ok(mut last_error) = self.last_maintenance_error.lock() {
            *last_error = None;
        }
    }

    fn record_maintenance_error(&self, error: String) {
        if let Ok(mut last_error) = self.last_maintenance_error.lock() {
            *last_error = Some(error);
        }
    }
}

impl Drop for RustStore {
    fn drop(&mut self) {
        // With no worker to hear it, as in a browser, the message would
        // wait for room that never comes.
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.maintenance_sender.send(MaintenanceMessage::Shutdown);
        if let Ok(mut worker) = self.maintenance_worker.lock() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

fn maintenance_worker(core: Arc<StoreCore>, receiver: Receiver<MaintenanceMessage>) {
    while let Ok(message) = receiver.recv() {
        match message {
            MaintenanceMessage::Checkpoint => run_scheduled_maintenance(&core),
            MaintenanceMessage::Drain(reply) => {
                run_scheduled_maintenance(&core);
                let _ = reply.send(());
            }
            MaintenanceMessage::Shutdown => break,
        }
    }
}

fn run_scheduled_maintenance(core: &StoreCore) {
    let before = core.checkpoint_sequence.load(Ordering::Acquire);
    if let Err(error) = core.maybe_auto_checkpoint() {
        core.record_maintenance_error(error.to_string());
    }
    // Only if a checkpoint was made. Looking whether one is due is not
    // a reason to publish.
    if core.checkpoint_sequence.load(Ordering::Acquire) != before {
        core.publish_if_asked();
    }
}

impl RustStore {
    /// Open a transaction and return its id.
    ///
    /// The transaction's writes are held apart until it commits: only
    /// statements run while it is bound to their thread see them (see
    /// [`Self::bind_transaction`]). Its reads see what is committed at the
    /// time of each read, plus its own writes. At commit it is refused if a
    /// document it wrote was changed by another commit after it began, so
    /// work computed from a value someone else has since replaced is never
    /// applied. It is not checked for documents it only read.
    pub fn begin_transaction(&self) -> OQLResult<u64> {
        let id = self.next_transaction_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut state = self
                .state
                .write()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let began_after = state.committed_lsn;
            state.conflicts.open.insert(id, began_after);
        }
        self.transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?
            .insert(id, PendingTransaction::default());
        Ok(id)
    }

    /// Make the current thread work in transaction `id` until the returned
    /// guard is dropped: its writes join the transaction and its reads see
    /// them. A server binds a transaction for the length of each statement
    /// a client sends in it.
    pub fn bind_transaction(&self, id: u64) -> OQLResult<TransactionBinding> {
        if !self
            .transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?
            .contains_key(&id)
        {
            return Err(Self::not_open(id));
        }
        let previous = BOUND_TRANSACTION.with(|bound| bound.replace(Some((self.binding_key(), id))));
        Ok(TransactionBinding {
            previous,
            _thread: std::marker::PhantomData,
        })
    }

    /// Commit a transaction: all of its writes, durably, or none of them.
    /// Whatever the outcome, the transaction is over. A refusal for which
    /// [`OQLError::is_transaction_conflict`] holds means another commit got
    /// there first; run the transaction again.
    pub fn commit_transaction(&self, id: u64) -> OQLResult<()> {
        let transaction = self.take_transaction(id)?;
        if transaction.operations.is_empty() {
            return self.close_transaction(id);
        }
        let outcome =
            self.write_batch_for(transaction.operations, Some(id), &transaction.reserved);
        // A successful commit closed it; a refused one still has to be.
        self.close_transaction(id)?;
        outcome.map(|_| ())
    }

    /// Run `work` as one transaction on the current thread and commit it.
    ///
    /// Everything `work` writes through this store commits together or not
    /// at all. When the commit is refused because another commit got there
    /// first, `work` is run again from the start, up to `attempts` times in
    /// all, so it must compute what it writes from what it reads each time.
    /// An error from `work` rolls the transaction back and is returned.
    pub fn in_transaction<T>(
        &self,
        attempts: usize,
        mut work: impl FnMut() -> OQLResult<T>,
    ) -> OQLResult<T> {
        let mut attempt = 1;
        loop {
            let id = self.begin_transaction()?;
            let outcome = {
                let _bound = self.bind_transaction(id)?;
                work()
            };
            let value = match outcome {
                Ok(value) => value,
                Err(error) => {
                    self.rollback_transaction(id)?;
                    return Err(error);
                }
            };
            match self.commit_transaction(id) {
                Ok(()) => return Ok(value),
                Err(error) if error.is_transaction_conflict() && attempt < attempts => {
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Abandon a transaction and everything it wrote.
    pub fn rollback_transaction(&self, id: u64) -> OQLResult<()> {
        self.take_transaction(id)?;
        self.close_transaction(id)
    }

    fn take_transaction(&self, id: u64) -> OQLResult<PendingTransaction> {
        let transaction = self
            .transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?
            .remove(&id)
            .ok_or_else(|| Self::not_open(id))?;
        // A thread still bound to it by `begin_local_transaction` is freed.
        let key = self.binding_key();
        BOUND_TRANSACTION.with(|bound| {
            if bound.get() == Some((key, id)) {
                bound.set(None);
            }
        });
        Ok(transaction)
    }

    fn close_transaction(&self, id: u64) -> OQLResult<()> {
        self.state
            .write()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .conflicts
            .close(id);
        Ok(())
    }

    /// Open a transaction and bind it to the current thread until it is
    /// committed or rolled back: the form a script's `transaction { .. }`
    /// block uses, where one thread runs every statement.
    pub fn begin_local_transaction(&self) -> OQLResult<u64> {
        if self.current_transaction().is_some() {
            return Err(OQLError::generic("a local transaction is already active"));
        }
        let id = self.begin_transaction()?;
        BOUND_TRANSACTION.with(|bound| bound.set(Some((self.binding_key(), id))));
        Ok(id)
    }

    pub fn commit_local_transaction(&self, id: u64) -> OQLResult<()> {
        self.commit_transaction(id)
    }

    pub fn rollback_local_transaction(&self, id: u64) -> OQLResult<()> {
        self.rollback_transaction(id)
    }

    pub fn resolve_index_id_for_payload_id(
        &self,
        schema: &str,
        payload_id: &str,
    ) -> OQLResult<Option<DocumentId>> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let id = payload_id.to_string();
        Ok(lookup_object_ref(&state, schema, &id)?.map(|_| id))
    }

    pub fn scalar_index_manager(
        &self,
    ) -> OQLResult<Option<Arc<crate::index_manager::IndexManager>>> {
        Ok(None)
    }

    fn queue_or_commit(&self, operation: WriteOperation) -> OQLResult<()> {
        self.queue_or_commit_many(vec![operation])
    }

    fn queue_or_commit_many(&self, operations: Vec<WriteOperation>) -> OQLResult<()> {
        if operations.is_empty() {
            return Ok(());
        }
        // Inside a transaction a write waits for the commit; outside one it
        // is its own commit.
        let Some(id) = self.current_transaction() else {
            return self.write_batch(operations).map(|_| ());
        };
        self.transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?
            .get_mut(&id)
            .ok_or_else(|| Self::not_open(id))?
            .operations
            .extend(operations);
        Ok(())
    }

    fn plan_mutations(
        state: &State,
        operations: Vec<WriteOperation>,
        reserved: &ReservedRefs,
    ) -> OQLResult<Vec<Mutation>> {
        let mut collection_ids = state.collection_ids.clone();
        let mut next_collection_id = state.next_collection_id.max(1);
        let mut next_object_ids = state.next_object_ids.clone();
        let mut known_documents = HashMap::<(String, DocumentId), Option<ObjRef>>::new();
        let mut logical_schemas = state.schemas.clone();
        let mut compiled_schemas = state.compiled_schemas.clone();
        let mut current_schema_versions = state.current_schema_versions.clone();
        let mut scalar_indexes = state.scalar_indexes.clone();
        let mut planned_documents: HashMap<ObjRef, Option<Document>> = HashMap::new();
        let mut mutations = Vec::with_capacity(operations.len() * 2);

        for operation in operations {
            match operation {
                WriteOperation::PutDocument(document) => {
                    if let Some(schema) = logical_schemas.get(&document.schema) {
                        schema.validate_document(&document.fields)?;
                    }
                    let key = (document.schema.clone(), document.id.clone());
                    let collection_id = ensure_collection_id(
                        &document.schema,
                        &mut collection_ids,
                        &mut next_collection_id,
                    )?;
                    let existing = known_documents
                        .get(&key)
                        .copied()
                        .unwrap_or(lookup_object_ref(state, &document.schema, &document.id)?);
                    let object_ref = if let Some(existing) = existing {
                        existing
                    } else if let Some(promised) = reserved.get(&key) {
                        // Other documents of this transaction already point
                        // at it under this reference.
                        *promised
                    } else {
                        let next = next_object_ids.entry(collection_id).or_insert(1);
                        let object_id = *next;
                        *next = next.checked_add(1).ok_or_else(|| {
                            OQLError::storage_error("object identifier space exhausted", None)
                        })?;
                        ObjRef::from_parts(collection_id, object_id, 1)
                    };
                    known_documents.insert(key, Some(object_ref));

                    for index in scalar_indexes.get(&document.schema).into_iter().flatten() {
                        validate_index_definition(index)?;
                        let _ = document_index_key(&document, index)?;
                        if index.unique {
                            validate_unique_candidate(
                                state,
                                &planned_documents,
                                index,
                                object_ref,
                                &document,
                            )?;
                        }
                    }
                    planned_documents.insert(object_ref, Some(document.clone()));

                    let previous = current_compiled(
                        &document.schema,
                        &current_schema_versions,
                        &compiled_schemas,
                        collection_id,
                    );
                    let compiled = if let Some(schema) = logical_schemas.get(&document.schema) {
                        match previous {
                            Some(previous) => {
                                CompiledSchema::evolve(previous, schema.fields.keys())?
                            }
                            None => CompiledSchema::first(schema, collection_id)?,
                        }
                    } else {
                        match previous {
                            Some(previous) => {
                                let mut names = previous
                                    .fields
                                    .iter()
                                    .map(|field| field.name.clone())
                                    .collect::<HashSet<_>>();
                                names.extend(document.fields.keys().cloned());
                                CompiledSchema::evolve(previous, names.iter())?
                            }
                            None => CompiledSchema::infer(&document, collection_id)?,
                        }
                    };
                    if previous != Some(&compiled) {
                        compiled_schemas
                            .insert((collection_id, compiled.version), compiled.clone());
                        current_schema_versions.insert(document.schema.clone(), compiled.version);
                        mutations.push(Mutation::PutCompiledSchema(compiled.clone()));
                    }
                    let record = RecordCodec::encode(&document, object_ref, &compiled)?;
                    mutations.push(Mutation::PutDocument { record, object_ref });
                }
                WriteOperation::DeleteDocument { schema, id } => {
                    let key = (schema.clone(), id.clone());
                    let object_ref = known_documents
                        .get(&key)
                        .copied()
                        .unwrap_or(lookup_object_ref(state, &schema, &id)?)
                        .ok_or_else(|| OQLError::generic("document not found for deletion"))?;
                    known_documents.insert(key, None);
                    planned_documents.insert(object_ref, None);
                    mutations.push(Mutation::DeleteDocument {
                        schema,
                        id,
                        object_ref,
                    });
                }
                WriteOperation::PutSchema(schema) => {
                    let collection_id = ensure_collection_id(
                        &schema.name,
                        &mut collection_ids,
                        &mut next_collection_id,
                    )?;
                    let previous = current_compiled(
                        &schema.name,
                        &current_schema_versions,
                        &compiled_schemas,
                        collection_id,
                    );
                    let compiled = match previous {
                        Some(previous) => CompiledSchema::evolve(previous, schema.fields.keys())?,
                        None => CompiledSchema::first(&schema, collection_id)?,
                    };
                    if previous != Some(&compiled) {
                        compiled_schemas
                            .insert((collection_id, compiled.version), compiled.clone());
                        current_schema_versions.insert(schema.name.clone(), compiled.version);
                        mutations.push(Mutation::PutCompiledSchema(compiled));
                    }
                    logical_schemas.insert(schema.name.clone(), schema.clone());
                    mutations.push(Mutation::PutSchema(schema));
                }
                WriteOperation::DeleteSchema { name } => {
                    logical_schemas.remove(&name);
                    mutations.push(Mutation::DeleteSchema(name));
                }
                WriteOperation::PutScalarIndex(index) => {
                    validate_index_definition(&index)?;
                    validate_index_build(state, &planned_documents, &index)?;
                    upsert(&mut scalar_indexes, index.clone());
                    mutations.push(Mutation::PutScalarIndex(index));
                }
                WriteOperation::DeleteScalarIndex { schema, name } => {
                    remove_named(&mut scalar_indexes, &schema, &name);
                    mutations.push(Mutation::DeleteScalarIndex { schema, name });
                }
                WriteOperation::PutVectorIndex(index) => {
                    mutations.push(Mutation::PutVectorIndex(index));
                }
                WriteOperation::DeleteVectorIndex { schema, name } => {
                    mutations.push(Mutation::DeleteVectorIndex { schema, name });
                }
            }
        }
        Ok(mutations)
    }

    /// Write one frame at the end of the WAL. It is in the file when this
    /// returns, and on disk once a sync follows.
    fn append_frame(
        &self,
        wal: &mut Option<Box<dyn FileHandle>>,
        lsn: u64,
        payload: &[u8],
    ) -> OQLResult<()> {
        let current_size =
            self.filesystem
                .file_size(&self.wal_path)
                .map_err(|e| storage_io("inspect WAL", &self.wal_path, e))? as usize;
        let frame_size = (WAL_HEADER_LEN + WAL_SYNCED_POINT_LEN).saturating_add(payload.len());
        if self.max_size_bytes > 0 && current_size.saturating_add(frame_size) > self.max_size_bytes
        {
            return Err(OQLError::storage_error(
                "configured storage size limit reached",
                None,
            ));
        }

        let head = encode_wal_frame_head(
            lsn,
            self.wal_synced_len.load(Ordering::Acquire).min(current_size as u64),
            payload,
        );

        let wal = wal
            .as_mut()
            .ok_or_else(|| OQLError::storage_error("active WAL handle is unavailable", None))?;
        let result = write_all(wal.as_mut(), &head)
            .and_then(|_| write_all(wal.as_mut(), payload))
            .and_then(|_| wal.flush());
        if let Err(error) = result {
            self.poisoned.store(true, Ordering::Release);
            let _ = wal.set_len(current_size as u64);
            let _ = wal.seek(SeekFrom::Start(current_size as u64));
            let _ = wal.sync();
            return Err(storage_io("append WAL batch", &self.wal_path, error));
        }
        Ok(())
    }

    fn recover_checkpoint(
        filesystem: Arc<dyn FileSystem>,
        manifest_paths: &[PathBuf; 2],
        segment_paths: &[PathBuf; 2],
        generation_root_paths: &[PathBuf; 2],
        generation_prefix: &Path,
        wal_delta_paths: &[PathBuf; 2],
        record_delta_paths: &[PathBuf; 2],
        index_delta_paths: &[PathBuf; 2],
        verification: SegmentVerification,
        decoded_budget: Option<usize>,
    ) -> OQLResult<(State, u64)> {
        Self::recover_checkpoint_auditing(
            filesystem,
            manifest_paths,
            segment_paths,
            generation_root_paths,
            generation_prefix,
            wal_delta_paths,
            record_delta_paths,
            index_delta_paths,
            verification,
            decoded_budget,
            true,
        )
    }

    /// [`Self::recover_checkpoint`], with the choice of whether the
    /// indexes, as the generations left them, are audited against the
    /// rows. A database being opened is; the starting state of a
    /// compaction is not, since a compaction builds its indexes from the
    /// checkpoint's own pages and from rows, and reads nothing through
    /// the indexes as the generations left them. The audit looks at every
    /// entry of every index, which a compaction cannot afford each time.
    #[allow(clippy::too_many_arguments)]
    fn recover_checkpoint_auditing(
        filesystem: Arc<dyn FileSystem>,
        manifest_paths: &[PathBuf; 2],
        segment_paths: &[PathBuf; 2],
        generation_root_paths: &[PathBuf; 2],
        generation_prefix: &Path,
        wal_delta_paths: &[PathBuf; 2],
        record_delta_paths: &[PathBuf; 2],
        index_delta_paths: &[PathBuf; 2],
        verification: SegmentVerification,
        decoded_budget: Option<usize>,
        audit_indexes: bool,
    ) -> OQLResult<(State, u64)> {
        let mut manifests = Vec::new();
        for (slot, path) in manifest_paths.iter().enumerate() {
            if !filesystem.exists(path) {
                continue;
            }
            let length = filesystem
                .file_size(path)
                .map_err(|error| storage_io("inspect checkpoint manifest", path, error))?;
            if length == 0 || length < 64 {
                // An incomplete inactive copy is ignored; the other copy remains authoritative.
                continue;
            }
            if length != 64 {
                return Err(OQLError::storage_error(
                    &format!(
                        "checkpoint manifest '{}' has an invalid length",
                        path.display()
                    ),
                    None,
                ));
            }
            let bytes = read_exact_file(filesystem.as_ref(), path)?;
            let manifest = Manifest::decode(&bytes)?;
            if manifest.slot as usize != slot {
                return Err(OQLError::storage_error(
                    "checkpoint manifest is stored in the wrong slot",
                    None,
                ));
            }
            manifests.push(manifest);
        }
        let Some(manifest) = manifests
            .into_iter()
            .max_by_key(|manifest| manifest.sequence)
        else {
            return Ok((
                State {
                    next_collection_id: 1,
                    decoded_budget,
                    decoded_bytes_budget: default_decoded_bytes(),
                    ..State::default()
                },
                0,
            ));
        };
        let segment_path = &segment_paths[manifest.segment_slot as usize];
        if !filesystem.exists(segment_path) {
            return Err(OQLError::storage_error(
                "published checkpoint segment is missing",
                None,
            ));
        }
        let segment_started = Instant::now();
        let image = decode_segment_file(
            filesystem.as_ref(),
            segment_path,
            manifest.segment_len,
            manifest.segment_checksum,
            verification,
        )?;
        tracing::debug!(
            target: "owl_db::storage_engine::recovery",
            elapsed_ms = segment_started.elapsed().as_secs_f64() * 1_000.0,
            "decoded checkpoint segment envelope"
        );
        let base_checkpoint_lsn = image.checkpoint_lsn;
        let base_sequence = image.sequence;
        let generation_root =
            generation_root_for_manifest(filesystem.as_ref(), generation_root_paths, manifest)?;
        if image.sequence > manifest.sequence
            || image.checkpoint_lsn > manifest.checkpoint_lsn
            || (manifest.delta_len == 0
                && generation_root.is_none()
                && (image.sequence != manifest.sequence
                    || image.checkpoint_lsn != manifest.checkpoint_lsn))
        {
            return Err(OQLError::storage_error(
                "checkpoint segment metadata conflicts with its manifest",
                None,
            ));
        }
        let restore_started = Instant::now();
        // The records of the base segment are in its file. With a handle
        // on that file, a database with a budget reads one when it is
        // wanted and never holds them all at once, even while it opens.
        // What is replayed over the base stays in memory, as before, and
        // is found there first.
        let base_records = BaseSegmentHandle::open(
            filesystem.clone(),
            &segment_paths[manifest.segment_slot as usize],
            manifest,
        )
        .ok();
        let mut state = restore_checkpoint(
            image,
            filesystem.clone(),
            decoded_budget,
            base_records.is_some(),
        )?;
        if base_records.is_some() {
            state.checkpoint_records = Some(Arc::new(Mutex::new(CheckpointReadHandles {
                record_root: None,
                generation_record_roots: Vec::new(),
                base_segment: base_records,
            })));
        }
        tracing::debug!(
            target: "owl_db::storage_engine::recovery",
            elapsed_ms = restore_started.elapsed().as_secs_f64() * 1_000.0,
            live_objects = logical_live_count(&state),
            "restored checkpoint records and base indexes"
        );
        let mut rooted_records = state
            .object_records
            .snapshot()
            .records()
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        if audit_indexes && generation_root.is_some() {
            // The state is the compaction's at this point.
            let mut audit = OpenAudit::default();
            for (identity, index) in &state.scalar_index_data {
                if index.is_base_alone() {
                    audit.from_pages.insert(identity.clone());
                    if !audit.base_counts.contains_key(&identity.0) {
                        let count = logical_collection_count(&state, &identity.0);
                        audit.base_counts.insert(identity.0.clone(), count);
                    }
                }
            }
            state.open_audit = Some(audit);
        }
        if let Some(root) = &generation_root {
            if root.base_sequence != base_sequence || root.base_lsn != base_checkpoint_lsn {
                return Err(OQLError::storage_error(
                    "flush generation root conflicts with its base segment",
                    None,
                ));
            }
            for generation in &root.generations {
                let path = flush_generation_path(generation_prefix, generation.sequence);
                if !filesystem.exists(&path) {
                    return Err(OQLError::storage_error(
                        "published flush generation is missing",
                        None,
                    ));
                }
                let bytes = read_exact_file(filesystem.as_ref(), &path)?;
                if bytes.len() as u64 != generation.len
                    || crc32fast::hash(&bytes) != generation.checksum
                {
                    return Err(OQLError::storage_error(
                        "published flush generation does not match its root",
                        None,
                    ));
                }
                let record_path =
                    flush_generation_record_path(generation_prefix, generation.sequence);
                if !filesystem.exists(&record_path) {
                    return Err(OQLError::storage_error(
                        "published flush generation record root is missing",
                        None,
                    ));
                }
                let record_root = RecordRootHandle::open(filesystem.clone(), &record_path)?;
                let record_len = filesystem.file_size(&record_path).map_err(|error| {
                    storage_io("inspect flush generation record root", &record_path, error)
                })?;
                if record_len != generation.record_len
                    || record_root.authority_checksum() != generation.record_checksum
                {
                    return Err(OQLError::storage_error(
                        "published flush generation record root does not match its descriptor",
                        None,
                    ));
                }
                if record_root.base_lsn() != generation.start_lsn - 1
                    || record_root.target_lsn() != generation.end_lsn
                {
                    return Err(OQLError::storage_error(
                        "flush generation record root has inconsistent LSN bounds",
                        None,
                    ));
                }
                let generation_indexed = generation.index_len > 0;
                let index_path =
                    flush_generation_index_path(generation_prefix, generation.sequence);
                if generation_indexed {
                    if !filesystem.exists(&index_path) {
                        return Err(OQLError::storage_error(
                            "published flush generation index root is missing",
                            None,
                        ));
                    }
                    let index_len = filesystem.file_size(&index_path).map_err(|error| {
                        storage_io("inspect flush generation index root", &index_path, error)
                    })?;
                    if index_len != generation.index_len
                        || checksum_file_path(filesystem.as_ref(), &index_path)?
                            != generation.index_checksum
                    {
                        return Err(OQLError::storage_error(
                            "published flush generation index root does not match its descriptor",
                            None,
                        ));
                    }
                }
                let (replayed, valid_length) = Self::recover_with_index_mode(
                    filesystem.as_ref(),
                    &path,
                    state,
                    if generation_indexed {
                        IndexReplayMode::DeferToRoot
                    } else {
                        IndexReplayMode::Maintain
                    },
                    WalTail::Synced,
                )?;
                if valid_length != generation.len || replayed.committed_lsn != generation.end_lsn {
                    return Err(OQLError::storage_error(
                        "flush generation does not reach its declared LSN",
                        None,
                    ));
                }
                state = replayed;
                if generation_indexed {
                    install_and_validate_index_root_file(
                        &mut state,
                        filesystem.as_ref(),
                        &index_path,
                        generation.start_lsn - 1,
                        generation.end_lsn,
                        false,
                    )?;
                }
                mark_scalar_indexes_clean(&mut state)?;
                // What this generation wrote is in its record file. A
                // database with a budget reads it from there from now on,
                // and so holds the records of one generation at a time
                // while it opens, not of all of them.
                if state.bounded() {
                    if let Some(handles) = state.checkpoint_records.clone() {
                        handles
                            .lock()
                            .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
                            .generation_record_roots
                            .insert(0, record_root);
                        state.object_records.mark_clean();
                        // Every record held clean at this point was read
                        // from this generation's file a moment ago, so
                        // none needs looking up there before it is let go.
                        state.object_records.evict_clean();
                    }
                }
            }
        } else if manifest.delta_len > 0 {
            let delta_path = &wal_delta_paths[manifest.slot as usize];
            if !filesystem.exists(delta_path) {
                return Err(OQLError::storage_error(
                    "published checkpoint WAL delta is missing",
                    None,
                ));
            }
            let delta = read_exact_file(filesystem.as_ref(), delta_path)?;
            if delta.len() as u64 != manifest.delta_len
                || crc32fast::hash(&delta) != manifest.delta_checksum
            {
                return Err(OQLError::storage_error(
                    "published checkpoint WAL delta does not match its manifest",
                    None,
                ));
            }
            let (replayed, valid_length) = Self::recover_with_index_mode(
                filesystem.as_ref(),
                delta_path,
                state,
                if manifest.has_index_root {
                    IndexReplayMode::DeferToRoot
                } else {
                    IndexReplayMode::Maintain
                },
                WalTail::Synced,
            )?;
            if valid_length != manifest.delta_len
                || replayed.committed_lsn != manifest.checkpoint_lsn
            {
                return Err(OQLError::storage_error(
                    "checkpoint WAL delta does not reach its manifest LSN",
                    None,
                ));
            }
            state = replayed;
        }
        if logical_live_count(&state) as u64 != manifest.live_objects {
            return Err(OQLError::storage_error(
                "checkpoint live-object count conflicts with its manifest",
                None,
            ));
        }
        if generation_root.is_none() && manifest.has_record_root {
            let root_path = &record_delta_paths[manifest.slot as usize];
            if !filesystem.exists(root_path) {
                return Err(OQLError::storage_error(
                    "published checkpoint record root is missing",
                    None,
                ));
            }
            let root = RecordPageDelta::decode(&read_exact_file(filesystem.as_ref(), root_path)?)?;
            for (object_ref, record) in &root.puts {
                rooted_records.insert(*object_ref, record.clone());
            }
            for object_ref in &root.deletes {
                rooted_records.remove(object_ref);
            }
            let recovered_records = state
                .object_records
                .snapshot()
                .records()
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            if root.base_lsn != base_checkpoint_lsn
                || root.target_lsn != manifest.checkpoint_lsn
                || rooted_records != recovered_records
            {
                return Err(OQLError::storage_error(
                    "published checkpoint record root conflicts with recovered records",
                    None,
                ));
            }
        }
        let mut index_root_changed = false;
        if generation_root.is_none() && manifest.has_index_root {
            let root_path = &index_delta_paths[manifest.slot as usize];
            if !filesystem.exists(root_path) {
                return Err(OQLError::storage_error(
                    "published checkpoint index root is missing",
                    None,
                ));
            }
            index_root_changed = install_and_validate_index_root_file(
                &mut state,
                filesystem.as_ref(),
                root_path,
                base_checkpoint_lsn,
                manifest.checkpoint_lsn,
                true,
            )?;
        }
        if audit_indexes
            && (generation_root.is_some()
                || (generation_root.is_none() && !manifest.has_index_root)
                || index_root_changed)
        {
            let audit_started = Instant::now();
            match state.open_audit.take() {
                Some(audit) => {
                    OPENS_AUDITED_AGAINST_PAGES.fetch_add(1, Ordering::Relaxed);
                    let against_pages = validate_scalar_indexes_against_pages(&state, &audit);
                    if AUDIT_OPENS_BOTH_WAYS.load(Ordering::Relaxed) {
                        let entry_by_entry = validate_runtime_scalar_indexes_structural(&state);
                        if against_pages.is_ok() != entry_by_entry.is_ok() {
                            return Err(OQLError::storage_error(
                                &format!(
                                    "the two audits of recovered indexes disagree: against pages {against_pages:?}, entry by entry {entry_by_entry:?}"
                                ),
                                None,
                            ));
                        }
                    }
                    against_pages?
                }
                None => validate_runtime_scalar_indexes_structural(&state)?,
            }
            tracing::debug!(
                target: "owl_db::storage_engine::recovery",
                elapsed_ms = audit_started.elapsed().as_secs_f64() * 1_000.0,
                indexes = state.scalar_index_data.len(),
                "audited recovered scalar-index coverage"
            );
        }
        state.open_audit = None;
        state.object_records.mark_clean();
        evict_clean_records(&mut state);
        mark_indexes_clean(&mut state)?;
        Ok((state, manifest.sequence))
    }

    fn recover(
        filesystem: &dyn FileSystem,
        path: &Path,
        state: State,
        tail: WalTail,
    ) -> OQLResult<(State, u64)> {
        Self::recover_with_index_mode(filesystem, path, state, IndexReplayMode::Maintain, tail)
    }

    fn recover_with_index_mode(
        filesystem: &dyn FileSystem,
        path: &Path,
        mut state: State,
        index_mode: IndexReplayMode,
        tail: WalTail,
    ) -> OQLResult<(State, u64)> {
        state.next_collection_id = state.next_collection_id.max(1);
        if !filesystem.exists(path) || filesystem.file_size(path).unwrap_or(0) == 0 {
            return Ok((state, 0));
        }
        let mut file = filesystem
            .open(path)
            .map_err(|e| storage_io("open WAL for recovery", path, e))?;

        let mut valid_len = 0u64;
        let mut previous_wal_lsn = None;
        loop {
            let Some(frame) = read_wal_frame(file.as_mut(), path, valid_len, tail)? else {
                break;
            };
            let (header_lsn, frame_len) = (frame.lsn, frame.len);
            let batch: WalBatch = bincode::deserialize(&frame.payload).map_err(|e| {
                OQLError::serialization_error(&format!("cannot decode WAL batch: {e}"), None)
            })?;
            if batch.lsn != header_lsn {
                return Err(OQLError::storage_error(
                    "WAL header and batch sequence disagree",
                    None,
                ));
            }
            if previous_wal_lsn
                .is_some_and(|previous: u64| previous.checked_add(1) != Some(batch.lsn))
            {
                return Err(OQLError::storage_error(
                    "WAL sequence is not contiguous",
                    None,
                ));
            }
            previous_wal_lsn = Some(batch.lsn);
            if batch.lsn > state.committed_lsn {
                if state.committed_lsn.checked_add(1) != Some(batch.lsn) {
                    return Err(OQLError::storage_error(
                        "WAL does not continue from checkpoint LSN",
                        None,
                    ));
                }
                Self::apply_batch_with_index_mode(&mut state, batch, index_mode)?;
            }
            valid_len = valid_len.saturating_add(frame_len);
        }
        Ok((state, valid_len))
    }

    fn apply_batch(state: &mut State, batch: WalBatch) -> OQLResult<()> {
        Self::apply_batch_with_index_mode(state, batch, IndexReplayMode::Maintain)
    }

    fn apply_batch_with_index_mode(
        state: &mut State,
        batch: WalBatch,
        index_mode: IndexReplayMode,
    ) -> OQLResult<()> {
        Self::apply_mutations(state, batch, index_mode)?;
        enforce_decoded_budget(state)
    }

    fn apply_mutations(
        state: &mut State,
        batch: WalBatch,
        index_mode: IndexReplayMode,
    ) -> OQLResult<()> {
        let batch_lsn = batch.lsn;
        for mutation in batch.mutations {
            match mutation {
                Mutation::PutCompiledSchema(compiled) => {
                    if let Some(existing) = state.collection_ids.get(&compiled.name) {
                        if *existing != compiled.collection_id {
                            return Err(OQLError::storage_error(
                                "compiled schema collection ID conflicts with catalog",
                                None,
                            ));
                        }
                    }
                    let key = (compiled.collection_id, compiled.version);
                    if let Some(existing) = state.compiled_schemas.get(&key) {
                        if existing != &compiled {
                            return Err(OQLError::storage_error(
                                "compiled schema version conflicts with catalog",
                                None,
                            ));
                        }
                    }
                    state
                        .collection_ids
                        .insert(compiled.name.clone(), compiled.collection_id);
                    state.next_collection_id = state
                        .next_collection_id
                        .max(compiled.collection_id.saturating_add(1));
                    state
                        .current_schema_versions
                        .insert(compiled.name.clone(), compiled.version);
                    state.compiled_schemas.insert(key, compiled);
                }
                Mutation::PutDocument { record, object_ref } => {
                    let header = RecordCodec::header(&record)?;
                    if header.object_ref != object_ref {
                        return Err(OQLError::storage_error(
                            "record object identity conflicts with WAL mutation",
                            None,
                        ));
                    }
                    let compiled = state
                        .compiled_schemas
                        .get(&(object_ref.collection_id(), header.schema_version))
                        .ok_or_else(|| {
                            OQLError::storage_error(
                                "record references an unknown compiled schema",
                                None,
                            )
                        })?;
                    let document = RecordCodec::decode(&record, compiled)?;
                    let next_object_id =
                        object_ref.object_id().checked_add(1).ok_or_else(|| {
                            OQLError::storage_error("object identifier space exhausted", None)
                        })?;
                    state
                        .next_object_ids
                        .entry(object_ref.collection_id())
                        .and_modify(|next| *next = (*next).max(next_object_id))
                        .or_insert(next_object_id);
                    let previous = logical_document(state, object_ref)?;
                    if let Some(audit) = &mut state.open_audit {
                        audit.touched.entry(object_ref).or_insert(previous.is_some());
                    }
                    if index_mode == IndexReplayMode::Maintain {
                        update_document_indexes(
                            state,
                            previous.as_deref(),
                            Some(&document),
                            object_ref,
                        )?;
                    }
                    if index_mode == IndexReplayMode::Maintain {
                        update_reverse_reference_index(
                            state,
                            previous.as_deref(),
                            Some(&document),
                            object_ref,
                        )?;
                    }
                    keep_vector_indexes(state, &document.schema, object_ref, Some(&document), batch_lsn);
                    let delta_schema = document.schema.clone();
                    let delta_id = document.id.clone();
                    state.collection_stamps.insert(delta_schema.clone(), batch_lsn);
                    state.forget_count(&delta_schema);
                    state
                        .collection_rows
                        .entry(document.schema.clone())
                        .or_default()
                        .insert(document.id.clone(), object_ref);
                    state
                        .object_directory
                        .insert(object_ref, Arc::new(document));
                    state.note_decoded(object_ref);
                    state.typed_row_locations.remove(&object_ref)?;
                    state
                        .object_schema_versions
                        .insert(object_ref, header.schema_version);
                    state.deleted_since_checkpoint.remove(&object_ref);
                    state.object_records.insert(object_ref, record);
                    if let Some((_, written)) = &mut state.written_since_snapshot {
                        written.push((delta_schema.clone(), delta_id.clone(), object_ref));
                    }
                    if state.has_rows_on_file() {
                        state
                            .typed_delta_rows
                            .entry(delta_schema)
                            .or_default()
                            .insert(delta_id, Some(object_ref));
                    }
                }
                Mutation::DeleteDocument {
                    schema,
                    id,
                    object_ref,
                } => {
                    let previous = logical_document(state, object_ref)?;
                    if let Some(audit) = &mut state.open_audit {
                        audit.touched.entry(object_ref).or_insert(previous.is_some());
                    }
                    if index_mode == IndexReplayMode::Maintain {
                        update_document_indexes(state, previous.as_deref(), None, object_ref)?;
                    }
                    if index_mode == IndexReplayMode::Maintain {
                        update_reverse_reference_index(
                            state,
                            previous.as_deref(),
                            None,
                            object_ref,
                        )?;
                        remove_reverse_reference_target(state, object_ref)?;
                    }
                    keep_vector_indexes(state, &schema, object_ref, None, batch_lsn);
                    state.collection_stamps.insert(schema.clone(), batch_lsn);
                    state.forget_count(&schema);
                    if let Some(rows) = state.collection_rows.get_mut(&schema) {
                        rows.remove(&id);
                        if rows.is_empty() {
                            state.collection_rows.remove(&schema);
                        }
                    }
                    state.object_directory.remove(&object_ref);
                    state.typed_row_locations.remove(&object_ref)?;
                    state.object_schema_versions.remove(&object_ref);
                    state.deleted_since_checkpoint.insert(object_ref);
                    state.object_records.remove_known(&object_ref);
                    if let Some((_, written)) = &mut state.written_since_snapshot {
                        written.push((schema.clone(), id.clone(), object_ref));
                    }
                    if state.has_rows_on_file() {
                        state
                            .typed_delta_rows
                            .entry(schema)
                            .or_default()
                            .insert(id, None);
                    }
                }
                Mutation::PutSchema(schema) => {
                    state.catalog_stamp = batch_lsn;
                    state.schemas.insert(schema.name.clone(), schema);
                }
                Mutation::DeleteSchema(name) => {
                    state.catalog_stamp = batch_lsn;
                    state.schemas.remove(&name);
                }
                Mutation::PutScalarIndex(index) => {
                    let identity = (index.schema.clone(), index.name.clone());
                    let data = if index_mode == IndexReplayMode::Maintain {
                        build_scalar_index(state, &index)?
                    } else {
                        validate_index_definition(&index)?;
                        BTreeIndex::create(16, 0)
                            .map_err(|error| OQLError::storage_error(&error, None))?
                    };
                    state
                        .scalar_index_data
                        .insert(identity.clone(), Arc::new(data));
                    state.scalar_index_deletes.remove(&identity);
                    if let Some(audit) = &mut state.open_audit {
                        audit.remade.insert(identity.clone());
                    }
                    state.scalar_index_resets.insert(identity);
                    upsert(&mut state.scalar_indexes, index);
                }
                Mutation::DeleteScalarIndex { schema, name } => {
                    let identity = (schema.clone(), name.clone());
                    remove_named(&mut state.scalar_indexes, &schema, &name);
                    state.scalar_index_data.remove(&identity);
                    state.scalar_index_resets.remove(&identity);
                    if let Some(audit) = &mut state.open_audit {
                        audit.remade.insert(identity.clone());
                    }
                    state.scalar_index_deletes.insert(identity);
                }
                Mutation::PutVectorIndex(index) => {
                    forget_vector_indexes(state, &index.schema);
                    upsert(&mut state.vector_indexes, index)
                }
                Mutation::DeleteVectorIndex { schema, name } => {
                    forget_vector_indexes(state, &schema);
                    remove_named(&mut state.vector_indexes, &schema, &name);
                }
            }
        }
        state.committed_lsn = batch.lsn;
        Ok(())
    }

    /// Perform every fallible record/catalog check before the WAL durability
    /// boundary without cloning document data or the object directory.
    fn validate_batch(state: &State, batch: &WalBatch) -> OQLResult<()> {
        let mut compiled_schemas = state.compiled_schemas.clone();
        let mut collection_ids = state.collection_ids.clone();
        for mutation in &batch.mutations {
            match mutation {
                Mutation::PutCompiledSchema(compiled) => {
                    if collection_ids
                        .get(&compiled.name)
                        .is_some_and(|id| *id != compiled.collection_id)
                    {
                        return Err(OQLError::storage_error(
                            "compiled schema collection ID conflicts with catalog",
                            None,
                        ));
                    }
                    let key = (compiled.collection_id, compiled.version);
                    if compiled_schemas
                        .get(&key)
                        .is_some_and(|existing| existing != compiled)
                    {
                        return Err(OQLError::storage_error(
                            "compiled schema version conflicts with catalog",
                            None,
                        ));
                    }
                    collection_ids.insert(compiled.name.clone(), compiled.collection_id);
                    compiled_schemas.insert(key, compiled.clone());
                }
                Mutation::PutDocument { record, object_ref } => {
                    let header = RecordCodec::header(record)?;
                    if header.object_ref != *object_ref {
                        return Err(OQLError::storage_error(
                            "record object identity conflicts with WAL mutation",
                            None,
                        ));
                    }
                    let compiled = compiled_schemas
                        .get(&(object_ref.collection_id(), header.schema_version))
                        .ok_or_else(|| {
                            OQLError::storage_error(
                                "record references an unknown compiled schema",
                                None,
                            )
                        })?;
                    let _ = RecordCodec::decode(record, compiled)?;
                    object_ref.object_id().checked_add(1).ok_or_else(|| {
                        OQLError::storage_error("object identifier space exhausted", None)
                    })?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn replay_wal_for_compaction(
    filesystem: Arc<dyn FileSystem>,
    path: &Path,
    state: &mut State,
    run_prefix: PathBuf,
    run_manifest: PathBuf,
) -> OQLResult<(CompactionPlan, u64)> {
    let mut overlay = CompactionOverlay::new(
        filesystem.clone(),
        run_prefix,
        run_manifest,
        COMPACTION_RUN_TARGET_BYTES,
    );
    state.next_collection_id = state.next_collection_id.max(1);
    if !filesystem.exists(path) || filesystem.file_size(path).unwrap_or(0) == 0 {
        return Ok((overlay.finish()?, 0));
    }
    let mut file = filesystem
        .open(path)
        .map_err(|error| storage_io("open WAL for compaction", path, error))?;
    let mut valid_len = 0u64;
    let mut previous_wal_lsn = None;
    loop {
        let Some(frame) = read_wal_frame(file.as_mut(), path, valid_len, WalTail::Synced)? else {
            break;
        };
        let (header_lsn, frame_len) = (frame.lsn, frame.len);
        let batch: WalBatch = bincode::deserialize(&frame.payload).map_err(|error| {
            OQLError::serialization_error(
                &format!("cannot decode WAL batch for compaction: {error}"),
                None,
            )
        })?;
        if batch.lsn != header_lsn
            || previous_wal_lsn
                .is_some_and(|previous: u64| previous.checked_add(1) != Some(batch.lsn))
        {
            return Err(OQLError::storage_error(
                "WAL compaction sequence is not contiguous",
                None,
            ));
        }
        previous_wal_lsn = Some(batch.lsn);
        if batch.lsn > state.committed_lsn {
            if state.committed_lsn.checked_add(1) != Some(batch.lsn) {
                return Err(OQLError::storage_error(
                    "WAL does not continue from checkpoint LSN",
                    None,
                ));
            }
            apply_compaction_batch(state, &mut overlay, batch)?;
        }
        valid_len = valid_len.saturating_add(frame_len);
    }
    Ok((overlay.finish()?, valid_len))
}

fn apply_compaction_batch(
    state: &mut State,
    overlay: &mut CompactionOverlay,
    batch: WalBatch,
) -> OQLResult<()> {
    let lsn = batch.lsn;
    for mutation in batch.mutations {
        match mutation {
            Mutation::PutCompiledSchema(compiled) => {
                if state
                    .collection_ids
                    .get(&compiled.name)
                    .is_some_and(|id| *id != compiled.collection_id)
                    || state
                        .compiled_schemas
                        .get(&(compiled.collection_id, compiled.version))
                        .is_some_and(|existing| existing != &compiled)
                {
                    return Err(OQLError::storage_error(
                        "compiled schema conflicts during compaction",
                        None,
                    ));
                }
                state
                    .collection_ids
                    .insert(compiled.name.clone(), compiled.collection_id);
                state.next_collection_id = state
                    .next_collection_id
                    .max(compiled.collection_id.saturating_add(1));
                state
                    .current_schema_versions
                    .insert(compiled.name.clone(), compiled.version);
                state
                    .compiled_schemas
                    .insert((compiled.collection_id, compiled.version), compiled);
            }
            Mutation::PutDocument { record, object_ref } => {
                let header = RecordCodec::header(&record)?;
                if header.object_ref != object_ref {
                    return Err(OQLError::storage_error(
                        "record identity conflicts during compaction",
                        None,
                    ));
                }
                let compiled = state
                    .compiled_schemas
                    .get(&(object_ref.collection_id(), header.schema_version))
                    .ok_or_else(|| {
                        OQLError::storage_error(
                            "record references an unknown compacted schema",
                            None,
                        )
                    })?;
                let id = RecordCodec::trusted_view(&record, compiled)?
                    .external_id()?
                    .to_string();
                state
                    .next_object_ids
                    .entry(object_ref.collection_id())
                    .and_modify(|next| {
                        *next = (*next).max(object_ref.object_id().saturating_add(1))
                    })
                    .or_insert_with(|| object_ref.object_id().saturating_add(1));
                overlay.put(compiled.name.clone(), id, object_ref, record)?;
            }
            Mutation::DeleteDocument {
                schema,
                id,
                object_ref,
            } => overlay.delete(schema, id, object_ref)?,
            Mutation::PutSchema(schema) => {
                state.schemas.insert(schema.name.clone(), schema);
            }
            Mutation::DeleteSchema(name) => {
                state.schemas.remove(&name);
            }
            Mutation::PutScalarIndex(index) => {
                validate_index_definition(&index)?;
                upsert(&mut state.scalar_indexes, index);
            }
            Mutation::DeleteScalarIndex { schema, name } => {
                remove_named(&mut state.scalar_indexes, &schema, &name);
            }
            Mutation::PutVectorIndex(index) => upsert(&mut state.vector_indexes, index),
            Mutation::DeleteVectorIndex { schema, name } => {
                remove_named(&mut state.vector_indexes, &schema, &name);
            }
        }
    }
    state.committed_lsn = lsn;
    state.catalog_stamp = lsn;
    Ok(())
}

fn take_index_delta(
    state: &mut State,
    base_lsn: u64,
    target_lsn: u64,
) -> OQLResult<IndexRootDelta> {
    let resets = std::mem::take(&mut state.scalar_index_resets);
    let deletes = std::mem::take(&mut state.scalar_index_deletes);
    let mut indexes = Vec::new();
    for (identity, index) in &state.scalar_index_data {
        let changes = index
            .take_delta()
            .map_err(|error| OQLError::storage_error(&error, None))?;
        let mode = if resets.contains(identity) {
            IndexDeltaMode::Reset
        } else {
            IndexDeltaMode::Patch
        };
        if mode == IndexDeltaMode::Reset || !changes.is_empty() {
            indexes.push(IndexDelta {
                schema: identity.0.clone(),
                name: identity.1.clone(),
                mode,
                changes,
            });
        }
    }
    for (schema, name) in deletes {
        indexes.push(IndexDelta {
            schema,
            name,
            mode: IndexDeltaMode::Delete,
            changes: Vec::new(),
        });
    }
    let reverse_changes = state
        .reverse_reference_index
        .take_delta()
        .map_err(|error| OQLError::storage_error(&error, None))?;
    if !reverse_changes.is_empty() {
        indexes.push(IndexDelta {
            schema: REVERSE_INDEX_SCHEMA.into(),
            name: REVERSE_INDEX_NAME.into(),
            mode: IndexDeltaMode::Patch,
            changes: reverse_changes,
        });
    }
    indexes.sort_by(|left, right| (&left.schema, &left.name).cmp(&(&right.schema, &right.name)));
    Ok(IndexRootDelta {
        base_lsn,
        target_lsn,
        indexes,
    })
}

fn install_and_validate_index_root_file(
    state: &mut State,
    filesystem: &dyn FileSystem,
    path: &Path,
    expected_base_lsn: u64,
    expected_target_lsn: u64,
    clear_existing_overlay: bool,
) -> OQLResult<bool> {
    if clear_existing_overlay {
        for index in state.scalar_index_data.values() {
            index
                .clear_overlay()
                .map_err(|error| OQLError::storage_error(&error, None))?;
        }
    }
    let mut modes = HashMap::new();
    let metadata = visit_index_root_file(filesystem, path, |page| {
        let identity = (page.schema.clone(), page.name.clone());
        if page.first {
            modes.insert(identity.clone(), page.mode);
        }
        match page.mode {
            IndexDeltaMode::Delete => {
                if identity.0 == REVERSE_INDEX_SCHEMA && identity.1 == REVERSE_INDEX_NAME {
                    return Err(OQLError::storage_error(
                        "OWLI cannot delete the reverse-reference index",
                        None,
                    ));
                }
                if state.scalar_index_data.contains_key(&identity) {
                    return Err(OQLError::storage_error(
                        "OWLI deletes an index present in the WAL-replayed catalog",
                        None,
                    ));
                }
            }
            IndexDeltaMode::Patch | IndexDeltaMode::Reset => {
                if identity.0 == REVERSE_INDEX_SCHEMA && identity.1 == REVERSE_INDEX_NAME {
                    if page.mode == IndexDeltaMode::Reset && page.first {
                        state
                            .reverse_reference_index
                            .clear_overlay()
                            .map_err(|error| OQLError::storage_error(&error, None))?;
                    }
                    for (key, postings) in page.changes {
                        state
                            .reverse_reference_index
                            .install_overlay_key(key, postings)
                            .map_err(|error| OQLError::storage_error(&error, None))?;
                    }
                    return Ok(());
                }
                let index = state.scalar_index_data.get(&identity).ok_or_else(|| {
                    OQLError::storage_error(
                        "OWLI changes an index absent from the WAL-replayed catalog",
                        None,
                    )
                })?;
                for (key, postings) in page.changes {
                    index
                        .install_overlay_key(key, postings)
                        .map_err(|error| OQLError::storage_error(&error, None))?;
                }
            }
        }
        Ok(())
    })?;
    if metadata.base_lsn != expected_base_lsn || metadata.target_lsn != expected_target_lsn {
        return Err(OQLError::storage_error(
            "published checkpoint index root has the wrong LSN range",
            None,
        ));
    }
    for identity in &state.scalar_index_resets {
        if modes.get(identity) != Some(&IndexDeltaMode::Reset) {
            return Err(OQLError::storage_error(
                "OWLI is missing a WAL-replayed index reset",
                None,
            ));
        }
    }
    for identity in &state.scalar_index_deletes {
        if modes.get(identity) != Some(&IndexDeltaMode::Delete) {
            return Err(OQLError::storage_error(
                "OWLI is missing a WAL-replayed index deletion",
                None,
            ));
        }
    }
    Ok(!modes.is_empty())
}

fn validate_runtime_scalar_indexes(state: &State) -> OQLResult<()> {
    let definitions = state
        .scalar_indexes
        .values()
        .flatten()
        .map(|definition| {
            (
                (definition.schema.clone(), definition.name.clone()),
                definition,
            )
        })
        .collect::<HashMap<_, _>>();
    if definitions.len() != state.scalar_index_data.len() {
        return Err(OQLError::storage_error(
            "runtime scalar indexes do not match their recovered catalog",
            None,
        ));
    }
    for (identity, definition) in definitions {
        let index = state.scalar_index_data.get(&identity).ok_or_else(|| {
            OQLError::storage_error("recovered scalar index has no runtime handle", None)
        })?;
        let mut posting_count = 0usize;
        index
            .visit_entries(&mut |key, postings| {
                if definition.unique && postings.len() > 1 {
                    return Err("recovered unique index has multiple postings".into());
                }
                for object_ref in postings {
                    let document = logical_document(state, *object_ref)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "recovered index references a missing object".to_string())?;
                    let actual = document_index_key(&document, definition)
                        .map_err(|error| error.to_string())?;
                    if actual.as_slice() != key {
                        return Err("recovered index key conflicts with its object".into());
                    }
                }
                posting_count = posting_count.saturating_add(postings.len());
                Ok(())
            })
            .map_err(|error| OQLError::storage_error(&error, None))?;
        let expected = logical_collection_count(state, &definition.schema);
        if posting_count != expected {
            return Err(OQLError::storage_error(
                "recovered scalar index does not cover every live object exactly once",
                None,
            ));
        }
    }
    Ok(())
}

/// The declared indexes with what holds each, having checked that the
/// two lists agree.
fn recovered_scalar_indexes(
    state: &State,
) -> OQLResult<Vec<(&ScalarIndexDefinition, &Arc<BTreeIndex>)>> {
    let definitions = state.scalar_indexes.values().flatten().collect::<Vec<_>>();
    if definitions.len() != state.scalar_index_data.len() {
        return Err(OQLError::storage_error(
            "runtime scalar indexes do not match their recovered catalog",
            None,
        ));
    }
    definitions
        .into_iter()
        .map(|definition| {
            let index = state
                .scalar_index_data
                .get(&(definition.schema.clone(), definition.name.clone()))
                .ok_or_else(|| {
                    OQLError::storage_error("recovered scalar index has no runtime handle", None)
                })?;
            Ok((definition, index))
        })
        .collect()
}

fn validate_runtime_scalar_indexes_structural(state: &State) -> OQLResult<()> {
    for (definition, index) in recovered_scalar_indexes(state)? {
        validate_scalar_index_structural(state, definition, index)?;
    }
    Ok(())
}

/// Check, entry by entry, that an index names every row of its collection
/// once and nothing else.
fn validate_scalar_index_structural(
    state: &State,
    definition: &ScalarIndexDefinition,
    index: &BTreeIndex,
) -> OQLResult<()> {
    let mut referenced = HashSet::new();
    // A row's reference names its collection, so the row need not be
    // read to know it is one of this index's.
    let collection = state.collection_ids.get(&definition.schema).copied();
    index
        .visit_entries(&mut |_key, postings| {
            if definition.unique && postings.len() > 1 {
                return Err("recovered unique index has multiple postings".into());
            }
            for object_ref in postings {
                if Some(object_ref.collection_id()) != collection
                    || !object_is_live(state, *object_ref).map_err(|error| error.to_string())?
                {
                    return Err(
                        "recovered index references an object in the wrong collection".into(),
                    );
                }
                if !referenced.insert(*object_ref) {
                    return Err("recovered index references one object more than once".into());
                }
            }
            Ok(())
        })
        .map_err(|error| OQLError::storage_error(&error, None))?;
    let expected = logical_collection_count(state, &definition.schema);
    if referenced.len() != expected {
        return Err(OQLError::storage_error(
            "recovered scalar index does not cover every live object exactly once",
            None,
        ));
    }
    Ok(())
}

/// Whether a database being opened also audits its indexes entry by
/// entry where it would audit them against their pages, and refuses to
/// open if the two disagree.
static AUDIT_OPENS_BOTH_WAYS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How many openings in this process audited their indexes against pages.
static OPENS_AUDITED_AGAINST_PAGES: AtomicU64 = AtomicU64::new(0);

impl RustStore {
    /// See [`OPENS_AUDITED_AGAINST_PAGES`].
    #[doc(hidden)]
    pub fn opens_audited_against_pages() -> u64 {
        OPENS_AUDITED_AGAINST_PAGES.load(Ordering::Relaxed)
    }

    /// See [`AUDIT_OPENS_BOTH_WAYS`]. For tests.
    #[doc(hidden)]
    pub fn audit_opens_both_ways(both: bool) {
        AUDIT_OPENS_BOTH_WAYS.store(both, Ordering::Relaxed);
    }
}

/// Check that each index names every row of its collection once and
/// nothing else, without looking at every entry.
///
/// A compaction's pages name each of its rows once: that is taken as
/// given, as it is when a compacted database is opened. What the
/// generations since laid over those pages is then all there is to
/// check, and the checks below cost in proportion to it.
///
/// Call the rows of the compaction B and the rows now N. Under each key
/// laid over the pages, a reference the pages hold and the index no
/// longer answers is taken out, and one the index answers and the pages
/// do not is put in. The index then holds B's references less those taken
/// out, and those put in. It is checked that
///
/// 1. those put in are rows of N, each put in once;
/// 2. each taken out is no longer a row, or is put in again;
/// 3. B less those taken out, with those put in, is as many as N;
/// 4. every row of N a generation wrote that is not in B is put in.
///
/// By 2 no row of both B and N is lost, and by 4 no row of N alone is, so
/// every row of N is named. By 3 the index names as many as N holds, so
/// it names nothing twice and nothing else.
///
/// An index a generation made or dropped, or one that did not answer
/// from pages alone, is checked entry by entry.
fn validate_scalar_indexes_against_pages(state: &State, audit: &OpenAudit) -> OQLResult<()> {
    for (definition, index) in recovered_scalar_indexes(state)? {
        let identity = (definition.schema.clone(), definition.name.clone());
        let base_count = audit.base_counts.get(&definition.schema).copied();
        let (true, false, Some(base_count)) = (
            audit.from_pages.contains(&identity),
            audit.remade.contains(&identity),
            base_count,
        ) else {
            validate_scalar_index_structural(state, definition, index)?;
            continue;
        };
        let collection = state.collection_ids.get(&definition.schema).copied();
        let mut put_in = HashSet::new();
        let mut taken_out = Vec::new();
        index
            .visit_changes_to_base(&mut |before, now| {
                if definition.unique && now.len() > 1 {
                    return Err("recovered unique index has multiple postings".into());
                }
                // A key can hold a great many rows, so neither list is
                // searched once for each row of the other.
                let before_held = before.iter().copied().collect::<HashSet<_>>();
                let now_held = now.iter().copied().collect::<HashSet<_>>();
                for object_ref in now {
                    if before_held.contains(object_ref) {
                        continue;
                    }
                    if Some(object_ref.collection_id()) != collection
                        || !object_is_live(state, *object_ref).map_err(|error| error.to_string())?
                    {
                        return Err(
                            "recovered index references an object in the wrong collection".into(),
                        );
                    }
                    if !put_in.insert(*object_ref) {
                        return Err("recovered index references one object more than once".into());
                    }
                }
                taken_out.extend(before.iter().filter(|object_ref| !now_held.contains(object_ref)).copied());
                Ok(())
            })
            .map_err(|error| OQLError::storage_error(&error, None))?;
        for object_ref in &taken_out {
            if !put_in.contains(object_ref) && object_is_live(state, *object_ref)? {
                return Err(OQLError::storage_error(
                    "recovered scalar index no longer names a live object",
                    None,
                ));
            }
        }
        let expected = logical_collection_count(state, &definition.schema);
        if base_count + put_in.len() != expected + taken_out.len() {
            return Err(OQLError::storage_error(
                "recovered scalar index does not cover every live object exactly once",
                None,
            ));
        }
        for (object_ref, in_base) in &audit.touched {
            if *in_base
                || Some(object_ref.collection_id()) != collection
                || put_in.contains(object_ref)
            {
                continue;
            }
            if object_is_live(state, *object_ref)? {
                return Err(OQLError::storage_error(
                    "recovered scalar index does not name a row written since the compaction",
                    None,
                ));
            }
        }
    }
    Ok(())
}

fn mark_indexes_clean(state: &mut State) -> OQLResult<()> {
    mark_scalar_indexes_clean(state)?;
    state
        .reverse_reference_index
        .mark_clean()
        .map_err(|error| OQLError::storage_error(&error, None))?;
    Ok(())
}

fn mark_scalar_indexes_clean(state: &mut State) -> OQLResult<()> {
    for index in state.scalar_index_data.values() {
        index
            .mark_clean()
            .map_err(|error| OQLError::storage_error(&error, None))?;
    }
    state.scalar_index_resets.clear();
    state.scalar_index_deletes.clear();
    Ok(())
}

fn checkpoint_image_with_records(
    state: &State,
    sequence: u64,
    mut records: Vec<(ObjRef, Arc<[u8]>)>,
) -> OQLResult<CheckpointImage> {
    let mut collection_ids = state
        .collection_ids
        .iter()
        .map(|(name, id)| (name.clone(), *id))
        .collect::<Vec<_>>();
    collection_ids.sort_by(|left, right| left.0.cmp(&right.0));
    let mut next_object_ids = state
        .next_object_ids
        .iter()
        .map(|(collection, next)| (*collection, *next))
        .collect::<Vec<_>>();
    next_object_ids.sort_by_key(|(collection, _)| *collection);
    let mut schemas = state.schemas.values().cloned().collect::<Vec<_>>();
    schemas.sort_by(|left, right| left.name.cmp(&right.name));
    let mut compiled_schemas = state.compiled_schemas.values().cloned().collect::<Vec<_>>();
    compiled_schemas.sort_by_key(|schema| (schema.collection_id, schema.version));
    let mut current_schema_versions = state
        .current_schema_versions
        .iter()
        .map(|(name, version)| (name.clone(), *version))
        .collect::<Vec<_>>();
    current_schema_versions.sort_by(|left, right| left.0.cmp(&right.0));
    let mut scalar_indexes = state
        .scalar_indexes
        .iter()
        .map(|(schema, indexes)| (schema.clone(), indexes.clone()))
        .collect::<Vec<_>>();
    scalar_indexes.sort_by(|left, right| left.0.cmp(&right.0));
    let mut vector_indexes = state
        .vector_indexes
        .iter()
        .map(|(schema, indexes)| (schema.clone(), indexes.clone()))
        .collect::<Vec<_>>();
    vector_indexes.sort_by(|left, right| left.0.cmp(&right.0));
    let indexes_started = Instant::now();
    let mut indexes = Vec::new();
    for definitions in state.scalar_indexes.values() {
        for definition in definitions {
            let data = state
                .scalar_index_data
                .get(&(definition.schema.clone(), definition.name.clone()))
                .ok_or_else(|| {
                    OQLError::storage_error("scalar index catalog has no checkpoint data", None)
                })?;
            indexes.push(CheckpointIndex {
                spooled: None,
                schema: definition.schema.clone(),
                name: definition.name.clone(),
                unique: definition.unique,
                entries: data
                    .snapshot()
                    .map_err(|error| OQLError::storage_error(&error, None))?,
            });
        }
    }
    indexes.push(CheckpointIndex {
        spooled: None,
        schema: REVERSE_INDEX_SCHEMA.into(),
        name: REVERSE_INDEX_NAME.into(),
        unique: false,
        entries: state
            .reverse_reference_index
            .snapshot()
            .map_err(|error| OQLError::storage_error(&error, None))?,
    });
    tracing::debug!(
        target: "owl_db::storage_engine::checkpoint",
        elapsed_ms = indexes_started.elapsed().as_secs_f64() * 1_000.0,
        indexes = indexes.len(),
        "snapshotted checkpoint indexes"
    );
    let typed_started = Instant::now();
    let (typed_segments, authoritative_rows) = build_typed_checkpoint_segments(state, &records)?;
    tracing::debug!(
        target: "owl_db::storage_engine::checkpoint",
        elapsed_ms = typed_started.elapsed().as_secs_f64() * 1_000.0,
        segments = typed_segments.len(),
        authoritative_rows = authoritative_rows.len(),
        "built typed checkpoint segments"
    );
    records.retain(|(object_ref, _)| !authoritative_rows.contains(object_ref));
    // The rows left outside the column segments, by collection and id.
    let mut id_entries = Vec::with_capacity(records.len());
    for (object_ref, record) in &records {
        let header = RecordCodec::trusted_header(record)?;
        let compiled = state
            .compiled_schemas
            .get(&(object_ref.collection_id(), header.schema_version))
            .ok_or_else(|| {
                OQLError::storage_error("checkpoint record has no compiled schema", None)
            })?;
        let id = RecordCodec::trusted_view(record, compiled)?.external_id()?;
        id_entries.push((id_index_key(object_ref.collection_id(), id), vec![*object_ref]));
    }
    id_entries.sort_by(|left, right| left.0.cmp(&right.0));
    indexes.push(CheckpointIndex {
        spooled: None,
        schema: REVERSE_INDEX_SCHEMA.into(),
        name: ID_INDEX_NAME.into(),
        unique: true,
        entries: id_entries,
    });
    Ok(CheckpointImage {
        segment_spool: None,
        record_spool: None,
        sequence,
        checkpoint_lsn: state.committed_lsn,
        metadata: CheckpointMetadata {
            collection_ids,
            next_collection_id: state.next_collection_id,
            next_object_ids,
            schemas,
            compiled_schemas,
            current_schema_versions,
            scalar_indexes,
            vector_indexes,
        },
        records,
        indexes,
        typed_segments,
    })
}

struct CompactionRow {
    object_ref: ObjRef,
    /// The row as a document. Absent for a row that is `stored`, which is
    /// made into a document only if one is needed.
    document: Option<Arc<Document>>,
    /// For an unchanged row of a column segment of the checkpoint being
    /// replaced: that segment, decoded, and the row's place in it. Its
    /// cells go into the new segment as they are.
    stored: Option<(Arc<crate::storage_engine::column_segment::TypedSegment>, usize)>,
    record: Option<Arc<[u8]>>,
    schema_version: u32,
    /// Where the checkpoint being replaced holds this very record, its
    /// length and its checksum, when it does.
    carried: Option<(u64, u32, u32)>,
    /// Set when the index pages of the checkpoint being replaced are
    /// built over and already hold this row's entries, because the row
    /// has not changed: whether it is kept as a record there. Such a
    /// row's entries are left as they are. Only the index of rows by id
    /// has anything to say, when the row stops or starts being a record.
    in_pages: Option<bool>,
}

impl CompactionRow {
    fn id(&self) -> &str {
        match (&self.document, &self.stored) {
            (Some(document), _) => &document.id,
            (None, Some((segment, row))) => &segment.document_ids[*row],
            (None, None) => "",
        }
    }

    /// The row as a document, made from its segment if that is how it is
    /// held.
    fn whole(&self, compiled: &CompiledSchema) -> OQLResult<Arc<Document>> {
        match (&self.document, &self.stored) {
            (Some(document), _) => Ok(document.clone()),
            (None, Some((segment, row))) => Ok(Arc::new(segment.decode_document(*row, compiled)?)),
            (None, None) => Err(OQLError::storage_error("a compaction row holds no row", None)),
        }
    }
}

/// Say in the index of rows by id that a row is kept as a record, unless
/// the pages being built over say so already.
fn note_row_is_record(ids: &mut IndexSorter, ids_number: usize, row: &CompactionRow) -> OQLResult<()> {
    if row.in_pages == Some(true) {
        return Ok(());
    }
    ids.push(
        ids_number,
        &id_index_key(row.object_ref.collection_id(), row.id()),
        row.object_ref,
    )
}

/// Put a row's record in the new checkpoint. One the checkpoint being
/// replaced holds as it is wanted is carried over from there; any other
/// is written, encoded first if the row was not read from a record.
fn push_row_record(
    records: &mut RecordSpool,
    row: &CompactionRow,
    compiled: &CompiledSchema,
) -> OQLResult<()> {
    if let Some((offset, length, checksum)) = row.carried {
        return records.push_carried(row.object_ref, offset, length, checksum);
    }
    match &row.record {
        Some(record) => records.push(row.object_ref, record),
        None => records.push(
            row.object_ref,
            &RecordCodec::encode(row.whole(compiled)?.as_ref(), row.object_ref, compiled)?,
        ),
    }
}

/// Where the checkpoint being replaced holds exactly this record, when it
/// does: the directory's entry for the row has this length and checksum.
fn record_carried(
    state: &State,
    object_ref: ObjRef,
    record: &[u8],
) -> OQLResult<Option<(u64, u32, u32)>> {
    let Some(files) = &state.checkpoint_records else {
        return Ok(None);
    };
    let files = files
        .lock()
        .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?;
    let Some(base) = &files.base_segment else {
        return Ok(None);
    };
    Ok(base
        .place(object_ref)?
        .filter(|(_, length, checksum)| {
            *length as usize == record.len() && *checksum == crc32fast::hash(record)
        }))
}

/// Where a row a [`LogicalIdentityCursor`] gives is held.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowFrom {
    /// A column segment of the checkpoint.
    Segment,
    /// A record of the checkpoint.
    Record,
    /// Memory: it was written after the checkpoint.
    Memory,
}

/// A row of the checkpoint's files that is no longer the row.
enum PassedOver {
    /// Row `row` of the segment at this place among the store's segments.
    Segment { segment_index: usize, row: usize },
    /// The record of this row in the checkpoint.
    Record(ObjRef),
}

struct LogicalIdentityCursor<'a> {
    state: &'a State,
    schema: &'a str,
    fences: &'a [TypedIdFence],
    fence_index: usize,
    row: usize,
    typed: Option<(DocumentId, ObjRef)>,
    /// Which segment, by its place among the fences, and which row of it
    /// `typed` is.
    typed_at: (usize, usize),
    /// The same for the row `next` gave last, when a segment had it.
    gave: Option<(usize, usize)>,
    /// The row `typed` held when `next` was last asked.
    gave_typed: Option<ObjRef>,
    /// Where the row `next` gave last came from.
    gave_from: RowFrom,
    /// The record `on_file` held once `next` had filled it.
    gave_record: Option<ObjRef>,
    records_noted: Vec<ObjRef>,
    /// Rows of the checkpoint's files this cursor passed over because the
    /// row has changed since: noted only when asked for, for whoever
    /// needs to know what the files hold that is no longer so.
    note_passed_over: bool,
    passed_over: Vec<PassedOver>,
    /// The checkpoint's rows outside its column segments, and the next of
    /// them that has not changed since.
    overflow: Option<OverflowIds>,
    on_file: Option<(DocumentId, ObjRef)>,
    resident:
        Option<std::iter::Peekable<std::collections::btree_map::Iter<'a, DocumentId, ObjRef>>>,
}

impl<'a> LogicalIdentityCursor<'a> {
    fn new(state: &'a State, schema: &'a str) -> Self {
        Self {
            state,
            schema,
            fences: state
                .typed_id_fences
                .get(schema)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            fence_index: 0,
            row: 0,
            typed: None,
            typed_at: (0, 0),
            gave: None,
            gave_typed: None,
            gave_from: RowFrom::Memory,
            gave_record: None,
            records_noted: Vec::new(),
            note_passed_over: false,
            passed_over: Vec::new(),
            overflow: OverflowIds::new(state, schema),
            on_file: None,
            resident: state
                .collection_rows
                .get(schema)
                .map(|rows| rows.iter().peekable()),
        }
    }

    /// The next row in id order from the three places a row can be: a
    /// column segment, the checkpoint's other rows, and memory. A row is
    /// in a segment or among the other rows, never both; one also in
    /// memory has changed, and memory has it as it is now.
    fn next(&mut self) -> OQLResult<Option<(DocumentId, ObjRef)>> {
        let had_record = self.on_file.as_ref().map(|(_, object_ref)| *object_ref);
        let row = self.next_row()?;
        // Only a row taken from `typed` leaves it empty with nothing
        // else having been read from the segments since.
        self.gave = match (&row, &self.typed) {
            (Some((_, object_ref)), None) if self.gave_typed == Some(*object_ref) => Some(self.typed_at),
            _ => None,
        };
        let given = row.as_ref().map(|(_, object_ref)| *object_ref);
        self.gave_from = match given {
            _ if self.gave.is_some() => RowFrom::Segment,
            Some(given)
                if self.on_file.is_none()
                    && (self.gave_record == Some(given) || had_record == Some(given)) =>
            {
                RowFrom::Record
            }
            _ => RowFrom::Memory,
        };
        // A row of the files that was held ready and has gone without
        // being given was hidden by one in memory under the same id.
        if self.note_passed_over {
            if let Some(typed) = self.gave_typed {
                if self.typed.is_none() && given != Some(typed) {
                    self.passed_over.push(PassedOver::Segment {
                        segment_index: self.fences[self.typed_at.0].segment_index,
                        row: self.typed_at.1,
                    });
                }
            }
            for record in [had_record, self.gave_record].into_iter().flatten() {
                if self.on_file.is_none() && given != Some(record) && !self.records_noted.contains(&record) {
                    self.records_noted.push(record);
                    self.passed_over.push(PassedOver::Record(record));
                }
            }
            self.records_noted.clear();
        }
        Ok(row)
    }

    /// The first row of a column segment was just given, and nothing has
    /// been read past it: leave the rest of that segment out.
    fn skip_rest_of_segment(&mut self) {
        if let Some((fence_index, 0)) = self.gave {
            self.fence_index = fence_index + 1;
            self.row = 0;
            self.typed = None;
            self.gave = None;
        }
    }

    fn next_row(&mut self) -> OQLResult<Option<(DocumentId, ObjRef)>> {
        if self.typed.is_none() {
            self.typed = self.next_typed()?;
        }
        self.gave_typed = self.typed.as_ref().map(|(_, object_ref)| *object_ref);
        if self.on_file.is_none() {
            self.on_file = self.next_on_file()?;
        }
        self.gave_record = self.on_file.as_ref().map(|(_, object_ref)| *object_ref);
        // Whichever of the two file sources comes first stands for both.
        let file_first = match (&self.typed, &self.on_file) {
            (Some((typed, _)), Some((other, _))) => other < typed,
            (None, Some(_)) => true,
            _ => false,
        };
        // A row is not in both file sources; should one ever be, the
        // column segment has it.
        if let (Some((typed, _)), Some((other, _))) = (&self.typed, &self.on_file) {
            if typed == other {
                self.on_file = None;
            }
        }
        if file_first {
            let resident_id = self
                .resident
                .as_mut()
                .and_then(|rows| rows.peek())
                .map(|(id, _)| (*id).clone());
            let on_file_id = self.on_file.as_ref().map(|(id, _)| id.clone());
            return match (on_file_id, resident_id) {
                (Some(on_file), Some(resident)) if resident <= on_file => {
                    if resident == on_file {
                        self.on_file = None;
                    }
                    Ok(self
                        .resident
                        .as_mut()
                        .and_then(|rows| rows.next())
                        .map(|(id, object_ref)| (id.clone(), *object_ref)))
                }
                _ => Ok(self.on_file.take()),
            };
        }
        let resident = self
            .resident
            .as_mut()
            .and_then(|rows| rows.peek())
            .map(|(id, object_ref)| ((*id).clone(), **object_ref));
        match (self.typed.as_ref(), resident) {
            (None, None) => Ok(None),
            (Some(_), None) => Ok(self.typed.take()),
            (None, Some(_)) => Ok(self
                .resident
                .as_mut()
                .and_then(|rows| rows.next())
                .map(|(id, object_ref)| (id.clone(), *object_ref))),
            (Some((typed_id, _)), Some((resident_id, _))) if typed_id < &resident_id => {
                Ok(self.typed.take())
            }
            (Some((typed_id, _)), Some((resident_id, _))) if typed_id == &resident_id => {
                self.typed.take();
                Ok(self
                    .resident
                    .as_mut()
                    .and_then(|rows| rows.next())
                    .map(|(id, object_ref)| (id.clone(), *object_ref)))
            }
            _ => Ok(self
                .resident
                .as_mut()
                .and_then(|rows| rows.next())
                .map(|(id, object_ref)| (id.clone(), *object_ref))),
        }
    }

    fn next_on_file(&mut self) -> OQLResult<Option<(DocumentId, ObjRef)>> {
        let Some(overflow) = self.overflow.as_mut() else {
            return Ok(None);
        };
        let changed = self.state.typed_delta_rows.get(self.schema);
        while let Some((id, object_ref)) = overflow.next()? {
            if changed.is_some_and(|rows| rows.contains_key(&id)) {
                if self.note_passed_over {
                    self.passed_over.push(PassedOver::Record(object_ref));
                }
                continue;
            }
            return Ok(Some((id, object_ref)));
        }
        Ok(None)
    }

    fn next_typed(&mut self) -> OQLResult<Option<(DocumentId, ObjRef)>> {
        loop {
            let Some(fence) = self.fences.get(self.fence_index) else {
                return Ok(None);
            };
            let segment = self
                .state
                .typed_segments
                .get(fence.segment_index)
                .ok_or_else(|| {
                    OQLError::storage_error("typed identity cursor lost segment", None)
                })?;
            if self.row == segment.row_count() {
                self.fence_index += 1;
                self.row = 0;
                continue;
            }
            let row = self.row;
            self.row += 1;
            let id = segment.document_id(row)?;
            if self
                .state
                .typed_delta_rows
                .get(self.schema)
                .is_some_and(|rows| rows.contains_key(&id))
            {
                if self.note_passed_over {
                    self.passed_over.push(PassedOver::Segment {
                        segment_index: fence.segment_index,
                        row,
                    });
                }
                continue;
            }
            self.typed_at = (self.fence_index, row);
            return Ok(Some((id, segment.object_ref(row)?)));
        }
    }
}

/// How many compactions in this process built their indexes over the
/// pages of the checkpoint they replaced, and how many set out to and
/// had to build them from every row instead.
static INDEX_CARRIES: AtomicU64 = AtomicU64::new(0);
static INDEX_CARRIES_REFUSED: AtomicU64 = AtomicU64::new(0);

impl RustStore {
    /// See [`INDEX_CARRIES`]: compactions that built their indexes over
    /// the pages they replaced, and those that could not.
    /// How many index pages the compactions of this process carried over
    /// as they were, and how many they wrote.
    #[doc(hidden)]
    pub fn index_pages_carried_and_written() -> (u64, u64) {
        crate::storage_engine::checkpoint::index_pages_carried_and_written()
    }

    /// How many index pages this process has read from files to answer
    /// for an index.
    #[doc(hidden)]
    pub fn index_pages_read() -> u64 {
        crate::storage_engine::checkpoint::index_pages_read()
    }

    /// About how many bytes this store's indexes hold in decoded pages
    /// read from its files, and the most they may; zero for no limit.
    #[doc(hidden)]
    pub fn index_page_bytes_held(&self) -> OQLResult<(usize, usize)> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        Ok((state.index_page_limit.held(), state.index_page_bytes()))
    }

    #[doc(hidden)]
    pub fn index_page_carries() -> (u64, u64) {
        (
            INDEX_CARRIES.load(Ordering::Relaxed),
            INDEX_CARRIES_REFUSED.load(Ordering::Relaxed),
        )
    }
}

/// The image of a compaction. Its indexes are built over the pages the
/// checkpoint being replaced has, where that can be done; where it turns
/// out it cannot, they are built from every row, as they always could be.
fn checkpoint_image_from_compaction(
    state: &State,
    plan: &CompactionPlan,
    sequence: u64,
) -> OQLResult<CheckpointImage> {
    match compaction_image(state, plan, sequence, true) {
        Err(error) if crate::storage_engine::checkpoint::index_carry_refused(&error) => {
            INDEX_CARRIES_REFUSED.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                target: "owl_db::storage_engine::checkpoint",
                %error,
                "building every index from its rows"
            );
            compaction_image(state, plan, sequence, false)
        }
        image => image,
    }
}

/// Take out of the indexes what a row of the checkpoint being replaced
/// put in them: the row is not carried over as it is, so whatever it
/// becomes is added afresh. `document` is the row as that checkpoint's
/// files hold it.
fn take_out_base_entries(
    state: &State,
    document: &Document,
    object_ref: ObjRef,
    was_record: bool,
    index_numbers: &HashMap<(String, String), usize>,
    reverse_number: usize,
    ids_number: usize,
    gone: &mut IndexSorter,
) -> OQLResult<()> {
    for definition in state.scalar_indexes.get(&document.schema).into_iter().flatten() {
        let number = *index_numbers
            .get(&(definition.schema.clone(), definition.name.clone()))
            .ok_or_else(|| OQLError::storage_error("compaction index builder is missing", None))?;
        gone.push(number, &document_index_key(document, definition)?, object_ref)?;
    }
    for key in document_reverse_keys(document) {
        gone.push(reverse_number, &key, object_ref)?;
    }
    if was_record {
        gone.push(ids_number, &id_index_key(object_ref.collection_id(), &document.id), object_ref)?;
    }
    Ok(())
}

/// A row as the files of the checkpoint being replaced hold it, whatever
/// has become of it since.
fn passed_over_document(state: &State, passed: &PassedOver) -> OQLResult<(ObjRef, Document)> {
    match passed {
        PassedOver::Segment { segment_index, row } => {
            let handle = state.typed_segments.get(*segment_index).ok_or_else(|| {
                OQLError::storage_error("a passed-over row names a missing segment", None)
            })?;
            let compiled = state
                .compiled_schemas
                .get(&(handle.collection_id(), handle.schema_version()))
                .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
            Ok((handle.object_ref(*row)?, handle.load()?.decode_document(*row, compiled)?))
        }
        PassedOver::Record(object_ref) => {
            let record = match &state.checkpoint_records {
                Some(files) => {
                    let files = files
                        .lock()
                        .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?;
                    match &files.base_segment {
                        Some(base) => base.lookup(*object_ref)?,
                        None => None,
                    }
                }
                None => None,
            }
            .ok_or_else(|| OQLError::storage_error("a passed-over row has no record in its checkpoint", None))?;
            let compiled = schema_of_record(state, *object_ref, &record)?;
            Ok((*object_ref, RecordCodec::decode(&record, compiled)?))
        }
    }
}

fn compaction_image(
    state: &State,
    plan: &CompactionPlan,
    sequence: u64,
    carry_index_pages: bool,
) -> OQLResult<CheckpointImage> {
    // The rows of a segment are held decoded while it is built. A database
    // short of memory builds smaller segments, which also lets it hold a
    // few of them at a time within its share for segments, not one.
    #[allow(non_snake_case)]
    let TARGET_ROWS: usize = if state.tight() { 8_192 } else { 65_536 };
    // Records go to a file of their own as they are produced, and are read
    // back when the image is written.
    let mut records = RecordSpool::create(
        plan.filesystem.clone(),
        sidecar_path(&plan.manifest_path, "records"),
    )?;
    // So do the column segments, each as it is built.
    let mut typed_segments = SegmentSpool::create(
        plan.filesystem.clone(),
        sidecar_path(&plan.manifest_path, "segments"),
    )?;
    // Index entries are sorted on disk a bounded amount at a time, and
    // come back as encoded pages: every declared index, then the index of
    // what refers to what.
    let mut index_identities = state
        .scalar_indexes
        .values()
        .flatten()
        .map(|definition| (definition.schema.clone(), definition.name.clone(), definition.unique))
        .collect::<Vec<_>>();
    let index_numbers = index_identities
        .iter()
        .enumerate()
        .map(|(number, (schema, name, _))| ((schema.clone(), name.clone()), number))
        .collect::<HashMap<_, _>>();
    let declared_indexes = index_identities
        .iter()
        .map(|(schema, name, _)| (schema.clone(), name.clone()))
        .collect::<Vec<_>>();
    let reverse_number = index_identities.len();
    index_identities.push((REVERSE_INDEX_SCHEMA.into(), REVERSE_INDEX_NAME.into(), false));
    let ids_number = index_identities.len();
    index_identities.push((REVERSE_INDEX_SCHEMA.into(), ID_INDEX_NAME.into(), true));
    let index_identities_again = index_identities.clone();
    let mut index_entries = IndexSorter::new(
        plan.filesystem.clone(),
        sidecar_path(&plan.manifest_path, "index-run"),
        index_identities,
    )?;
    let mut collection_names = state.collection_ids.keys().cloned().collect::<Vec<_>>();
    for schema in &plan.schemas {
        if !collection_names.contains(schema) {
            collection_names.push(schema.clone());
        }
    }
    collection_names.sort();
    collection_names.dedup();
    // Records and column segments the checkpoint being replaced holds
    // as they are wanted are carried over from it.
    let base_path = match &state.checkpoint_records {
        Some(files) => files
            .lock()
            .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
            .base_segment
            .as_ref()
            .map(|base| base.path().to_path_buf()),
        None => None,
    };
    let carries = base_path.is_some();
    // Each index's pages in that checkpoint, when every index has them
    // there: the declared ones, what refers to what, and rows by id. The
    // indexes are then built over those pages, and a row that is carried
    // over is not read at all.
    let index_bases = base_path.as_ref().filter(|_| carry_index_pages).and_then(|base_path| {
        let in_base = |base: Arc<dyn OrderedIndexBase>| {
            base.file_base()
                .is_some_and(|pages| pages.is_in(base_path))
                .then_some(base)
        };
        let mut bases = Vec::with_capacity(ids_number + 1);
        for (schema, name) in declared_indexes.iter() {
            bases.push(in_base(
                state.scalar_index_data.get(&(schema.clone(), name.clone()))?.base()?,
            )?);
        }
        bases.push(in_base(state.reverse_reference_index.base()?)?);
        bases.push(in_base(state.overflow_ids.clone()?)?);
        Some(bases)
    });
    let over_pages = index_bases.is_some();
    // What the rows that are not carried over as they are had put in
    // those pages.
    let mut gone = IndexSorter::new(
        plan.filesystem.clone(),
        sidecar_path(&plan.manifest_path, "index-gone"),
        index_identities_again,
    )?;
    if let Some(base_path) = base_path {
        records.carry_from(base_path.clone());
        typed_segments.carry_from(base_path);
    }
    let mut changed_rows = CompactionChangeCursor::new(plan.filesystem.as_ref(), &plan.runs)?;
    let mut changed = changed_rows.next()?;

    for schema_name in collection_names {
        let mut base_rows = LogicalIdentityCursor::new(state, &schema_name);
        base_rows.note_passed_over = over_pages;
        let mut base = base_rows.next()?;
        let mut typed_chunk = Vec::<CompactionRow>::with_capacity(TARGET_ROWS);
        // The column segment the last such row was read from, decoded.
        let mut held_segment: Option<(usize, Arc<crate::storage_engine::column_segment::TypedSegment>)> = None;
        // A row of the checkpoint's files that will not be carried over
        // as it is has its entries taken out of the pages.
        let take_out = |gone: &mut IndexSorter, object_ref: ObjRef, from: RowFrom| -> OQLResult<()> {
            if from == RowFrom::Memory {
                return Ok(());
            }
            let document = logical_document(state, object_ref)?.ok_or_else(|| {
                OQLError::storage_error("compaction base row is missing", None)
            })?;
            take_out_base_entries(
                state,
                &document,
                object_ref,
                from == RowFrom::Record,
                &index_numbers,
                reverse_number,
                ids_number,
                gone,
            )
        };

        loop {
            while base
                .as_ref()
                .is_some_and(|(_, object_ref)| plan.overridden.contains(object_ref))
            {
                if over_pages {
                    if let Some((_, object_ref)) = &base {
                        take_out(&mut gone, *object_ref, base_rows.gave_from)?;
                    }
                }
                base = base_rows.next()?;
            }
            for passed in std::mem::take(&mut base_rows.passed_over) {
                let (object_ref, document) = passed_over_document(state, &passed)?;
                take_out_base_entries(
                    state,
                    &document,
                    object_ref,
                    matches!(passed, PassedOver::Record(_)),
                    &index_numbers,
                    reverse_number,
                    ids_number,
                    &mut gone,
                )?;
            }
            if changed
                .as_ref()
                .is_some_and(|row| row.schema.as_str() < schema_name.as_str())
            {
                return Err(OQLError::storage_error(
                    "compaction merge cursor escaped collection order",
                    None,
                ));
            }
            let collection_change = changed.as_ref().filter(|row| row.schema == schema_name);
            let take_changed = match (base.as_ref(), collection_change) {
                (None, None) => break,
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (Some((base_id, _)), Some(row)) => &row.id <= base_id,
            };
            // A column segment none of whose rows has changed, and into
            // whose span of ids nothing else falls, goes into the new
            // checkpoint as it is. Its rows are not decoded and built
            // into a segment again; only the fields its indexes are made
            // from are read.
            if !take_changed {
                if let Some((fence_index, 0)) = base_rows.gave {
                    let next_change = collection_change.map(|row| row.id.as_str());
                    if let Some(encoded) = segment_carried_over(
                        state,
                        plan,
                        &schema_name,
                        fence_index,
                        next_change,
                        typed_segments.carries(),
                    )? {
                        flush_compaction_typed_chunk(
                            state,
                            &schema_name,
                            &mut typed_chunk,
                            &mut typed_segments,
                            &mut records,
                            (&mut index_entries, &mut gone, ids_number),
                        )?;
                        // Its rows' entries are in the pages already when
                        // the indexes are built over those.
                        if !over_pages {
                            add_carried_segment_indexes(
                                state,
                                &schema_name,
                                fence_index,
                                &index_numbers,
                                reverse_number,
                                &mut index_entries,
                            )?;
                        }
                        match encoded {
                            CarriedSegment::Listed(offset, descriptor) => {
                                typed_segments.push_carried(offset, descriptor)?
                            }
                            CarriedSegment::Bytes(encoded) => typed_segments.push(&encoded)?,
                        }
                        base_rows.skip_rest_of_segment();
                        base = base_rows.next()?;
                        continue;
                    }
                }
            }
            // A row the checkpoint keeps as a record, unchanged, stays
            // one: it is listed where it is, with its entries in the
            // pages as they are, and is not read.
            if !take_changed && over_pages && base_rows.gave_from == RowFrom::Record {
                if let Some((_, object_ref)) = &base {
                    let place = match &state.checkpoint_records {
                        Some(files) => {
                            let files = files.lock().map_err(|_| {
                                OQLError::generic("checkpoint read handle lock poisoned")
                            })?;
                            match &files.base_segment {
                                Some(base) => base.place(*object_ref)?,
                                None => None,
                            }
                        }
                        None => None,
                    };
                    if let Some((offset, length, checksum)) = place {
                        records.push_carried(*object_ref, offset, length, checksum)?;
                        base = base_rows.next()?;
                        continue;
                    }
                }
            }
            let row = if take_changed {
                let change = changed.take().ok_or_else(|| {
                    OQLError::storage_error("compaction change cursor is empty", None)
                })?;
                if base
                    .as_ref()
                    .is_some_and(|(base_id, _)| base_id == &change.id)
                {
                    if over_pages {
                        if let Some((_, object_ref)) = &base {
                            take_out(&mut gone, *object_ref, base_rows.gave_from)?;
                        }
                    }
                    base = base_rows.next()?;
                }
                changed = changed_rows.next()?;
                let Some(record) = change.record else {
                    continue;
                };
                let record: Arc<[u8]> = Arc::from(record);
                let header = RecordCodec::trusted_header(&record)?;
                let compiled = state
                    .compiled_schemas
                    .get(&(change.object_ref.collection_id(), header.schema_version))
                    .ok_or_else(|| {
                        OQLError::storage_error("compaction record has no compiled schema", None)
                    })?;
                CompactionRow {
                    object_ref: change.object_ref,
                    document: Some(Arc::new(RecordCodec::decode(&record, compiled)?)),
                    stored: None,
                    record: Some(record),
                    schema_version: header.schema_version,
                    carried: None,
                    in_pages: None,
                }
            } else {
                let (_, object_ref) = base
                    .take()
                    .ok_or_else(|| OQLError::storage_error("compaction cursor is empty", None))?;
                let from = base_rows.gave_from;
                let at = base_rows.gave;
                base = base_rows.next()?;
                // The row has not changed, so what the old checkpoint's
                // pages say of it still holds, wherever in the new one it
                // ends up.
                let in_pages = (over_pages && from != RowFrom::Memory).then_some(from == RowFrom::Record);
                // One in a column segment that says everything of its rows
                // is not made a document: its cells go from that segment
                // to the new one as they are.
                let stored = match (in_pages, at) {
                    (Some(false), Some((fence_index, row))) => {
                        let handle = state
                            .typed_id_fences
                            .get(&schema_name)
                            .and_then(|fences| fences.get(fence_index))
                            .and_then(|fence| state.typed_segments.get(fence.segment_index));
                        match handle {
                            Some(handle) if handle.authoritative() => {
                                let decoded = match &held_segment {
                                    Some((held, decoded)) if *held == fence_index => decoded.clone(),
                                    _ => {
                                        let decoded = handle.load()?;
                                        held_segment = Some((fence_index, decoded.clone()));
                                        decoded
                                    }
                                };
                                Some((decoded, row, handle.schema_version()))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some((segment, row, schema_version)) = stored {
                    CompactionRow {
                        object_ref,
                        document: None,
                        stored: Some((segment, row)),
                        record: None,
                        schema_version,
                        carried: None,
                        in_pages,
                    }
                } else {
                let (document, source) = sourced_document(state, object_ref)?.ok_or_else(|| {
                    OQLError::storage_error("compaction base row is missing", None)
                })?;

                // A row read from its record has the record to hand: it
                // goes into the new checkpoint as it is, not encoded again.
                let read = match source {
                    DocumentSource::Record(record) => Some(record),
                    _ => None,
                };
                let record = state.object_records.get(&object_ref).cloned().or(read);
                // Its record names the schema version it was written under.
                let schema_version = match &record {
                    Some(record) => RecordCodec::trusted_header(record)?.schema_version,
                    None => logical_schema_version(state, object_ref)?.ok_or_else(|| {
                        OQLError::storage_error("compaction base row has no schema version", None)
                    })?,
                };
                // A record the checkpoint being replaced holds as it is
                // goes into the new one by being listed, not written.
                let carried = match (&record, carries) {
                    (Some(record), true) => record_carried(state, object_ref, record)?,
                    _ => None,
                };
                CompactionRow {
                    object_ref,
                    document: Some(document),
                    stored: None,
                    record,
                    schema_version,
                    carried,
                    in_pages,
                }
                }
            };
            if row.in_pages.is_none() {
                add_compaction_indexes(state, &row, &index_numbers, reverse_number, &mut index_entries)?;
            }
            let current_version = state.current_schema_versions.get(&schema_name).copied();
            if current_version == Some(row.schema_version) {
                typed_chunk.push(row);
                if typed_chunk.len() == TARGET_ROWS {
                    flush_compaction_typed_chunk(
                        state,
                        &schema_name,
                        &mut typed_chunk,
                        &mut typed_segments,
                        &mut records,
                        (&mut index_entries, &mut gone, ids_number),
                    )?;
                }
            } else {
                flush_compaction_typed_chunk(
                    state,
                    &schema_name,
                    &mut typed_chunk,
                    &mut typed_segments,
                    &mut records,
                    (&mut index_entries, &mut gone, ids_number),
                )?;
                note_row_is_record(&mut index_entries, ids_number, &row)?;
                let compiled = state
                    .compiled_schemas
                    .get(&(row.object_ref.collection_id(), row.schema_version))
                    .ok_or_else(|| {
                        OQLError::storage_error("compaction row has no compiled schema", None)
                    })?;
                push_row_record(&mut records, &row, compiled)?;
            }
        }
        flush_compaction_typed_chunk(
            state,
            &schema_name,
            &mut typed_chunk,
            &mut typed_segments,
            &mut records,
            (&mut index_entries, &mut gone, ids_number),
        )?;
    }
    if changed.is_some() {
        return Err(OQLError::storage_error(
            "compaction changes reference an unknown collection",
            None,
        ));
    }

    let indexes = match index_bases {
        Some(bases) => {
            let indexes = index_entries.finish_over(
                bases.into_iter().map(Some).collect(),
                Some(gone),
                // What refers to what may have had entries dropped while
                // the database ran, for rows that no longer exist.
                &[reverse_number],
            )?;
            INDEX_CARRIES.fetch_add(1, Ordering::Relaxed);
            indexes
        }
        None => index_entries.finish()?,
    };

    Ok(CheckpointImage {
        segment_spool: Some(Arc::new(typed_segments.finish()?)),
        record_spool: Some(Arc::new(records.finish()?)),
        sequence,
        checkpoint_lsn: state.committed_lsn,
        metadata: checkpoint_metadata_from_state(state),
        records: Vec::new(),
        indexes,
        typed_segments: Vec::new(),
    })
}

/// A column segment of the base may go into the new checkpoint as it is
/// when it is written the way segments are now, under the collection's
/// current schema, is not small, and holds exactly the rows the new
/// checkpoint would put in its place: none of its rows has been written
/// again or deleted, and no other row has an id within its span. Gives
/// the segment as encoded when so.
fn segment_carried_over(
    state: &State,
    plan: &CompactionPlan,
    schema: &str,
    fence_index: usize,
    next_change: Option<&str>,
    listed: bool,
) -> OQLResult<Option<CarriedSegment>> {
    let Some(fence) = state
        .typed_id_fences
        .get(schema)
        .and_then(|fences| fences.get(fence_index))
    else {
        return Ok(None);
    };
    let segment = state
        .typed_segments
        .get(fence.segment_index)
        .ok_or_else(|| OQLError::storage_error("typed ID fence has no segment", None))?;
    // A segment far below the usual size is built again with its
    // neighbours, or the segments would only ever get smaller.
    const SMALLEST_CARRIED: usize = 2_048;
    if !segment.authoritative()
        || segment.row_count() < SMALLEST_CARRIED
        || state.current_schema_versions.get(schema).copied() != Some(segment.schema_version())
        || !state.schemas.contains_key(schema)
    {
        return Ok(None);
    }
    let span = (
        Bound::Included(fence.first.as_str()),
        Bound::Included(fence.last.as_str()),
    );
    // A row written since the frozen commit.
    if next_change.is_some_and(|id| id <= fence.last.as_str()) {
        return Ok(None);
    }
    // A row the base holds in memory, or has noted as changed.
    if state
        .collection_rows
        .get(schema)
        .is_some_and(|rows| rows.range::<str, _>(span).next().is_some())
        || state
            .typed_delta_rows
            .get(schema)
            .is_some_and(|rows| rows.range::<str, _>(span).next().is_some())
    {
        return Ok(None);
    }
    // A row the base keeps as a record.
    if state.overflow_id_counts.get(schema).copied().unwrap_or(0) != 0 {
        let (Some(ids), Some(collection)) = (&state.overflow_ids, state.collection_ids.get(schema))
        else {
            return Ok(None);
        };
        let between = ids
            .range_entries(
                Bound::Included(id_index_key(*collection, &fence.first)),
                Bound::Included(id_index_key(*collection, &fence.last)),
                Some(1),
                false,
            )
            .map_err(|error| OQLError::storage_error(&error, None))?;
        if !between.is_empty() {
            return Ok(None);
        }
    }
    for row in 0..segment.row_count() {
        let object_ref = segment.object_ref(row)?;
        if plan.overridden.contains(&object_ref)
            || !state
                .typed_row_locations
                .still_at(&object_ref, (fence.segment_index, row))?
        {
            return Ok(None);
        }
    }
    if listed {
        if let Some((offset, descriptor)) = segment.place() {
            return Ok(Some(CarriedSegment::Listed(offset, descriptor)));
        }
    }
    Ok(segment.encoded()?.map(CarriedSegment::Bytes))
}

/// A column segment that goes into the new checkpoint as it is.
enum CarriedSegment {
    /// Where the checkpoint being replaced holds it, and what its header
    /// says of it: it is listed from there, not written again.
    Listed(u64, crate::storage_engine::column_segment::TypedSegmentDescriptor),
    /// Its bytes, to be written.
    Bytes(Vec<u8>),
}

/// The index entries of every row of a column segment that is carried
/// over, made as [`add_compaction_indexes`] makes them, from rows with
/// only the fields those entries are made from.
fn add_carried_segment_indexes(
    state: &State,
    schema: &str,
    fence_index: usize,
    index_numbers: &HashMap<(String, String), usize>,
    reverse_number: usize,
    entries: &mut IndexSorter,
) -> OQLResult<()> {
    let fence = state
        .typed_id_fences
        .get(schema)
        .and_then(|fences| fences.get(fence_index))
        .ok_or_else(|| OQLError::storage_error("carried segment has no ID fence", None))?;
    let handle = state
        .typed_segments
        .get(fence.segment_index)
        .ok_or_else(|| OQLError::storage_error("typed ID fence has no segment", None))?;
    let compiled = state
        .compiled_schemas
        .get(&(handle.collection_id(), handle.schema_version()))
        .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
    let segment = handle.load()?;
    // The fields an index is declared on, and the fields that refer to
    // another row.
    let mut wanted = HashSet::new();
    for definition in state.scalar_indexes.get(schema).into_iter().flatten() {
        for path in &definition.fields {
            let field = path.trim_start_matches('.');
            wanted.insert(field.split('.').next().unwrap_or(field).to_string());
        }
    }
    for field in &compiled.fields {
        if segment.column(field.id).is_some_and(|column| column.is_reference()) {
            wanted.insert(field.name.clone());
        }
    }
    if wanted.is_empty() && state.scalar_indexes.get(schema).is_none_or(|indexes| indexes.is_empty()) {
        return Ok(());
    }
    for row in 0..segment.row_count() {
        let row = CompactionRow {
            object_ref: handle.object_ref(row)?,
            document: Some(Arc::new(segment.decode_document_reading(row, compiled, Some(&wanted))?)),
            stored: None,
            record: None,
            schema_version: handle.schema_version(),
            carried: None,
            in_pages: None,
        };
        add_compaction_indexes(state, &row, index_numbers, reverse_number, entries)?;
    }
    Ok(())
}

fn add_compaction_indexes(
    state: &State,
    row: &CompactionRow,
    index_numbers: &HashMap<(String, String), usize>,
    reverse_number: usize,
    entries: &mut IndexSorter,
) -> OQLResult<()> {
    let document = row
        .document
        .as_ref()
        .ok_or_else(|| OQLError::storage_error("a row to index was not made a document", None))?;
    for definition in state
        .scalar_indexes
        .get(&document.schema)
        .into_iter()
        .flatten()
    {
        let number = *index_numbers
            .get(&(definition.schema.clone(), definition.name.clone()))
            .ok_or_else(|| OQLError::storage_error("compaction index builder is missing", None))?;
        entries.push(number, &document_index_key(document, definition)?, row.object_ref)?;
    }
    for key in document_reverse_keys(document) {
        entries.push(reverse_number, &key, row.object_ref)?;
    }
    Ok(())
}

fn flush_compaction_typed_chunk(
    state: &State,
    schema_name: &str,
    chunk: &mut Vec<CompactionRow>,
    typed_segments: &mut SegmentSpool,
    records: &mut RecordSpool,
    (ids, gone, ids_number): (&mut IndexSorter, &mut IndexSorter, usize),
) -> OQLResult<()> {
    if chunk.is_empty() {
        return Ok(());
    }
    let version = chunk[0].schema_version;
    if chunk.iter().any(|row| row.schema_version != version) {
        return Err(OQLError::storage_error(
            "compaction typed chunk mixes schema versions",
            None,
        ));
    }
    let collection_id = chunk[0].object_ref.collection_id();
    let compiled = state
        .compiled_schemas
        .get(&(collection_id, version))
        .ok_or_else(|| OQLError::storage_error("compaction compiled schema is missing", None))?;
    let Some(schema) = state.schemas.get(schema_name) else {
        for row in chunk.iter() {
            note_row_is_record(ids, ids_number, row)?;
            push_row_record(records, row, compiled)?;
        }
        chunk.clear();
        return Ok(());
    };
    use crate::storage_engine::column_segment::SegmentRow;
    let rows = chunk
        .iter()
        .map(|row| match (&row.document, &row.stored) {
            (Some(document), _) => Ok(SegmentRow::Document(row.object_ref, document.as_ref())),
            (None, Some((segment, at))) => Ok(SegmentRow::Stored(segment.as_ref(), *at)),
            (None, None) => Err(OQLError::storage_error("a compaction row holds no row", None)),
        })
        .collect::<OQLResult<Vec<_>>>()?;
    let segment = TypedSegment::build_from(compiled, schema, &rows)?;
    drop(rows);
    if segment.authoritative {
        typed_segments.push(&segment.encode()?)?;
        // A row the pages list as a record is one no longer.
        for row in chunk.iter().filter(|row| row.in_pages == Some(true)) {
            gone.push(
                ids_number,
                &id_index_key(row.object_ref.collection_id(), row.id()),
                row.object_ref,
            )?;
        }
    } else {
        for row in chunk.iter() {
            note_row_is_record(ids, ids_number, row)?;
            push_row_record(records, row, compiled)?;
        }
    }
    chunk.clear();
    Ok(())
}

fn checkpoint_metadata_from_state(state: &State) -> CheckpointMetadata {
    let mut collection_ids = state
        .collection_ids
        .iter()
        .map(|(name, id)| (name.clone(), *id))
        .collect::<Vec<_>>();
    collection_ids.sort_by(|left, right| left.0.cmp(&right.0));
    let mut next_object_ids = state
        .next_object_ids
        .iter()
        .map(|(collection, next)| (*collection, *next))
        .collect::<Vec<_>>();
    next_object_ids.sort_by_key(|(collection, _)| *collection);
    let mut schemas = state.schemas.values().cloned().collect::<Vec<_>>();
    schemas.sort_by(|left, right| left.name.cmp(&right.name));
    let mut compiled_schemas = state.compiled_schemas.values().cloned().collect::<Vec<_>>();
    compiled_schemas.sort_by_key(|schema| (schema.collection_id, schema.version));
    let mut current_schema_versions = state
        .current_schema_versions
        .iter()
        .map(|(name, version)| (name.clone(), *version))
        .collect::<Vec<_>>();
    current_schema_versions.sort_by(|left, right| left.0.cmp(&right.0));
    let mut scalar_indexes = state
        .scalar_indexes
        .iter()
        .map(|(schema, indexes)| (schema.clone(), indexes.clone()))
        .collect::<Vec<_>>();
    scalar_indexes.sort_by(|left, right| left.0.cmp(&right.0));
    let mut vector_indexes = state
        .vector_indexes
        .iter()
        .map(|(schema, indexes)| (schema.clone(), indexes.clone()))
        .collect::<Vec<_>>();
    vector_indexes.sort_by(|left, right| left.0.cmp(&right.0));
    CheckpointMetadata {
        collection_ids,
        next_collection_id: state.next_collection_id,
        next_object_ids,
        schemas,
        compiled_schemas,
        current_schema_versions,
        scalar_indexes,
        vector_indexes,
    }
}

fn build_typed_checkpoint_segments(
    state: &State,
    records: &[(ObjRef, Arc<[u8]>)],
) -> OQLResult<(Vec<Arc<[u8]>>, HashSet<ObjRef>)> {
    const TARGET_ROWS: usize = 65_536;
    let mut record_versions = HashMap::with_capacity(logical_live_count(state));
    // A record names the version it was written under.
    for (object_ref, record) in records {
        let header = RecordCodec::trusted_header(record)?;
        if header.object_ref != *object_ref
            || record_versions.insert(*object_ref, header.schema_version).is_some()
        {
            return Err(OQLError::storage_error(
                "typed checkpoint input contains a duplicate or conflicting record",
                None,
            ));
        }
    }
    // The rest are rows of column segments, which name theirs.
    for name in state.collection_ids.keys() {
        for object_ref in logical_collection_object_refs(state, name)? {
            if record_versions.contains_key(&object_ref) {
                continue;
            }
            let version = logical_schema_version(state, object_ref)?.ok_or_else(|| {
                OQLError::storage_error("live object has no compiled schema identity", None)
            })?;
            record_versions.insert(object_ref, version);
        }
    }
    let mut encoded = Vec::new();
    let mut authoritative_rows = HashSet::new();
    let mut collection_names = state.collection_ids.keys().collect::<Vec<_>>();
    collection_names.sort();
    for name in collection_names {
        let Some(schema) = state.schemas.get(name) else {
            continue;
        };
        let Some(collection_id) = state.collection_ids.get(name).copied() else {
            continue;
        };
        let Some(schema_version) = state.current_schema_versions.get(name).copied() else {
            continue;
        };
        let Some(compiled) = state.compiled_schemas.get(&(collection_id, schema_version)) else {
            continue;
        };
        let object_refs = logical_collection_object_refs(state, name)?;
        let mut chunk = Vec::with_capacity(TARGET_ROWS.min(object_refs.len()));
        for object_ref in object_refs {
            if record_versions.get(&object_ref) != Some(&schema_version) {
                continue;
            }
            let Some(document) = logical_document(state, object_ref)? else {
                continue;
            };
            chunk.push((object_ref, document));
            if chunk.len() == TARGET_ROWS {
                let segment = TypedSegment::build(compiled, schema, &chunk)?;
                if segment.authoritative {
                    authoritative_rows.extend(segment.object_refs.iter().copied());
                }
                encoded.push(Arc::from(segment.encode()?));
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            let segment = TypedSegment::build(compiled, schema, &chunk)?;
            if segment.authoritative {
                authoritative_rows.extend(segment.object_refs.iter().copied());
            }
            encoded.push(Arc::from(segment.encode()?));
        }
    }
    Ok((encoded, authoritative_rows))
}

fn encode_authoritative_row(state: &State, object_ref: ObjRef) -> OQLResult<Arc<[u8]>> {
    let document = logical_document(state, object_ref)?.ok_or_else(|| {
        OQLError::storage_error(
            "live object is absent from the logical object directory",
            None,
        )
    })?;
    let schema_version = logical_schema_version(state, object_ref)?.ok_or_else(|| {
        OQLError::storage_error("live object has no compiled schema identity", None)
    })?;
    let compiled = state
        .compiled_schemas
        .get(&(object_ref.collection_id(), schema_version))
        .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
    Ok(Arc::from(RecordCodec::encode(
        &document, object_ref, compiled,
    )?))
}

fn rebuild_typed_id_fences(state: &mut State) -> OQLResult<()> {
    let mut fences = HashMap::<String, Vec<TypedIdFence>>::new();
    for (segment_index, segment) in state.typed_segments.iter().enumerate() {
        if !segment.authoritative() || segment.row_count() == 0 {
            continue;
        }
        let compiled = state
            .compiled_schemas
            .get(&(segment.collection_id(), segment.schema_version()))
            .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
        let first = segment
            .first_document_id()?
            .ok_or_else(|| OQLError::storage_error("typed segment has no first ID", None))?;
        let last = segment
            .last_document_id()?
            .ok_or_else(|| OQLError::storage_error("typed segment has no last ID", None))?;
        if first > last {
            return Err(OQLError::storage_error(
                "typed segment ID fence is reversed",
                None,
            ));
        }
        let collection_fences = fences.entry(compiled.name.clone()).or_default();
        if collection_fences
            .last()
            .is_some_and(|previous| previous.last >= first)
        {
            return Err(OQLError::storage_error(
                "typed segment ID fences overlap or are unordered",
                None,
            ));
        }
        collection_fences.push(TypedIdFence {
            first,
            last,
            segment_index,
        });
    }
    state.typed_id_fences = fences;
    Ok(())
}

fn install_typed_id_fences(state: &mut State, locator: &TypedLocatorLeaf) -> OQLResult<bool> {
    if locator.fences().is_empty() {
        return Ok(false);
    }
    let mut fences = HashMap::<String, Vec<TypedIdFence>>::new();
    for persisted in locator.fences() {
        let segment = state
            .typed_segments
            .get(persisted.segment_index)
            .ok_or_else(|| OQLError::storage_error("typed ID fence has no segment", None))?;
        if !segment.authoritative() || segment.row_count() == 0 {
            return Err(OQLError::storage_error(
                "typed ID fence references a non-authoritative segment",
                None,
            ));
        }
        let compiled = state
            .compiled_schemas
            .get(&(segment.collection_id(), segment.schema_version()))
            .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
        let collection_fences = fences.entry(compiled.name.clone()).or_default();
        if collection_fences
            .last()
            .is_some_and(|previous| previous.last >= persisted.first)
        {
            return Err(OQLError::storage_error(
                "typed segment ID fences overlap or are unordered",
                None,
            ));
        }
        collection_fences.push(TypedIdFence {
            first: persisted.first.clone(),
            last: persisted.last.clone(),
            segment_index: persisted.segment_index,
        });
    }
    state.typed_id_fences = fences;
    Ok(true)
}

fn lookup_object_ref(state: &State, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>> {
    if let Some(delta) = state
        .typed_delta_rows
        .get(schema)
        .and_then(|rows| rows.get(id))
    {
        return Ok(*delta);
    }
    // The ordered directory holds the same rows as the hashed one and is
    // searched by borrowed text, where the hashed one needs a key built
    // from two fresh strings for every lookup.
    if let Some(object_ref) = state
        .collection_rows
        .get(schema)
        .and_then(|rows| rows.get(id))
        .copied()
    {
        return Ok(Some(object_ref));
    }
    if !state.has_rows_on_file() {
        return Ok(None);
    }
    if let Ok(found) = state.ids_found_on_file.read() {
        if let Some(known) = found.0.get(schema).and_then(|ids| ids.get(id)) {
            return Ok(*known);
        }
    }
    let on_file = match base_location_by_id(state, schema, id)? {
        Some((_, _, object_ref)) => Some(object_ref),
        None => overflow_base_by_id(state, schema, id)?,
    };
    if let Ok(mut found) = state.ids_found_on_file.write() {
        if found.1 >= IDS_REMEMBERED {
            *found = Default::default();
        }
        let ids = match found.0.get_mut(schema) {
            Some(ids) => ids,
            None => found.0.entry(schema.to_string()).or_default(),
        };
        if ids.insert(id.clone(), on_file).is_none() {
            found.1 += 1;
        }
    }
    Ok(on_file)
}

/// How many rows of each collection the id index lists.
fn count_overflow_ids(
    state: &State,
    ids: &dyn OrderedIndexBase,
) -> OQLResult<HashMap<String, usize>> {
    let names = state
        .collection_ids
        .iter()
        .map(|(name, collection)| (*collection, name))
        .collect::<HashMap<_, _>>();
    let mut counts = HashMap::<String, usize>::new();
    ids.visit_entries(&mut |key, rows| {
        let collection = key
            .get(..2)
            .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
            .ok_or_else(|| "a key of the id index is too short".to_string())?;
        let name = names
            .get(&collection)
            .ok_or_else(|| "the id index lists a row of an unknown collection".to_string())?;
        *counts.entry((*name).clone()).or_default() += rows.len();
        Ok(())
    })
    .map_err(|error| OQLError::storage_error(&error, None))?;
    Ok(counts)
}

/// Stop reading the checkpoint's index of rows by id: list in memory the
/// rows it holds that have not changed since, beside those that have.
fn list_overflow_ids_in_memory(state: &mut State) -> OQLResult<()> {
    if state.overflow_ids.is_none() {
        return Ok(());
    }
    let names = state.collection_ids.keys().cloned().collect::<Vec<_>>();
    for name in names {
        let mut unchanged = Vec::new();
        if let Some(mut on_file) = OverflowIds::new(state, &name) {
            let changed = state.typed_delta_rows.get(&name);
            while let Some((id, object_ref)) = on_file.next()? {
                if !changed.is_some_and(|rows| rows.contains_key(&id)) {
                    unchanged.push((id, object_ref));
                }
            }
        }
        if !unchanged.is_empty() {
            state.collection_rows.entry(name).or_default().extend(unchanged);
        }
    }
    state.overflow_ids = None;
    state.overflow_id_counts.clear();
    state.forget_counts();
    Ok(())
}

/// The row the checkpoint holds outside its column segments under this
/// id, whether or not it has changed since.
fn overflow_base_by_id(state: &State, schema: &str, id: &str) -> OQLResult<Option<ObjRef>> {
    let (Some(ids), Some(collection)) = (&state.overflow_ids, state.collection_ids.get(schema))
    else {
        return Ok(None);
    };
    Ok(ids
        .search_key(&id_index_key(*collection, id))
        .map_err(|error| OQLError::storage_error(&error, None))?
        .first()
        .copied())
}

/// The rows the checkpoint holds outside its column segments, a
/// collection at a time in id order, read a stretch at a time.
struct OverflowIds {
    ids: Arc<dyn OrderedIndexBase>,
    end: Bound<Vec<u8>>,
    /// Where the next stretch starts.
    from: Bound<Vec<u8>>,
    held: std::collections::VecDeque<(DocumentId, ObjRef)>,
    finished: bool,
}

impl OverflowIds {
    /// How many ids one read takes in.
    const STRETCH: usize = 1_024;

    fn new(state: &State, schema: &str) -> Option<Self> {
        let ids = state.overflow_ids.clone()?;
        let collection = *state.collection_ids.get(schema)?;
        let (from, end) = id_index_range(collection);
        Some(Self {
            ids,
            end,
            from,
            held: std::collections::VecDeque::new(),
            finished: false,
        })
    }

    fn next(&mut self) -> OQLResult<Option<(DocumentId, ObjRef)>> {
        if self.held.is_empty() && !self.finished {
            let stretch = self
                .ids
                .range_entries(self.from.clone(), self.end.clone(), Some(Self::STRETCH), false)
                .map_err(|error| OQLError::storage_error(&error, None))?;
            if stretch.len() < Self::STRETCH {
                self.finished = true;
            }
            if let Some((last, _)) = stretch.last() {
                self.from = Bound::Excluded(last.clone());
            }
            for (key, rows) in stretch {
                let id = std::str::from_utf8(key.get(2..).unwrap_or_default())
                    .map_err(|_| OQLError::storage_error("an id in the id index is not text", None))?
                    .to_string();
                let object_ref = rows.first().copied().ok_or_else(|| {
                    OQLError::storage_error("an id in the id index names no row", None)
                })?;
                self.held.push_back((id, object_ref));
            }
        }
        Ok(self.held.pop_front())
    }
}

fn base_location_by_id(
    state: &State,
    schema: &str,
    id: &DocumentId,
) -> OQLResult<Option<(usize, usize, ObjRef)>> {
    let Some(fences) = state.typed_id_fences.get(schema) else {
        return Ok(None);
    };
    let index = fences.partition_point(|fence| fence.last.as_str() < id.as_str());
    let Some(fence) = fences.get(index) else {
        return Ok(None);
    };
    if id.as_str() < fence.first.as_str() {
        return Ok(None);
    }
    let segment = state
        .typed_segments
        .get(fence.segment_index)
        .ok_or_else(|| OQLError::storage_error("typed ID fence has no segment", None))?;
    let Some(row) = segment.find_document_id(id)? else {
        return Ok(None);
    };
    Ok(Some((fence.segment_index, row, segment.object_ref(row)?)))
}

/// Whether two states list the same rows under the same ids, however each
/// keeps the list.
fn same_identities(left: &State, right: &State) -> OQLResult<bool> {
    let mut names = left.collection_ids.keys().collect::<Vec<_>>();
    names.extend(right.collection_ids.keys());
    names.sort();
    names.dedup();
    for name in names {
        if logical_collection_identities(left, name)? != logical_collection_identities(right, name)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn logical_live_count(state: &State) -> usize {
    state
        .collection_ids
        .keys()
        .map(|schema| logical_collection_count(state, schema))
        .sum()
}

fn logical_collection_count(state: &State, schema: &str) -> usize {
    if let Some(known) = state
        .collection_counts
        .lock()
        .ok()
        .and_then(|counts| counts.get(schema).copied())
    {
        return known;
    }
    let count = count_collection(state, schema);
    if let Ok(mut counts) = state.collection_counts.lock() {
        counts.insert(schema.to_string(), count);
    }
    count
}

fn count_collection(state: &State, schema: &str) -> usize {
    let base_rows = state
        .typed_id_fences
        .get(schema)
        .into_iter()
        .flatten()
        .map(|fence| state.typed_segments[fence.segment_index].row_count())
        .sum::<usize>();
    let overridden_base_rows = state.typed_delta_rows.get(schema).map_or(0, |rows| {
        rows.keys()
            .filter(|id| {
                base_location_by_id(state, schema, id).is_ok_and(|row| row.is_some())
                    || overflow_base_by_id(state, schema, id).is_ok_and(|row| row.is_some())
            })
            .count()
    });
    base_rows
        .saturating_add(state.overflow_id_counts.get(schema).copied().unwrap_or(0))
        .saturating_sub(overridden_base_rows)
        .saturating_add(state.collection_rows.get(schema).map_or(0, BTreeMap::len))
}

fn object_is_live(state: &State, object_ref: ObjRef) -> OQLResult<bool> {
    Ok(state.object_directory.contains_key(&object_ref)
        || state.typed_row_locations.contains_key(&object_ref)?
        || record_row_is_live(state, object_ref)?)
}

/// Whether this is a live row outside the column segments. One written
/// since the checkpoint is listed in memory. Any other is live while the
/// checkpoint files hold its record and it has not been deleted since.
fn record_row_is_live(state: &State, object_ref: ObjRef) -> OQLResult<bool> {
    if state.object_schema_versions.contains_key(&object_ref) {
        return Ok(true);
    }
    if state.deleted_since_checkpoint.contains(&object_ref) {
        return Ok(false);
    }
    match &state.checkpoint_records {
        Some(files) => files
            .lock()
            .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
            .holds(object_ref),
        None => Ok(false),
    }
}

/// The record of a live row outside the column segments: the one held in
/// memory, or the one in the checkpoint files. `None` when there is no
/// such row.
fn live_record(state: &State, object_ref: ObjRef) -> OQLResult<Option<Arc<[u8]>>> {
    if let Some(record) = state.object_records.get(&object_ref) {
        return Ok(Some(record.clone()));
    }
    let listed = state.object_schema_versions.contains_key(&object_ref);
    if !listed && state.deleted_since_checkpoint.contains(&object_ref) {
        return Ok(None);
    }
    let record = match (&state.checkpoint_records, listed) {
        (Some(_), true) => record_on_file(state, object_ref)?,
        (Some(files), false) => files
            .lock()
            .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
            .record_if_held(object_ref)?,
        (None, _) => None,
    };
    if record.is_none() && listed {
        // Such a row always has its record in memory or in the checkpoint
        // files. Saying it does not exist would lose it silently, so
        // finding it in neither is an error.
        return Err(OQLError::storage_error(
            "a live row is neither decoded nor held as a record",
            None,
        ));
    }
    Ok(record)
}

/// The compiled schema a record was written under, which its header names.
fn schema_of_record<'a>(
    state: &'a State,
    object_ref: ObjRef,
    record: &[u8],
) -> OQLResult<&'a CompiledSchema> {
    let version = RecordCodec::header(record)?.schema_version;
    state
        .compiled_schemas
        .get(&(object_ref.collection_id(), version))
        .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))
}

fn logical_collection_identities(
    state: &State,
    schema: &str,
) -> OQLResult<Vec<(DocumentId, ObjRef)>> {
    let delta = state.typed_delta_rows.get(schema);
    let mut identities = Vec::with_capacity(logical_collection_count(state, schema));
    for fence in state.typed_id_fences.get(schema).into_iter().flatten() {
        let segment = state
            .typed_segments
            .get(fence.segment_index)
            .ok_or_else(|| OQLError::storage_error("typed ID fence has no segment", None))?;
        for row in 0..segment.row_count() {
            let id = segment.document_id(row)?;
            if delta.is_some_and(|rows| rows.contains_key(&id)) {
                continue;
            }
            identities.push((id, segment.object_ref(row)?));
        }
    }
    if let Some(mut on_file) = OverflowIds::new(state, schema) {
        while let Some((id, object_ref)) = on_file.next()? {
            if delta.is_some_and(|rows| rows.contains_key(&id)) {
                continue;
            }
            identities.push((id, object_ref));
        }
    }
    if let Some(rows) = state.collection_rows.get(schema) {
        identities.extend(
            rows.iter()
                .map(|(id, object_ref)| (id.clone(), *object_ref)),
        );
    }
    identities.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(identities)
}

/// The live rows of a collection in id order, which is the order a scan
/// yields them. Rows held in memory are already kept in that order, so only
/// a collection with rows in typed segments needs the ids to merge by.
/// Bring every vector index built for `schema` up to date with one row
/// written or deleted, as of commit `lsn`.
fn keep_vector_indexes(
    state: &mut State,
    schema: &str,
    object_ref: ObjRef,
    document: Option<&Document>,
    lsn: u64,
) {
    let built = state.built_vectors.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner());
    for ((collection, field), index) in built.iter_mut() {
        if collection != schema {
            continue;
        }
        // No search holds the index while a commit is applied; if one
        // somehow did, it keeps the copy it began with.
        let index = Arc::make_mut(index);
        match document.and_then(|document| document.fields.get(field)).and_then(Value::as_vector) {
            Some(vector) => index.put(object_ref, &vector),
            None => index.remove(object_ref),
        }
        index.stamps.0 = lsn;
    }
}

/// Drop the vector indexes built for `schema`: what they were built to
/// has changed.
fn forget_vector_indexes(state: &mut State, schema: &str) {
    state
        .built_vectors
        .get_mut()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|(collection, _), _| collection != schema);
}

/// A vector index for one field of `schema`, built from its rows as they
/// stand. Only the field is read of each row.
fn build_vector_index(
    state: &State,
    schema: &str,
    field: &str,
    dimension: usize,
    metric: crate::vector::Metric,
    stamps: (u64, u64),
) -> OQLResult<Option<crate::storage_engine::VectorIndex>> {
    let Some(mut index) = crate::storage_engine::VectorIndex::new(dimension, metric) else {
        return Ok(None);
    };
    let wanted: HashSet<String> = HashSet::from([field.to_string()]);
    for object_ref in logical_collection_object_refs(state, schema)? {
        let Some(document) = sourced_document_reading(state, object_ref, Some(&wanted))? else {
            continue;
        };
        if let Some(vector) = document.fields.get(field).and_then(Value::as_vector) {
            index.put(object_ref, &vector);
        }
    }
    index.stamps = stamps;
    Ok(Some(index))
}

fn logical_collection_object_refs(state: &State, schema: &str) -> OQLResult<Vec<ObjRef>> {
    if !state.typed_id_fences.contains_key(schema)
        && state.overflow_id_counts.get(schema).copied().unwrap_or(0) == 0
    {
        return Ok(state
            .collection_rows
            .get(schema)
            .into_iter()
            .flat_map(BTreeMap::values)
            .copied()
            .collect());
    }
    if !state.typed_id_fences.contains_key(schema) {
        // No row is in a column segment: the checkpoint's index of rows by
        // id has them in order already. Read through it without making
        // text of each id, slipping in the rows held in memory where their
        // ids fall and leaving out the ones that have changed.
        if let (Some(ids), Some(collection)) =
            (&state.overflow_ids, state.collection_ids.get(schema))
        {
            let changed = state.typed_delta_rows.get(schema);
            let mut resident = state
                .collection_rows
                .get(schema)
                .map(|rows| rows.iter().peekable());
            let mut listed = Vec::with_capacity(logical_collection_count(state, schema));
            let mut not_text = false;
            let (start, end) = id_index_range(*collection);
            ids.visit_range(start, end, &mut |key, rows| {
                let Ok(id) = std::str::from_utf8(key.get(2..).unwrap_or_default()) else {
                    not_text = true;
                    return;
                };
                while let Some((held, object_ref)) = resident.as_mut().and_then(|rows| rows.peek()) {
                    if held.as_str() > id {
                        break;
                    }
                    listed.push(**object_ref);
                    resident.as_mut().and_then(|rows| rows.next());
                }
                if changed.is_some_and(|rows| rows.contains_key(id)) {
                    return;
                }
                listed.extend(rows.first().copied());
            })
            .map_err(|error| OQLError::storage_error(&error, None))?;
            if not_text {
                return Err(OQLError::storage_error("an id in the id index is not text", None));
            }
            listed.extend(resident.into_iter().flatten().map(|(_, object_ref)| *object_ref));
            return Ok(listed);
        }
    }
    if let Some(listed) = segment_rows_with_a_few_newer(state, schema)? {
        #[cfg(test)]
        assert_eq!(
            listed,
            logical_collection_identities(state, schema)?
                .into_iter()
                .map(|(_, object_ref)| object_ref)
                .collect::<Vec<_>>(),
            "the two ways of listing a collection disagree"
        );
        return Ok(listed);
    }
    Ok(logical_collection_identities(state, schema)?
        .into_iter()
        .map(|(_, object_ref)| object_ref)
        .collect())
}

/// The rows of a collection kept in column segments, with the few rows
/// written since slipped in where their ids fall. The segments hold their
/// rows in id order, so an id is read only to find where a newer row
/// goes, not for every row, unless the newer rows are many. `None` when
/// the collection also has rows the checkpoint keeps as records.
fn segment_rows_with_a_few_newer(state: &State, schema: &str) -> OQLResult<Option<Vec<ObjRef>>> {
    let Some(fences) = state.typed_id_fences.get(schema) else {
        return Ok(None);
    };
    if state.overflow_id_counts.get(schema).copied().unwrap_or(0) != 0 {
        return Ok(None);
    }
    let mut segments = Vec::with_capacity(fences.len());
    let mut in_segments = 0usize;
    for fence in fences {
        let segment = state
            .typed_segments
            .get(fence.segment_index)
            .ok_or_else(|| OQLError::storage_error("typed ID fence has no segment", None))?;
        in_segments += segment.row_count();
        segments.push((fence, segment));
    }
    let newer = state.collection_rows.get(schema);
    let newer_rows = newer.map_or(0, BTreeMap::len);
    // Finding where one newer row goes reads about as many ids as the
    // segment has rows, halved over and over. Past this many newer rows
    // it is cheaper to read every id once.
    let few = newer_rows.saturating_mul(16) <= in_segments;
    let mut newer = newer.into_iter().flatten().peekable();
    let mut listed = Vec::with_capacity(in_segments + newer_rows);
    for (fence, segment) in segments {
        if !few {
            for row in 0..segment.row_count() {
                let object_ref = segment.object_ref(row)?;
                if !state
                    .typed_row_locations
                    .still_at(&object_ref, (fence.segment_index, row))?
                {
                    continue;
                }
                if newer.peek().is_some() {
                    let id = segment.document_id(row)?;
                    while let Some((_, newer_ref)) =
                        newer.next_if(|(newer_id, _)| newer_id.as_str() < id.as_str())
                    {
                        listed.push(*newer_ref);
                    }
                }
                listed.push(object_ref);
            }
            continue;
        }
        let rows = segment.row_count();
        let mut from = 0usize;
        let take = |listed: &mut Vec<ObjRef>, rows: std::ops::Range<usize>| -> OQLResult<()> {
            for row in rows {
                let object_ref = segment.object_ref(row)?;
                if state
                    .typed_row_locations
                    .still_at(&object_ref, (fence.segment_index, row))?
                {
                    listed.push(object_ref);
                }
            }
            Ok(())
        };
        while let Some((id, object_ref)) = newer.peek() {
            if id.as_str() > fence.last.as_str() {
                break;
            }
            // The first row of the segment whose id is not below this one.
            let (mut low, mut high) = (from, rows);
            if id.as_str() <= fence.first.as_str() {
                high = low;
            }
            while low < high {
                let middle = low + (high - low) / 2;
                if segment.document_id(middle)?.as_str() < id.as_str() {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            take(&mut listed, from..low)?;
            from = low;
            listed.push(**object_ref);
            newer.next();
        }
        take(&mut listed, from..rows)?;
    }
    listed.extend(newer.map(|(_, object_ref)| *object_ref));
    Ok(Some(listed))
}

fn logical_document(state: &State, object_ref: ObjRef) -> OQLResult<Option<Arc<Document>>> {
    Ok(sourced_document(state, object_ref)?.map(|(document, _)| document))
}

/// Where a document handed out by [`sourced_document`] came from.
enum DocumentSource {
    /// It was already held decoded.
    Held,
    /// It was decoded from a column segment.
    Segment,
    /// It was decoded from this record.
    Record(Arc<[u8]>),
}

/// A row that lives outside the column segments and is not held decoded,
/// decoded from its record. `None` when `object_ref` is not such a row.
/// The record of a row from the checkpoint files, when they hold it.
fn record_on_file(state: &State, object_ref: ObjRef) -> OQLResult<Option<Arc<[u8]>>> {
    state
        .checkpoint_records
        .as_ref()
        .map(|files| {
            files
                .lock()
                .map_err(|_| OQLError::generic("checkpoint read handle lock poisoned"))?
                .record(object_ref)
        })
        .transpose()
        .map(Option::flatten)
}

fn record_document(
    state: &State,
    object_ref: ObjRef,
) -> OQLResult<Option<(Arc<Document>, Arc<[u8]>)>> {
    let Some(record) = live_record(state, object_ref)? else {
        return Ok(None);
    };
    let compiled = schema_of_record(state, object_ref, &record)?;
    Ok(Some((
        Arc::new(RecordCodec::decode(&record, compiled)?),
        record,
    )))
}

/// [`sourced_document`], decoding a row that is read from its record only
/// as far as the fields in `wanted`. A row held decoded, or read from a
/// column segment, comes whole.
fn sourced_document_reading(
    state: &State,
    object_ref: ObjRef,
    wanted: Option<&HashSet<String>>,
) -> OQLResult<Option<Arc<Document>>> {
    let Some(wanted) = wanted else {
        return Ok(sourced_document(state, object_ref)?.map(|(document, _)| document));
    };
    if state.object_directory.contains_key(&object_ref)
        || state.typed_row_locations.get(&object_ref)?.is_some()
    {
        return Ok(sourced_document(state, object_ref)?.map(|(document, _)| document));
    }
    let Some(record) = live_record(state, object_ref)? else {
        return Ok(None);
    };
    let compiled = schema_of_record(state, object_ref, &record)?;
    Ok(Some(Arc::new(RecordCodec::decode_projection(&record, compiled, Some(wanted))?)))
}

/// The fields a scan's filter reads.
fn filter_fields(predicate: &ScanPredicate, fields: &mut HashSet<String>) {
    match predicate {
        ScanPredicate::Comparison { field, .. } => {
            fields.insert(field.clone());
        }
        ScanPredicate::And(left, right) | ScanPredicate::Or(left, right) => {
            filter_fields(left, fields);
            filter_fields(right, fields);
        }
    }
}

/// A row of a column segment that is not held decoded, with only the
/// fields in `wanted` built. `None` for any other row.
fn segment_row_reading(
    state: &State,
    object_ref: ObjRef,
    wanted: &HashSet<String>,
) -> OQLResult<Option<Document>> {
    if state.object_directory.contains_key(&object_ref) {
        return Ok(None);
    }
    let Some((segment_index, row)) = state.typed_row_locations.get(&object_ref)? else {
        return Ok(None);
    };
    let segment = state.typed_segments.get(segment_index).ok_or_else(|| {
        OQLError::storage_error("typed row location references a missing segment", None)
    })?;
    if segment.object_ref(row)? != object_ref {
        return Err(OQLError::storage_error(
            "typed row location conflicts with segment identity",
            None,
        ));
    }
    let compiled = state
        .compiled_schemas
        .get(&(segment.collection_id(), segment.schema_version()))
        .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
    Ok(Some(segment.load()?.decode_document_reading(row, compiled, Some(wanted))?))
}

fn sourced_document(
    state: &State,
    object_ref: ObjRef,
) -> OQLResult<Option<(Arc<Document>, DocumentSource)>> {
    if let Some(document) = state.object_directory.get(&object_ref) {
        return Ok(Some((document.clone(), DocumentSource::Held)));
    }
    let Some((segment_index, row)) = state.typed_row_locations.get(&object_ref)? else {
        return Ok(record_document(state, object_ref)?
            .map(|(document, record)| (document, DocumentSource::Record(record))));
    };
    let segment = state.typed_segments.get(segment_index).ok_or_else(|| {
        OQLError::storage_error("typed row location references a missing segment", None)
    })?;
    if segment.object_ref(row)? != object_ref {
        return Err(OQLError::storage_error(
            "typed row location conflicts with segment identity",
            None,
        ));
    }
    let compiled = state
        .compiled_schemas
        .get(&(segment.collection_id(), segment.schema_version()))
        .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
    Ok(Some((
        Arc::new(segment.load()?.decode_document(row, compiled)?),
        DocumentSource::Segment,
    )))
}

fn logical_object_identity(
    state: &State,
    object_ref: ObjRef,
) -> OQLResult<Option<(String, DocumentId)>> {
    if let Some(document) = state.object_directory.get(&object_ref) {
        return Ok(Some((document.schema.clone(), document.id.clone())));
    }
    let Some((segment_index, row)) = state.typed_row_locations.get(&object_ref)? else {
        return Ok(record_document(state, object_ref)?
            .map(|(document, _)| (document.schema.clone(), document.id.clone())));
    };
    let segment = state.typed_segments.get(segment_index).ok_or_else(|| {
        OQLError::storage_error("typed row location references a missing segment", None)
    })?;
    let schema = state
        .compiled_schemas
        .get(&(segment.collection_id(), segment.schema_version()))
        .map(|compiled| compiled.name.clone())
        .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
    Ok(Some((schema, segment.document_id(row)?)))
}

fn logical_schema_version(state: &State, object_ref: ObjRef) -> OQLResult<Option<u32>> {
    if let Some(version) = state.object_schema_versions.get(&object_ref).copied() {
        return Ok(Some(version));
    }
    if let Some((segment_index, _)) = state.typed_row_locations.get(&object_ref)? {
        return Ok(state
            .typed_segments
            .get(segment_index)
            .map(|segment| segment.schema_version()));
    }
    // A row the checkpoint files hold names its version in its record.
    live_record(state, object_ref)?
        .map(|record| RecordCodec::header(&record).map(|header| header.schema_version))
        .transpose()
}

fn typed_row_remains_checkpoint_authoritative(
    state: &State,
    object_ref: ObjRef,
) -> OQLResult<bool> {
    let Some((segment_index, _)) = state.typed_row_locations.get(&object_ref)? else {
        return Ok(false);
    };
    let Some(segment) = state.typed_segments.get(segment_index) else {
        return Ok(false);
    };
    let Some(compiled) = state
        .compiled_schemas
        .get(&(segment.collection_id(), segment.schema_version()))
    else {
        return Ok(false);
    };
    Ok(segment.authoritative()
        && state.current_schema_versions.get(&compiled.name) == Some(&segment.schema_version())
        && state.schemas.get(&compiled.name).is_some_and(|schema| {
            segment
                .load()
                .is_ok_and(|segment| segment.matches_schema(compiled, schema))
        }))
}

/// A document for a reader holding the state shared. One that had to be
/// decoded from a column segment is noted in `decoded`, to be kept by
/// [`RustStore::keep_decoded`] once the reader has let go.
fn shared_document(
    state: &State,
    object_ref: ObjRef,
    decoded: &mut Vec<Decoded>,
) -> OQLResult<Option<Arc<Document>>> {
    Ok(match sourced_document(state, object_ref)? {
        None => None,
        Some((document, DocumentSource::Held)) => Some(document),
        Some((document, source)) => {
            decoded.push(Decoded {
                read_at: state.committed_lsn,
                object_ref,
                document: document.clone(),
                record: match source {
                    DocumentSource::Record(record) => Some(record),
                    _ => None,
                },
            });
            Some(document)
        }
    })
}

/// A document a reader decoded, and the record it was decoded from when it
/// was not a column segment.
struct Decoded {
    object_ref: ObjRef,
    document: Arc<Document>,
    record: Option<Arc<[u8]>>,
    /// What had been committed when the row was read.
    read_at: u64,
}

fn logical_documents(state: &State) -> OQLResult<Vec<(ObjRef, Arc<Document>)>> {
    let mut object_refs = Vec::with_capacity(logical_live_count(state));
    for schema in state.collection_ids.keys() {
        object_refs.extend(
            logical_collection_identities(state, schema)?
                .into_iter()
                .map(|(_, object_ref)| object_ref),
        );
    }
    object_refs
        .into_iter()
        .map(|object_ref| {
            logical_document(state, object_ref)?
                .map(|document| (object_ref, document))
                .ok_or_else(|| {
                    OQLError::storage_error(
                        "document identity points to a missing logical object",
                        None,
                    )
                })
        })
        .collect()
}

fn logical_document_map(state: &State) -> OQLResult<HashMap<ObjRef, Arc<Document>>> {
    Ok(logical_documents(state)?.into_iter().collect())
}

fn restore_checkpoint(
    image: DecodedCheckpoint,
    filesystem: Arc<dyn FileSystem>,
    decoded_budget: Option<usize>,
    records_on_file: bool,
) -> OQLResult<State> {
    let DecodedCheckpoint {
        sequence: _,
        checkpoint_lsn,
        metadata,
        indexes: checkpoint_indexes,
        records,
        typed_segment_descriptors,
        typed_segments,
        typed_locator,
    } = image;
    if typed_segment_descriptors.len() != typed_segments.len() {
        return Err(OQLError::storage_error(
            "typed checkpoint descriptor count conflicts with decoded segments",
            None,
        ));
    }
    let collection_ids = unique_map(metadata.collection_ids, "collection ID")?;
    let next_object_ids = unique_map(metadata.next_object_ids, "next object ID")?;
    let current_schema_versions =
        unique_map(metadata.current_schema_versions, "current schema version")?;
    let scalar_indexes = unique_map(metadata.scalar_indexes, "scalar index catalog")?;
    let vector_indexes = unique_map(metadata.vector_indexes, "vector index catalog")?;
    let mut schemas = HashMap::new();
    for schema in metadata.schemas {
        if schemas.insert(schema.name.clone(), schema).is_some() {
            return Err(OQLError::storage_error(
                "checkpoint contains duplicate logical schemas",
                None,
            ));
        }
    }
    let mut compiled_schemas = HashMap::new();
    for compiled in metadata.compiled_schemas {
        if collection_ids.get(&compiled.name) != Some(&compiled.collection_id) {
            return Err(OQLError::storage_error(
                "checkpoint compiled schema conflicts with collection catalog",
                None,
            ));
        }
        if compiled_schemas
            .insert((compiled.collection_id, compiled.version), compiled)
            .is_some()
        {
            return Err(OQLError::storage_error(
                "checkpoint contains duplicate compiled schema versions",
                None,
            ));
        }
    }
    for (name, version) in &current_schema_versions {
        let collection_id = collection_ids.get(name).ok_or_else(|| {
            OQLError::storage_error("current schema version has no collection", None)
        })?;
        if !compiled_schemas.contains_key(&(*collection_id, *version)) {
            return Err(OQLError::storage_error(
                "current schema version is missing from checkpoint",
                None,
            ));
        }
    }
    let next_collection_id = metadata.next_collection_id.max(1);
    if collection_ids
        .values()
        .copied()
        .max()
        .is_some_and(|maximum| next_collection_id <= maximum)
    {
        return Err(OQLError::storage_error(
            "checkpoint collection high-water mark overlaps existing collections",
            None,
        ));
    }

    let mut state = State {
        committed_lsn: checkpoint_lsn,
        typed_base_lsn: checkpoint_lsn,
        collection_ids,
        next_collection_id,
        next_object_ids,
        schemas,
        compiled_schemas,
        current_schema_versions,
        scalar_indexes,
        vector_indexes,
        ..State::default()
    };
    let records_started = Instant::now();
    validate_typed_segment_catalog(&state, &typed_segments)?;
    state.typed_segments = typed_segments;
    let installed_fences = typed_locator
        .as_ref()
        .is_some_and(|locator| !locator.fences().is_empty());
    if let Some(locator) = typed_locator.as_ref() {
        install_typed_id_fences(&mut state, locator)?;
    }
    if !installed_fences {
        rebuild_typed_id_fences(&mut state)?;
    }
    let authoritative_rows = state
        .typed_segments
        .iter()
        .filter(|segment| segment.authoritative())
        .map(|segment| segment.row_count())
        .sum();
    state.collection_rows.reserve(state.collection_ids.len());
    let has_persisted_locator = typed_locator.is_some();
    if let Some(locator) = typed_locator {
        if locator.len() != authoritative_rows {
            return Err(OQLError::storage_error(
                "typed locator row count conflicts with authoritative segments",
                None,
            ));
        }
        state.typed_row_locations.install_base(locator);
    } else {
        state.typed_row_locations.reserve(authoritative_rows);
    }
    for (segment_index, segment) in state.typed_segments.iter().enumerate() {
        if !segment.authoritative() {
            continue;
        }
        state
            .compiled_schemas
            .get(&(segment.collection_id(), segment.schema_version()))
            .ok_or_else(|| {
                OQLError::storage_error(
                    "typed checkpoint segment references an unknown compiled schema",
                    None,
                )
            })?;
        if state.typed_row_locations.base.is_some() {
            continue;
        }
        for row in 0..segment.row_count() {
            let object_ref = segment.object_ref(row)?;
            let next = state
                .next_object_ids
                .get(&object_ref.collection_id())
                .copied()
                .ok_or_else(|| {
                    OQLError::storage_error(
                        "typed checkpoint object has no allocator high-water mark",
                        None,
                    )
                })?;
            if next <= object_ref.object_id() {
                return Err(OQLError::storage_error(
                    "checkpoint allocator high-water mark overlaps typed objects",
                    None,
                ));
            }
            if state
                .typed_row_locations
                .insert(object_ref, (segment_index, row))?
                .is_some()
            {
                return Err(OQLError::storage_error(
                    "checkpoint contains duplicate typed-authoritative objects",
                    None,
                ));
            }
        }
    }
    state.decoded_budget = decoded_budget;
    state.decoded_bytes_budget = default_decoded_bytes();
    // A checkpoint that carries the index of its rows by id needs none of
    // them listed in memory.
    let mut ids_on_file = false;
    checkpoint_indexes.again().for_each(filesystem.as_ref(), |page| {
        ids_on_file |= page.schema == REVERSE_INDEX_SCHEMA && page.name == ID_INDEX_NAME;
        Ok(())
    })?;
    // Such a checkpoint, with its records readable from their file, needs
    // nothing kept for each row, so its records are not read at all: a
    // database opens without cost in proportion to its rows. The id index
    // says how many rows each collection has.
    let rows_left_on_file = ids_on_file && records_on_file && state.bounded();
    if !rows_left_on_file {
    records.for_each_chunk(filesystem.as_ref(), 4_096, |records| {
        // With a budget, only as many rows as it has room for are decoded.
        // The rest are known by the schema and id their records state, and
        // decoded if they are ever asked for.
        // When the records can be read from their file later, none is
        // decoded now: opening a database should not cost in proportion to
        // rows nobody has asked for yet, in time or in memory.
        let room = match records_on_file
            || state
                .rows_bytes()
                .is_some_and(|bytes| state.object_directory.bytes() >= bytes)
        {
            true => 0,
            false => state
                .decoded_budget
                .map_or(usize::MAX, |budget| budget.saturating_sub(state.object_directory.len())),
        };
        let records: Vec<(usize, (ObjRef, Arc<[u8]>))> = records.into_iter().enumerate().collect();
        let hydrated = {
            let compiled_schemas = &state.compiled_schemas;
            let next_object_ids = &state.next_object_ids;
            let decode_record = |(position, (object_ref, record)): (usize, (ObjRef, Arc<[u8]>))| {
                let header = RecordCodec::header(&record)?;
                if header.object_ref != object_ref {
                    return Err(OQLError::storage_error(
                        "checkpoint directory identity conflicts with object record",
                        None,
                    ));
                }
                let compiled = compiled_schemas
                    .get(&(object_ref.collection_id(), header.schema_version))
                    .ok_or_else(|| {
                        OQLError::storage_error(
                            "checkpoint object references an unknown compiled schema",
                            None,
                        )
                    })?;
                let (schema, id, document) = if position < room {
                    let document = RecordCodec::decode(&record, compiled)?;
                    (document.schema.clone(), document.id.clone(), Some(document))
                } else {
                    let id = RecordCodec::trusted_view(&record, compiled)?
                        .external_id()?
                        .to_string();
                    (compiled.name.clone(), id, None)
                };
                let next = next_object_ids
                    .get(&object_ref.collection_id())
                    .copied()
                    .ok_or_else(|| {
                        OQLError::storage_error(
                            "checkpoint object has no allocator high-water mark",
                            None,
                        )
                    })?;
                if next <= object_ref.object_id() {
                    return Err(OQLError::storage_error(
                        "checkpoint allocator high-water mark overlaps live objects",
                        None,
                    ));
                }
                Ok((object_ref, record, schema, id, document, header.schema_version))
            };
            if records.len() >= 1_024 {
                records
                    .into_par_iter()
                    .map(decode_record)
                    .collect::<OQLResult<Vec<_>>>()?
            } else {
                records
                    .into_iter()
                    .map(decode_record)
                    .collect::<OQLResult<Vec<_>>>()?
            }
        };
        for (object_ref, record, schema, id, document, schema_version) in hydrated {
            let listed_twice = if ids_on_file {
                *state.overflow_id_counts.entry(schema).or_default() += 1;
                false
            } else {
                lookup_object_ref(&state, &schema, &id)?.is_some()
                    || state
                        .collection_rows
                        .entry(schema)
                        .or_default()
                        .insert(id, object_ref)
                        .is_some()
            };
            if listed_twice
                || {
                    // Past the budget the row is left as its record, which
                    // is held below, and decoded when it is asked for.
                    let room = state
                        .decoded_budget
                        .map_or(true, |budget| state.object_directory.len() < budget)
                        && state
                            .rows_bytes()
                            .map_or(true, |bytes| state.object_directory.bytes() < bytes);
                    match document {
                        Some(document) if room => {
                            state.note_decoded(object_ref);
                            state
                                .object_directory
                                .insert(object_ref, Arc::new(document))
                                .is_some()
                        }
                        _ => false,
                    }
                }
                || state
                    .object_schema_versions
                    .insert(object_ref, schema_version)
                    .is_some()
                // A record is kept in memory unless the database has a
                // budget and can read it from the file it came from.
                || (!(records_on_file && state.bounded())
                    && state.object_records.insert(object_ref, record).is_some())
            {
                return Err(OQLError::storage_error(
                    "checkpoint contains duplicate live objects",
                    None,
                ));
            }
        }
        Ok(())
    })?;
    }
    tracing::debug!(
        target: "owl_db::storage_engine::recovery",
        elapsed_ms = records_started.elapsed().as_secs_f64() * 1_000.0,
        overflow_documents = state.object_directory.len(),
        lazy_typed_rows = state.typed_row_locations.len(),
        "restored checkpoint object directories"
    );
    if !has_persisted_locator {
        validate_typed_checkpoint_segments(&state, &state.typed_segments)?;
    }
    state.typed_complete_collections = complete_typed_collections(&state, &state.typed_segments);
    let indexes_started = Instant::now();
    restore_scalar_indexes(&mut state, checkpoint_indexes, filesystem, rows_left_on_file)?;
    tracing::debug!(
        target: "owl_db::storage_engine::recovery",
        elapsed_ms = indexes_started.elapsed().as_secs_f64() * 1_000.0,
        indexes = state.scalar_index_data.len(),
        "restored checkpoint scalar-index descriptors"
    );
    state.object_records.mark_clean();
    state.forget_counts();
    Ok(state)
}

fn validate_typed_segment_catalog(
    state: &State,
    segments: &[Arc<TypedSegmentHandle>],
) -> OQLResult<()> {
    for segment in segments {
        let compiled = state
            .compiled_schemas
            .get(&(segment.collection_id(), segment.schema_version()))
            .ok_or_else(|| {
                OQLError::storage_error(
                    "typed checkpoint segment references an unknown schema version",
                    None,
                )
            })?;
        let _schema = state.schemas.get(&compiled.name).ok_or_else(|| {
            OQLError::storage_error("typed checkpoint segment has no logical schema", None)
        })?;
        if state.current_schema_versions.get(&compiled.name) != Some(&segment.schema_version())
            || (segment.authoritative() && segment.column_count() != compiled.fields.len())
        {
            return Err(OQLError::storage_error(
                "typed checkpoint segment conflicts with its compiled schema",
                None,
            ));
        }
    }
    Ok(())
}

fn validate_typed_checkpoint_segments(
    state: &State,
    segments: &[Arc<TypedSegmentHandle>],
) -> OQLResult<()> {
    let mut seen = HashSet::new();
    let mut last_ids = HashMap::<(u16, u32), String>::new();
    for (segment_index, segment) in segments.iter().enumerate() {
        let compiled = state
            .compiled_schemas
            .get(&(segment.collection_id(), segment.schema_version()))
            .expect("typed catalog was validated");
        for row in 0..segment.row_count() {
            let object_ref = segment.object_ref(row)?;
            let key = (segment.collection_id(), segment.schema_version());
            let document_id = segment.document_id(row)?;
            let identity_matches =
                lookup_object_ref(state, &compiled.name, &document_id)? == Some(object_ref);
            let row_matches = if segment.authoritative() {
                let already_installed = state
                    .typed_segments
                    .get(segment_index)
                    .is_some_and(|installed| Arc::ptr_eq(installed, segment))
                    && state.typed_row_locations.get(&object_ref)? == Some((segment_index, row));
                (already_installed
                    || logical_metadata_matches(
                        state,
                        object_ref,
                        &compiled.name,
                        &document_id,
                        segment.document_version(row)?,
                        segment.created_at(row)?,
                        segment.updated_at(row)?,
                    )?)
                    && logical_schema_version(state, object_ref)? == Some(segment.schema_version())
            } else {
                logical_document(state, object_ref)?
                    .is_some_and(|document| {
                        document.schema == compiled.name
                            && document.id == document_id
                            && segment
                                .document_version(row)
                                .is_ok_and(|version| document.version == version)
                            && segment
                                .created_at(row)
                                .is_ok_and(|created_at| document.created_at == created_at)
                            && segment
                                .updated_at(row)
                                .is_ok_and(|updated_at| document.updated_at == updated_at)
                    })
            };
            if last_ids
                .insert(key, document_id.clone())
                .is_some_and(|previous| previous >= document_id)
                || !seen.insert(object_ref)
                || object_ref.collection_id() != segment.collection_id()
                || !identity_matches
                || !row_matches
            {
                return Err(OQLError::storage_error(
                    "typed checkpoint segment conflicts with its authoritative row",
                    None,
                ));
            }
        }
    }
    Ok(())
}

fn logical_metadata_matches(
    state: &State,
    object_ref: ObjRef,
    schema: &str,
    id: &str,
    version: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
) -> OQLResult<bool> {
    let held = match state.object_directory.get(&object_ref) {
        Some(document) => Some(document.clone()),
        None if state.typed_row_locations.contains_key(&object_ref)? => None,
        None => match record_document(state, object_ref)? {
            Some((document, _)) => Some(document),
            None => return Ok(false),
        },
    };
    if let Some(document) = held {
        return Ok(document.schema == schema
            && document.id == id
            && document.version == version
            && document.created_at == created_at
            && document.updated_at == updated_at);
    }
    let Some((segment_index, row)) = state.typed_row_locations.get(&object_ref)? else {
        return Ok(false);
    };
    let Some(segment) = state.typed_segments.get(segment_index) else {
        return Ok(false);
    };
    Ok(segment.object_ref(row)? == object_ref
        && segment.document_id(row)? == id
        && segment.document_version(row)? == version
        && segment.created_at(row)? == created_at
        && segment.updated_at(row)? == updated_at
        && state
            .compiled_schemas
            .get(&(segment.collection_id(), segment.schema_version()))
            .is_some_and(|compiled| compiled.name == schema))
}

fn complete_typed_collections(state: &State, segments: &[Arc<TypedSegmentHandle>]) -> HashSet<u16> {
    let mut typed_rows = HashMap::<u16, usize>::new();
    for segment in segments {
        *typed_rows.entry(segment.collection_id()).or_default() += segment.row_count();
    }
    state
        .collection_ids
        .iter()
        .filter_map(|(name, collection_id)| {
            (typed_rows.get(collection_id).copied().unwrap_or(0)
                == logical_collection_count(state, name))
            .then_some(*collection_id)
        })
        .collect()
}

fn unique_map<K, V>(entries: Vec<(K, V)>, name: &str) -> OQLResult<HashMap<K, V>>
where
    K: std::hash::Hash + Eq,
{
    let mut map = HashMap::with_capacity(entries.len());
    for (key, value) in entries {
        if map.insert(key, value).is_some() {
            return Err(OQLError::storage_error(
                &format!("checkpoint contains duplicate {name} entries"),
                None,
            ));
        }
    }
    Ok(map)
}

fn validate_index_definition(index: &ScalarIndexDefinition) -> OQLResult<()> {
    if index.name.is_empty() || index.schema.is_empty() || index.fields.is_empty() {
        return Err(OQLError::storage_error(
            "scalar index requires a name, schema, and at least one field",
            None,
        ));
    }
    if index.schema == REVERSE_INDEX_SCHEMA && index.name == REVERSE_INDEX_NAME {
        return Err(OQLError::storage_error(
            "scalar index identity is reserved for reverse references",
            None,
        ));
    }
    let mut fields = HashSet::new();
    if index.fields.iter().any(|field| !fields.insert(field)) {
        return Err(OQLError::storage_error(
            "scalar index contains a duplicate field",
            None,
        ));
    }
    Ok(())
}

fn document_index_key(document: &Document, index: &ScalarIndexDefinition) -> OQLResult<Vec<u8>> {
    if document.schema != index.schema {
        return Err(OQLError::storage_error(
            "document schema does not match scalar index",
            None,
        ));
    }
    let values = index
        .fields
        .iter()
        .map(|field| document_index_value(document, field).unwrap_or(Value::Null))
        .collect::<Vec<_>>();
    TypedKey::encode(&values)
}

fn document_index_value(document: &Document, path: &str) -> Option<Value> {
    let normalized = path.trim_start_matches('.');
    match normalized {
        "id" => return Some(Value::String(document.id.clone())),
        "schema" => return Some(Value::String(document.schema.clone())),
        "created_at" => return Some(Value::DateTime(document.created_at)),
        "updated_at" => return Some(Value::DateTime(document.updated_at)),
        "version" => return i64::try_from(document.version).ok().map(Value::Integer),
        _ => {}
    }
    let mut parts = normalized.split('.');
    let mut value = document.fields.get(parts.next()?)?;
    for part in parts {
        value = match value {
            Value::Object(fields) => fields.get(part)?,
            _ => return None,
        };
    }
    Some(value.clone())
}

/// What to tell whoever tried to write `candidate`: a refusal in the terms of
/// the schema they wrote, since a repeated value is their mistake to correct
/// and not a fault in storage.
fn unique_violation(index: &ScalarIndexDefinition, candidate: &Document) -> OQLError {
    let held = index
        .fields
        .iter()
        .map(|field| {
            let name = field.trim_start_matches('.');
            match document_index_value(candidate, field) {
                Some(Value::String(text)) => format!("{name} \"{text}\""),
                Some(Value::Integer(integer)) => format!("{name} {integer}"),
                Some(Value::Float(float)) => format!("{name} {float}"),
                Some(Value::Boolean(boolean)) => format!("{name} {boolean}"),
                Some(Value::DateTime(instant)) => format!("{name} {}", instant.to_rfc3339()),
                Some(Value::Null) | None => format!("{name} null"),
                Some(_) => format!("that {name}"),
            }
        })
        .collect::<Vec<_>>()
        .join(" with ");
    let rule = if index.fields.len() == 1 {
        "which must be unique"
    } else {
        "which together must be unique"
    };
    OQLError::SchemaValidation {
        message: format!(
            "'{}' document '{}' has {held}, {rule}, and another document already has it",
            index.schema, candidate.id
        ),
    }
}

fn validate_unique_candidate(
    state: &State,
    planned: &HashMap<ObjRef, Option<Document>>,
    index: &ScalarIndexDefinition,
    candidate_ref: ObjRef,
    candidate: &Document,
) -> OQLResult<()> {
    let candidate_key = document_index_key(candidate, index)?;
    let duplicate = || unique_violation(index, candidate);
    // The index already knows which stored documents hold this key. Only
    // an index this same batch creates, which has no entries yet, leaves
    // every document to be read.
    let indexed = state
        .scalar_index_data
        .get(&(index.schema.clone(), index.name.clone()));
    if let Some(data) = indexed {
        for object_ref in data
            .search_key(&candidate_key)
            .map_err(|error| OQLError::storage_error(&error, None))?
        {
            // A document this batch rewrites or deletes is judged below by
            // what the batch makes of it.
            if object_ref != candidate_ref && !planned.contains_key(&object_ref) {
                return Err(duplicate());
            }
        }
        for (object_ref, document) in planned {
            if *object_ref == candidate_ref {
                continue;
            }
            if let Some(document) = document {
                if document.schema == index.schema
                    && document_index_key(document, index)? == candidate_key
                {
                    return Err(duplicate());
                }
            }
        }
        return Ok(());
    }
    for (object_ref, stored) in logical_documents(state)? {
        if object_ref == candidate_ref {
            continue;
        }
        let visible = match planned.get(&object_ref) {
            Some(Some(document)) => document,
            Some(None) => continue,
            None => &stored,
        };
        if visible.schema == index.schema && document_index_key(visible, index)? == candidate_key {
            return Err(duplicate());
        }
    }
    for (object_ref, document) in planned {
        if *object_ref == candidate_ref || object_is_live(state, *object_ref)? {
            continue;
        }
        if let Some(document) = document {
            if document.schema == index.schema
                && document_index_key(document, index)? == candidate_key
            {
                return Err(duplicate());
            }
        }
    }
    Ok(())
}

fn validate_index_build(
    state: &State,
    planned: &HashMap<ObjRef, Option<Document>>,
    index: &ScalarIndexDefinition,
) -> OQLResult<()> {
    let mut owners = HashMap::<Vec<u8>, ObjRef>::new();
    let mut check = |object_ref: ObjRef, document: &Document| -> OQLResult<()> {
        if document.schema != index.schema {
            return Ok(());
        }
        let key = document_index_key(document, index)?;
        if index.unique && owners.insert(key, object_ref).is_some() {
            return Err(OQLError::storage_error(
                &format!("unique scalar index '{}' has duplicate values", index.name),
                None,
            ));
        }
        Ok(())
    };
    for (object_ref, stored) in logical_documents(state)? {
        match planned.get(&object_ref) {
            Some(Some(document)) => check(object_ref, document)?,
            Some(None) => {}
            None => check(object_ref, &stored)?,
        }
    }
    for (object_ref, document) in planned {
        if !object_is_live(state, *object_ref)? {
            if let Some(document) = document {
                check(*object_ref, document)?;
            }
        }
    }
    Ok(())
}

fn build_scalar_index(state: &State, definition: &ScalarIndexDefinition) -> OQLResult<BTreeIndex> {
    validate_index_definition(definition)?;
    let index = BTreeIndex::create(16, 0).map_err(|error| OQLError::storage_error(&error, None))?;
    for (object_ref, document) in logical_documents(state)? {
        if document.schema == definition.schema {
            index
                .insert_key(
                    document_index_key(&document, definition)?,
                    object_ref,
                    definition.unique,
                )
                .map_err(|error| OQLError::storage_error(&error, None))?;
        }
    }
    Ok(index)
}

/// How many rows of each collection the id index lists, from what its
/// pages say of themselves. Only a page that holds rows of more than one
/// collection is read.
fn count_overflow_ids_by_page(
    state: &State,
    ids: &dyn OrderedIndexBase,
    pages: &[(Option<Vec<u8>>, Option<Vec<u8>>, usize)],
) -> OQLResult<HashMap<String, usize>> {
    let names = state
        .collection_ids
        .iter()
        .map(|(name, collection)| (*collection, name))
        .collect::<HashMap<_, _>>();
    let collection_of = |key: &[u8]| key.get(..2).map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]));
    let mut by_collection = HashMap::<u16, usize>::new();
    for (first, last, postings) in pages {
        let (Some(first), Some(last)) = (first, last) else {
            if *postings != 0 {
                return Err(OQLError::storage_error(
                    "a page of the id index has rows and no keys",
                    None,
                ));
            }
            continue;
        };
        match (collection_of(first), collection_of(last)) {
            (Some(start), Some(end)) if start == end => {
                *by_collection.entry(start).or_default() += postings;
            }
            (Some(_), Some(_)) => {
                let mut short = false;
                ids.visit_range(
                    Bound::Included(first.clone()),
                    Bound::Included(last.clone()),
                    &mut |key, rows| match collection_of(key) {
                        Some(collection) => *by_collection.entry(collection).or_default() += rows.len(),
                        None => short = true,
                    },
                )
                .map_err(|error| OQLError::storage_error(&error, None))?;
                if short {
                    return Err(OQLError::storage_error("a key of the id index is too short", None));
                }
            }
            _ => {
                return Err(OQLError::storage_error("a key of the id index is too short", None));
            }
        }
    }
    by_collection
        .into_iter()
        .map(|(collection, rows)| {
            names
                .get(&collection)
                .map(|name| ((*name).clone(), rows))
                .ok_or_else(|| {
                    OQLError::storage_error("the id index lists a row of an unknown collection", None)
                })
        })
        .collect()
}

fn restore_scalar_indexes(
    state: &mut State,
    snapshots: SegmentIndexReader,
    filesystem: Arc<dyn FileSystem>,
    count_ids: bool,
) -> OQLResult<()> {
    let definitions = state
        .scalar_indexes
        .values()
        .flatten()
        .map(|definition| {
            (
                (definition.schema.clone(), definition.name.clone()),
                definition.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let reverse_identity = (
        REVERSE_INDEX_SCHEMA.to_string(),
        REVERSE_INDEX_NAME.to_string(),
    );
    let ids_identity = (REVERSE_INDEX_SCHEMA.to_string(), ID_INDEX_NAME.to_string());
    let mut page_descriptors = HashMap::<(String, String), Vec<IndexPageDescriptor>>::new();
    let mut previous_keys = HashMap::<(String, String), Vec<u8>>::new();
    let mut posting_counts = HashMap::<(String, String), usize>::new();
    let mut completed = HashSet::with_capacity(definitions.len());
    let mut id_pages = Vec::new();
    let mut page_count = 0usize;
    let mut routing_key_bytes = 0usize;
    snapshots.for_each(filesystem.as_ref(), |page| {
        page_count += 1;
        routing_key_bytes += page.first_key.as_ref().map_or(0, Vec::len)
            + page.last_key.as_ref().map_or(0, Vec::len);
        let identity = (page.schema.clone(), page.name.clone());
        let expected_unique = if identity == reverse_identity {
            false
        } else if identity == ids_identity {
            true
        } else {
            definitions
                .get(&identity)
                .map(|definition| definition.unique)
                .ok_or_else(|| {
                    OQLError::storage_error(
                        "checkpoint contains an unknown scalar index block",
                        None,
                    )
                })?
        };
        if page.unique != expected_unique {
            return Err(OQLError::storage_error(
                "checkpoint index uniqueness conflicts with its catalog definition",
                None,
            ));
        }
        if page.first {
            if page_descriptors
                .insert(identity.clone(), Vec::new())
                .is_some()
            {
                return Err(OQLError::storage_error(
                    "checkpoint contains duplicate scalar index pages",
                    None,
                ));
            }
        }
        if let Some(first_key) = page.first_key.as_ref() {
            if previous_keys
                .get(&identity)
                .is_some_and(|previous| previous >= first_key)
            {
                return Err(OQLError::storage_error(
                    "checkpoint scalar index pages are not strictly ordered",
                    None,
                ));
            }
        }
        if let Some(last_key) = page.last_key.as_ref() {
            previous_keys.insert(identity.clone(), last_key.clone());
        }
        *posting_counts.entry(identity.clone()).or_default() += page.posting_count;
        if count_ids && identity == ids_identity {
            id_pages.push((page.first_key.clone(), page.last_key.clone(), page.posting_count));
        }
        let pages = page_descriptors.get_mut(&identity).ok_or_else(|| {
            OQLError::storage_error("checkpoint scalar index page has no first page", None)
        })?;
        let last = page.last;
        pages.push(page.descriptor()?);
        if last {
            completed.insert(identity);
        }
        Ok(())
    })?;
    if definitions
        .keys()
        .any(|identity| !page_descriptors.contains_key(identity) || !completed.contains(identity))
        || page_descriptors
            .keys()
            .any(|identity| {
                !definitions.contains_key(identity)
                    && identity != &reverse_identity
                    && identity != &ids_identity
            })
        || (page_descriptors.contains_key(&reverse_identity)
            && !completed.contains(&reverse_identity))
        || (page_descriptors.contains_key(&ids_identity) && !completed.contains(&ids_identity))
    {
        return Err(OQLError::storage_error(
            "checkpoint index blocks do not match the scalar index catalog",
            None,
        ));
    }
    // The rows outside the column segments are counted before any index
    // is checked against how many rows its collection has.
    if let Some(pages) = page_descriptors.remove(&ids_identity) {
        let ids: Arc<dyn OrderedIndexBase> = Arc::new(FileIndexBase::new(
            filesystem.clone(),
            ids_identity.0.clone(),
            ids_identity.1.clone(),
            true,
            pages,
            state.index_pages(),
        ));
        if count_ids {
            state.overflow_id_counts = count_overflow_ids_by_page(state, ids.as_ref(), &id_pages)?;
        }
        let listed = posting_counts.get(&ids_identity).copied().unwrap_or(0);
        if listed != state.overflow_id_counts.values().sum::<usize>() {
            return Err(OQLError::storage_error(
                "checkpoint id index does not list every row outside the column segments once",
                None,
            ));
        }
        state.overflow_ids = Some(ids);
    }
    state.forget_counts();
    let mut restored = HashMap::with_capacity(definitions.len());
    for (identity, definition) in &definitions {
        let pages = page_descriptors.remove(identity).ok_or_else(|| {
            OQLError::storage_error("checkpoint is missing scalar index pages", None)
        })?;
        restored.insert(
            identity.clone(),
            Arc::new(BTreeIndex::with_base(Arc::new(FileIndexBase::new(
                filesystem.clone(),
                identity.0.clone(),
                identity.1.clone(),
                definition.unique,
                pages,
                state.index_pages(),
            )))),
        );
    }
    for (identity, definition) in &definitions {
        restored.get(identity).ok_or_else(|| {
            OQLError::storage_error("checkpoint is missing a scalar index block", None)
        })?;
        let expected = logical_collection_count(state, &definition.schema);
        if posting_counts.get(identity).copied().unwrap_or(0) != expected {
            return Err(OQLError::storage_error(
                "checkpoint scalar index does not cover every live object exactly once",
                None,
            ));
        }
    }
    state.reverse_reference_index = if let Some(pages) = page_descriptors.remove(&reverse_identity)
    {
        Arc::new(BTreeIndex::with_base(Arc::new(FileIndexBase::new(
            filesystem,
            reverse_identity.0,
            reverse_identity.1,
            false,
            pages,
            state.index_pages(),
        ))))
    } else {
        build_reverse_reference_index(state)?
    };
    tracing::debug!(
        target: "owl_db::storage_engine::recovery",
        pages = page_count,
        routing_key_bytes,
        postings = posting_counts.values().sum::<usize>(),
        "installed lazy checkpoint index routing"
    );
    state.scalar_index_data = restored;
    Ok(())
}

fn update_document_indexes(
    state: &mut State,
    previous: Option<&Document>,
    current: Option<&Document>,
    object_ref: ObjRef,
) -> OQLResult<()> {
    let schema = current
        .map(|document| document.schema.as_str())
        .or_else(|| previous.map(|document| document.schema.as_str()));
    let Some(schema) = schema else {
        return Ok(());
    };
    let definitions = state
        .scalar_indexes
        .get(schema)
        .cloned()
        .unwrap_or_default();
    for definition in definitions {
        let identity = (definition.schema.clone(), definition.name.clone());
        let index = state.scalar_index_data.get(&identity).ok_or_else(|| {
            OQLError::storage_error("scalar index catalog has no runtime index", None)
        })?;
        if let Some(document) = previous {
            index
                .delete_key(&document_index_key(document, &definition)?, object_ref)
                .map_err(|error| OQLError::storage_error(&error, None))?;
        }
        if let Some(document) = current {
            index
                .insert_key(
                    document_index_key(document, &definition)?,
                    object_ref,
                    definition.unique,
                )
                .map_err(|error| OQLError::storage_error(&error, None))?;
        }
    }
    Ok(())
}

fn sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}", database_path.display(), suffix))
}

fn flush_generation_path(prefix: &Path, sequence: u64) -> PathBuf {
    PathBuf::from(format!("{}.{}", prefix.display(), sequence))
}

fn flush_generation_record_path(prefix: &Path, sequence: u64) -> PathBuf {
    PathBuf::from(format!("{}-record.{sequence}", prefix.display()))
}

fn flush_generation_index_path(prefix: &Path, sequence: u64) -> PathBuf {
    PathBuf::from(format!("{}-index.{sequence}", prefix.display()))
}

fn generation_root_for_manifest(
    filesystem: &dyn FileSystem,
    paths: &[PathBuf; 2],
    manifest: Manifest,
) -> OQLResult<Option<GenerationRoot>> {
    let path = &paths[manifest.slot as usize];
    if !filesystem.exists(path) {
        return Ok(None);
    }
    let bytes = read_exact_file(filesystem, path)?;
    if bytes.len() < 16 {
        return Err(OQLError::storage_error(
            "flush generation root is truncated",
            None,
        ));
    }
    // A full compaction can durably reuse the slot before it reclaims the
    // stale root. Ignore that older root without authenticating it as current.
    if u64::from_le_bytes(bytes[8..16].try_into().unwrap()) != manifest.sequence {
        return Ok(None);
    }
    let root = GenerationRoot::decode(&bytes)?;
    if root.sequence != manifest.sequence || root.checkpoint_lsn != manifest.checkpoint_lsn {
        // A full compaction may reuse a slot that still contains an older
        // generation root. Only an exact sequence match can be authoritative.
        return Ok(None);
    }
    Ok(Some(root))
}

fn cleanup_unpublished_flush_generation(
    filesystem: &dyn FileSystem,
    manifest_paths: &[PathBuf; 2],
    root_paths: &[PathBuf; 2],
    prefix: &Path,
) -> OQLResult<()> {
    let Some(manifest) = latest_manifest(filesystem, manifest_paths)? else {
        return Ok(());
    };
    let candidate_sequence = manifest.sequence.saturating_add(1);
    let candidate = flush_generation_path(prefix, candidate_sequence);
    let record_candidate = flush_generation_record_path(prefix, candidate_sequence);
    let index_candidate = flush_generation_index_path(prefix, candidate_sequence);
    // A checkpoint that wrote its list of generations and died before its
    // manifest leaves that list in the slot the next checkpoint will use.
    // The next checkpoint takes the same sequence, so the list would pass
    // for its own and name generation files that are gone.
    let unpublished_root = &root_paths[1 - (manifest.slot as usize & 1)];
    if filesystem.exists(unpublished_root) {
        let bytes = read_exact_file(filesystem, unpublished_root)?;
        let names_unpublished = bytes.len() < 16
            || u64::from_le_bytes(bytes[8..16].try_into().unwrap()) > manifest.sequence;
        if names_unpublished {
            filesystem.remove(unpublished_root).map_err(|error| {
                storage_io("remove unpublished generation root", unpublished_root, error)
            })?;
        }
    }
    if !filesystem.exists(&candidate)
        && !filesystem.exists(&record_candidate)
        && !filesystem.exists(&index_candidate)
    {
        return Ok(());
    }
    let referenced =
        generation_root_for_manifest(filesystem, root_paths, manifest)?.is_some_and(|root| {
            root.generations
                .iter()
                .any(|generation| generation.sequence == candidate_sequence)
        });
    if !referenced {
        if filesystem.exists(&candidate) {
            filesystem.remove(&candidate).map_err(|error| {
                storage_io("remove unpublished flush generation", &candidate, error)
            })?;
        }
        if filesystem.exists(&record_candidate) {
            filesystem.remove(&record_candidate).map_err(|error| {
                storage_io(
                    "remove unpublished flush generation records",
                    &record_candidate,
                    error,
                )
            })?;
        }
        if filesystem.exists(&index_candidate) {
            filesystem.remove(&index_candidate).map_err(|error| {
                storage_io(
                    "remove unpublished flush generation indexes",
                    &index_candidate,
                    error,
                )
            })?;
        }
        filesystem
            .sync_all()
            .map_err(|error| storage_io("sync unpublished generation cleanup", prefix, error))?;
    }
    Ok(())
}

fn open_writable_wal(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<Box<dyn FileHandle>> {
    let mut wal = filesystem
        .open_with_options(
            path,
            &FileOptions {
                read: true,
                write: true,
                create: false,
                truncate: false,
                append: false,
            },
        )
        .map_err(|error| storage_io("open WAL", path, error))?;
    let length = wal
        .size()
        .map_err(|error| storage_io("inspect WAL", path, error))?;
    wal.seek(SeekFrom::Start(length))
        .map_err(|error| storage_io("seek WAL", path, error))?;
    Ok(wal)
}

fn create_synced_wal(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<Box<dyn FileHandle>> {
    let mut wal = filesystem
        .create(path)
        .map_err(|error| storage_io("create WAL", path, error))?;
    wal.sync()
        .map_err(|error| storage_io("sync new WAL", path, error))?;
    Ok(wal)
}

fn truncate_recovered_wal(
    filesystem: &dyn FileSystem,
    path: &Path,
    valid_length: u64,
) -> OQLResult<()> {
    let current_length = filesystem
        .file_size(path)
        .map_err(|error| storage_io("inspect recovered WAL", path, error))?;
    if current_length == valid_length {
        return Ok(());
    }
    let mut wal = open_writable_wal(filesystem, path)?;
    wal.set_len(valid_length)
        .and_then(|_| wal.sync())
        .map_err(|error| storage_io("truncate torn WAL tail", path, error))
}

fn write_synced_file(filesystem: &dyn FileSystem, path: &Path, bytes: &[u8]) -> OQLResult<()> {
    // A checkpoint that failed part of the way is tried again, and writes
    // the same files under the same names. One that already holds these
    // bytes is left as it is and only synced: a manifest written by the
    // first attempt may have reached the disk though the attempt failed,
    // and emptying a file it names, to fill it again, would leave the
    // database unreadable if power went in between.
    if filesystem.exists(path)
        && filesystem
            .file_size(path)
            .is_ok_and(|length| length == bytes.len() as u64)
        && read_exact_file(filesystem, path).is_ok_and(|held| held == bytes)
    {
        return filesystem
            .open_with_options(
                path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .and_then(|mut file| file.sync())
            .map_err(|error| storage_io("sync checkpoint file", path, error));
    }
    let mut file = filesystem
        .create(path)
        .map_err(|error| storage_io("create checkpoint file", path, error))?;
    write_all(file.as_mut(), bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync())
        .map_err(|error| storage_io("write checkpoint file", path, error))
}

fn remove_reclaimed_file(
    filesystem: &dyn FileSystem,
    path: &Path,
    report: &mut ReclamationReport,
) -> OQLResult<()> {
    if !filesystem.exists(path) {
        return Ok(());
    }
    let bytes = filesystem
        .file_size(path)
        .map_err(|error| storage_io("inspect obsolete storage file", path, error))?;
    filesystem
        .remove(path)
        .map_err(|error| storage_io("reclaim obsolete storage file", path, error))?;
    report.files_removed = report.files_removed.saturating_add(1);
    report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(bytes);
    Ok(())
}

fn inspect_integrity(
    database_path: &Path,
    filesystem: Arc<dyn FileSystem>,
) -> OQLResult<(IntegrityReport, State)> {
    // The files are read as a store reads them: a checkpoint kept as a
    // root over parts as one file.
    let filesystem = crate::storage_engine::parts::PartedFileSystem::over(filesystem);
    let sealed_wal_path = sidecar_path(database_path, "wal.sealed");
    let manifest_paths = [
        sidecar_path(database_path, "manifest.0"),
        sidecar_path(database_path, "manifest.1"),
    ];
    let segment_paths = [
        sidecar_path(database_path, "segment.0"),
        sidecar_path(database_path, "segment.1"),
    ];
    let generation_root_paths = [
        sidecar_path(database_path, "generations.0"),
        sidecar_path(database_path, "generations.1"),
    ];
    let generation_prefix = sidecar_path(database_path, "generation");
    let record_delta_paths = [
        sidecar_path(database_path, "record-delta.0"),
        sidecar_path(database_path, "record-delta.1"),
    ];
    let wal_delta_paths = [
        sidecar_path(database_path, "wal-delta.0"),
        sidecar_path(database_path, "wal-delta.1"),
    ];
    let index_delta_paths = [
        sidecar_path(database_path, "index-delta.0"),
        sidecar_path(database_path, "index-delta.1"),
    ];
    if !filesystem.exists(database_path)
        && !manifest_paths.iter().any(|path| filesystem.exists(path))
        && !filesystem.exists(&sealed_wal_path)
    {
        return Err(OQLError::storage_error(
            &format!(
                "no Owl storage files exist for '{}'",
                database_path.display()
            ),
            None,
        ));
    }
    let manifest = latest_manifest(filesystem.as_ref(), &manifest_paths)?;
    let (checkpoint_state, sequence) = RustStore::recover_checkpoint(
        filesystem.clone(),
        &manifest_paths,
        &segment_paths,
        &generation_root_paths,
        &generation_prefix,
        &wal_delta_paths,
        &record_delta_paths,
        &index_delta_paths,
        SegmentVerification::Audit,
        None,
    )?;
    let checkpoint_lsn = checkpoint_state.committed_lsn;
    let (checkpoint_state, sealed_len) =
        RustStore::recover(filesystem.as_ref(), &sealed_wal_path, checkpoint_state, WalTail::Synced)?;
    require_complete_wal(filesystem.as_ref(), &sealed_wal_path, sealed_len)?;
    let (state, active_len) =
        RustStore::recover(filesystem.as_ref(), database_path, checkpoint_state, WalTail::Synced)?;
    require_complete_wal(filesystem.as_ref(), database_path, active_len)?;
    validate_state_integrity(&state)?;

    let mut files_checked = usize::from(filesystem.exists(database_path))
        + usize::from(filesystem.exists(&sealed_wal_path));
    if let Some(manifest) = manifest {
        files_checked += 2; // selected manifest and base segment
        if let Some(root) =
            generation_root_for_manifest(filesystem.as_ref(), &generation_root_paths, manifest)?
        {
            for generation in &root.generations {
                let record_path =
                    flush_generation_record_path(&generation_prefix, generation.sequence);
                let bytes = read_exact_file(filesystem.as_ref(), &record_path)?;
                let records = RecordPageDelta::decode(&bytes)?;
                if bytes.len() as u64 != generation.record_len
                    || RecordPageDelta::authority_checksum(&bytes)? != generation.record_checksum
                    || records.base_lsn != generation.start_lsn - 1
                    || records.target_lsn != generation.end_lsn
                {
                    return Err(OQLError::storage_error(
                        "flush generation record root failed its integrity audit",
                        None,
                    ));
                }
                if generation.index_len > 0 {
                    let index_path =
                        flush_generation_index_path(&generation_prefix, generation.sequence);
                    let metadata =
                        visit_index_root_file(filesystem.as_ref(), &index_path, |_| Ok(()))?;
                    if metadata.base_lsn != generation.start_lsn - 1
                        || metadata.target_lsn != generation.end_lsn
                    {
                        return Err(OQLError::storage_error(
                            "flush generation index root failed its integrity audit",
                            None,
                        ));
                    }
                }
            }
            files_checked += 1 + root
                .generations
                .iter()
                .map(|generation| 2 + usize::from(generation.index_len > 0))
                .sum::<usize>();
        } else {
            files_checked += usize::from(manifest.has_record_root);
            files_checked += usize::from(manifest.has_index_root);
            files_checked += usize::from(manifest.delta_len > 0);
        }
    }
    let scalar_indexes = state.scalar_index_data.len();
    Ok((
        IntegrityReport {
            checkpoint_sequence: sequence,
            checkpoint_lsn,
            durable_lsn: state.committed_lsn,
            live_objects: logical_live_count(&state),
            scalar_indexes,
            files_checked,
        },
        state,
    ))
}

fn require_complete_wal(filesystem: &dyn FileSystem, path: &Path, valid_len: u64) -> OQLResult<()> {
    let actual_len = if filesystem.exists(path) {
        filesystem
            .file_size(path)
            .map_err(|error| storage_io("inspect WAL integrity", path, error))?
    } else {
        0
    };
    if valid_len != actual_len {
        return Err(OQLError::storage_error(
            &format!(
                "WAL '{}' has a torn or unauthenticated tail",
                path.display()
            ),
            None,
        ));
    }
    Ok(())
}

fn validate_state_integrity(state: &State) -> OQLResult<()> {
    for (schema, id, object_ref) in state
        .collection_rows
        .iter()
        .flat_map(|(schema, rows)| rows.iter().map(move |(id, object_ref)| (schema, id, object_ref)))
    {
        let document = logical_document(state, *object_ref)?.ok_or_else(|| {
            OQLError::storage_error("document identity points to a missing object", None)
        })?;
        if &document.schema != schema || &document.id != id || object_ref.is_null() {
            return Err(OQLError::storage_error(
                "document identity conflicts with its object directory entry",
                None,
            ));
        }
        if state.collection_ids.get(schema).copied() != Some(object_ref.collection_id()) {
            return Err(OQLError::storage_error(
                "object reference collection conflicts with its schema",
                None,
            ));
        }
        if state
            .collection_rows
            .get(schema)
            .and_then(|rows| rows.get(id))
            != Some(object_ref)
        {
            return Err(OQLError::storage_error(
                "document identity is missing from ordered collection directory",
                None,
            ));
        }
    }
    for (object_ref, document) in &state.object_directory {
        // A decoded copy of a row a column segment holds is only a cache;
        // the segment's own directory answers for it.
        if state.typed_row_locations.contains_key(object_ref)? {
            continue;
        }
        if lookup_object_ref(state, &document.schema, &document.id)?.as_ref() != Some(object_ref) {
            return Err(OQLError::storage_error(
                "object directory entry has no matching document identity",
                None,
            ));
        }
    }
    validate_runtime_scalar_indexes(state)?;

    let mut reverse_references = derive_reverse_references(state)?;
    let mut indexed_references = materialize_reverse_reference_index(state)?;
    for references in reverse_references.values_mut() {
        references.sort_by(|left, right| {
            (&left.source_schema, &left.source_id, &left.field_path).cmp(&(
                &right.source_schema,
                &right.source_id,
                &right.field_path,
            ))
        });
    }
    for references in indexed_references.values_mut() {
        references.sort_by(|left, right| {
            (&left.source_schema, &left.source_id, &left.field_path).cmp(&(
                &right.source_schema,
                &right.source_id,
                &right.field_path,
            ))
        });
    }
    if reverse_references != indexed_references {
        return Err(OQLError::storage_error(
            "reverse-reference index conflicts with live documents",
            None,
        ));
    }
    Ok(())
}

fn read_exact_file(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<Vec<u8>> {
    let length = filesystem
        .file_size(path)
        .map_err(|error| storage_io("inspect checkpoint file", path, error))?;
    let length = usize::try_from(length)
        .map_err(|_| OQLError::storage_error("checkpoint file is too large to map", None))?;
    let mut file = filesystem
        .open(path)
        .map_err(|error| storage_io("open checkpoint file", path, error))?;
    let mut bytes = vec![0; length];
    if read_up_to(file.as_mut(), &mut bytes)
        .map_err(|error| storage_io("read checkpoint file", path, error))?
        != length
    {
        return Err(OQLError::storage_error(
            "checkpoint file changed or was truncated while reading",
            None,
        ));
    }
    Ok(bytes)
}

fn checksum_file_path(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<u32> {
    let mut file = filesystem
        .open(path)
        .map_err(|error| storage_io("open checkpoint file for checksum", path, error))?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = read_up_to(file.as_mut(), &mut buffer)
            .map_err(|error| storage_io("read checkpoint file for checksum", path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize())
}

fn latest_manifest(
    filesystem: &dyn FileSystem,
    manifest_paths: &[PathBuf; 2],
) -> OQLResult<Option<Manifest>> {
    let mut manifests = Vec::new();
    for (slot, path) in manifest_paths.iter().enumerate() {
        if !filesystem.exists(path) {
            continue;
        }
        let length = filesystem
            .file_size(path)
            .map_err(|error| storage_io("inspect checkpoint manifest", path, error))?;
        if length < 64 {
            continue;
        }
        if length != 64 {
            return Err(OQLError::storage_error(
                &format!(
                    "checkpoint manifest '{}' has an invalid length",
                    path.display()
                ),
                None,
            ));
        }
        let manifest = Manifest::decode(&read_exact_file(filesystem, path)?)?;
        if manifest.slot as usize != slot {
            return Err(OQLError::storage_error(
                "checkpoint manifest is stored in the wrong slot",
                None,
            ));
        }
        manifests.push(manifest);
    }
    Ok(manifests
        .into_iter()
        .max_by_key(|manifest| manifest.sequence))
}

fn ensure_collection_id(
    schema: &str,
    collection_ids: &mut HashMap<String, u16>,
    next_collection_id: &mut u16,
) -> OQLResult<u16> {
    if let Some(id) = collection_ids.get(schema).copied() {
        return Ok(id);
    }
    if *next_collection_id == 0 {
        return Err(OQLError::storage_error(
            "collection identifier space exhausted",
            None,
        ));
    }
    let id = *next_collection_id;
    *next_collection_id = next_collection_id
        .checked_add(1)
        .ok_or_else(|| OQLError::storage_error("collection identifier space exhausted", None))?;
    collection_ids.insert(schema.to_string(), id);
    Ok(id)
}

fn current_compiled<'a>(
    schema: &str,
    current_versions: &HashMap<String, u32>,
    compiled_schemas: &'a HashMap<(u16, u32), CompiledSchema>,
    collection_id: u16,
) -> Option<&'a CompiledSchema> {
    current_versions
        .get(schema)
        .and_then(|version| compiled_schemas.get(&(collection_id, *version)))
}

#[derive(Debug)]
enum CompiledScanField {
    User(Option<usize>),
    Id(Option<usize>),
    Schema,
    CreatedAt,
    UpdatedAt,
    Version,
}

#[derive(Debug)]
enum CompiledScanPredicate {
    Comparison {
        field: CompiledScanField,
        operator: ComparisonOperator,
        value: Value,
    },
    And(Box<CompiledScanPredicate>, Box<CompiledScanPredicate>),
    Or(Box<CompiledScanPredicate>, Box<CompiledScanPredicate>),
}

#[derive(Debug)]
enum TypedScanField {
    User(Option<u16>),
    Id,
    Schema,
    CreatedAt,
    UpdatedAt,
    Version,
}

#[derive(Debug)]
enum TypedScanPredicate {
    Comparison {
        field: TypedScanField,
        operator: ComparisonOperator,
        value: Value,
    },
    And(Box<TypedScanPredicate>, Box<TypedScanPredicate>),
    Or(Box<TypedScanPredicate>, Box<TypedScanPredicate>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SegmentMatch {
    None,
    All,
    Rows,
}

fn bitmap_covers_rows(bitmap: &[u8], rows: usize) -> bool {
    bitmap
        .iter()
        .map(|byte| byte.count_ones() as usize)
        .sum::<usize>()
        == rows
}

fn typed_segment_match(
    segment: &TypedSegment,
    schema: &str,
    predicate: &TypedScanPredicate,
) -> SegmentMatch {
    match predicate {
        TypedScanPredicate::Comparison {
            field: TypedScanField::Schema,
            operator,
            value,
        } => {
            let equal = matches!(value, Value::String(value) if value == schema);
            match operator {
                ComparisonOperator::Equal => {
                    if equal {
                        SegmentMatch::All
                    } else {
                        SegmentMatch::None
                    }
                }
                ComparisonOperator::NotEqual => {
                    if equal {
                        SegmentMatch::None
                    } else {
                        SegmentMatch::All
                    }
                }
                _ => SegmentMatch::Rows,
            }
        }
        TypedScanPredicate::Comparison {
            field: TypedScanField::User(Some(field_id)),
            operator,
            value: Value::Integer(value),
        } => {
            let Some(column) = segment.column(*field_id) else {
                return SegmentMatch::Rows;
            };
            let (Some(minimum), Some(maximum)) = (column.integer_min, column.integer_max) else {
                return SegmentMatch::Rows;
            };
            let complete = bitmap_covers_rows(&column.presence, segment.row_count())
                && !column.nulls.iter().any(|byte| *byte != 0);
            match operator {
                ComparisonOperator::Equal if *value < minimum || *value > maximum => {
                    SegmentMatch::None
                }
                ComparisonOperator::Equal if complete && minimum == *value && maximum == *value => {
                    SegmentMatch::All
                }
                ComparisonOperator::NotEqual if *value < minimum || *value > maximum => {
                    SegmentMatch::All
                }
                ComparisonOperator::NotEqual
                    if complete && minimum == *value && maximum == *value =>
                {
                    SegmentMatch::None
                }
                ComparisonOperator::LessThan if *value <= minimum => SegmentMatch::None,
                ComparisonOperator::LessThan if complete && maximum < *value => SegmentMatch::All,
                ComparisonOperator::LessEqual if *value < minimum => SegmentMatch::None,
                ComparisonOperator::LessEqual if complete && maximum <= *value => SegmentMatch::All,
                ComparisonOperator::GreaterThan if *value >= maximum => SegmentMatch::None,
                ComparisonOperator::GreaterThan if complete && minimum > *value => {
                    SegmentMatch::All
                }
                ComparisonOperator::GreaterEqual if *value > maximum => SegmentMatch::None,
                ComparisonOperator::GreaterEqual if complete && minimum >= *value => {
                    SegmentMatch::All
                }
                _ => SegmentMatch::Rows,
            }
        }
        TypedScanPredicate::And(left, right) => {
            match (
                typed_segment_match(segment, schema, left),
                typed_segment_match(segment, schema, right),
            ) {
                (SegmentMatch::None, _) | (_, SegmentMatch::None) => SegmentMatch::None,
                (SegmentMatch::All, SegmentMatch::All) => SegmentMatch::All,
                _ => SegmentMatch::Rows,
            }
        }
        TypedScanPredicate::Or(left, right) => {
            match (
                typed_segment_match(segment, schema, left),
                typed_segment_match(segment, schema, right),
            ) {
                (SegmentMatch::All, _) | (_, SegmentMatch::All) => SegmentMatch::All,
                (SegmentMatch::None, SegmentMatch::None) => SegmentMatch::None,
                _ => SegmentMatch::Rows,
            }
        }
        _ => SegmentMatch::Rows,
    }
}

fn compile_typed_scan_predicate(
    predicate: &ScanPredicate,
    compiled: &CompiledSchema,
    segments: &[Arc<TypedSegment>],
) -> Option<TypedScanPredicate> {
    match predicate {
        ScanPredicate::Comparison {
            field,
            operator,
            value,
        } => {
            let field = match DocumentSystemField::from_name(field) {
                Some(DocumentSystemField::Id) => TypedScanField::Id,
                Some(DocumentSystemField::Schema) => TypedScanField::Schema,
                Some(DocumentSystemField::CreatedAt) => TypedScanField::CreatedAt,
                Some(DocumentSystemField::UpdatedAt) => TypedScanField::UpdatedAt,
                Some(DocumentSystemField::Version) => TypedScanField::Version,
                None => {
                    let field_id = compiled
                        .fields
                        .iter()
                        .find(|candidate| candidate.name == *field)
                        .map(|field| field.id);
                    if field_id.is_some_and(|field_id| {
                        segments
                            .iter()
                            .any(|segment| segment.column(field_id).is_none())
                    }) {
                        return None;
                    }
                    TypedScanField::User(field_id)
                }
            };
            Some(TypedScanPredicate::Comparison {
                field,
                operator: operator.clone(),
                value: value.clone(),
            })
        }
        ScanPredicate::And(left, right) => Some(TypedScanPredicate::And(
            Box::new(compile_typed_scan_predicate(left, compiled, segments)?),
            Box::new(compile_typed_scan_predicate(right, compiled, segments)?),
        )),
        ScanPredicate::Or(left, right) => Some(TypedScanPredicate::Or(
            Box::new(compile_typed_scan_predicate(left, compiled, segments)?),
            Box::new(compile_typed_scan_predicate(right, compiled, segments)?),
        )),
    }
}

fn typed_cell_equal(left: TypedCell<'_>, right: &Value) -> bool {
    match (left, right) {
        (TypedCell::Missing | TypedCell::Null, Value::Null) => true,
        (TypedCell::String(a), Value::String(b)) => a == b,
        (TypedCell::Integer(a), Value::Integer(b)) => a == *b,
        (TypedCell::Float(a), Value::Float(b)) => a == *b,
        // Numeric equality crosses the integer/float boundary, matching
        // `crate::value_semantics` and the ordered comparisons below.
        (TypedCell::Integer(a), Value::Float(_)) => {
            crate::value_semantics::equal(&Value::Integer(a), right)
        }
        (TypedCell::Float(a), Value::Integer(_)) => {
            crate::value_semantics::equal(&Value::Float(a), right)
        }
        (TypedCell::Boolean(a), Value::Boolean(b)) => a == *b,
        (TypedCell::DateTime(a), Value::DateTime(b)) => a == *b,
        (TypedCell::Reference(a), Value::Reference(b)) => a == *b,
        _ => false,
    }
}

fn typed_cell_less(left: TypedCell<'_>, right: &Value) -> bool {
    match (left, right) {
        (TypedCell::Integer(a), Value::Integer(b)) => a < *b,
        (TypedCell::Float(a), Value::Float(b)) => a < *b,
        (TypedCell::Integer(a), Value::Float(b)) => (a as f64) < *b,
        (TypedCell::Float(a), Value::Integer(b)) => a < (*b as f64),
        (TypedCell::String(a), Value::String(b)) => a < b.as_str(),
        (TypedCell::DateTime(a), Value::DateTime(b)) => a < *b,
        // Timestamps compare with timestamp strings by instant, never by
        // text; see `crate::value_semantics`.
        (TypedCell::DateTime(a), Value::String(_)) => {
            crate::value_semantics::compare(&Value::DateTime(a), right)
                == Some(std::cmp::Ordering::Less)
        }
        (TypedCell::String(a), Value::DateTime(_)) => {
            crate::value_semantics::compare(&Value::String(a.to_string()), right)
                == Some(std::cmp::Ordering::Less)
        }
        (TypedCell::Boolean(a), Value::Boolean(b)) => !a && *b,
        _ => false,
    }
}

fn typed_value_less(left: &Value, right: TypedCell<'_>) -> bool {
    match (left, right) {
        (Value::Integer(a), TypedCell::Integer(b)) => *a < b,
        (Value::Float(a), TypedCell::Float(b)) => *a < b,
        (Value::Integer(a), TypedCell::Float(b)) => (*a as f64) < b,
        (Value::Float(a), TypedCell::Integer(b)) => *a < (b as f64),
        (Value::String(a), TypedCell::String(b)) => a.as_str() < b,
        (Value::DateTime(a), TypedCell::DateTime(b)) => *a < b,
        (Value::DateTime(_), TypedCell::String(b)) => {
            crate::value_semantics::compare(left, &Value::String(b.to_string()))
                == Some(std::cmp::Ordering::Less)
        }
        (Value::String(_), TypedCell::DateTime(b)) => {
            crate::value_semantics::compare(left, &Value::DateTime(b))
                == Some(std::cmp::Ordering::Less)
        }
        (Value::Boolean(a), TypedCell::Boolean(b)) => !a && b,
        _ => false,
    }
}

fn typed_scan_predicate_matches(
    segment: &TypedSegment,
    row: usize,
    schema: &str,
    predicate: &TypedScanPredicate,
) -> OQLResult<bool> {
    match predicate {
        TypedScanPredicate::Comparison {
            field,
            operator,
            value,
        } => {
            let left = match field {
                TypedScanField::User(Some(field_id)) => segment
                    .column(*field_id)
                    .expect("typed predicate columns were prevalidated")
                    .cell(row)?,
                TypedScanField::User(None) => TypedCell::Missing,
                TypedScanField::Id => TypedCell::String(&segment.document_ids[row]),
                TypedScanField::Schema => TypedCell::String(schema),
                TypedScanField::CreatedAt => TypedCell::DateTime(segment.created_at[row]),
                TypedScanField::UpdatedAt => TypedCell::DateTime(segment.updated_at[row]),
                TypedScanField::Version => i64::try_from(segment.document_versions[row])
                    .map(TypedCell::Integer)
                    .unwrap_or(TypedCell::Null),
            };
            Ok(match operator {
                ComparisonOperator::Equal => typed_cell_equal(left, value),
                ComparisonOperator::NotEqual => !typed_cell_equal(left, value),
                ComparisonOperator::LessThan => typed_cell_less(left, value),
                ComparisonOperator::LessEqual => {
                    typed_cell_equal(left, value) || typed_cell_less(left, value)
                }
                ComparisonOperator::GreaterThan => typed_value_less(value, left),
                ComparisonOperator::GreaterEqual => {
                    typed_cell_equal(left, value) || typed_value_less(value, left)
                }
                _ => false,
            })
        }
        TypedScanPredicate::And(left, right) => {
            Ok(typed_scan_predicate_matches(segment, row, schema, left)?
                && typed_scan_predicate_matches(segment, row, schema, right)?)
        }
        TypedScanPredicate::Or(left, right) => {
            Ok(typed_scan_predicate_matches(segment, row, schema, left)?
                || typed_scan_predicate_matches(segment, row, schema, right)?)
        }
    }
}

fn compile_scan_predicate(
    predicate: &ScanPredicate,
    field_slots: &HashMap<&str, usize>,
) -> CompiledScanPredicate {
    match predicate {
        ScanPredicate::Comparison {
            field,
            operator,
            value,
        } => {
            let slot = field_slots.get(field.as_str()).copied();
            let field = match DocumentSystemField::from_name(field) {
                Some(DocumentSystemField::Id) => CompiledScanField::Id(slot),
                Some(DocumentSystemField::Schema) => CompiledScanField::Schema,
                Some(DocumentSystemField::CreatedAt) => CompiledScanField::CreatedAt,
                Some(DocumentSystemField::UpdatedAt) => CompiledScanField::UpdatedAt,
                Some(DocumentSystemField::Version) => CompiledScanField::Version,
                None => CompiledScanField::User(slot),
            };
            CompiledScanPredicate::Comparison {
                field,
                operator: operator.clone(),
                value: value.clone(),
            }
        }
        ScanPredicate::And(left, right) => CompiledScanPredicate::And(
            Box::new(compile_scan_predicate(left, field_slots)),
            Box::new(compile_scan_predicate(right, field_slots)),
        ),
        ScanPredicate::Or(left, right) => CompiledScanPredicate::Or(
            Box::new(compile_scan_predicate(left, field_slots)),
            Box::new(compile_scan_predicate(right, field_slots)),
        ),
    }
}

fn borrowed_value_equal(left: &BorrowedRecordValue<'_>, right: &Value) -> bool {
    match (left, right) {
        (BorrowedRecordValue::Missing | BorrowedRecordValue::Null, Value::Null) => true,
        (BorrowedRecordValue::String(a), Value::String(b)) => *a == b,
        (BorrowedRecordValue::Integer(a), Value::Integer(b)) => a == b,
        (BorrowedRecordValue::Float(a), Value::Float(b)) => a == b,
        (BorrowedRecordValue::Boolean(a), Value::Boolean(b)) => a == b,
        (BorrowedRecordValue::DateTime(a), Value::DateTime(b)) => a == b,
        (BorrowedRecordValue::Reference(a), Value::Reference(b)) => a == b,
        (BorrowedRecordValue::Owned(a), b) => a == b,
        _ => false,
    }
}

fn borrowed_value_less(left: &BorrowedRecordValue<'_>, right: &Value) -> bool {
    match (left, right) {
        (BorrowedRecordValue::Integer(a), Value::Integer(b)) => a < b,
        (BorrowedRecordValue::Float(a), Value::Float(b)) => a < b,
        (BorrowedRecordValue::Integer(a), Value::Float(b)) => (*a as f64) < *b,
        (BorrowedRecordValue::Float(a), Value::Integer(b)) => *a < (*b as f64),
        (BorrowedRecordValue::String(a), Value::String(b)) => *a < b.as_str(),
        (BorrowedRecordValue::DateTime(a), Value::DateTime(b)) => a < b,
        (BorrowedRecordValue::DateTime(a), Value::String(b)) => a.to_rfc3339() < *b,
        (BorrowedRecordValue::String(a), Value::DateTime(b)) => *a < b.to_rfc3339().as_str(),
        (BorrowedRecordValue::Boolean(a), Value::Boolean(b)) => !a && *b,
        (BorrowedRecordValue::Owned(a), b) => {
            let owned = BorrowedRecordValue::Owned(b.clone());
            scan_owned_value_less(a, &owned)
        }
        _ => false,
    }
}

fn scan_owned_value_less(left: &Value, right: &BorrowedRecordValue<'_>) -> bool {
    match (left, right) {
        (Value::Integer(a), BorrowedRecordValue::Integer(b)) => a < b,
        (Value::Float(a), BorrowedRecordValue::Float(b)) => a < b,
        (Value::Integer(a), BorrowedRecordValue::Float(b)) => (*a as f64) < *b,
        (Value::Float(a), BorrowedRecordValue::Integer(b)) => *a < (*b as f64),
        (Value::String(a), BorrowedRecordValue::String(b)) => a.as_str() < *b,
        (Value::DateTime(a), BorrowedRecordValue::DateTime(b)) => a < b,
        (Value::DateTime(a), BorrowedRecordValue::String(b)) => a.to_rfc3339().as_str() < *b,
        (Value::String(a), BorrowedRecordValue::DateTime(b)) => *a < b.to_rfc3339(),
        (Value::Boolean(a), BorrowedRecordValue::Boolean(b)) => !a && *b,
        (a, BorrowedRecordValue::Owned(b)) => scan_predicate_owned_less(a, b),
        _ => false,
    }
}

fn scan_predicate_owned_less(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => a < b,
        (Value::Float(a), Value::Float(b)) => a < b,
        (Value::Integer(a), Value::Float(b)) => (*a as f64) < *b,
        (Value::Float(a), Value::Integer(b)) => *a < (*b as f64),
        (Value::String(a), Value::String(b)) => a < b,
        (Value::DateTime(a), Value::DateTime(b)) => a < b,
        (Value::DateTime(a), Value::String(b)) => a.to_rfc3339() < *b,
        (Value::String(a), Value::DateTime(b)) => *a < b.to_rfc3339(),
        (Value::Boolean(a), Value::Boolean(b)) => !a && *b,
        _ => false,
    }
}

fn compiled_scan_predicate_matches(
    view: &RecordView<'_>,
    schema: &CompiledSchema,
    predicate: &CompiledScanPredicate,
) -> OQLResult<bool> {
    match predicate {
        CompiledScanPredicate::Comparison {
            field,
            operator,
            value,
        } => {
            let left = match field {
                CompiledScanField::User(Some(slot)) => view.field_at(*slot)?,
                CompiledScanField::User(None) => BorrowedRecordValue::Missing,
                CompiledScanField::Id(Some(slot)) => match view.field_at(*slot)? {
                    BorrowedRecordValue::Missing => {
                        BorrowedRecordValue::String(view.external_id()?)
                    }
                    value => value,
                },
                CompiledScanField::Id(None) => BorrowedRecordValue::String(view.external_id()?),
                CompiledScanField::Schema => BorrowedRecordValue::String(&schema.name),
                CompiledScanField::CreatedAt => BorrowedRecordValue::DateTime(view.created_at()?),
                CompiledScanField::UpdatedAt => BorrowedRecordValue::DateTime(view.updated_at()?),
                CompiledScanField::Version => i64::try_from(view.version())
                    .map(BorrowedRecordValue::Integer)
                    .unwrap_or(BorrowedRecordValue::Null),
            };
            Ok(match operator {
                ComparisonOperator::Equal => borrowed_value_equal(&left, value),
                ComparisonOperator::NotEqual => !borrowed_value_equal(&left, value),
                ComparisonOperator::LessThan => borrowed_value_less(&left, value),
                ComparisonOperator::LessEqual => {
                    borrowed_value_equal(&left, value) || borrowed_value_less(&left, value)
                }
                ComparisonOperator::GreaterThan => scan_owned_value_less(value, &left),
                ComparisonOperator::GreaterEqual => {
                    borrowed_value_equal(&left, value) || scan_owned_value_less(value, &left)
                }
                _ => false,
            })
        }
        CompiledScanPredicate::And(left, right) => {
            Ok(compiled_scan_predicate_matches(view, schema, left)?
                && compiled_scan_predicate_matches(view, schema, right)?)
        }
        CompiledScanPredicate::Or(left, right) => {
            Ok(compiled_scan_predicate_matches(view, schema, left)?
                || compiled_scan_predicate_matches(view, schema, right)?)
        }
    }
}

fn decoded_scan_predicate_matches(document: &Document, predicate: &ScanPredicate) -> bool {
    match predicate {
        ScanPredicate::Comparison {
            field,
            operator,
            value,
        } => {
            let system_value =
                DocumentSystemField::from_name(field).map(|field| field.value(document));
            let left = system_value
                .as_ref()
                .or_else(|| document.fields.get(field))
                .unwrap_or(&Value::Null);
            match operator {
                ComparisonOperator::Equal => left == value,
                ComparisonOperator::NotEqual => left != value,
                ComparisonOperator::LessThan => scan_predicate_owned_less(left, value),
                ComparisonOperator::LessEqual => {
                    left == value || scan_predicate_owned_less(left, value)
                }
                ComparisonOperator::GreaterThan => scan_predicate_owned_less(value, left),
                ComparisonOperator::GreaterEqual => {
                    left == value || scan_predicate_owned_less(value, left)
                }
                _ => false,
            }
        }
        ScanPredicate::And(left, right) => {
            decoded_scan_predicate_matches(document, left)
                && decoded_scan_predicate_matches(document, right)
        }
        ScanPredicate::Or(left, right) => {
            decoded_scan_predicate_matches(document, left)
                || decoded_scan_predicate_matches(document, right)
        }
    }
}

fn scan_typed_base_with_row_delta(
    state: &State,
    schema: &str,
    filters: &[ScanPredicate],
    fields: &[String],
    batch_rows: usize,
) -> OQLResult<Option<Vec<ProjectedRowBatch>>> {
    let (Some(collection_id), Some(schema_version)) = (
        state.collection_ids.get(schema).copied(),
        state.current_schema_versions.get(schema).copied(),
    ) else {
        return Ok(None);
    };
    if !state.typed_complete_collections.contains(&collection_id) {
        return Ok(None);
    }
    let segment_handles = state
        .typed_segments
        .iter()
        .filter(|segment| {
            segment.collection_id() == collection_id && segment.schema_version() == schema_version
        })
        .cloned()
        .collect::<Vec<_>>();
    if segment_handles.is_empty() {
        return Ok(None);
    }
    let segments = segment_handles
        .iter()
        .map(|segment| segment.load())
        .collect::<OQLResult<Vec<_>>>()?;
    let compiled = state
        .compiled_schemas
        .get(&(collection_id, schema_version))
        .ok_or_else(|| OQLError::storage_error("typed delta scan has no compiled schema", None))?;
    let Some(typed_filters) = filters
        .iter()
        .map(|filter| compile_typed_scan_predicate(filter, compiled, &segments))
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(None);
    };

    let mut typed_projection_ids = Vec::with_capacity(fields.len());
    let mut typed_projection_supported = true;
    for name in fields {
        if DocumentSystemField::from_name(name).is_some() {
            typed_projection_ids.push(None);
            continue;
        }
        let field_id = compiled
            .fields
            .iter()
            .find(|field| field.name == *name)
            .map(|field| field.id);
        if field_id.is_some_and(|field_id| {
            segments
                .iter()
                .any(|segment| segment.column(field_id).is_none())
        }) {
            typed_projection_supported = false;
            break;
        }
        typed_projection_ids.push(field_id);
    }
    let filter_modes = segments
        .iter()
        .map(|segment| {
            typed_filters
                .iter()
                .map(|filter| typed_segment_match(segment, schema, filter))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    enum PendingBatch {
        Typed {
            segment_index: usize,
            batch: ProjectedRowBatch,
        },
        Shared(ProjectedRowBatch),
    }

    fn flush_pending(pending: &mut Option<PendingBatch>, batches: &mut Vec<ProjectedRowBatch>) {
        if let Some(pending) = pending.take() {
            let batch = match pending {
                PendingBatch::Typed { batch, .. } | PendingBatch::Shared(batch) => batch,
            };
            if !batch.is_empty() {
                batches.push(batch);
            }
        }
    }

    let batch_rows = batch_rows.max(1);
    let mut batches = Vec::new();
    let mut pending = None;
    let mut segment_index = 0usize;
    let mut typed_row = 0usize;
    let empty_delta = BTreeMap::new();
    let mut delta = state
        .typed_delta_rows
        .get(schema)
        .unwrap_or(&empty_delta)
        .iter()
        .peekable();

    loop {
        while segment_index < segments.len() && typed_row == segments[segment_index].row_count() {
            segment_index += 1;
            typed_row = 0;
        }
        let base = (segment_index < segments.len()).then(|| {
            (
                segments[segment_index].document_ids[typed_row].as_str(),
                segments[segment_index].object_refs[typed_row],
            )
        });
        let delta_row = delta
            .peek()
            .map(|(id, object_ref)| (id.as_str(), **object_ref));
        let (typed_location, delta_object_ref) = match (base, delta_row) {
            (None, None) => break,
            (Some(_), None) => {
                let location = (segment_index, typed_row);
                typed_row += 1;
                (Some(location), None)
            }
            (None, Some(_)) => {
                let (_, object_ref) = delta.next().expect("peeked delta row");
                let Some(object_ref) = *object_ref else {
                    continue;
                };
                (None, Some(object_ref))
            }
            (Some((base_id, _)), Some((delta_id, _))) => match base_id.cmp(delta_id) {
                std::cmp::Ordering::Less => {
                    let location = (segment_index, typed_row);
                    typed_row += 1;
                    (Some(location), None)
                }
                std::cmp::Ordering::Equal => {
                    typed_row += 1;
                    let (_, object_ref) = delta.next().expect("peeked delta row");
                    let Some(object_ref) = *object_ref else {
                        continue;
                    };
                    (None, Some(object_ref))
                }
                std::cmp::Ordering::Greater => {
                    let (_, object_ref) = delta.next().expect("peeked delta row");
                    let Some(object_ref) = *object_ref else {
                        continue;
                    };
                    (None, Some(object_ref))
                }
            },
        };

        if let Some((typed_segment_index, row)) = typed_location {
            let segment = &segments[typed_segment_index];
            let mut matches = true;
            for (filter, mode) in typed_filters.iter().zip(&filter_modes[typed_segment_index]) {
                if *mode == SegmentMatch::Rows
                    && !typed_scan_predicate_matches(segment, row, schema, filter)?
                {
                    matches = false;
                    break;
                }
                if *mode == SegmentMatch::None {
                    matches = false;
                    break;
                }
            }
            if !matches {
                continue;
            }

            if typed_projection_supported {
                let needs_new = !matches!(
                    pending.as_ref(),
                    Some(PendingBatch::Typed {
                        segment_index,
                        batch,
                    }) if *segment_index == typed_segment_index && batch.len() < batch_rows
                );
                if needs_new {
                    flush_pending(&mut pending, &mut batches);
                    pending = Some(PendingBatch::Typed {
                        segment_index: typed_segment_index,
                        batch: ProjectedRowBatch::typed(
                            schema.to_string(),
                            fields,
                            typed_projection_ids.clone(),
                            segment.clone(),
                            batch_rows.min(segment.row_count()),
                        ),
                    });
                }
                if let Some(PendingBatch::Typed { batch, .. }) = &mut pending {
                    batch.push_typed(row);
                }
            } else {
                let object_ref = segment.object_refs[row];
                let document = logical_document(state, object_ref)?.ok_or_else(|| {
                    OQLError::storage_error(
                        "typed delta scan row disappeared from the document directory",
                        None,
                    )
                })?;
                let needs_new = !matches!(
                    pending.as_ref(),
                    Some(PendingBatch::Shared(batch)) if batch.len() < batch_rows
                );
                if needs_new {
                    flush_pending(&mut pending, &mut batches);
                    pending = Some(PendingBatch::Shared(
                        ProjectedRowBatch::shared_with_capacity(
                            schema.to_string(),
                            fields,
                            batch_rows,
                        ),
                    ));
                }
                if let Some(PendingBatch::Shared(batch)) = &mut pending {
                    batch.push_shared(document);
                }
            }
        } else {
            let object_ref = delta_object_ref.expect("merged row has one source");
            let document = logical_document(state, object_ref)?.ok_or_else(|| {
                OQLError::storage_error(
                    "typed delta row disappeared from the document directory",
                    None,
                )
            })?;
            if !filters
                .iter()
                .all(|filter| decoded_scan_predicate_matches(&document, filter))
            {
                continue;
            }
            let needs_new = !matches!(
                pending.as_ref(),
                Some(PendingBatch::Shared(batch)) if batch.len() < batch_rows
            );
            if needs_new {
                flush_pending(&mut pending, &mut batches);
                pending = Some(PendingBatch::Shared(
                    ProjectedRowBatch::shared_with_capacity(schema.to_string(), fields, batch_rows),
                ));
            }
            if let Some(PendingBatch::Shared(batch)) = &mut pending {
                batch.push_shared(document);
            }
        }
        let full = match pending.as_ref() {
            Some(PendingBatch::Typed { batch, .. }) | Some(PendingBatch::Shared(batch)) => {
                batch.len() == batch_rows
            }
            None => false,
        };
        if full {
            flush_pending(&mut pending, &mut batches);
        }
    }
    flush_pending(&mut pending, &mut batches);
    Ok(Some(batches))
}

impl RustStore {
    /// The columns of `fields`, and with `with_rows` the rows they are of.
    /// Columns are built from the held rows when first asked for and kept
    /// until the next commit.
    fn rows_and_columns(
        &self,
        schema: &str,
        fields: &[String],
        with_rows: bool,
        for_once: bool,
        vectors: bool,
    ) -> OQLResult<Option<(Option<Arc<Vec<Arc<Document>>>>, ColumnBatch)>> {
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }
        // Held for the whole of this, so that every column is of the same
        // rows: nothing is committed while they are read.
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let Some(declared) = state.schemas.get(schema) else {
            return Ok(None);
        };
        // A vector's column is given only to a caller that asks for one.
        let holds_vector = |name: &String| {
            declared.fields.get(name).is_some_and(|field| field.vector_dimension().is_some())
        };
        if !vectors && fields.iter().any(holds_vector) {
            return Ok(None);
        }
        let mut built = self
            .built_columns
            .lock()
            .map_err(|_| OQLError::generic("column cache lock poisoned"))?;
        let held_rows = |state: &State| -> OQLResult<Option<Vec<Arc<Document>>>> {
            let object_refs = logical_collection_object_refs(state, schema)?;
            let mut held = Vec::with_capacity(object_refs.len());
            for object_ref in object_refs {
                match sourced_document(state, object_ref)? {
                    Some((document, DocumentSource::Held)) => held.push(document),
                    Some(_) => return Ok(None),
                    None => {}
                }
            }
            Ok(Some(held))
        };
        let pointer = std::mem::size_of::<Arc<Document>>();
        {
            if !built.of(&state, schema).asks() {
                return Ok(None);
            }
        }
        let bounded = state.bounded();
        // The collection's rows, listed, when every one of them is held.
        // With no limit on decoded rows the list is kept as it is. With
        // one it is kept without holding the rows, and is good while they
        // are all still held.
        let listing = |built: &mut BuiltColumns| -> OQLResult<Option<Arc<Vec<Arc<Document>>>>> {
            let kept = built.of(&state, schema);
            if let Some(rows) = &kept.rows {
                return Ok(Some(rows.clone()));
            }
            if let Some(weak) = kept.weak_rows.clone() {
                let rows: Option<Vec<Arc<Document>>> = weak.iter().map(std::sync::Weak::upgrade).collect();
                match rows {
                    Some(rows) => return Ok(Some(Arc::new(rows))),
                    None => kept.weak_rows = None,
                }
            }
            let Some(held) = held_rows(&state)? else {
                return Ok(None);
            };
            let held = Arc::new(held);
            let bytes = held.len() * pointer;
            if !built.makes_room(schema, bytes) {
                return Ok(Some(held));
            }
            let kept = built.of(&state, schema);
            match bounded {
                true => kept.weak_rows = Some(Arc::new(held.iter().map(Arc::downgrade).collect())),
                false => kept.rows = Some(held.clone()),
            }
            kept.bytes += bytes;
            Ok(Some(held))
        };
        let missing_any = {
            let kept = built.of(&state, schema);
            fields.iter().any(|name| !kept.columns.contains_key(name))
        };
        let mut rows: Option<Arc<Vec<Arc<Document>>>> = None;
        if with_rows || missing_any {
            rows = listing(&mut built)?;
        }
        // Not every row is held. Without a limit on decoded rows that is a
        // collection in column segments, which another way reads. With one,
        // the columns are made in a pass that decodes a row, takes its
        // fields and lets it go, and what is kept is which row is where.
        if (with_rows || missing_any) && rows.is_none() {
            if !bounded || with_rows {
                return Ok(None);
            }
            let kept = built.of(&state, schema);
            let missing: Vec<&String> =
                fields.iter().filter(|name| !kept.columns.contains_key(*name)).collect();
            if !missing.is_empty() {
                // Columns already found to be more than may be kept are
                // not made again, unless the caller has no other way.
                let wanted: Vec<String> = missing.iter().map(|name| (*name).clone()).collect();
                if !for_once {
                    if let Some(known) = kept.too_big.get(&wanted).copied() {
                        if kept.bytes + known > built.limit {
                            return Ok(None);
                        }
                    }
                }
                let kept = built.of(&state, schema);
                let refs = match &kept.refs {
                    Some(refs) => refs.clone(),
                    None => Arc::new(logical_collection_object_refs(&state, schema)?),
                };
                let mut builders = Vec::with_capacity(missing.len());
                for name in missing {
                    let Some(builder) = declared
                        .fields
                        .get(name)
                        .and_then(|field_type| ColumnBuilder::new(name, field_type, refs.len(), vectors))
                    else {
                        return Ok(None);
                    };
                    builders.push(builder);
                }
                for object_ref in refs.iter() {
                    // Every listed row must be there, or positions in these
                    // columns would not be positions in the list.
                    let Some((document, _)) = sourced_document(&state, *object_ref)? else {
                        return Ok(None);
                    };
                    for builder in &mut builders {
                        builder.push(&document)?;
                    }
                }
                let mut made = Vec::with_capacity(builders.len());
                let mut bytes = refs.len() * std::mem::size_of::<ObjRef>();
                for builder in builders {
                    let name = builder.name().to_string();
                    let Some(column) = builder.finish()? else {
                        return Ok(None);
                    };
                    bytes += column.bytes();
                    made.push((name, Arc::new(column)));
                }
                if !built.makes_room(schema, bytes) {
                    // More than may be kept between queries, which is
                    // remembered: a caller with another way is told so at
                    // once the next time, where making these again for
                    // every query would cost it a pass over the collection
                    // each time.
                    built.of(&state, schema).too_big.insert(wanted, bytes);
                    if !for_once {
                        return Ok(None);
                    }
                    // For one whose other way is to read every row, they
                    // are made already: given for this once and not kept.
                    let kept = built.of(&state, schema);
                    let mut columns = Vec::with_capacity(fields.len());
                    for name in fields {
                        let column = made
                            .iter()
                            .find(|(made, _)| made == name)
                            .map(|(_, column)| column.clone())
                            .or_else(|| kept.columns.get(name).cloned());
                        match column {
                            Some(column) => columns.push(column),
                            None => return Ok(None),
                        }
                    }
                    let Some(count) = columns.first().map(|column| column.rows()) else {
                        return Ok(None);
                    };
                    return Ok(Some((None, ColumnBatch::new(kept.stamp, count, columns))));
                }
                let kept = built.of(&state, schema);
                if kept.refs.is_none() {
                    kept.refs = Some(refs);
                }
                kept.columns.extend(made);
                kept.bytes += bytes;
            }
            let kept = built.of(&state, schema);
            let mut columns = Vec::with_capacity(fields.len());
            for name in fields {
                match kept.columns.get(name) {
                    Some(column) => columns.push(column.clone()),
                    None => return Ok(None),
                }
            }
            let Some(count) = columns.first().map(|column| column.rows()) else {
                return Ok(None);
            };
            return Ok(Some((None, ColumnBatch::new(kept.stamp, count, columns))));
        }
        let mut columns = Vec::with_capacity(fields.len());
        for name in fields {
            if let Some(known) = built.of(&state, schema).columns.get(name) {
                columns.push(known.clone());
                continue;
            }
            let Some(field_type) = declared.fields.get(name) else {
                return Ok(None);
            };
            let Some(listed) = rows.as_deref() else {
                return Ok(None);
            };
            let Some(column) = TypedColumn::of_rows(name, field_type, listed.as_slice(), vectors)?
            else {
                return Ok(None);
            };
            let bytes = column.bytes();
            let column = Arc::new(column);
            if !built.makes_room(schema, bytes) {
                // More than may be kept. A search has no cheaper way, so
                // it is given the column for this once.
                if !vectors {
                    return Ok(None);
                }
                columns.push(column);
                continue;
            }
            let kept = built.of(&state, schema);
            // Making room may have dropped the rows list this column is
            // of; then the column is used for this query and not kept.
            // Under a budget no list is kept, and the rows are listed in
            // the same order each time until the collection is written.
            if bounded
                || kept.rows.as_ref().is_some_and(|listed| rows.as_ref().is_some_and(|rows| Arc::ptr_eq(listed, rows)))
            {
                kept.columns.insert(name.clone(), column.clone());
                kept.bytes += bytes;
            }
            columns.push(column);
        }
        let Some(count) = columns.first().map(|column| column.rows()) else {
            return Ok(None);
        };
        let stamp = built.of(&state, schema).stamp;
        Ok(Some((rows.filter(|_| with_rows), ColumnBatch::new(stamp, count, columns))))
    }
}

impl RustStore {
    /// The documents that refer to one document, through one field when it
    /// is named. With `read`, a row that would have to be decoded is
    /// decoded only as far as those fields.
    fn referencing_documents(
        &self,
        target_schema: &str,
        target_id: &DocumentId,
        source_schema: Option<&str>,
        field_path: Option<&str>,
        read: Option<&HashSet<String>>,
    ) -> OQLResult<Vec<Arc<Document>>> {
        if !self.pending_operations()?.is_empty() {
            return Ok(self
                .scan_referencing_documents(target_schema, target_id, source_schema, field_path)?
                .into_iter()
                .map(Arc::new)
                .collect());
        }
        // One pass under one lock: the postings, each source's identity, and
        // the document the store already holds for it.
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let Some(target) = lookup_object_ref(&state, target_schema, target_id)? else {
            return Ok(Vec::new());
        };
        let mut decoded = Vec::new();
        let prefix = target.packed().to_be_bytes().to_vec();
        let end = target
            .packed()
            .checked_add(1)
            .map(|next| Bound::Excluded(next.to_be_bytes().to_vec()))
            .unwrap_or(Bound::Unbounded);
        // The postings under this target, through the wanted field when one
        // is named. The keys are only looked at, never kept.
        let mut rows = Vec::new();
        state
            .reverse_reference_index
            .visit_range(Bound::Included(prefix), end, &mut |key, source| {
                if field_path.is_none_or(|expected| {
                    key.get(8..).unwrap_or_default() == expected.as_bytes()
                }) {
                    rows.push(source);
                }
            })
            .map_err(|error| OQLError::storage_error(&error, None))?;
        let mut found = Vec::with_capacity(rows.len());
        let mut documents = Vec::with_capacity(rows.len());
        // A reference says which collection its document is in, so a
        // posting from another collection is passed over unread.
        let wanted_collection = match source_schema {
            Some(expected) => match state.collection_ids.get(expected) {
                Some(collection) => Some(*collection),
                None => return Ok(Vec::new()),
            },
            None => None,
        };
        for source in rows {
            if wanted_collection.is_some_and(|collection| collection != source.collection_id()) {
                continue;
            }
            // A source already held decoded is the current version of its
            // document: it is entered under its id and removed with it, in
            // one step.
            if let Some(document) = state.object_directory.get(&source) {
                found.push(source);
                documents.push(document.clone());
                continue;
            }
            // One that is not held is decoded once: to learn which
            // document it is, and to be the answer when it is that
            // document's current version, as it nearly always is.
            // When only some fields will be read and the row is a record,
            // only those are decoded, and the row is not kept: a row with
            // long lists in it is otherwise built whole to be counted.
            let partly = match (read, state.typed_row_locations.contains_key(&source)?) {
                (Some(read), false) => {
                    match live_record(&state, source)? {
                        Some(record) => {
                            let compiled = schema_of_record(&state, source, &record)?;
                            Some(Arc::new(RecordCodec::decode_projection(&record, compiled, Some(read))?))
                        }
                        None => None,
                    }
                }
                _ => None,
            };
            let document = match partly {
                Some(document) => document,
                None => match shared_document(&state, source, &mut decoded)? {
                    Some(document) => document,
                    None => continue,
                },
            };
            let current = match lookup_object_ref(&state, &document.schema, &document.id)? {
                Some(current) => current,
                None => continue,
            };
            let document = match current == source {
                true => Some(document),
                false => shared_document(&state, current, &mut decoded)?,
            };
            if let Some(document) = document {
                found.push(current);
                documents.push(document);
            }
        }
        // Each document once, where it first appears. Two postings naming
        // one document are rare, so that is looked for before paying to
        // remove them.
        let mut ordered = found.clone();
        ordered.sort_unstable();
        if ordered.windows(2).any(|pair| pair[0] == pair[1]) {
            let mut seen = HashSet::with_capacity(found.len());
            let mut current = found.iter();
            documents.retain(|_| current.next().is_some_and(|current| seen.insert(*current)));
        }
        drop(state);
        self.keep_decoded(decoded)?;
        Ok(documents)
    }
}

impl DocumentStore for RustStore {
    fn supports_storage_owned_scan(&self) -> bool {
        true
    }

    fn has_pending_writes(&self) -> OQLResult<bool> {
        let Some(id) = self.current_transaction() else {
            return Ok(false);
        };
        Ok(self
            .transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?
            .get(&id)
            .is_some_and(|transaction| !transaction.operations.is_empty()))
    }

    fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>> {
        let mut decoded = Vec::new();
        let mut document = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let object_ref = lookup_object_ref(&state, schema, id)?;
            object_ref
                .map(|object_ref| shared_document(&state, object_ref, &mut decoded))
                .transpose()?
                .flatten()
                .map(|document| document.as_ref().clone())
        };
        self.keep_decoded(decoded)?;
        for operation in self.pending_operations()? {
            match operation {
                WriteOperation::PutDocument(pending)
                    if pending.schema == schema && pending.id == *id =>
                {
                    document = Some(pending);
                }
                WriteOperation::DeleteDocument {
                    schema: pending_schema,
                    id: pending_id,
                } if pending_schema == schema && pending_id == *id => {
                    document = None;
                }
                _ => {}
            }
        }
        Ok(document)
    }

    fn put_document(&self, document: &Document) -> OQLResult<()> {
        self.queue_or_commit(WriteOperation::PutDocument(document.clone()))
    }

    fn put_documents(&self, documents: &[Document]) -> OQLResult<()> {
        self.queue_or_commit_many(
            documents
                .iter()
                .cloned()
                .map(WriteOperation::PutDocument)
                .collect(),
        )
    }

    fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool> {
        let exists = self.get_document(schema, id)?.is_some();
        if !exists {
            return Ok(false);
        }
        self.queue_or_commit(WriteOperation::DeleteDocument {
            schema: schema.to_string(),
            id: id.clone(),
        })?;
        Ok(true)
    }

    fn scan_documents(&self, schema: &str) -> OQLResult<Vec<Document>> {
        let pending = self.pending_operations()?;
        if pending.is_empty() {
            let documents = {
                let state = self
                    .state
                    .read()
                    .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
                logical_collection_object_refs(&state, schema)?
                    .into_iter()
                    .map(|object_ref| {
                        logical_document(&state, object_ref).and_then(|document| {
                            document
                                .map(|document| document.as_ref().clone())
                                .ok_or_else(|| {
                                    OQLError::storage_error(
                                        "ordered row points to a missing logical object",
                                        None,
                                    )
                                })
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?
            };
            return Ok(documents);
        }

        let mut by_id = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            logical_collection_object_refs(&state, schema)?
                .iter()
                .map(|object_ref| {
                    logical_document(&state, *object_ref).and_then(|document| {
                        document
                            .map(|document| document.as_ref().clone())
                            .ok_or_else(|| {
                                OQLError::storage_error(
                                    "document identity points to a missing logical object",
                                    None,
                                )
                            })
                    })
                })
                .collect::<OQLResult<Vec<_>>>()?
                .into_iter()
                .map(|document| (document.id.clone(), document))
                .collect::<HashMap<_, _>>()
        };
        for operation in pending {
            match operation {
                WriteOperation::PutDocument(document) if document.schema == schema => {
                    by_id.insert(document.id.clone(), document);
                }
                WriteOperation::DeleteDocument {
                    schema: pending_schema,
                    id,
                } if pending_schema == schema => {
                    by_id.remove(&id);
                }
                _ => {}
            }
        }
        let mut documents = by_id.into_values().collect::<Vec<_>>();
        documents.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(documents)
    }

    fn scan_documents_projection(
        &self,
        schema: &str,
        fields: &[String],
    ) -> OQLResult<Vec<Document>> {
        fn project(document: &Document, fields: &[String]) -> Document {
            let projected = fields
                .iter()
                .filter_map(|field| {
                    document
                        .fields
                        .get(field)
                        .cloned()
                        .map(|value| (field.clone(), value))
                })
                .collect();
            document.with_projected_fields(projected)
        }

        let pending = self.pending_operations()?;
        if pending.is_empty() {
            let documents = {
                let state = self
                    .state
                    .read()
                    .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
                logical_collection_object_refs(&state, schema)?
                    .into_iter()
                    .map(|object_ref| {
                        logical_document(&state, object_ref).and_then(|document| {
                            document
                                .map(|document| project(&document, fields))
                                .ok_or_else(|| {
                                    OQLError::storage_error(
                                        "ordered row points to a missing logical object",
                                        None,
                                    )
                                })
                        })
                    })
                    .collect::<OQLResult<Vec<_>>>()?
            };
            return Ok(documents);
        }

        let mut by_id = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            logical_collection_object_refs(&state, schema)?
                .iter()
                .map(|object_ref| {
                    logical_document(&state, *object_ref).and_then(|document| {
                        document
                            .map(|document| (document.id.clone(), project(&document, fields)))
                            .ok_or_else(|| {
                                OQLError::storage_error(
                                    "document identity points to a missing logical object",
                                    None,
                                )
                            })
                    })
                })
                .collect::<OQLResult<Vec<_>>>()?
                .into_iter()
                .collect::<HashMap<_, _>>()
        };
        for operation in pending {
            match operation {
                WriteOperation::PutDocument(document) if document.schema == schema => {
                    by_id.insert(document.id.clone(), project(&document, fields));
                }
                WriteOperation::DeleteDocument {
                    schema: pending_schema,
                    id,
                } if pending_schema == schema => {
                    by_id.remove(&id);
                }
                _ => {}
            }
        }
        let mut documents = by_id.into_values().collect::<Vec<_>>();
        documents.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(documents)
    }

    fn collection_len(&self, schema: &str) -> OQLResult<Option<usize>> {
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        Ok(Some(logical_collection_count(&state, schema)))
    }

    fn collection_was_made(&self, schema: &str) -> OQLResult<bool> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        Ok(state.collection_ids.contains_key(schema))
    }

    fn scan_first_documents(
        &self,
        schema: &str,
        filters: &[ScanPredicate],
        fields: Option<&[String]>,
        limit: usize,
    ) -> OQLResult<Option<Vec<Document>>> {
        // Uncommitted writes are overlaid by the general route.
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        // A collection's rows are listed in order of id, so the first that
        // pass are the ones with the smallest ids.
        let rows = logical_collection_object_refs(&state, schema)?;
        let mut documents = Vec::with_capacity(limit.min(1024));
        for object_ref in &rows {
            if documents.len() >= limit {
                break;
            }
            let Some(document) = logical_document(&state, *object_ref)? else {
                continue;
            };
            if !filters
                .iter()
                .all(|filter| decoded_scan_predicate_matches(&document, filter))
            {
                continue;
            }
            documents.push(match fields {
                Some(fields) => document.with_projected_fields(
                    fields
                        .iter()
                        .filter_map(|field| {
                            document
                                .fields
                                .get(field)
                                .cloned()
                                .map(|value| (field.clone(), value))
                        })
                        .collect(),
                ),
                None => document.as_ref().clone(),
            });
        }
        Ok(Some(documents))
    }

    fn scan_documents_in_chunks(
        &self,
        schema: &str,
        fields: Option<&[String]>,
        visit: &mut dyn FnMut(&[Arc<Document>]) -> OQLResult<bool>,
    ) -> OQLResult<Option<bool>> {
        // A row read from its record is decoded only as far as the fields
        // asked for: a long list nobody reads is most of the work of
        // decoding the row that holds it.
        let wanted: Option<HashSet<String>> = fields.map(|fields| fields.iter().cloned().collect());
        /// Documents decoded and shown at a time: few to begin with, since
        /// a caller may have what it wants from the first of them, and
        /// more each time up to this many.
        const CHUNK: usize = 1_024;
        const FIRST_CHUNK: usize = 32;
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }
        let written = |state: &State| state.collection_stamps.get(schema).copied().unwrap_or(0);
        let (object_refs, stamp) = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            (logical_collection_object_refs(&state, schema)?, written(&state))
        };
        let mut remaining = object_refs.as_slice();
        let mut size = FIRST_CHUNK;
        while !remaining.is_empty() {
            let (chunk, rest) = remaining.split_at(size.min(remaining.len()));
            remaining = rest;
            size = (size * 2).min(CHUNK);
            // The state is read for one chunk and let go before the
            // documents are shown, since what is done with them may read
            // the store again.
            let documents = {
                let state = self
                    .state
                    .read()
                    .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
                if written(&state) != stamp {
                    return Ok(None);
                }
                // The rows are shown in the order listed, and read in the
                // order their records lie in the file, which is the order
                // of their references: one stretch of the file read then
                // serves the rows that follow, where reading them as
                // listed would seek back and forth for each.
                let mut in_file_order: Vec<usize> = (0..chunk.len()).collect();
                in_file_order.sort_unstable_by_key(|at| chunk[*at].packed());
                let mut read: Vec<Option<Arc<Document>>> = vec![None; chunk.len()];
                for at in in_file_order {
                    match sourced_document_reading(&state, chunk[at], wanted.as_ref())? {
                        Some(document) => read[at] = Some(document),
                        // Every row listed is there while nothing has
                        // been written.
                        None => return Ok(None),
                    }
                }
                read.into_iter().flatten().collect::<Vec<_>>()
            };
            if !visit(&documents)? {
                return Ok(Some(false));
            }
        }
        Ok(Some(true))
    }

    fn scan_documents_shared(&self, schema: &str) -> OQLResult<Option<Vec<Arc<Document>>>> {
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        // The list is kept from one query to the next until something is
        // committed; making it is a pass over the collection.
        let mut built = self
            .built_columns
            .lock()
            .map_err(|_| OQLError::generic("column cache lock poisoned"))?;
        {
            let kept = built.of(&state, schema);
            if let Some(known) = &kept.rows {
                return Ok(Some(known.as_ref().clone()));
            }
            if let Some(weak) = kept.weak_rows.clone() {
                let rows: Option<Vec<Arc<Document>>> = weak.iter().map(std::sync::Weak::upgrade).collect();
                match rows {
                    Some(rows) => return Ok(Some(rows)),
                    None => kept.weak_rows = None,
                }
            }
        }
        let object_refs = logical_collection_object_refs(&state, schema)?;
        // Under a budget, a collection that is small beside it is worth
        // holding: its rows are decoded here and kept, within the budget,
        // so the next statement finds them held. Without that, every scan
        // of it decodes every row and lets it go again.
        let worth_holding = state
            .decoded_budget
            .is_some_and(|budget| object_refs.len() <= budget / 4);
        let mut decoded = Vec::new();
        let mut documents = Vec::with_capacity(object_refs.len());
        for object_ref in object_refs {
            match sourced_document(&state, object_ref)? {
                Some((document, DocumentSource::Held)) => documents.push(document),
                Some(_) if worth_holding => match shared_document(&state, object_ref, &mut decoded)? {
                    Some(document) => documents.push(document),
                    None => {}
                },
                // A row that would have to be decoded to be lent is better
                // read by a scan that decodes only the fields it needs.
                Some(_) => return Ok(None),
                None => {}
            }
        }
        let bytes = documents.len() * std::mem::size_of::<Arc<Document>>();
        // The list is made either way, so keeping it costs a copy of its
        // pointers and saves the next statement making it again.
        built.of(&state, schema).asks();
        // With any limit on decoded rows the list is kept so that it
        // holds no row in memory, and only when nothing here was decoded
        // for the purpose: such rows may not stay.
        if decoded.is_empty() && built.makes_room(schema, bytes) {
            let kept = built.of(&state, schema);
            match state.bounded() {
                true => kept.weak_rows = Some(Arc::new(documents.iter().map(Arc::downgrade).collect())),
                false => kept.rows = Some(Arc::new(documents.clone())),
            }
            kept.bytes += bytes;
        }
        drop(built);
        drop(state);
        self.keep_decoded(decoded)?;
        Ok(Some(documents))
    }

    fn scan_documents_filtered_projection(
        &self,
        schema: &str,
        filters: &[ScanPredicate],
        fields: Option<&[String]>,
    ) -> OQLResult<Option<Vec<Document>>> {
        // Falling back while a transaction has queued writes preserves the
        // established overlay behavior without duplicating transaction merge
        // logic in the pushed scan.
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }

        let (object_refs, decoded_rows_resident, records_resident) = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let object_refs = logical_collection_object_refs(&state, schema)?;
            let decoded_rows_resident =
                object_refs
                    .iter()
                    .try_fold(true, |all_resident, object_ref| {
                        Ok::<_, OQLError>(
                            all_resident
                                && (state.object_directory.contains_key(object_ref)
                                    || state.typed_row_locations.contains_key(object_ref)?),
                        )
                    })?;
            let records_resident = object_refs
                .iter()
                .all(|object_ref| state.object_records.get(object_ref).is_some());
            (object_refs, decoded_rows_resident, records_resident)
        };
        if object_refs.is_empty() {
            return Ok(Some(Vec::new()));
        }
        // The decoded directory is deliberately preferred while resident.
        // Benchmarks show that its borrowed HashMap lookup is cheaper than
        // reparsing record regions. Compiled slots are the fallback for a
        // future memory-bounded mode that evicts decoded rows.
        // With a budget some rows are held decoded and the rest as records.
        // Each row is then fetched whichever way it is held, which the
        // decoded route already does.
        if decoded_rows_resident || !records_resident {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let mut documents = Vec::with_capacity(object_refs.len());
            // A row read from a column segment for this scan alone has
            // only the fields the scan filters on or returns built.
            let wanted = fields.map(|fields| {
                let mut wanted: HashSet<String> = fields.iter().cloned().collect();
                for filter in filters {
                    filter_fields(filter, &mut wanted);
                }
                wanted
            });
            // While memory is not short a row decoded here is kept, so
            // the next scan finds it held, as it would in a database that
            // had never been checkpointed.
            let hold = !state.tight();
            let mut decoded = Vec::new();
            for object_ref in &object_refs {
                let from_segment = match &wanted {
                    Some(wanted) if !hold => segment_row_reading(&state, *object_ref, wanted)?,
                    _ => None,
                };
                let document = match from_segment {
                    Some(document) => Arc::new(document),
                    None => {
                        let read = match hold {
                            true => shared_document(&state, *object_ref, &mut decoded)?,
                            false => logical_document(&state, *object_ref)?,
                        };
                        match read {
                            Some(document) => document,
                            None => continue,
                        }
                    }
                };
                if !filters
                    .iter()
                    .all(|filter| decoded_scan_predicate_matches(&document, filter))
                {
                    continue;
                }
                documents.push(match fields {
                    Some(fields) => {
                        let projected = fields
                            .iter()
                            .filter_map(|field| {
                                document
                                    .fields
                                    .get(field)
                                    .cloned()
                                    .map(|value| (field.clone(), value))
                            })
                            .collect();
                        document.with_projected_fields(projected)
                    }
                    None => document.as_ref().clone(),
                });
            }
            drop(state);
            self.keep_decoded(decoded)?;
            return Ok(Some(documents));
        }
        if !records_resident {
            return Err(OQLError::storage_error(
                "collection has neither decoded rows nor resident compiled records",
                None,
            ));
        }
        struct ScanVersion {
            schema: CompiledSchema,
            filters: Vec<CompiledScanPredicate>,
            projection: Option<Vec<(usize, String)>>,
        }

        let mut versions = HashMap::<(u16, u32), ScanVersion>::new();
        let mut documents = Vec::new();
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        for object_ref in object_refs {
            let record = state
                .object_records
                .get(&object_ref)
                .ok_or_else(|| OQLError::storage_error("resident scan record disappeared", None))?;
            let header = RecordCodec::trusted_header(record)?;
            if header.object_ref != object_ref {
                return Err(OQLError::storage_error(
                    "compiled scan record identity conflicts with its row directory",
                    None,
                ));
            }
            let key = (object_ref.collection_id(), header.schema_version);
            if !versions.contains_key(&key) {
                let compiled = state.compiled_schemas.get(&key).cloned().ok_or_else(|| {
                    OQLError::storage_error(
                        "compiled scan record references an unknown schema version",
                        None,
                    )
                })?;
                let field_slots = compiled
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(slot, field)| (field.name.as_str(), slot))
                    .collect::<HashMap<_, _>>();
                let compiled_filters = filters
                    .iter()
                    .map(|filter| compile_scan_predicate(filter, &field_slots))
                    .collect();
                let projection = fields.map(|fields| {
                    fields
                        .iter()
                        .filter_map(|field| {
                            field_slots
                                .get(field.as_str())
                                .copied()
                                .map(|slot| (slot, field.clone()))
                        })
                        .collect()
                });
                versions.insert(
                    key,
                    ScanVersion {
                        schema: compiled,
                        filters: compiled_filters,
                        projection,
                    },
                );
            }
            let version = versions.get(&key).expect("scan version was inserted");
            let view = RecordCodec::trusted_view(record, &version.schema)?;
            let mut matches = true;
            for filter in &version.filters {
                if !compiled_scan_predicate_matches(&view, &version.schema, filter)? {
                    matches = false;
                    break;
                }
            }
            if !matches {
                continue;
            }
            documents.push(match &version.projection {
                Some(projection) => view.decode_slots(projection)?,
                None => RecordCodec::decode(record, &version.schema)?,
            });
        }
        Ok(Some(documents))
    }

    fn rows_of_columns(
        &self,
        schema: &str,
        stamp: u64,
        positions: &[u32],
    ) -> OQLResult<Option<Vec<Arc<Document>>>> {
        let mut decoded = Vec::new();
        let rows = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let built = self
                .built_columns
                .lock()
                .map_err(|_| OQLError::generic("column cache lock poisoned"))?;
            let Some(kept) = built.collections.get(schema).filter(|kept| kept.stamp == stamp) else {
                return Ok(None);
            };
            let mut rows = Vec::with_capacity(positions.len());
            if let Some(listed) = &kept.rows {
                for position in positions {
                    match listed.get(*position as usize) {
                        Some(row) => rows.push(row.clone()),
                        None => return Ok(None),
                    }
                }
            } else {
                // Which row is where: kept with the columns when they were
                // made a row at a time, and otherwise listed now, which is
                // the same order for as long as the stamp holds.
                let listed;
                let refs: &[ObjRef] = match &kept.refs {
                    Some(refs) => refs.as_slice(),
                    None => {
                        listed = logical_collection_object_refs(&state, schema)?;
                        &listed
                    }
                };
                for position in positions {
                    let Some(object_ref) = refs.get(*position as usize) else {
                        return Ok(None);
                    };
                    match shared_document(&state, *object_ref, &mut decoded)? {
                        Some(row) => rows.push(row),
                        None => return Ok(None),
                    }
                }
            }
            rows
        };
        self.keep_decoded(decoded)?;
        Ok(Some(rows))
    }

    fn kept_with_columns(&self, key: &str, stamp: u64) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        let built = self.built_columns.lock().ok()?;
        built
            .collections
            .values()
            .find(|kept| kept.stamp == stamp)
            .and_then(|kept| kept.kept.get(key).cloned())
    }

    fn keep_with_columns(&self, key: &str, stamp: u64, value: Arc<dyn std::any::Any + Send + Sync>) {
        if let Ok(mut built) = self.built_columns.lock() {
            if let Some(kept) = built.collections.values_mut().find(|kept| kept.stamp == stamp) {
                kept.kept.insert(key.to_string(), value);
            }
        }
    }

    fn scan_columns(&self, schema: &str, fields: &[String]) -> OQLResult<Option<Vec<ColumnBatch>>> {
        Ok(self
            .rows_and_columns(schema, fields, false, false, false)?
            .map(|(_, batch)| vec![batch]))
    }

    fn scan_columns_for_once(&self, schema: &str, fields: &[String]) -> OQLResult<Option<Vec<ColumnBatch>>> {
        Ok(self
            .rows_and_columns(schema, fields, false, true, false)?
            .map(|(_, batch)| vec![batch]))
    }

    fn search_vector_index(
        &self,
        schema: &str,
        field: &str,
        query: &[f32],
        metric: crate::vector::Metric,
        k: usize,
        exact: bool,
        compare: Option<usize>,
    ) -> OQLResult<Option<(Vec<(Arc<Document>, f32)>, crate::storage_engine::VectorSearched)>> {
        use crate::storage::{VectorIndexState, VectorMetricType};
        use crate::vector::Metric;
        let declared_as = match metric {
            Metric::Cosine => VectorMetricType::Cosine,
            Metric::L2 => VectorMetricType::L2,
            // No index serves the dot product; every vector is compared.
            Metric::Dot => return Ok(None),
        };
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }
        // Held throughout: nothing is committed while the index is read,
        // so it and the rows it names are of one moment.
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let declared = state.vector_indexes.get(schema).is_some_and(|indexes| {
            indexes.iter().any(|index| {
                index.field == field && index.metric == declared_as && index.state == VectorIndexState::Ready
            })
        });
        let dimension = state
            .schemas
            .get(schema)
            .and_then(|declared| declared.fields.get(field))
            .and_then(|field| field.vector_dimension());
        let Some(dimension) = dimension.filter(|dimension| declared && *dimension as usize == query.len()) else {
            return Ok(None);
        };
        let stamps = (
            state.collection_stamps.get(schema).copied().unwrap_or(0),
            state.catalog_stamp,
        );
        let key = (schema.to_string(), field.to_string());
        let good = |index: &Arc<crate::storage_engine::VectorIndex>| {
            index.stamps == stamps && index.metric() == metric && index.dimension() == dimension as usize
        };
        let lock = || OQLError::generic("vector index lock poisoned");
        let ready = state
            .built_vectors
            .read()
            .map_err(|_| lock())?
            .get(&key)
            .filter(|index| good(index) && !index.wants_dividing())
            .cloned();
        let index = match ready {
            Some(index) => index,
            None => {
                let mut built = state.built_vectors.write().map_err(|_| lock())?;
                // Another search may have built or divided it meanwhile.
                let mut index = match built.get(&key).filter(|index| good(index)).cloned() {
                    Some(index) => index,
                    None => match build_vector_index(&state, schema, field, dimension as usize, metric, stamps)? {
                        Some(index) => Arc::new(index),
                        None => return Ok(None),
                    },
                };
                if index.wants_dividing() {
                    Arc::make_mut(&mut index).divide();
                }
                built.insert(key, index.clone());
                index
            }
        };
        let (found, searched) = index.search(query, k, exact, compare);
        // The rows found, each scored from its own vector as a search
        // that reads every row scores it, and put in that search's order.
        let mut rows = Vec::with_capacity(found.len());
        for (object_ref, _) in found {
            let Some(document) = logical_document(&state, object_ref)? else {
                continue;
            };
            let Some(score) = document
                .fields
                .get(field)
                .and_then(Value::as_vector)
                .map(|vector| crate::vector::score(metric, &vector, query))
            else {
                continue;
            };
            rows.push((document, score));
        }
        rows.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
        rows.truncate(k);
        Ok(Some((rows, searched)))
    }

    fn scan_vector_column(&self, schema: &str, field: &str) -> OQLResult<Option<ColumnBatch>> {
        Ok(self
            .rows_and_columns(schema, &[field.to_string()], false, true, true)?
            .map(|(_, batch)| batch))
    }

    fn scan_rows_and_columns(
        &self,
        schema: &str,
        fields: &[String],
    ) -> OQLResult<Option<(Arc<Vec<Arc<Document>>>, ColumnBatch)>> {
        Ok(self
            .rows_and_columns(schema, fields, true, false, false)?
            .and_then(|(rows, batch)| Some((rows?, batch))))
    }
    fn scan_projected_batches(
        &self,
        schema: &str,
        filters: &[ScanPredicate],
        fields: &[String],
        batch_rows: usize,
    ) -> OQLResult<Option<Vec<ProjectedRowBatch>>> {
        if !self.pending_operations()?.is_empty() {
            return Ok(None);
        }
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let live_rows = logical_collection_count(&state, schema);
        if live_rows == 0 {
            return Ok(Some(Vec::new()));
        }
        let batch_rows = batch_rows.max(1);
        if state.committed_lsn == state.typed_base_lsn {
            if let (Some(collection_id), Some(schema_version)) = (
                state.collection_ids.get(schema).copied(),
                state.current_schema_versions.get(schema).copied(),
            ) {
                let segment_handles = state
                    .typed_segments
                    .iter()
                    .filter(|segment| {
                        segment.collection_id() == collection_id
                            && segment.schema_version() == schema_version
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let segments = segment_handles
                    .iter()
                    .map(|segment| segment.load())
                    .collect::<OQLResult<Vec<_>>>()?;
                let covered_rows = segments.iter().try_fold(0usize, |count, segment| {
                    count.checked_add(segment.row_count())
                });
                if !segments.is_empty() && covered_rows == Some(live_rows) {
                    let compiled = state
                        .compiled_schemas
                        .get(&(collection_id, schema_version))
                        .ok_or_else(|| {
                            OQLError::storage_error("typed scan has no compiled schema", None)
                        })?;
                    let typed_filters = filters
                        .iter()
                        .map(|filter| compile_typed_scan_predicate(filter, compiled, &segments))
                        .collect::<Option<Vec<_>>>();
                    if let Some(typed_filters) = typed_filters {
                        let mut typed_projection_ids = Vec::with_capacity(fields.len());
                        let mut typed_projection_supported = true;
                        for name in fields {
                            if DocumentSystemField::from_name(name).is_some() {
                                typed_projection_ids.push(None);
                                continue;
                            }
                            let field_id = compiled
                                .fields
                                .iter()
                                .find(|field| field.name == *name)
                                .map(|field| field.id);
                            if field_id.is_some_and(|field_id| {
                                segments
                                    .iter()
                                    .any(|segment| segment.column(field_id).is_none())
                            }) {
                                typed_projection_supported = false;
                                break;
                            }
                            typed_projection_ids.push(field_id);
                        }
                        let mut batches = Vec::new();
                        for segment in segments {
                            let mut batch = if typed_projection_supported {
                                ProjectedRowBatch::typed(
                                    schema.to_string(),
                                    fields,
                                    typed_projection_ids.clone(),
                                    segment.clone(),
                                    batch_rows.min(segment.row_count()),
                                )
                            } else {
                                ProjectedRowBatch::shared_with_capacity(
                                    schema.to_string(),
                                    fields,
                                    batch_rows.min(segment.row_count()),
                                )
                            };
                            let filter_modes = typed_filters
                                .iter()
                                .map(|filter| typed_segment_match(&segment, schema, filter))
                                .collect::<Vec<_>>();
                            if filter_modes.contains(&SegmentMatch::None) {
                                continue;
                            }
                            for row in 0..segment.row_count() {
                                let mut matches = true;
                                for (filter, mode) in typed_filters.iter().zip(&filter_modes) {
                                    if *mode == SegmentMatch::Rows
                                        && !typed_scan_predicate_matches(
                                            &segment, row, schema, filter,
                                        )?
                                    {
                                        matches = false;
                                        break;
                                    }
                                }
                                if !matches {
                                    continue;
                                }
                                if typed_projection_supported {
                                    batch.push_typed(row);
                                } else {
                                    let document = logical_document(
                                        &state,
                                        segment.object_refs[row],
                                    )?
                                    .ok_or_else(|| {
                                            OQLError::storage_error(
                                                "typed scan row disappeared from its authoritative directory",
                                                None,
                                            )
                                        })?;
                                    batch.push_shared(document);
                                }
                                if batch.len() == batch_rows {
                                    batches.push(batch);
                                    batch = if typed_projection_supported {
                                        ProjectedRowBatch::typed(
                                            schema.to_string(),
                                            fields,
                                            typed_projection_ids.clone(),
                                            segment.clone(),
                                            batch_rows,
                                        )
                                    } else {
                                        ProjectedRowBatch::shared_with_capacity(
                                            schema.to_string(),
                                            fields,
                                            batch_rows,
                                        )
                                    };
                                }
                            }
                            if !batch.is_empty() {
                                batches.push(batch);
                            }
                        }
                        return Ok(Some(batches));
                    }
                }
            }
        }
        if let Some(batches) =
            scan_typed_base_with_row_delta(&state, schema, filters, fields, batch_rows)?
        {
            return Ok(Some(batches));
        }
        let object_refs = logical_collection_object_refs(&state, schema)?;
        let mut batches = Vec::new();
        let mut batch = ProjectedRowBatch::shared_with_capacity(
            schema.to_string(),
            fields,
            batch_rows.min(object_refs.len()),
        );
        // A row held only as its record is decoded for this scan alone,
        // and only the fields the scan filters on or returns: a count
        // decodes none of a row's lists or text.
        let mut wanted: HashSet<String> = fields.iter().cloned().collect();
        for filter in filters {
            filter_fields(filter, &mut wanted);
        }
        for object_ref in object_refs {
            let held_as_record = !state.object_directory.contains_key(&object_ref)
                && !state.typed_row_locations.contains_key(&object_ref)?;
            // A row held as a record, in memory or in a checkpoint file,
            // has only the fields asked for decoded: its long lists are
            // skipped over, not built.
            let record = match held_as_record {
                true => live_record(&state, object_ref)?,
                false => None,
            };
            let document = match record {
                Some(record) => {
                    let compiled = schema_of_record(&state, object_ref, &record)?;
                    Arc::new(RecordCodec::decode_projection(&record, compiled, Some(&wanted))?)
                }
                None => match logical_document(&state, object_ref)? {
                    Some(document) => document,
                    None => continue,
                },
            };
            if !filters
                .iter()
                .all(|filter| decoded_scan_predicate_matches(&document, filter))
            {
                continue;
            }
            batch.push_shared(document);
            if batch.len() == batch_rows {
                batches.push(batch);
                batch = ProjectedRowBatch::shared_with_capacity(
                    schema.to_string(),
                    fields,
                    batch_rows.min(live_rows.saturating_sub(batches.len() * batch_rows)),
                );
            }
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        Ok(Some(batches))
    }

    fn get_schema(&self, name: &str) -> OQLResult<Option<Schema>> {
        Ok(self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .schemas
            .get(name)
            .cloned())
    }

    fn put_schema(&self, schema: &Schema) -> OQLResult<()> {
        self.queue_or_commit(WriteOperation::PutSchema(schema.clone()))
    }

    fn delete_schema(&self, name: &str) -> OQLResult<bool> {
        let exists = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .schemas
            .contains_key(name);
        if exists {
            self.queue_or_commit(WriteOperation::DeleteSchema {
                name: name.to_string(),
            })?;
        }
        Ok(exists)
    }

    fn list_schemas(&self) -> OQLResult<Vec<String>> {
        let mut schemas = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .schemas
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        schemas.sort();
        Ok(schemas)
    }

    fn upsert_scalar_index(&self, index: ScalarIndexDefinition) -> OQLResult<()> {
        // Declaring an index that already exists exactly as declared changes
        // nothing. Writing it again would build it afresh in memory beside
        // the copy a checkpoint already holds.
        if self.pending_operations()?.is_empty() {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            let unchanged = catalog_values(&state.scalar_indexes, Some(&index.schema))
                .iter()
                .any(|existing| existing == &index)
                && state
                    .scalar_index_data
                    .contains_key(&(index.schema.clone(), index.name.clone()));
            if unchanged {
                return Ok(());
            }
        }
        self.queue_or_commit(WriteOperation::PutScalarIndex(index))
    }

    fn delete_scalar_index(&self, schema: &str, name: &str) -> OQLResult<bool> {
        let exists = self
            .list_scalar_indexes(Some(schema))?
            .iter()
            .any(|index| index.name == name);
        if exists {
            self.queue_or_commit(WriteOperation::DeleteScalarIndex {
                schema: schema.to_string(),
                name: name.to_string(),
            })?;
        }
        Ok(exists)
    }

    fn list_scalar_indexes(&self, schema: Option<&str>) -> OQLResult<Vec<ScalarIndexDefinition>> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        Ok(catalog_values(&state.scalar_indexes, schema))
    }

    fn scan_scalar_index(
        &self,
        schema: &str,
        name: &str,
        range: &ScalarIndexRange,
    ) -> OQLResult<Option<ScalarIndexScanPage>> {
        if range.after.as_ref().is_some_and(|cursor| {
            cursor.version != 2
                || cursor.schema != schema
                || cursor.index_name != name
                || cursor.direction != range.direction
        }) {
            return Err(OQLError::generic(
                "index cursor does not belong to the selected index",
            ));
        }
        let lower = match &range.lower {
            Some(bound) => {
                let key = TypedKey::encode(&bound.values)?;
                let key = if bound.prefix_end {
                    let Some(successor) = TypedKey::prefix_successor(key) else {
                        return Ok(Some(ScalarIndexScanPage {
                            object_refs: Vec::new(),
                            encoded_keys: Vec::new(),
                            next_cursor: None,
                        }));
                    };
                    successor
                } else {
                    key
                };
                if bound.inclusive {
                    Bound::Included(key)
                } else {
                    Bound::Excluded(key)
                }
            }
            None => Bound::Unbounded,
        };
        let upper = match &range.upper {
            Some(bound) => {
                let key = TypedKey::encode(&bound.values)?;
                let key = if bound.prefix_end {
                    TypedKey::prefix_successor(key).ok_or_else(|| {
                        OQLError::storage_error(
                            "typed index upper prefix has no finite successor",
                            None,
                        )
                    })?
                } else {
                    key
                };
                if bound.inclusive {
                    Bound::Included(key)
                } else {
                    Bound::Excluded(key)
                }
            }
            None => Bound::Unbounded,
        };
        let Some(index) = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .scalar_index_data
            .get(&(schema.to_string(), name.to_string()))
            .cloned()
        else {
            return Ok(None);
        };
        let after = range
            .after
            .as_ref()
            .map(|cursor| (cursor.key.as_slice(), cursor.object_ref));
        let (entries, next) = index
            .range_entry_page(
                lower,
                upper,
                after,
                range.limit,
                range.direction == ScalarIndexDirection::Descending,
            )
            .map_err(|error| OQLError::storage_error(&error, None))?;
        let (encoded_keys, object_refs): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
        Ok(Some(ScalarIndexScanPage {
            object_refs,
            encoded_keys,
            next_cursor: next.map(|(key, object_ref)| ScalarIndexCursor {
                version: 2,
                schema: schema.to_string(),
                index_name: name.to_string(),
                key,
                object_ref,
                direction: range.direction,
            }),
        }))
    }

    fn upsert_vector_index(&self, index: VectorIndexDefinition) -> OQLResult<()> {
        self.queue_or_commit(WriteOperation::PutVectorIndex(index))
    }

    fn delete_vector_index(&self, schema: &str, name: &str) -> OQLResult<bool> {
        let exists = self
            .list_vector_indexes(Some(schema))?
            .iter()
            .any(|index| index.name == name);
        if exists {
            self.queue_or_commit(WriteOperation::DeleteVectorIndex {
                schema: schema.to_string(),
                name: name.to_string(),
            })?;
        }
        Ok(exists)
    }

    fn list_vector_indexes(&self, schema: Option<&str>) -> OQLResult<Vec<VectorIndexDefinition>> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        Ok(catalog_values(&state.vector_indexes, schema))
    }

    fn list_references_to(
        &self,
        target_schema: &str,
        target_id: &DocumentId,
        source_schema: Option<&str>,
        field_path: Option<&str>,
    ) -> OQLResult<Vec<ReverseReferenceEntry>> {
        let lookup_started = Instant::now();
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        let Some(target) = lookup_object_ref(&state, target_schema, target_id)? else {
            return Ok(Vec::new());
        };
        let prefix = target.packed().to_be_bytes().to_vec();
        let end = target
            .packed()
            .checked_add(1)
            .map(|next| Bound::Excluded(next.to_be_bytes().to_vec()))
            .unwrap_or(Bound::Unbounded);
        let (rows, _) = state
            .reverse_reference_index
            .range_entry_page(Bound::Included(prefix), end, None, None, false)
            .map_err(|error| OQLError::storage_error(&error, None))?;
        let index_elapsed = lookup_started.elapsed();
        let posting_count = rows.len();
        let mut references = Vec::with_capacity(rows.len());
        for (key, source) in rows {
            let persisted_path =
                std::str::from_utf8(key.get(8..).unwrap_or_default()).map_err(|_| {
                    OQLError::storage_error("reverse-reference path is not UTF-8", None)
                })?;
            if field_path.is_some_and(|expected| expected != persisted_path) {
                continue;
            }
            let Some((persisted_schema, source_id)) = logical_object_identity(&state, source)?
            else {
                continue;
            };
            if source_schema.is_some_and(|expected| expected != persisted_schema) {
                continue;
            }
            references.push(ReverseReferenceEntry {
                source_schema: persisted_schema,
                source_id,
                field_path: persisted_path.to_owned(),
            });
        }
        tracing::debug!(
            target: "owl_db::storage_engine::reverse_lookup",
            index_ms = index_elapsed.as_secs_f64() * 1_000.0,
            identity_ms = (lookup_started.elapsed() - index_elapsed).as_secs_f64() * 1_000.0,
            postings = posting_count,
            returned = references.len(),
            "resolved reverse-reference lookup"
        );
        Ok(references)
    }

    fn run_atomically(&self, work: &mut dyn FnMut() -> OQLResult<()>) -> OQLResult<()> {
        // Inside a transaction the statement is part of it, and it is the
        // transaction that commits or is refused.
        if self.current_transaction().is_some() {
            return work();
        }
        // Someone always wins a conflict, so losing this many times in a
        // row means the document is being written without pause.
        self.in_transaction(1_024, work)
    }

    fn document_exists(&self, schema: &str, id: &DocumentId) -> OQLResult<bool> {
        let mut exists = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            lookup_object_ref(&state, schema, id)?.is_some()
        };
        let active = self
            .transactions
            .lock()
            .map_err(|_| OQLError::generic("transaction lock poisoned"))?;
        let own = self.current_transaction().and_then(|id| active.get(&id));
        for operation in own.iter().flat_map(|transaction| &transaction.operations) {
            match operation {
                WriteOperation::PutDocument(pending)
                    if pending.schema == schema && pending.id == *id =>
                {
                    exists = true;
                }
                WriteOperation::DeleteDocument {
                    schema: pending_schema,
                    id: pending_id,
                } if pending_schema == schema && pending_id == id => {
                    exists = false;
                }
                _ => {}
            }
        }
        Ok(exists)
    }

    fn object_ref_of(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>> {
        let state = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
        lookup_object_ref(&state, schema, id)
    }

    fn scan_referencing_documents_shared(
        &self,
        target_schema: &str,
        target_id: &DocumentId,
        source_schema: Option<&str>,
        field_path: Option<&str>,
    ) -> OQLResult<Vec<Arc<Document>>> {
        self.referencing_documents(target_schema, target_id, source_schema, field_path, None)
    }

    fn scan_referencing_documents_reading(
        &self,
        target_schema: &str,
        target_id: &DocumentId,
        source_schema: Option<&str>,
        field_path: Option<&str>,
        read: &[String],
    ) -> OQLResult<Vec<Arc<Document>>> {
        let read: HashSet<String> = read.iter().cloned().collect();
        self.referencing_documents(target_schema, target_id, source_schema, field_path, Some(&read))
    }


    fn navigate_reference(&self, object_ref: &ObjRef) -> OQLResult<Option<Document>> {
        if object_ref.is_null() {
            return Ok(None);
        }
        Ok(self
            .navigate_reference_shared(object_ref)?
            .map(|document| document.as_ref().clone()))
    }

    fn navigate_reference_shared(&self, object_ref: &ObjRef) -> OQLResult<Option<Arc<Document>>> {
        if object_ref.is_null() {
            return Ok(None);
        }
        let mut decoded = Vec::new();
        let document = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            shared_document(&state, *object_ref, &mut decoded)?
        };
        self.keep_decoded(decoded)?;
        Ok(document)
    }

    fn navigate_references(&self, object_refs: &[ObjRef]) -> OQLResult<Vec<Option<Document>>> {
        let mut decoded = Vec::new();
        let documents = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            object_refs
                .iter()
                .map(|object_ref| {
                    shared_document(&state, *object_ref, &mut decoded)
                        .map(|document| document.map(|document| document.as_ref().clone()))
                })
                .collect::<OQLResult<Vec<_>>>()?
        };
        self.keep_decoded(decoded)?;
        Ok(documents)
    }

    fn navigate_reference_projection(
        &self,
        object_ref: &ObjRef,
        fields: &[String],
    ) -> OQLResult<Option<Document>> {
        if !object_is_live(
            &*self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?,
            *object_ref,
        )? {
            return Ok(None);
        }
        let record = self.read_compiled_record(*object_ref)?;
        let header = RecordCodec::header(&record)?;
        if header.object_ref != *object_ref {
            return Err(OQLError::storage_error(
                "checkpoint record identity conflicts with its directory",
                None,
            ));
        }
        let compiled = self
            .state
            .read()
            .map_err(|_| OQLError::generic("storage state lock poisoned"))?
            .compiled_schemas
            .get(&(object_ref.collection_id(), header.schema_version))
            .cloned()
            .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
        let projection = fields.iter().cloned().collect::<HashSet<_>>();
        decode_record_projection(&record, &compiled, &projection).map(Some)
    }

    fn navigate_references_projection(
        &self,
        object_refs: &[ObjRef],
        fields: &[String],
    ) -> OQLResult<Vec<Option<Document>>> {
        if object_refs.is_empty() {
            return Ok(Vec::new());
        }

        let live = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            object_refs
                .iter()
                .map(|object_ref| object_is_live(&state, *object_ref))
                .collect::<OQLResult<Vec<_>>>()?
        };
        let live_refs = object_refs
            .iter()
            .zip(&live)
            .filter_map(|(object_ref, live)| live.then_some(*object_ref))
            .collect::<Vec<_>>();
        let fetched = self.read_compiled_records(&live_refs)?;
        let mut fetched = fetched.into_iter();
        let records = live
            .iter()
            .map(|live| live.then(|| fetched.next().expect("live record batch is complete")))
            .collect::<Vec<_>>();
        if fetched.next().is_some() {
            return Err(OQLError::storage_error(
                "live record batch exceeds requested references",
                None,
            ));
        }

        let headers = records
            .iter()
            .zip(object_refs)
            .map(|(record, object_ref)| {
                record
                    .as_ref()
                    .map(|record| {
                        let header = RecordCodec::header(record)?;
                        if header.object_ref != *object_ref {
                            return Err(OQLError::storage_error(
                                "checkpoint record identity conflicts with its directory",
                                None,
                            ));
                        }
                        Ok(header)
                    })
                    .transpose()
            })
            .collect::<OQLResult<Vec<_>>>()?;
        let compiled = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            headers
                .iter()
                .zip(object_refs)
                .map(|(header, object_ref)| {
                    header
                        .as_ref()
                        .map(|header| {
                            state
                                .compiled_schemas
                                .get(&(object_ref.collection_id(), header.schema_version))
                                .cloned()
                                .ok_or_else(|| {
                                    OQLError::storage_error("compiled schema is missing", None)
                                })
                        })
                        .transpose()
                })
                .collect::<OQLResult<Vec<_>>>()?
        };
        let projection = fields.iter().cloned().collect::<HashSet<_>>();
        records
            .into_iter()
            .zip(compiled)
            .map(|(record, compiled)| match (record, compiled) {
                (Some(record), Some(compiled)) => {
                    decode_record_projection(&record, &compiled, &projection).map(Some)
                }
                (None, None) => Ok(None),
                _ => Err(OQLError::storage_error(
                    "record and compiled schema availability disagree",
                    None,
                )),
            })
            .collect()
    }

    fn get_objref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>> {
        let stored = {
            let state = self
                .state
                .read()
                .map_err(|_| OQLError::generic("storage state lock poisoned"))?;
            lookup_object_ref(&state, schema, id)?
        };
        match stored {
            Some(object_ref) => Ok(Some(object_ref)),
            None => self.reserve_pending_ref(schema, id),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn decode_record_projection(
    record: &[u8],
    compiled: &CompiledSchema,
    projection: &HashSet<String>,
) -> OQLResult<Document> {
    let mut document = RecordCodec::decode_projection(record, compiled, Some(projection))?;
    for field in projection {
        if let Some(system_field) = DocumentSystemField::from_name(field) {
            let value = system_field.value(&document);
            document.fields.insert(field.clone(), value);
        }
    }
    Ok(document)
}

trait NamedIndex {
    fn name(&self) -> &str;
    fn schema(&self) -> &str;
}

impl NamedIndex for ScalarIndexDefinition {
    fn name(&self) -> &str {
        &self.name
    }
    fn schema(&self) -> &str {
        &self.schema
    }
}

impl NamedIndex for VectorIndexDefinition {
    fn name(&self) -> &str {
        &self.name
    }
    fn schema(&self) -> &str {
        &self.schema
    }
}

fn upsert<T: NamedIndex>(catalog: &mut HashMap<String, Vec<T>>, index: T) {
    let entries = catalog.entry(index.schema().to_string()).or_default();
    if let Some(position) = entries
        .iter()
        .position(|entry| entry.name() == index.name())
    {
        entries[position] = index;
    } else {
        entries.push(index);
    }
}

fn remove_named<T: NamedIndex>(catalog: &mut HashMap<String, Vec<T>>, schema: &str, name: &str) {
    if let Some(entries) = catalog.get_mut(schema) {
        entries.retain(|entry| entry.name() != name);
    }
}

fn catalog_values<T: Clone>(catalog: &HashMap<String, Vec<T>>, schema: Option<&str>) -> Vec<T> {
    match schema {
        Some(schema) => catalog.get(schema).cloned().unwrap_or_default(),
        None => catalog.values().flatten().cloned().collect(),
    }
}

fn derive_reverse_references(
    state: &State,
) -> OQLResult<HashMap<(String, DocumentId), Vec<ReverseReferenceEntry>>> {
    let mut targets = HashMap::<ObjRef, Vec<ReverseReferenceEntry>>::new();
    // Every row outside the column segments, decoded at the moment or not.
    for name in state.collection_ids.keys() {
        for object_ref in logical_collection_object_refs(state, name)? {
            if state.typed_row_locations.contains_key(&object_ref)? {
                continue;
            }
            let Some(document) = logical_document(state, object_ref)? else {
                continue;
            };
            for (field, value) in &document.fields {
                collect_reference_targets(value, field, &document, &mut targets);
            }
        }
    }
    for (segment_index, segment_handle) in state.typed_segments.iter().enumerate() {
        let compiled = state
            .compiled_schemas
            .get(&(
                segment_handle.collection_id(),
                segment_handle.schema_version(),
            ))
            .ok_or_else(|| OQLError::storage_error("compiled schema is missing", None))?;
        let schema = state
            .schemas
            .get(&compiled.name)
            .ok_or_else(|| OQLError::storage_error("logical schema is missing", None))?;
        if !schema
            .fields
            .values()
            .any(|field_type| field_type.referenced_schema().is_some())
        {
            continue;
        }
        let segment = segment_handle.load()?;
        let reference_columns = segment
            .columns
            .iter()
            .filter(|column| column.is_reference())
            .map(|column| {
                let field = compiled
                    .fields
                    .iter()
                    .find(|field| field.id == column.field_id)
                    .ok_or_else(|| {
                        OQLError::storage_error(
                            "typed reference column has no compiled field",
                            None,
                        )
                    })?;
                Ok((column, field))
            })
            .collect::<OQLResult<Vec<_>>>()?;
        if reference_columns.is_empty() {
            continue;
        }
        for (row, object_ref) in segment.object_refs.iter().copied().enumerate() {
            if state.typed_row_locations.get(&object_ref)? != Some((segment_index, row)) {
                continue;
            }
            for (column, field) in &reference_columns {
                if let TypedCell::Reference(target) = column.cell(row)? {
                    targets
                        .entry(target)
                        .or_default()
                        .push(ReverseReferenceEntry {
                            source_schema: compiled.name.clone(),
                            source_id: segment.document_ids[row].clone(),
                            field_path: field.name.clone(),
                        });
                }
            }
        }
    }
    resolve_reference_targets_from_state(targets, state)
}

fn build_reverse_reference_index(state: &State) -> OQLResult<Arc<BTreeIndex>> {
    let index =
        Arc::new(BTreeIndex::create(16, 0).map_err(|error| OQLError::storage_error(&error, None))?);
    for ((target_schema, target_id), edges) in derive_reverse_references(state)? {
        let Some(target) = lookup_object_ref(state, &target_schema, &target_id)? else {
            continue;
        };
        for edge in edges {
            let Some(source) = lookup_object_ref(state, &edge.source_schema, &edge.source_id)?
            else {
                continue;
            };
            index
                .insert_key(
                    reverse_reference_key(target, &edge.field_path),
                    source,
                    false,
                )
                .map_err(|error| OQLError::storage_error(&error, None))?;
        }
    }
    Ok(index)
}

fn materialize_reverse_reference_index(
    state: &State,
) -> OQLResult<HashMap<(String, DocumentId), Vec<ReverseReferenceEntry>>> {
    let mut references = HashMap::<(String, DocumentId), Vec<ReverseReferenceEntry>>::new();
    for (key, postings) in state
        .reverse_reference_index
        .snapshot()
        .map_err(|error| OQLError::storage_error(&error, None))?
    {
        if key.len() < 8 {
            return Err(OQLError::storage_error(
                "reverse-reference index key is truncated",
                None,
            ));
        }
        let target = ObjRef::new(u64::from_be_bytes(key[..8].try_into().unwrap()));
        let path = std::str::from_utf8(&key[8..])
            .map_err(|_| OQLError::storage_error("reverse-reference path is not UTF-8", None))?;
        let Some(target_identity) = logical_object_identity(state, target)? else {
            continue;
        };
        for source in postings {
            let Some((source_schema, source_id)) = logical_object_identity(state, source)? else {
                continue;
            };
            references
                .entry(target_identity.clone())
                .or_default()
                .push(ReverseReferenceEntry {
                    source_schema,
                    source_id,
                    field_path: path.to_owned(),
                });
        }
    }
    Ok(references)
}

fn collect_reference_targets(
    value: &Value,
    path: &str,
    source: &Document,
    output: &mut HashMap<ObjRef, Vec<ReverseReferenceEntry>>,
) {
    match value {
        Value::Reference(object_ref) => {
            output
                .entry(*object_ref)
                .or_default()
                .push(ReverseReferenceEntry {
                    source_schema: source.schema.clone(),
                    source_id: source.id.clone(),
                    field_path: path.to_string(),
                });
        }
        Value::List(values) => {
            for value in values {
                collect_reference_targets(value, path, source, output);
            }
        }
        Value::Object(fields) => {
            for (name, value) in fields {
                collect_reference_targets(value, &format!("{path}.{name}"), source, output);
            }
        }
        _ => {}
    }
}

fn reverse_reference_key(target: ObjRef, field_path: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(8 + field_path.len());
    key.extend_from_slice(&target.packed().to_be_bytes());
    key.extend_from_slice(field_path.as_bytes());
    key
}

fn document_reverse_keys(document: &Document) -> HashSet<Vec<u8>> {
    // The same rows and field paths [`collect_reference_targets`] finds,
    // as keys, without a copy of the row's collection and id for each.
    fn keys(value: &Value, path: &mut String, output: &mut HashSet<Vec<u8>>) {
        match value {
            Value::Reference(object_ref) => {
                output.insert(reverse_reference_key(*object_ref, path));
            }
            Value::List(values) => {
                for value in values {
                    keys(value, path, output);
                }
            }
            Value::Object(fields) => {
                let length = path.len();
                for (name, value) in fields {
                    path.push('.');
                    path.push_str(name);
                    keys(value, path, output);
                    path.truncate(length);
                }
            }
            _ => {}
        }
    }
    let mut output = HashSet::new();
    let mut path = String::new();
    for (field, value) in &document.fields {
        path.clear();
        path.push_str(field);
        keys(value, &mut path, &mut output);
    }
    output
}

fn update_reverse_reference_index(
    state: &State,
    previous: Option<&Document>,
    current: Option<&Document>,
    source: ObjRef,
) -> OQLResult<()> {
    let previous = previous.map(document_reverse_keys).unwrap_or_default();
    let current = current.map(document_reverse_keys).unwrap_or_default();
    for key in previous.difference(&current) {
        state
            .reverse_reference_index
            .delete_key(key, source)
            .map_err(|error| OQLError::storage_error(&error, None))?;
    }
    for key in current.difference(&previous) {
        state
            .reverse_reference_index
            .insert_key(key.clone(), source, false)
            .map_err(|error| OQLError::storage_error(&error, None))?;
    }
    Ok(())
}

fn remove_reverse_reference_target(state: &State, target: ObjRef) -> OQLResult<()> {
    let start = target.packed().to_be_bytes().to_vec();
    let end = target
        .packed()
        .checked_add(1)
        .map(|next| Bound::Excluded(next.to_be_bytes().to_vec()))
        .unwrap_or(Bound::Unbounded);
    let (entries, _) = state
        .reverse_reference_index
        .range_entry_page(Bound::Included(start), end, None, None, false)
        .map_err(|error| OQLError::storage_error(&error, None))?;
    for (key, source) in entries {
        state
            .reverse_reference_index
            .delete_key(&key, source)
            .map_err(|error| OQLError::storage_error(&error, None))?;
    }
    Ok(())
}

fn resolve_reference_targets_from_state(
    targets: HashMap<ObjRef, Vec<ReverseReferenceEntry>>,
    state: &State,
) -> OQLResult<HashMap<(String, DocumentId), Vec<ReverseReferenceEntry>>> {
    if targets.is_empty() {
        return Ok(HashMap::new());
    }
    let mut identities = HashMap::new();
    for schema in state.collection_ids.keys() {
        identities.extend(
            logical_collection_identities(state, schema)?
                .into_iter()
                .map(|(id, object_ref)| (object_ref, (schema.clone(), id))),
        );
    }
    let mut edges = HashMap::with_capacity(targets.len());
    for (object_ref, mut references) in targets {
        let Some(identity) = identities.get(&object_ref) else {
            continue;
        };
        let mut seen = HashSet::new();
        references.retain(|edge| seen.insert(edge.clone()));
        edges.insert(identity.clone(), references);
    }
    Ok(edges)
}

fn read_up_to(file: &mut dyn FileHandle, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut read = 0;
    while read < buffer.len() {
        match file.read(&mut buffer[read..]) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(read)
}

fn write_all(file: &mut dyn FileHandle, buffer: &[u8]) -> std::io::Result<()> {
    let mut written = 0;
    while written < buffer.len() {
        match file.write(&buffer[written..]) {
            Ok(0) => return Err(std::io::Error::new(ErrorKind::WriteZero, "short WAL write")),
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn storage_io(action: &str, path: &Path, error: std::io::Error) -> OQLError {
    OQLError::storage_error(
        &format!("failed to {action} '{}': {error}", path.display()),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldType;
    use crate::simulation::filesystem::SimulatedFileSystem;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn compaction_runs_merge_encoded_updates_and_tombstones() {
        let first = ObjRef::from_parts(1, 1, 1);
        let second = ObjRef::from_parts(1, 2, 1);
        let filesystem: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(7));
        let manifest: PathBuf = "compaction-test.manifest".into();
        let mut overlay = CompactionOverlay::new(
            filesystem.clone(),
            "compaction-test".into(),
            manifest.clone(),
            1,
        );
        overlay
            .put("events".into(), "same".into(), first, b"old".to_vec())
            .unwrap();
        overlay
            .put("events".into(), "same".into(), first, b"new".to_vec())
            .unwrap();
        overlay
            .put(
                "events".into(),
                "deleted".into(),
                second,
                b"deleted".to_vec(),
            )
            .unwrap();
        overlay
            .delete("events".into(), "deleted".into(), second)
            .unwrap();
        let plan = overlay.finish().unwrap();

        assert_eq!(plan.runs.len(), 4);
        assert!(plan.overridden.contains(&first));
        assert!(plan.overridden.contains(&second));
        let paths = plan.runs.clone();
        let mut cursor = CompactionChangeCursor::new(plan.filesystem.as_ref(), &plan.runs).unwrap();
        let deleted = cursor.next().unwrap().unwrap();
        assert_eq!(deleted.id, "deleted");
        assert!(deleted.record.is_none());
        let updated = cursor.next().unwrap().unwrap();
        assert_eq!(updated.id, "same");
        assert_eq!(updated.record.as_deref(), Some(&b"new"[..]));
        assert!(cursor.next().unwrap().is_none());
        drop(plan);
        assert!(paths.iter().all(|path| !filesystem.exists(path)));
        assert!(!filesystem.exists(&manifest));
    }

    #[test]
    fn open_removes_runs_left_by_an_interrupted_compaction() {
        let filesystem: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(8));
        let database: PathBuf = "interrupted-compaction.owl".into();
        let manifest = sidecar_path(&database, "compaction-runs");
        let run_prefix = sidecar_path(&database, "compaction");
        let run = compaction_run_path(&run_prefix, 0);
        filesystem.create(&run).unwrap().sync().unwrap();
        publish_compaction_run_manifest(filesystem.as_ref(), &manifest, 1).unwrap();
        let mut journal = filesystem
            .open_with_options(
                &manifest,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        journal.seek(SeekFrom::End(0)).unwrap();
        journal.write(&[0x4f, 0x57, 0x43]).unwrap();
        journal.sync().unwrap();

        let store = RustStore::new_with_fs(&database, 0, filesystem.clone()).unwrap();

        assert!(!filesystem.exists(&run));
        assert!(!filesystem.exists(&manifest));
        drop(store);
    }

    #[test]
    fn compaction_run_reader_rejects_a_truncated_frame() {
        let filesystem: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(9));
        let mut overlay = CompactionOverlay::new(
            filesystem.clone(),
            "corrupt-compaction".into(),
            "corrupt-compaction.manifest".into(),
            1,
        );
        overlay
            .put(
                "events".into(),
                "event-1".into(),
                ObjRef::from_parts(1, 1, 1),
                b"record".to_vec(),
            )
            .unwrap();
        let plan = overlay.finish().unwrap();
        let path = plan.runs[0].clone();
        filesystem
            .open_with_options(
                &path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap()
            .set_len(4)
            .unwrap();

        assert!(CompactionChangeCursor::new(plan.filesystem.as_ref(), &plan.runs).is_err());
    }

    #[test]
    fn compaction_run_reader_rejects_corrupt_block_payload() {
        let filesystem: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(10));
        let mut overlay = CompactionOverlay::new(
            filesystem.clone(),
            "checksum-compaction".into(),
            "checksum-compaction.manifest".into(),
            1,
        );
        overlay
            .put(
                "events".into(),
                "event-1".into(),
                ObjRef::from_parts(1, 1, 1),
                b"record".to_vec(),
            )
            .unwrap();
        let plan = overlay.finish().unwrap();
        let path = plan.runs[0].clone();
        let mut file = filesystem
            .open_with_options(
                &path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        let payload_offset =
            (COMPACTION_RUN_FILE_HEADER_LEN + COMPACTION_RUN_BLOCK_HEADER_LEN) as u64;
        file.seek(SeekFrom::Start(payload_offset)).unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(file.read(&mut byte).unwrap(), 1);
        byte[0] ^= 0x80;
        file.seek(SeekFrom::Start(payload_offset)).unwrap();
        assert_eq!(file.write(&byte).unwrap(), 1);

        assert!(CompactionChangeCursor::new(plan.filesystem.as_ref(), &plan.runs).is_err());
    }

    #[test]
    fn compaction_run_fan_in_is_bounded_without_losing_order() {
        let filesystem: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(11));
        let mut overlay = CompactionOverlay::new(
            filesystem.clone(),
            "fanin-compaction".into(),
            "fanin-compaction.manifest".into(),
            1,
        );
        for index in (0..40).rev() {
            overlay
                .put(
                    "events".into(),
                    format!("event-{index:03}"),
                    ObjRef::from_parts(1, index + 1, 1),
                    vec![index as u8],
                )
                .unwrap();
        }
        let plan = overlay.finish().unwrap();
        assert!(plan.runs.len() <= COMPACTION_MAX_MERGE_FAN_IN);
        assert!(plan.peak_run_bytes >= plan.run_bytes);

        let mut cursor = CompactionChangeCursor::new(plan.filesystem.as_ref(), &plan.runs).unwrap();
        for index in 0..40 {
            let change = cursor.next().unwrap().unwrap();
            assert_eq!(change.id, format!("event-{index:03}"));
            assert_eq!(change.record, Some(vec![index as u8]));
        }
        assert!(cursor.next().unwrap().is_none());
    }

    struct CheckpointFailingFileSystem {
        inner: SimulatedFileSystem,
        fail_checkpoints: AtomicBool,
        fail_manifests: AtomicBool,
        fail_sealed_wal_reads: AtomicBool,
        block_checkpoints: AtomicBool,
        checkpoint_entered: (Mutex<bool>, std::sync::Condvar),
        checkpoint_release: (Mutex<bool>, std::sync::Condvar),
    }

    impl CheckpointFailingFileSystem {
        fn new(seed: u64) -> Self {
            Self {
                inner: SimulatedFileSystem::new(seed),
                fail_checkpoints: AtomicBool::new(false),
                fail_manifests: AtomicBool::new(false),
                fail_sealed_wal_reads: AtomicBool::new(false),
                block_checkpoints: AtomicBool::new(false),
                checkpoint_entered: (Mutex::new(false), std::sync::Condvar::new()),
                checkpoint_release: (Mutex::new(false), std::sync::Condvar::new()),
            }
        }

        fn set_checkpoint_failure(&self, fail: bool) {
            self.fail_checkpoints.store(fail, Ordering::Release);
        }

        /// Fail manifest publication only, so a base segment is fully written
        /// but never becomes authoritative: the state a crash between the two
        /// steps leaves behind.
        fn set_manifest_failure(&self, fail: bool) {
            self.fail_manifests.store(fail, Ordering::Release);
        }

        /// Fail reads of the sealed WAL, which interrupts an incremental
        /// checkpoint after rotation and before anything is published.
        fn set_sealed_wal_read_failure(&self, fail: bool) {
            self.fail_sealed_wal_reads.store(fail, Ordering::Release);
        }

        fn block_checkpoints(&self) {
            self.block_checkpoints.store(true, Ordering::Release);
            *self.checkpoint_entered.0.lock().unwrap() = false;
            *self.checkpoint_release.0.lock().unwrap() = false;
        }

        fn wait_until_checkpoint_entered(&self) {
            let entered = self.checkpoint_entered.0.lock().unwrap();
            let (entered, timeout) = self
                .checkpoint_entered
                .1
                .wait_timeout_while(entered, web_time::Duration::from_secs(2), |entered| {
                    !*entered
                })
                .unwrap();
            assert!(*entered && !timeout.timed_out());
        }

        fn release_checkpoint(&self) {
            *self.checkpoint_release.0.lock().unwrap() = true;
            self.checkpoint_release.1.notify_all();
            self.block_checkpoints.store(false, Ordering::Release);
        }

        fn block_checkpoint_io_if_requested(&self, path: &Path) {
            let is_checkpoint = path.to_string_lossy().contains(".segment.")
                || path.to_string_lossy().contains(".manifest.");
            if !is_checkpoint || !self.block_checkpoints.load(Ordering::Acquire) {
                return;
            }
            *self.checkpoint_entered.0.lock().unwrap() = true;
            self.checkpoint_entered.1.notify_all();
            let released = self.checkpoint_release.0.lock().unwrap();
            drop(
                self.checkpoint_release
                    .1
                    .wait_while(released, |released| !*released)
                    .unwrap(),
            );
        }
    }

    impl FileSystem for CheckpointFailingFileSystem {
        fn create(&self, path: &Path) -> std::io::Result<Box<dyn FileHandle>> {
            let is_checkpoint = path.to_string_lossy().contains(".segment.")
                || path.to_string_lossy().contains(".manifest.");
            self.block_checkpoint_io_if_requested(path);
            if is_checkpoint && self.fail_checkpoints.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    ErrorKind::Other,
                    "injected checkpoint failure",
                ));
            }
            if path.to_string_lossy().contains(".manifest.")
                && self.fail_manifests.load(Ordering::Acquire)
            {
                return Err(std::io::Error::new(
                    ErrorKind::Other,
                    "injected manifest failure",
                ));
            }
            self.inner.create(path)
        }

        fn open(&self, path: &Path) -> std::io::Result<Box<dyn FileHandle>> {
            self.block_checkpoint_io_if_requested(path);
            if path.to_string_lossy().ends_with(".wal.sealed")
                && self.fail_sealed_wal_reads.load(Ordering::Acquire)
            {
                return Err(std::io::Error::new(
                    ErrorKind::Other,
                    "injected sealed WAL read failure",
                ));
            }
            self.inner.open(path)
        }

        fn open_with_options(
            &self,
            path: &Path,
            options: &FileOptions,
        ) -> std::io::Result<Box<dyn FileHandle>> {
            self.inner.open_with_options(path, options)
        }

        fn remove(&self, path: &Path) -> std::io::Result<()> {
            self.inner.remove(path)
        }

        fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            self.inner.rename(from, to)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn file_size(&self, path: &Path) -> std::io::Result<u64> {
            self.inner.file_size(path)
        }

        fn sync_all(&self) -> std::io::Result<()> {
            self.inner.sync_all()
        }

        fn mmap(
            &self,
            path: &Path,
            size: usize,
        ) -> std::io::Result<Box<dyn crate::simulation::filesystem::MemoryMap>> {
            self.inner.mmap(path, size)
        }

        fn list_dir(&self, directory: &Path) -> std::io::Result<Vec<PathBuf>> {
            self.inner.list_dir(directory)
        }
    }

    fn document(id: &str) -> Document {
        Document::with_id(id.to_string(), "events".to_string(), crate::document::Fields::default())
    }

    /// A checkpoint used to be one file. A database kept that way opens
    /// as it is, and its next compaction writes a root over a part.
    #[test]
    fn a_checkpoint_kept_as_one_file_opens_and_becomes_a_root_when_next_compacted() {
        use crate::storage_engine::parts::{part_path, parts_of, PartedFileSystem, PartsOf};
        let fs = Arc::new(SimulatedFileSystem::new(71));
        let path = Path::new("one-file.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        for number in 0..200 {
            let mut event = document(&format!("event-{number:04}"));
            event.fields.insert("code".into(), Value::Integer(number));
            store.put_document(&event).unwrap();
        }
        store.checkpoint().unwrap();
        drop(store);

        // Put each checkpoint back the way it used to be kept: its bytes
        // in one file at its path, and no parts.
        let segments = [sidecar_path(path, "segment.0"), sidecar_path(path, "segment.1")];
        let parted = PartedFileSystem::over(fs.clone());
        let mut rewritten = 0;
        for segment in &segments {
            let PartsOf::Root(parts) = parts_of(fs.as_ref(), segment) else {
                continue;
            };
            let mut bytes = vec![0u8; parted.file_size(segment).unwrap() as usize];
            let mut whole = parted.open(segment).unwrap();
            let mut read = 0;
            while read < bytes.len() {
                read += whole.read(&mut bytes[read..]).unwrap();
            }
            let mut file = fs.create(segment).unwrap();
            write_all(file.as_mut(), &bytes).unwrap();
            file.sync().unwrap();
            for part in parts {
                fs.remove(&part_path(segment, part)).unwrap();
            }
            rewritten += 1;
        }
        fs.sync_all().unwrap();
        assert!(rewritten >= 1);
        assert!(segments
            .iter()
            .all(|segment| !matches!(parts_of(fs.as_ref(), segment), PartsOf::Root(_))));

        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        assert_eq!(store.scan_documents("events").unwrap().len(), 200);
        store.verify_integrity().unwrap();
        let mut event = document("event-0200");
        event.fields.insert("code".into(), Value::Integer(200));
        store.put_document(&event).unwrap();
        store.compact().unwrap();
        assert!(segments
            .iter()
            .any(|segment| matches!(parts_of(fs.as_ref(), segment), PartsOf::Root(_))));
        assert_eq!(store.scan_documents("events").unwrap().len(), 201);
        store.verify_integrity().unwrap();
        drop(store);

        let store = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(store.scan_documents("events").unwrap().len(), 201);
        assert_eq!(
            store.get_document("events", &"event-0200".to_string()).unwrap().unwrap().fields.get("code"),
            Some(&Value::Integer(200))
        );
        store.verify_integrity().unwrap();
    }

    #[test]
    fn integrity_checker_validates_checkpoint_wals_indexes_and_live_state() {
        let fs = Arc::new(SimulatedFileSystem::new(50));
        let path = Path::new("integrity.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "by_code".into(),
                schema: "events".into(),
                fields: vec!["code".into()],
                unique: false,
            })
            .unwrap();
        let mut first = document("first");
        first.fields.insert("code".into(), Value::Integer(10));
        store.put_document(&first).unwrap();
        store.checkpoint().unwrap();
        let mut second = document("second");
        second.fields.insert("code".into(), Value::Integer(20));
        store.put_document(&second).unwrap();

        let report = store.verify_integrity().unwrap();
        assert_eq!(report.checkpoint_lsn, 2);
        assert_eq!(report.durable_lsn, 3);
        assert_eq!(report.live_objects, 2);
        assert_eq!(report.scalar_indexes, 1);
        assert!(report.files_checked >= 5);
        let mut third = document("third");
        third.fields.insert("code".into(), Value::Integer(30));
        store.put_document(&third).unwrap();
        assert_eq!(store.verify_integrity().unwrap().durable_lsn, 4);
        drop(store);

        assert_eq!(
            RustStore::check_integrity_with_fs(path, fs)
                .unwrap()
                .durable_lsn,
            4
        );
    }

    #[test]
    fn standalone_integrity_checker_rejects_a_torn_wal_tail() {
        let fs = Arc::new(SimulatedFileSystem::new(51));
        let path = Path::new("integrity-torn.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("durable")).unwrap();
        }
        let mut wal = fs
            .open_with_options(
                path,
                &FileOptions {
                    read: false,
                    write: true,
                    create: false,
                    truncate: false,
                    append: true,
                },
            )
            .unwrap();
        write_all(wal.as_mut(), &[0xaa, 0xbb, 0xcc]).unwrap();
        wal.sync().unwrap();
        let error = RustStore::check_integrity_with_fs(path, fs)
            .expect_err("integrity checker accepted a torn WAL tail");
        assert!(error.to_string().contains("torn or unauthenticated tail"));
    }

    #[test]
    fn reachability_reclaimer_removes_authority_before_obsolete_artifacts() {
        let fs = Arc::new(SimulatedFileSystem::new(52));
        let path = Path::new("reachability-reclaim.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store.put_document(&document("first")).unwrap();
        store.checkpoint().unwrap();
        store.put_document(&document("second")).unwrap();
        let current = store.compact().unwrap();
        assert_eq!(current.slot, 0);
        assert!(fs.exists(Path::new("reachability-reclaim.owl.manifest.1")));
        assert!(fs.exists(Path::new("reachability-reclaim.owl.segment.1")));

        let report = store.reclaim_obsolete_files().unwrap();
        assert_eq!(report.retained_sequence, 2);
        assert!(report.files_removed >= 4);
        assert!(report.bytes_reclaimed > 0);
        assert!(!fs.exists(Path::new("reachability-reclaim.owl.manifest.1")));
        assert!(!fs.exists(Path::new("reachability-reclaim.owl.segment.1")));
        assert!(fs.exists(Path::new("reachability-reclaim.owl.manifest.0")));
        assert!(fs.exists(Path::new("reachability-reclaim.owl.segment.0")));
        assert_eq!(store.verify_integrity().unwrap().live_objects, 2);
        drop(store);

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn reclaimer_preserves_old_authority_when_current_checkpoint_is_corrupt() {
        let fs = Arc::new(SimulatedFileSystem::new(54));
        let path = Path::new("reclaim-corrupt.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store.put_document(&document("first")).unwrap();
        store.compact().unwrap();
        store.put_document(&document("second")).unwrap();
        store.compact().unwrap();
        let current_segment = Path::new("reclaim-corrupt.owl.segment.0");
        let mut segment = fs
            .open_with_options(
                current_segment,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        segment.seek(SeekFrom::Start(0)).unwrap();
        write_all(segment.as_mut(), b"BAD!").unwrap();
        segment.sync().unwrap();

        assert!(store.reclaim_obsolete_files().is_err());
        assert!(fs.exists(Path::new("reclaim-corrupt.owl.manifest.1")));
        assert!(fs.exists(Path::new("reclaim-corrupt.owl.segment.1")));
    }

    #[test]
    fn randomized_index_mutations_match_oracle_across_checkpoints_and_reopen() {
        fn assert_oracle(store: &RustStore, oracle: &HashMap<String, (i64, u64)>) {
            for code in 0..24i64 {
                let mut expected = oracle
                    .iter()
                    .filter(|(_, (value, _))| *value == code)
                    .map(|(id, _)| store.get_objref("events", id).unwrap().unwrap())
                    .collect::<Vec<_>>();
                expected.sort();
                let mut actual = store
                    .lookup_scalar_index("events", "by_code", &[Value::Integer(code)])
                    .unwrap();
                actual.sort();
                assert_eq!(actual, expected, "point lookup differs for code {code}");
            }
            let mut expected = oracle
                .iter()
                .filter(|(_, (value, _))| (5..=17).contains(value))
                .map(|(id, (value, _))| (*value, store.get_objref("events", id).unwrap().unwrap()))
                .collect::<Vec<_>>();
            expected.sort();
            let actual = store
                .range_scalar_index(
                    "events",
                    "by_code",
                    &[Value::Integer(5)],
                    &[Value::Integer(17)],
                )
                .unwrap();
            assert_eq!(
                actual,
                expected
                    .into_iter()
                    .map(|(_, object)| object)
                    .collect::<Vec<_>>()
            );
            assert_eq!(store.scan_documents("events").unwrap().len(), oracle.len());
        }

        let fs = Arc::new(SimulatedFileSystem::new(53));
        let path = Path::new("randomized-index.owl");
        let mut oracle = HashMap::<String, (i64, u64)>::new();
        {
            let store = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            store
                .upsert_scalar_index(ScalarIndexDefinition {
                    name: "by_code".into(),
                    schema: "events".into(),
                    fields: vec!["code".into()],
                    unique: false,
                })
                .unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 11,
                })
                .unwrap();

            let mut random = 0x7a31_9d42_c5e8_106fu64;
            for step in 0..320u64 {
                random = random
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let id = format!("doc-{}", (random >> 24) % 64);
                if random & 7 == 0 && oracle.contains_key(&id) {
                    assert!(store.delete_document("events", &id).unwrap());
                    oracle.remove(&id);
                } else {
                    let code = ((random >> 40) % 24) as i64;
                    let version = oracle.get(&id).map_or(1, |(_, version)| version + 1);
                    let mut document = document(&id);
                    document.version = version;
                    document.fields.insert("code".into(), Value::Integer(code));
                    store.put_document(&document).unwrap();
                    oracle.insert(id, (code, version));
                }
                if step % 37 == 0 {
                    store.wait_for_maintenance().unwrap();
                    assert_oracle(&store, &oracle);
                }
            }
            store.wait_for_maintenance().unwrap();
            assert_oracle(&store, &oracle);
            assert_eq!(store.verify_integrity().unwrap().live_objects, oracle.len());
        }

        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs).unwrap();
        assert_oracle(&reopened, &oracle);
        assert_eq!(
            reopened.verify_integrity().unwrap().live_objects,
            oracle.len()
        );
    }

    #[test]
    fn ordered_collection_directory_survives_mutation_overlay_and_recovery() {
        fn ids(store: &RustStore) -> Vec<String> {
            store
                .scan_documents("events")
                .unwrap()
                .into_iter()
                .map(|document| document.id)
                .collect()
        }

        let fs = Arc::new(SimulatedFileSystem::new(59));
        let path = Path::new("ordered-collection-directory.owl");
        {
            let store = RustStore::new_with_fs(path, 4 * 1024 * 1024, fs.clone()).unwrap();
            for id in ["z", "a", "m"] {
                store.put_document(&document(id)).unwrap();
            }
            assert_eq!(ids(&store), ["a", "m", "z"]);

            let mut updated = document("m");
            updated
                .fields
                .insert("message".into(), Value::String("updated".into()));
            store.put_document(&updated).unwrap();
            assert!(store.delete_document("events", &"a".to_string()).unwrap());
            assert_eq!(ids(&store), ["m", "z"]);

            let transaction = store.begin_local_transaction().unwrap();
            store.put_document(&document("b")).unwrap();
            assert!(store.delete_document("events", &"z".to_string()).unwrap());
            assert_eq!(ids(&store), ["b", "m"]);
            store.rollback_local_transaction(transaction).unwrap();
            assert_eq!(ids(&store), ["m", "z"]);

            store.checkpoint().unwrap();
            store.verify_integrity().unwrap();
        }

        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 4 * 1024 * 1024, fs).unwrap();
        assert_eq!(ids(&reopened), ["m", "z"]);
        reopened.verify_integrity().unwrap();
    }

    #[test]
    fn compiled_slot_scan_preserves_null_versions_projection_and_checkpoint_reads() {
        fn scan(store: &RustStore) -> Vec<Document> {
            let filters = vec![ScanPredicate::And(
                Box::new(ScanPredicate::Comparison {
                    field: "score".into(),
                    operator: ComparisonOperator::GreaterEqual,
                    value: Value::Integer(10),
                }),
                Box::new(ScanPredicate::Or(
                    Box::new(ScanPredicate::Comparison {
                        field: "environment".into(),
                        operator: ComparisonOperator::Equal,
                        value: Value::Null,
                    }),
                    Box::new(ScanPredicate::Comparison {
                        field: "environment".into(),
                        operator: ComparisonOperator::Equal,
                        value: Value::String("production".into()),
                    }),
                )),
            )];
            store
                .scan_documents_filtered_projection(
                    "events",
                    &filters,
                    Some(&[
                        "level".to_string(),
                        "score".to_string(),
                        "environment".to_string(),
                    ]),
                )
                .unwrap()
                .unwrap()
        }

        let fs = Arc::new(SimulatedFileSystem::new(60));
        let path = Path::new("compiled-slot-scan.owl");
        {
            let store = RustStore::new_with_fs(path, 4 * 1024 * 1024, fs.clone()).unwrap();
            let mut old = document("old");
            old.fields
                .insert("level".into(), Value::String("info".into()));
            old.fields.insert("score".into(), Value::Integer(10));
            store.put_document(&old).unwrap();

            let mut current = document("current");
            current
                .fields
                .insert("level".into(), Value::String("error".into()));
            current.fields.insert("score".into(), Value::Float(11.5));
            current
                .fields
                .insert("environment".into(), Value::String("production".into()));
            store.put_document(&current).unwrap();

            let rows = scan(&store);
            assert_eq!(
                rows.iter()
                    .map(|document| document.id.as_str())
                    .collect::<Vec<_>>(),
                ["current", "old"]
            );
            assert!(!rows[1].fields.contains_key("environment"));
            assert_eq!(rows[1].fields["score"], Value::Integer(10));

            let id_filter = [ScanPredicate::Comparison {
                field: "id".into(),
                operator: ComparisonOperator::Equal,
                value: Value::String("old".into()),
            }];
            assert_eq!(
                store
                    .scan_documents_filtered_projection(
                        "events",
                        &id_filter,
                        Some(&["level".into()])
                    )
                    .unwrap()
                    .unwrap()
                    .len(),
                1
            );

            store.checkpoint().unwrap();
            assert_eq!(scan(&store).len(), 2);
            let state = store.state.read().unwrap();
            let object_ref = lookup_object_ref(&state, "events", &"old".to_string())
                .unwrap()
                .unwrap();
            assert!(state.object_records.get(&object_ref).is_none());
        }

        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 4 * 1024 * 1024, fs).unwrap();
        assert_eq!(scan(&reopened).len(), 2);
    }

    #[test]
    fn acknowledged_batch_recovers_atomically() {
        let fs = Arc::new(SimulatedFileSystem::new(7));
        let path = Path::new("atomic.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .write_batch(vec![
                    WriteOperation::PutDocument(document("a")),
                    WriteOperation::PutDocument(document("b")),
                ])
                .unwrap();
        }
        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert!(reopened
            .get_document("events", &"a".to_string())
            .unwrap()
            .is_some());
        assert!(reopened
            .get_document("events", &"b".to_string())
            .unwrap()
            .is_some());
        assert_eq!(reopened.committed_lsn(), 1);
    }

    #[test]
    fn update_keeps_stable_reference() {
        let fs = Arc::new(SimulatedFileSystem::new(8));
        let store = RustStore::new_with_fs("stable.owl", 1024 * 1024, fs).unwrap();
        let mut original = document("a");
        store.put_document(&original).unwrap();
        let before = store.get_objref("events", &original.id).unwrap().unwrap();
        original.version += 1;
        store.put_document(&original).unwrap();
        let after = store.get_objref("events", &original.id).unwrap().unwrap();
        assert_eq!(before, after);
        assert_eq!(
            store.navigate_reference(&before).unwrap().unwrap().version,
            2
        );
    }

    #[test]
    fn rollback_does_not_publish_queued_writes() {
        let fs = Arc::new(SimulatedFileSystem::new(9));
        let store = RustStore::new_with_fs("rollback.owl", 1024 * 1024, fs).unwrap();
        let id = store.begin_local_transaction().unwrap();
        store.put_document(&document("a")).unwrap();
        store.rollback_local_transaction(id).unwrap();
        assert!(store
            .get_document("events", &"a".to_string())
            .unwrap()
            .is_none());
        assert_eq!(store.committed_lsn(), 0);
    }

    #[test]
    fn torn_tail_is_removed_before_new_commits() {
        let fs = Arc::new(SimulatedFileSystem::new(10));
        let path = Path::new("torn.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
        }
        let valid_len = fs.file_size(path).unwrap();
        let mut file = fs
            .open_with_options(
                path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        write_all(file.as_mut(), b"OWLW\x01").unwrap();
        file.sync().unwrap();
        assert!(fs.file_size(path).unwrap() > valid_len);

        {
            let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            assert_eq!(reopened.committed_lsn(), 1);
            assert_eq!(
                reopened
                    .maintenance_status()
                    .unwrap()
                    .commits_since_checkpoint,
                1
            );
            assert_eq!(fs.file_size(path).unwrap(), valid_len);
            reopened.put_document(&document("b")).unwrap();
        }
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert!(reopened
            .get_document("events", &"b".to_string())
            .unwrap()
            .is_some());
    }

    #[test]
    fn complete_corrupt_frame_is_not_treated_as_missing_data() {
        let fs = Arc::new(SimulatedFileSystem::new(11));
        let path = Path::new("corrupt.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            // Written after the first frame was synced, and so a witness
            // that damage to the first frame is damage to synced data.
            store.put_document(&document("b")).unwrap();
        }
        let mut file = fs
            .open_with_options(
                path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        file.seek(SeekFrom::Start(WAL_HEADER_LEN as u64)).unwrap();
        write_all(file.as_mut(), &[0xff]).unwrap();
        file.sync().unwrap();

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("corrupt complete frame must fail open");
        assert!(error.to_string().contains("checksum"));
    }

    #[test]
    fn durable_records_do_not_repeat_top_level_field_names() {
        let fs = Arc::new(SimulatedFileSystem::new(12));
        let path = Path::new("compiled.owl");
        let schema = Schema::new(
            "events".to_string(),
            HashMap::from([
                ("severity_code".to_string(), FieldType::String),
                ("sequence_number".to_string(), FieldType::Integer),
            ]),
        );
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_schema(&schema).unwrap();
        }
        let catalog_bytes = read_file(fs.as_ref(), path);
        let catalog_name_count = count_bytes(&catalog_bytes, b"severity_code");
        assert!(catalog_name_count > 0);

        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            for id in ["one", "two"] {
                let document = Document::with_id(
                    id.to_string(),
                    "events".to_string(),
                    crate::document::Fields::from_iter([
                        ("severity_code".to_string(), Value::String("E".to_string())),
                        ("sequence_number".to_string(), Value::Integer(7)),
                    ]),
                );
                store.put_document(&document).unwrap();
            }
        }
        let all_bytes = read_file(fs.as_ref(), path);
        assert_eq!(
            count_bytes(&all_bytes, b"severity_code"),
            catalog_name_count,
            "field names must not be repeated by document records"
        );
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
        let projected = reopened
            .get_document_projection("events", &"one".to_string(), &["severity_code".to_string()])
            .unwrap()
            .unwrap();
        assert_eq!(projected.fields.len(), 1);
        assert_eq!(
            projected.fields["severity_code"],
            Value::String("E".to_string())
        );
    }

    #[test]
    fn full_checkpoint_stores_scalar_rows_once_and_keeps_nested_rows_as_overflow() {
        let fs = Arc::new(SimulatedFileSystem::new(120));
        let store = RustStore::new_with_fs("authoritative.owl", 1024 * 1024, fs.clone()).unwrap();
        let scalar_schema = Schema::new(
            "logs".into(),
            HashMap::from([
                ("message".into(), FieldType::String),
                ("level".into(), FieldType::Integer),
            ]),
        );
        store.put_schema(&scalar_schema).unwrap();
        let scalar = Document::with_id(
            "scalar".into(),
            "logs".into(),
            crate::document::Fields::from_iter([
                ("message".into(), Value::String("failed".into())),
                ("level".into(), Value::Integer(50)),
            ]),
        );
        store.put_document(&scalar).unwrap();

        let nested_schema = Schema::new(
            "payloads".into(),
            HashMap::from([("context".into(), FieldType::Object)]),
        );
        store.put_schema(&nested_schema).unwrap();
        let nested = Document::with_id(
            "nested".into(),
            "payloads".into(),
            crate::document::Fields::from_iter([(
                "context".into(),
                Value::Object(crate::document::Fields::from_iter([(
                    "request".into(),
                    Value::String("req-1".into()),
                )])),
            )]),
        );
        store.put_document(&nested).unwrap();

        let state = store.core.state.read().unwrap();
        let image = store.core.checkpoint_image_from_state(&state, 1).unwrap();
        assert_eq!(
            image.records.len(),
            1,
            "only the nested overflow row should retain OWLR"
        );
        assert_eq!(
            RecordCodec::decode(
                &image.records[0].1,
                state
                    .compiled_schemas
                    .get(&(
                        image.records[0].0.collection_id(),
                        RecordCodec::header(&image.records[0].1)
                            .unwrap()
                            .schema_version,
                    ))
                    .unwrap()
            )
            .unwrap(),
            nested
        );
        let segments = image
            .typed_segments
            .iter()
            .map(|bytes| TypedSegment::decode(bytes).unwrap())
            .collect::<Vec<_>>();
        assert!(segments.iter().any(|segment| segment.authoritative));
        assert!(segments.iter().any(|segment| !segment.authoritative));
        drop(state);

        store.checkpoint().unwrap();
        drop(store);
        let reopened = RustStore::new_with_fs("authoritative.owl", 1024 * 1024, fs).unwrap();
        let scalar_layout = reopened.storage_layout_stats("logs").unwrap();
        assert_eq!(scalar_layout.typed_authoritative_rows, 1);
        assert_eq!(scalar_layout.row_overflow_rows, 0);
        // Opening decodes nothing: the nested row is read from its record
        // when it is first asked for, as the scalar one is from its segment.
        assert_eq!(scalar_layout.resident_documents, 0);
        assert_eq!(scalar_layout.lazy_typed_rows, 1);
        let nested_layout = reopened.storage_layout_stats("payloads").unwrap();
        assert_eq!(nested_layout.typed_authoritative_rows, 0);
        assert_eq!(nested_layout.row_overflow_rows, 1);
        assert_eq!(
            reopened
                .get_document("logs", &"scalar".to_string())
                .unwrap(),
            Some(scalar)
        );
        let materialized_layout = reopened.storage_layout_stats("logs").unwrap();
        assert_eq!(materialized_layout.resident_documents, 1);
        assert_eq!(materialized_layout.lazy_typed_rows, 0);
        assert_eq!(
            reopened
                .get_document("payloads", &"nested".to_string())
                .unwrap(),
            Some(nested)
        );
        let both = reopened.storage_layout_stats("logs").unwrap();
        assert_eq!(both.resident_documents, 2);
    }

    #[test]
    fn schemaless_field_evolution_recovers_old_and_new_record_versions() {
        let fs = Arc::new(SimulatedFileSystem::new(13));
        let path = Path::new("evolution.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            let first = Document::with_id(
                "first".to_string(),
                "events".to_string(),
                crate::document::Fields::from_iter([("message".to_string(), Value::String("old".to_string()))]),
            );
            let second = Document::with_id(
                "second".to_string(),
                "events".to_string(),
                crate::document::Fields::from_iter([
                    ("message".to_string(), Value::String("new".to_string())),
                    ("code".to_string(), Value::Integer(500)),
                ]),
            );
            store.put_document(&first).unwrap();
            store.put_document(&second).unwrap();
        }
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        let first = reopened
            .get_document("events", &"first".to_string())
            .unwrap()
            .unwrap();
        let second = reopened
            .get_document("events", &"second".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(first.fields.len(), 1);
        assert_eq!(second.fields["code"], Value::Integer(500));
    }

    #[test]
    fn checkpoint_bounds_wal_and_preserves_restart_then_insert() {
        let fs = Arc::new(SimulatedFileSystem::new(14));
        let path = Path::new("checkpoint.owl");
        let object_ref;
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            object_ref = store
                .get_objref("events", &"a".to_string())
                .unwrap()
                .unwrap();
            let checkpoint = store.checkpoint().unwrap();
            assert_eq!(checkpoint.sequence, 1);
            assert_eq!(checkpoint.checkpoint_lsn, 1);
            assert_eq!(checkpoint.live_objects, 1);
            assert_eq!(checkpoint.slot, 1);
            assert_eq!(fs.file_size(path).unwrap(), 0);
        }
        {
            let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            assert_eq!(reopened.committed_lsn(), 1);
            assert_eq!(
                reopened
                    .get_objref("events", &"a".to_string())
                    .unwrap()
                    .unwrap(),
                object_ref
            );
            reopened.put_document(&document("b")).unwrap();
            assert_eq!(reopened.committed_lsn(), 2);
        }
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert!(reopened
            .get_document("events", &"a".to_string())
            .unwrap()
            .is_some());
        assert!(reopened
            .get_document("events", &"b".to_string())
            .unwrap()
            .is_some());
    }

    #[test]
    fn real_filesystem_reopens_a_checkpoint_with_an_empty_index_root() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("real-checkpoint.owl");
        {
            let store = RustStore::new(&path, 8 * 1024 * 1024).unwrap();
            store.put_document(&document("a")).unwrap();
            store.compact().unwrap();
        }
        let reopened = RustStore::new(&path, 8 * 1024 * 1024).unwrap();
        assert_eq!(reopened.committed_lsn(), 1);
        assert!(reopened
            .get_document("events", &"a".to_string())
            .unwrap()
            .is_some());
    }

    #[test]
    fn checkpoint_publishes_and_recovers_a_validated_typed_segment() {
        let fs = Arc::new(SimulatedFileSystem::new(61));
        let path = Path::new("typed-checkpoint.owl");
        let schema = Schema::new(
            "events".into(),
            HashMap::from([
                ("message".into(), FieldType::String),
                ("level_rank".into(), FieldType::Integer),
            ]),
        );
        {
            let store = RustStore::new_with_fs(path, 4 * 1024 * 1024, fs.clone()).unwrap();
            store.put_schema(&schema).unwrap();
            for (id, rank) in [("a", 20), ("b", 50)] {
                store
                    .put_document(&Document::with_id(
                        id.into(),
                        "events".into(),
                        crate::document::Fields::from_iter([
                            ("message".into(), Value::String(format!("message-{id}"))),
                            ("level_rank".into(), Value::Integer(rank)),
                        ]),
                    ))
                    .unwrap();
            }
            store.checkpoint().unwrap();
            let layout = store.storage_layout_stats("events").unwrap();
            assert_eq!(layout.resident_documents, 0);
            assert_eq!(layout.lazy_typed_rows, 2);
            assert_eq!(layout.typed_loaded_segments, 0);
            assert_eq!(layout.typed_identity_bytes, 0);
        }

        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 4 * 1024 * 1024, fs).unwrap();
        {
            let state = reopened.state.read().unwrap();
            assert_eq!(state.typed_base_lsn, state.committed_lsn);
            assert_eq!(state.typed_segments.len(), 1);
            let segment = &state.typed_segments[0];
            assert_eq!(segment.row_count(), 2);
            assert!(!segment.is_loaded().unwrap());
            let field_id = state
                .compiled_schemas
                .get(&(segment.collection_id(), segment.schema_version()))
                .unwrap()
                .fields
                .iter()
                .find(|field| field.name == "level_rank")
                .unwrap()
                .id;
            assert_eq!(
                segment
                    .load()
                    .unwrap()
                    .column(field_id)
                    .unwrap()
                    .cell(1)
                    .unwrap(),
                TypedCell::Integer(50)
            );
            assert!(segment.is_loaded().unwrap());
        }
        let rank_filter = [ScanPredicate::Comparison {
            field: "level_rank".into(),
            operator: ComparisonOperator::GreaterEqual,
            value: Value::Integer(50),
        }];
        let batches = reopened
            .scan_projected_batches("events", &rank_filter, &["message".into()], 64)
            .unwrap()
            .unwrap();
        let row = batches[0].rows().next().unwrap();
        assert_eq!(
            row.fields().next(),
            Some((
                "message",
                crate::storage::ProjectedValueRef::String("message-b")
            ))
        );
        let ids = batches
            .into_iter()
            .flat_map(|batch| {
                batch
                    .rows()
                    .map(|row| row.id().to_string())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, ["b"]);

        let mut updated = reopened
            .get_document("events", &"a".into())
            .unwrap()
            .unwrap();
        updated.set_field("level_rank".into(), Value::Integer(60));
        reopened.put_document(&updated).unwrap();
        {
            let state = reopened.state.read().unwrap();
            assert_ne!(state.typed_base_lsn, state.committed_lsn);
        }
        let ids = reopened
            .scan_projected_batches("events", &rank_filter, &["message".into()], 64)
            .unwrap()
            .unwrap()
            .into_iter()
            .flat_map(|batch| {
                batch
                    .rows()
                    .map(|row| row.id().to_string())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn replacement_checkpoint_keeps_typed_coverage_for_evicted_base_rows() {
        let fs = Arc::new(SimulatedFileSystem::new(62));
        let path = Path::new("typed-replacement.owl");
        let schema = Schema::new(
            "events".into(),
            HashMap::from([("sequence".into(), FieldType::Integer)]),
        );
        {
            let store = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            store.put_schema(&schema).unwrap();
            for sequence in 0..64 {
                store
                    .put_document(&Document::with_id(
                        format!("event-{sequence:03}"),
                        "events".into(),
                        crate::document::Fields::from_iter([("sequence".into(), Value::Integer(sequence))]),
                    ))
                    .unwrap();
            }
            store.checkpoint().unwrap();
            assert_eq!(store.state.read().unwrap().object_records.len(), 0);
            for sequence in 64..80 {
                store
                    .put_document(&Document::with_id(
                        format!("event-{sequence:03}"),
                        "events".into(),
                        crate::document::Fields::from_iter([("sequence".into(), Value::Integer(sequence))]),
                    ))
                    .unwrap();
            }
            store.compact().unwrap();
        }
        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs).unwrap();
        let layout = reopened.storage_layout_stats("events").unwrap();
        assert_eq!(layout.typed_rows, 80);
        assert_eq!(layout.live_rows, 80);
        assert!(layout.typed_scan_current);
    }

    #[test]
    fn typed_base_merges_insert_update_delete_and_reopen_in_id_order() {
        fn scan(store: &RustStore) -> Vec<(String, bool)> {
            let filter = [ScanPredicate::Comparison {
                field: "level_rank".into(),
                operator: ComparisonOperator::GreaterEqual,
                value: Value::Integer(20),
            }];
            store
                .scan_projected_batches("events", &filter, &["message".into()], 2)
                .unwrap()
                .unwrap()
                .into_iter()
                .flat_map(|batch| {
                    batch
                        .rows()
                        .map(|row| {
                            let typed = matches!(
                                row.fields().next().unwrap().1,
                                crate::storage::ProjectedValueRef::String(_)
                            );
                            (row.id().to_string(), typed)
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        }

        let fs = Arc::new(SimulatedFileSystem::new(64));
        let path = Path::new("typed-delta-merge.owl");
        let schema = Schema::new(
            "events".into(),
            HashMap::from([
                ("message".into(), FieldType::String),
                ("level_rank".into(), FieldType::Integer),
            ]),
        );
        {
            let store = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            store.put_schema(&schema).unwrap();
            for (id, rank) in [("a", 10), ("b", 20), ("c", 30), ("d", 40)] {
                store
                    .put_document(&Document::with_id(
                        id.into(),
                        "events".into(),
                        crate::document::Fields::from_iter([
                            ("message".into(), Value::String(format!("message-{id}"))),
                            ("level_rank".into(), Value::Integer(rank)),
                        ]),
                    ))
                    .unwrap();
            }
            store.checkpoint().unwrap();
            assert!(
                store
                    .storage_layout_stats("events")
                    .unwrap()
                    .typed_scan_current
            );

            let mut updated = store.get_document("events", &"b".into()).unwrap().unwrap();
            updated.set_field("level_rank".into(), Value::Integer(5));
            store.put_document(&updated).unwrap();
            store.delete_document("events", &"c".into()).unwrap();
            store
                .put_document(&Document::with_id(
                    "aa".into(),
                    "events".into(),
                    crate::document::Fields::from_iter([
                        ("message".into(), Value::String("message-aa".into())),
                        ("level_rank".into(), Value::Integer(35)),
                    ]),
                ))
                .unwrap();

            let layout = store.storage_layout_stats("events").unwrap();
            assert!(layout.typed_scan_available);
            assert!(!layout.typed_scan_current);
            assert_eq!(layout.typed_delta_rows, 2);
            assert_eq!(layout.typed_stale_rows, 3);
            assert_eq!(
                scan(&store),
                [("aa".to_string(), false), ("d".to_string(), true)]
            );
        }

        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
        let layout = reopened.storage_layout_stats("events").unwrap();
        assert!(layout.typed_scan_available);
        assert_eq!(layout.typed_delta_rows, 2);
        assert_eq!(layout.typed_stale_rows, 3);
        assert_eq!(
            scan(&reopened),
            [("aa".to_string(), false), ("d".to_string(), true)]
        );

        reopened.compact().unwrap();
        let layout = reopened.storage_layout_stats("events").unwrap();
        assert!(layout.typed_scan_current);
        assert_eq!(layout.typed_delta_rows, 0);
        assert_eq!(
            scan(&reopened),
            [("aa".to_string(), true), ("d".to_string(), true)]
        );
    }

    #[test]
    fn incremental_checkpoint_preserves_typed_row_overlay() {
        let fs = Arc::new(SimulatedFileSystem::new(65));
        let path = Path::new("typed-incremental-overlay.owl");
        let schema = Schema::new(
            "events".into(),
            HashMap::from([("sequence".into(), FieldType::Integer)]),
        );
        {
            let store = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            store.put_schema(&schema).unwrap();
            store
                .put_document(&Document::with_id(
                    "a".into(),
                    "events".into(),
                    crate::document::Fields::from_iter([("sequence".into(), Value::Integer(1))]),
                ))
                .unwrap();
            store.checkpoint().unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            let mut updated = store.get_document("events", &"a".into()).unwrap().unwrap();
            updated.set_field("sequence".into(), Value::Integer(2));
            store.put_document(&updated).unwrap();
            store.wait_for_maintenance().unwrap();
            let layout = store.storage_layout_stats("events").unwrap();
            assert!(layout.typed_scan_available);
            assert_eq!(layout.typed_delta_rows, 1);
            assert_eq!(layout.typed_stale_rows, 1);
        }

        fs.crash();
        fs.recover();
        let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs).unwrap();
        let layout = reopened.storage_layout_stats("events").unwrap();
        assert!(layout.typed_scan_available);
        assert_eq!(layout.typed_delta_rows, 1);
        let filter = [ScanPredicate::Comparison {
            field: "sequence".into(),
            operator: ComparisonOperator::Equal,
            value: Value::Integer(2),
        }];
        assert_eq!(
            reopened
                .scan_projected_batches("events", &filter, &["sequence".into()], 64)
                .unwrap()
                .unwrap()
                .iter()
                .map(ProjectedRowBatch::len)
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn checkpoint_recovery_resolves_forward_reverse_reference_targets() {
        let fs = Arc::new(SimulatedFileSystem::new(47));
        let path = Path::new("checkpoint-references.owl");
        {
            let store = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("target")).unwrap();
            let target = store
                .get_objref("events", &"target".to_string())
                .unwrap()
                .unwrap();
            let mut source = document("source");
            source
                .fields
                .insert("parent".into(), Value::Reference(target));
            store.put_document(&source).unwrap();
            store.checkpoint().unwrap();
        }
        let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs).unwrap();
        assert_eq!(
            reopened
                .list_references_to(
                    "events",
                    &"target".to_string(),
                    Some("events"),
                    Some("parent"),
                )
                .unwrap(),
            vec![ReverseReferenceEntry {
                source_schema: "events".into(),
                source_id: "source".into(),
                field_path: "parent".into(),
            }]
        );
        let mut source = reopened
            .get_document("events", &"source".to_string())
            .unwrap()
            .unwrap();
        source.fields.remove("parent");
        reopened.put_document(&source).unwrap();
        assert!(reopened
            .list_references_to("events", &"target".to_string(), None, None)
            .unwrap()
            .is_empty());
        let target = reopened
            .get_objref("events", &"target".to_string())
            .unwrap()
            .unwrap();
        source
            .fields
            .insert("parent".into(), Value::Reference(target));
        reopened.put_document(&source).unwrap();
        assert_eq!(
            reopened
                .list_references_to("events", &"target".to_string(), None, Some("parent"))
                .unwrap()
                .len(),
            1
        );
        reopened
            .delete_document("events", &"source".to_string())
            .unwrap();
        assert!(reopened
            .list_references_to("events", &"target".to_string(), None, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn immutable_generation_restores_reverse_reference_patches() {
        let fs = Arc::new(SimulatedFileSystem::new(48));
        let path = Path::new("generation-references.owl");
        {
            let store = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("target")).unwrap();
            store.checkpoint().unwrap();
            let target = store
                .get_objref("events", &"target".to_string())
                .unwrap()
                .unwrap();
            let mut source = document("source");
            source
                .fields
                .insert("parent".into(), Value::Reference(target));
            store.put_document(&source).unwrap();
            store.checkpoint().unwrap();
        }
        {
            let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            assert_eq!(
                reopened
                    .list_references_to("events", &"target".into(), None, Some("parent"))
                    .unwrap()
                    .len(),
                1
            );
            let mut source = reopened
                .get_document("events", &"source".into())
                .unwrap()
                .unwrap();
            source.fields.remove("parent");
            reopened.put_document(&source).unwrap();
            reopened.checkpoint().unwrap();
        }
        let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs).unwrap();
        assert!(reopened
            .list_references_to("events", &"target".into(), None, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn typed_reference_recovery_builds_reverse_edges_without_hydrating_rows() {
        let fs = Arc::new(SimulatedFileSystem::new(147));
        let path = Path::new("lazy-typed-references.owl");
        let schema = Schema::new(
            "nodes".into(),
            HashMap::from([(
                "parent".into(),
                FieldType::Optional(Box::new(FieldType::Reference(
                    crate::schema::ReferenceType::new("nodes".into()),
                ))),
            )]),
        );
        {
            let store = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs.clone()).unwrap();
            store.put_schema(&schema).unwrap();
            let target = Document::with_id("target".into(), "nodes".into(), crate::document::Fields::default());
            store.put_document(&target).unwrap();
            let target_ref = store
                .get_objref("nodes", &"target".to_string())
                .unwrap()
                .unwrap();
            let source = Document::with_id(
                "source".into(),
                "nodes".into(),
                crate::document::Fields::from_iter([("parent".into(), Value::Reference(target_ref))]),
            );
            store.put_document(&source).unwrap();
            store.checkpoint().unwrap();
        }

        let reopened = RustStore::new_with_fs(path, 8 * 1024 * 1024, fs).unwrap();
        let layout = reopened.storage_layout_stats("nodes").unwrap();
        assert_eq!(layout.resident_documents, 0);
        assert_eq!(layout.lazy_typed_rows, 2);
        assert_eq!(layout.typed_loaded_segments, 0);
        assert_eq!(layout.typed_identity_bytes, 0);
        assert_eq!(layout.objref_locator_cached_pages, 0);
        assert_eq!(layout.objref_locator_cached_bytes, 0);
        assert_eq!(
            reopened
                .list_references_to(
                    "nodes",
                    &"target".to_string(),
                    Some("nodes"),
                    Some("parent"),
                )
                .unwrap(),
            vec![ReverseReferenceEntry {
                source_schema: "nodes".into(),
                source_id: "source".into(),
                field_path: "parent".into(),
            }]
        );
        assert_eq!(
            reopened
                .storage_layout_stats("nodes")
                .unwrap()
                .resident_documents,
            0
        );
        assert_eq!(
            reopened
                .storage_layout_stats("nodes")
                .unwrap()
                .typed_loaded_segments,
            0
        );
        let layout = reopened.storage_layout_stats("nodes").unwrap();
        assert!(layout.objref_locator_cached_pages <= 1);
        assert!(layout.objref_locator_cached_bytes <= 16 * 1024);
    }

    #[test]
    fn full_compactions_alternate_manifest_and_segment_slots() {
        let fs = Arc::new(SimulatedFileSystem::new(15));
        let path = Path::new("alternating.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            assert_eq!(store.compact().unwrap().slot, 1);
            store.put_document(&document("b")).unwrap();
            let second = store.compact().unwrap();
            assert_eq!(second.sequence, 2);
            assert_eq!(second.slot, 0);
            assert_eq!(fs.file_size(path).unwrap(), 0);
        }
        assert!(fs.exists(Path::new("alternating.owl.manifest.0")));
        assert!(fs.exists(Path::new("alternating.owl.manifest.1")));
        assert!(fs.exists(Path::new("alternating.owl.segment.0")));
        assert!(fs.exists(Path::new("alternating.owl.segment.1")));
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn ordinary_checkpoint_publishes_changes_without_rewriting_the_base() {
        let fs = Arc::new(SimulatedFileSystem::new(17));
        let path = Path::new("incremental-checkpoint.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("base")).unwrap();
            store.checkpoint().unwrap();
            let base_bytes = fs
                .file_size(Path::new("incremental-checkpoint.owl.segment.1"))
                .unwrap();

            store.put_document(&document("delta")).unwrap();
            let second = store.checkpoint().unwrap();
            assert_eq!(second.sequence, 2);
            assert_eq!(
                fs.file_size(Path::new("incremental-checkpoint.owl.segment.1"))
                    .unwrap(),
                base_bytes
            );
            assert!(!fs.exists(Path::new("incremental-checkpoint.owl.segment.0")));
            let manifest = Manifest::decode(&read_file(
                fs.as_ref(),
                Path::new("incremental-checkpoint.owl.manifest.0"),
            ))
            .unwrap();
            assert_eq!(manifest.segment_slot, 1);
            assert_eq!(manifest.delta_len, 0);
            let root = GenerationRoot::decode(&read_file(
                fs.as_ref(),
                Path::new("incremental-checkpoint.owl.generations.0"),
            ))
            .unwrap();
            assert_eq!(root.generations.len(), 1);
            assert!(fs.exists(Path::new("incremental-checkpoint.owl.generation.2")));
            assert!(fs.exists(Path::new("incremental-checkpoint.owl.generation-record.2")));
            assert!(fs.exists(Path::new("incremental-checkpoint.owl.generation-index.2")));
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn checkpoint_without_new_commits_is_a_noop() {
        let fs = Arc::new(SimulatedFileSystem::new(18));
        let path = Path::new("noop-checkpoint.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store.put_document(&document("base")).unwrap();
        let first = store.checkpoint().unwrap();
        let second = store.checkpoint().unwrap();
        assert_eq!(second.sequence, first.sequence);
        assert_eq!(second.checkpoint_lsn, first.checkpoint_lsn);
        assert_eq!(second.segment_bytes, 0);
        assert!(!fs.exists(Path::new("noop-checkpoint.owl.wal.sealed")));
        assert!(!fs.exists(Path::new("noop-checkpoint.owl.generation.2")));
        assert!(!fs.exists(Path::new("noop-checkpoint.owl.generation-index.2")));
    }

    #[test]
    fn reopen_reclaims_a_generation_not_published_by_a_manifest() {
        let fs = Arc::new(SimulatedFileSystem::new(19));
        let path = Path::new("unpublished-generation.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("base")).unwrap();
            store.checkpoint().unwrap();
        }
        let orphan = Path::new("unpublished-generation.owl.generation.2");
        let orphan_record = Path::new("unpublished-generation.owl.generation-record.2");
        let orphan_index = Path::new("unpublished-generation.owl.generation-index.2");
        write_synced_file(fs.as_ref(), orphan, b"unpublished").unwrap();
        write_synced_file(fs.as_ref(), orphan_record, b"unpublished").unwrap();
        write_synced_file(fs.as_ref(), orphan_index, b"unpublished").unwrap();
        assert!(fs.exists(orphan));

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        assert!(!fs.exists(orphan));
        assert!(!fs.exists(orphan_record));
        assert!(!fs.exists(orphan_index));
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 1);
    }

    #[test]
    fn automatic_checkpoint_bounds_wal_and_reports_maintenance_state() {
        let fs = Arc::new(SimulatedFileSystem::new(25));
        let path = Path::new("automatic-checkpoint.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 2,
                })
                .unwrap();

            store.put_document(&document("a")).unwrap();
            let before = store.maintenance_status().unwrap();
            assert_eq!(before.commits_since_checkpoint, 1);
            assert!(before.wal_bytes > 0);
            assert!(before.last_checkpoint.is_none());

            store.put_document(&document("b")).unwrap();
            store.wait_for_maintenance().unwrap();
            let after = store.maintenance_status().unwrap();
            assert_eq!(after.wal_bytes, 0);
            assert_eq!(after.commits_since_checkpoint, 0);
            assert_eq!(after.checkpoint_sequence, 1);
            assert_eq!(after.last_checkpoint.unwrap().checkpoint_lsn, 2);
            assert!(after.last_error.is_none());
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn automatic_checkpoint_honors_the_wal_byte_threshold() {
        let fs = Arc::new(SimulatedFileSystem::new(28));
        let path = Path::new("automatic-wal-threshold.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        store
            .configure_auto_checkpoint(AutoCheckpointPolicy {
                wal_bytes: 1,
                commits: 0,
            })
            .unwrap();

        store.put_document(&document("a")).unwrap();
        store.wait_for_maintenance().unwrap();
        let status = store.maintenance_status().unwrap();
        assert_eq!(status.wal_bytes, 0);
        assert_eq!(status.checkpoint_sequence, 1);
        assert_eq!(status.last_checkpoint.unwrap().checkpoint_lsn, 1);
    }

    #[test]
    fn automatic_incremental_checkpoint_reopens_from_base_plus_delta() {
        let fs = Arc::new(SimulatedFileSystem::new(33));
        let path = Path::new("automatic-incremental.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            store.put_document(&document("base")).unwrap();
            store.wait_for_maintenance().unwrap();
            store.put_document(&document("delta")).unwrap();
            store.wait_for_maintenance().unwrap();

            let manifest = Manifest::decode(&read_file(
                fs.as_ref(),
                Path::new("automatic-incremental.owl.manifest.0"),
            ))
            .unwrap();
            assert_eq!(manifest.sequence, 2);
            assert_eq!(manifest.segment_slot, 1);
            assert_eq!(manifest.delta_len, 0);
            assert!(!manifest.has_record_root);
            let root = GenerationRoot::decode(&read_file(
                fs.as_ref(),
                Path::new("automatic-incremental.owl.generations.0"),
            ))
            .unwrap();
            assert_eq!(root.generations.len(), 1);
            assert!(fs.exists(Path::new("automatic-incremental.owl.generation.2")));
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn explicit_compaction_folds_flush_generations_into_a_new_base() {
        let fs = Arc::new(SimulatedFileSystem::new(39));
        let path = Path::new("index-root-upgrade.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            store.put_document(&document("base")).unwrap();
            store.wait_for_maintenance().unwrap();
            store.put_document(&document("delta")).unwrap();
            store.wait_for_maintenance().unwrap();
        }
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        let compacted = store.compact().unwrap();
        assert_eq!(compacted.sequence, 3);

        let upgraded = Manifest::decode(&read_file(
            fs.as_ref(),
            Path::new("index-root-upgrade.owl.manifest.1"),
        ))
        .unwrap();
        assert_eq!(upgraded.sequence, compacted.sequence);
        assert_eq!(upgraded.delta_len, 0);
        // Sequence 3 lands in manifest slot 1, but the base it replaces also
        // lives in segment slot 1 (published by sequence 1 and still
        // referenced by the incremental sequence 2). The new base must go to
        // the other segment file.
        assert_eq!(upgraded.slot, 1);
        assert_eq!(upgraded.segment_slot, 0);
        assert!(upgraded.has_index_root);
    }

    #[test]
    fn interrupted_compaction_never_overwrites_the_published_base_segment() {
        let fs = Arc::new(CheckpointFailingFileSystem::new(61));
        let path = Path::new("base-overwrite.owl");
        let base_segment = Path::new("base-overwrite.owl.segment.1");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("base")).unwrap();
            // Sequence 1: full checkpoint, base in segment slot 1.
            assert_eq!(store.checkpoint().unwrap().sequence, 1);
            store.put_document(&document("delta")).unwrap();
            // Sequence 2: incremental, manifest slot 0 still references the
            // base in segment slot 1.
            assert_eq!(store.checkpoint().unwrap().sequence, 2);
            let published = Manifest::decode(&read_file(
                fs.as_ref(),
                Path::new("base-overwrite.owl.manifest.0"),
            ))
            .unwrap();
            assert_eq!(published.segment_slot, 1);
            let base_bytes = read_file(fs.as_ref(), base_segment);

            store.put_document(&document("tail")).unwrap();
            // Sequence 3 writes its whole segment, then "crashes" before the
            // manifest that would publish it.
            fs.set_manifest_failure(true);
            store
                .compact()
                .expect_err("manifest publication was injected to fail");
            fs.set_manifest_failure(false);

            assert_eq!(
                read_file(fs.as_ref(), base_segment),
                base_bytes,
                "an unpublished compaction rewrote the authoritative base segment"
            );
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 3);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 3);
    }

    #[test]
    fn failed_incremental_checkpoint_does_not_lose_the_sealed_wal() {
        let fs = Arc::new(CheckpointFailingFileSystem::new(62));
        let path = Path::new("sealed-wal-retained.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("base")).unwrap();
            store.checkpoint().unwrap();
            store.put_document(&document("delta")).unwrap();

            // The incremental checkpoint rotates the WAL, then fails before
            // it can build or publish anything from the sealed generation.
            fs.set_sealed_wal_read_failure(true);
            store
                .checkpoint()
                .expect_err("sealed WAL read was injected to fail");
            fs.set_sealed_wal_read_failure(false);
            assert!(fs.exists(Path::new("sealed-wal-retained.owl.wal.sealed")));

            // The store must refuse further work rather than rotate again:
            // another rotation would reclaim the sealed WAL holding "delta".
            assert!(store.put_document(&document("after")).is_err());
            assert!(store.checkpoint().is_err());
            assert!(fs.exists(Path::new("sealed-wal-retained.owl.wal.sealed")));
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert!(reopened
            .get_document("events", &"delta".to_string())
            .unwrap()
            .is_some());
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    fn overwrite_at(fs: &SimulatedFileSystem, path: &Path, offset: u64, bytes: &[u8]) {
        let mut file = fs
            .open_with_options(
                path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        write_all(file.as_mut(), bytes).unwrap();
        file.sync().unwrap();
    }

    fn replace_file(fs: &SimulatedFileSystem, path: &Path, bytes: &[u8]) {
        let mut file = fs
            .open_with_options(
                path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        file.set_len(0).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        write_all(file.as_mut(), bytes).unwrap();
        file.sync().unwrap();
    }

    /// The frames of a log file, as the offset and length of each.
    fn frame_spans(wal: &[u8]) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut offset = 0usize;
        while offset < wal.len() {
            let body_len =
                u32::from_le_bytes(wal[offset + 16..offset + 20].try_into().unwrap()) as usize;
            spans.push((offset, WAL_HEADER_LEN + body_len));
            offset += WAL_HEADER_LEN + body_len;
        }
        spans
    }

    /// A log of three commits, each synced before the next was written,
    /// with the frames of the last two taken off and handed back: the
    /// state of a disk that synced the first and was sent the others.
    fn one_synced_commit_and_two_frames(
        fs: &Arc<SimulatedFileSystem>,
        path: &Path,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            for id in ["a", "b", "c"] {
                store.put_document(&document(id)).unwrap();
            }
        }
        let wal = read_file(fs.as_ref(), path);
        let spans = frame_spans(&wal);
        assert_eq!(spans.len(), 3);
        let frame = |index: usize| wal[spans[index].0..spans[index].0 + spans[index].1].to_vec();
        (frame(0), frame(1), frame(2))
    }

    /// Give a frame another synced point, as if it had been written when
    /// that much of the file was on disk.
    fn with_synced_point(frame: &[u8], synced_len: u64) -> Vec<u8> {
        let lsn = u64::from_le_bytes(frame[8..16].try_into().unwrap());
        let payload = &frame[WAL_HEADER_LEN + WAL_SYNCED_POINT_LEN..];
        let mut rewritten = encode_wal_frame_head(lsn, synced_len, payload).to_vec();
        rewritten.extend_from_slice(payload);
        rewritten
    }

    #[test]
    fn a_later_write_kept_and_an_earlier_one_lost_is_the_end_of_the_log() {
        let fs = Arc::new(SimulatedFileSystem::new(66));
        let path = Path::new("reordered.owl");
        let (first, second, third) = one_synced_commit_and_two_frames(&fs, path);
        // The second and third commits were written together after the
        // first was synced, and never synced themselves. The crash lost
        // the second frame's header and kept everything after it.
        let synced = first.len() as u64;
        let mut wal = first.clone();
        let mut lost = with_synced_point(&second, synced);
        lost[..WAL_HEADER_LEN].fill(0);
        wal.extend_from_slice(&lost);
        wal.extend_from_slice(&with_synced_point(&third, synced));
        replace_file(fs.as_ref(), path, &wal);

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        assert_eq!(reopened.committed_lsn(), 1);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 1);
        // The unsynced writes are gone from the file, and the log goes on.
        assert_eq!(fs.file_size(path).unwrap(), synced);
        reopened.put_document(&document("d")).unwrap();
        drop(reopened);
        let again = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(again.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn damage_that_a_later_frame_saw_synced_is_corruption() {
        let fs = Arc::new(SimulatedFileSystem::new(67));
        let path = Path::new("rot.owl");
        let (first, second, third) = one_synced_commit_and_two_frames(&fs, path);
        // The same damage, but the third frame was written after the
        // second was on disk. An acknowledged commit has been damaged.
        let mut wal = first.clone();
        let mut damaged = second.clone();
        damaged[..WAL_HEADER_LEN].fill(0);
        wal.extend_from_slice(&damaged);
        wal.extend_from_slice(&third);
        let wal_len = wal.len() as u64;
        replace_file(fs.as_ref(), path, &wal);

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs.clone())
            .err()
            .expect("damage to synced frames must fail open");
        assert!(error.to_string().contains("magic"));
        assert_eq!(fs.file_size(path).unwrap(), wal_len);
    }

    #[test]
    fn a_frame_after_the_damage_that_names_no_synced_point_is_no_witness() {
        let fs = Arc::new(SimulatedFileSystem::new(68));
        let path = Path::new("unmarked.owl");
        let (first, second, third) = one_synced_commit_and_two_frames(&fs, path);
        // The frame after the damage is in the version before synced
        // points. It cannot say the damage was unsynced, so it is not.
        let payload = &third[WAL_HEADER_LEN + WAL_SYNCED_POINT_LEN..];
        let mut header = [0u8; WAL_HEADER_LEN];
        header[0..4].copy_from_slice(&WAL_MAGIC);
        header[4..6].copy_from_slice(&WAL_UNMARKED_VERSION.to_le_bytes());
        header[8..16].copy_from_slice(&third[8..16]);
        header[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        let check = wal_header_check(&header);
        header[6..8].copy_from_slice(&check.to_le_bytes());
        let mut hasher = Hasher::new();
        hasher.update(&header[0..20]);
        hasher.update(payload);
        let checksum = hasher.finalize();
        header[20..24].copy_from_slice(&checksum.to_le_bytes());

        let mut wal = first.clone();
        let mut damaged = with_synced_point(&second, first.len() as u64);
        damaged[..WAL_HEADER_LEN].fill(0);
        wal.extend_from_slice(&damaged);
        wal.extend_from_slice(&header);
        wal.extend_from_slice(payload);
        replace_file(fs.as_ref(), path, &wal);

        assert!(RustStore::new_with_fs(path, 1024 * 1024, fs).is_err());
    }

    /// The limit of the rule, written down so that it is not a surprise:
    /// damage to the last frames synced, with nothing written after that
    /// sync, has no witness and reads as the end of the log.
    #[test]
    fn damage_to_the_last_synced_frame_has_no_witness() {
        let fs = Arc::new(SimulatedFileSystem::new(69));
        let path = Path::new("last-frame.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            store.put_document(&document("b")).unwrap();
        }
        let wal = read_file(fs.as_ref(), path);
        let (last, _) = *frame_spans(&wal).last().unwrap();
        overwrite_at(fs.as_ref(), path, (last + WAL_HEADER_LEN + 12) as u64, &[0xff]);

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 1);
    }

    #[test]
    fn a_frame_from_a_newer_owl_is_never_read_as_the_end_of_the_log() {
        let fs = Arc::new(SimulatedFileSystem::new(70));
        let path = Path::new("newer.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
        }
        let wal = read_file(fs.as_ref(), path);
        let mut header: [u8; WAL_HEADER_LEN] = wal[..WAL_HEADER_LEN].try_into().unwrap();
        header[4..6].copy_from_slice(&(WAL_VERSION + 1).to_le_bytes());
        let check = wal_header_check(&header);
        header[6..8].copy_from_slice(&check.to_le_bytes());
        overwrite_at(fs.as_ref(), path, 0, &header);

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("a log from a newer Owl must not be cut short");
        assert!(error.to_string().contains("unsupported WAL format version"));
    }

    #[test]
    fn damaged_frame_length_is_corruption_not_a_torn_tail() {
        let fs = Arc::new(SimulatedFileSystem::new(63));
        let path = Path::new("header-length.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            store.put_document(&document("b")).unwrap();
        }
        let wal_len = fs.file_size(path).unwrap();
        let wal = read_file(fs.as_ref(), path);
        // Enlarge the first frame's length so its payload appears to run past
        // the end of the file, exactly like an interrupted append would.
        let mut length = u32::from_le_bytes(wal[16..20].try_into().unwrap());
        length |= 1 << 20;
        overwrite_at(fs.as_ref(), path, 16, &length.to_le_bytes());

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs.clone())
            .err()
            .expect("a damaged length in a complete frame must fail open");
        assert!(error.to_string().contains("header checksum"));
        // Both committed frames are still on disk for repair.
        assert_eq!(fs.file_size(path).unwrap(), wal_len);
    }

    #[test]
    fn damaged_frame_lsn_is_detected_by_the_header_check() {
        let fs = Arc::new(SimulatedFileSystem::new(64));
        let path = Path::new("header-lsn.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            store.put_document(&document("b")).unwrap();
        }
        let wal = read_file(fs.as_ref(), path);
        overwrite_at(fs.as_ref(), path, 8, &[wal[8] ^ 0x04]);
        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("a damaged header LSN must fail open");
        assert!(error.to_string().contains("header checksum"));
    }

    #[test]
    fn legacy_payload_only_checksum_frames_still_recover() {
        let fs = Arc::new(SimulatedFileSystem::new(65));
        let path = Path::new("legacy-frames.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            store.put_document(&document("b")).unwrap();
        }
        // Rewrite every frame in the version 2 layout: no synced point,
        // reserved bytes zero and a CRC-32 over the payload alone.
        let wal = read_file(fs.as_ref(), path);
        let mut legacy = Vec::new();
        let mut offset = 0usize;
        let mut frames = 0;
        while offset < wal.len() {
            let body_len =
                u32::from_le_bytes(wal[offset + 16..offset + 20].try_into().unwrap()) as usize;
            let body = &wal[offset + WAL_HEADER_LEN..offset + WAL_HEADER_LEN + body_len];
            let payload = &body[WAL_SYNCED_POINT_LEN..];
            let mut header = [0u8; WAL_HEADER_LEN];
            header[0..4].copy_from_slice(&WAL_MAGIC);
            header[4..6].copy_from_slice(&WAL_LEGACY_VERSION.to_le_bytes());
            header[8..16].copy_from_slice(&wal[offset + 8..offset + 16]);
            header[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            header[20..24].copy_from_slice(&crc32fast::hash(payload).to_le_bytes());
            legacy.extend_from_slice(&header);
            legacy.extend_from_slice(payload);
            offset += WAL_HEADER_LEN + body_len;
            frames += 1;
        }
        replace_file(fs.as_ref(), path, &legacy);
        assert_eq!(frames, 2);

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
        // New commits append current-format frames after the legacy ones.
        reopened.put_document(&document("c")).unwrap();
        drop(reopened);
        let again = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(again.scan_documents("events").unwrap().len(), 3);
    }

    #[test]
    fn immutable_flush_generation_root_accumulates_ordered_descriptors() {
        let fs = Arc::new(SimulatedFileSystem::new(35));
        let path = Path::new("cumulative-record-root.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            store.put_document(&document("base")).unwrap();
            store.wait_for_maintenance().unwrap();
            store.put_document(&document("current")).unwrap();
            store.wait_for_maintenance().unwrap();

            let mut current = document("current");
            current.version = 2;
            current
                .fields
                .insert("message".to_string(), Value::String("updated".to_string()));
            store.put_document(&current).unwrap();
            store.wait_for_maintenance().unwrap();
            store
                .delete_document("events", &"base".to_string())
                .unwrap();
            store.wait_for_maintenance().unwrap();

            let manifest = Manifest::decode(&read_file(
                fs.as_ref(),
                Path::new("cumulative-record-root.owl.manifest.0"),
            ))
            .unwrap();
            assert_eq!(manifest.sequence, 4);
            assert!(!manifest.has_record_root);
            let root = GenerationRoot::decode(&read_file(
                fs.as_ref(),
                Path::new("cumulative-record-root.owl.generations.0"),
            ))
            .unwrap();
            assert_eq!(root.base_lsn, 1);
            assert_eq!(root.checkpoint_lsn, 4);
            assert_eq!(root.generations.len(), 3);
            assert_eq!(root.generations[0].sequence, 2);
            assert_eq!(root.generations[2].sequence, 4);
            for sequence in 2..=4 {
                assert!(fs.exists(Path::new(&format!(
                    "cumulative-record-root.owl.generation.{sequence}"
                ))));
            }
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 4);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 1);
        assert_eq!(
            reopened
                .get_document("events", &"current".to_string())
                .unwrap()
                .unwrap()
                .fields["message"],
            Value::String("updated".to_string())
        );
    }

    #[test]
    fn projection_reads_clean_base_and_incremental_records_from_lazy_handles() {
        let fs = Arc::new(SimulatedFileSystem::new(40));
        let path = Path::new("lazy-record-root.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        store
            .configure_auto_checkpoint(AutoCheckpointPolicy {
                wal_bytes: 0,
                commits: 1,
            })
            .unwrap();
        let mut current = document("current");
        store.put_document(&current).unwrap();
        store.wait_for_maintenance().unwrap();
        let object_ref = store
            .get_objref("events", &"current".to_string())
            .unwrap()
            .unwrap();
        assert!(store
            .state
            .read()
            .unwrap()
            .object_records
            .get(&object_ref)
            .is_none());
        let projected = store
            .get_document_projection("events", &"current".to_string(), &[])
            .unwrap()
            .unwrap();
        assert_eq!(projected.id, "current");
        assert_eq!(projected.version, 1);

        current.version = 2;
        current
            .fields
            .insert("message".into(), Value::String("checkpoint-page".into()));
        store.put_document(&current).unwrap();
        store.wait_for_maintenance().unwrap();
        let handles = store.checkpoint_read_handles.lock().unwrap();
        assert!(handles.record_root.is_none());
        assert_eq!(handles.generation_record_roots.len(), 1);
        assert!(handles.base_segment.is_some());
        drop(handles);
        assert!(store
            .state
            .read()
            .unwrap()
            .object_records
            .get(&object_ref)
            .is_none());
        let projected = store
            .get_document_projection("events", &"current".to_string(), &["message".into()])
            .unwrap()
            .unwrap();
        assert_eq!(
            projected.fields["message"],
            Value::String("checkpoint-page".into())
        );

        store
            .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
            .unwrap();
        current.version = 3;
        current
            .fields
            .insert("message".into(), Value::String("live-dirty".into()));
        store.put_document(&current).unwrap();
        let projected = store
            .get_document_projection("events", &"current".to_string(), &["message".into()])
            .unwrap()
            .unwrap();
        assert_eq!(
            projected.fields["message"],
            Value::String("live-dirty".into())
        );
        let missing = ObjRef::from_parts(
            object_ref.collection_id(),
            object_ref.object_id().saturating_add(10_000),
            1,
        );
        let batch = store
            .navigate_references_projection(&[object_ref, missing], &["message".to_string()])
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(
            batch[0].as_ref().unwrap().fields["message"],
            Value::String("live-dirty".into())
        );
        assert!(batch[1].is_none());
    }

    #[test]
    fn manifest_referenced_generation_corruption_fails_closed() {
        let fs = Arc::new(SimulatedFileSystem::new(36));
        let path = Path::new("corrupt-record-root.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            store.put_document(&document("base")).unwrap();
            store.wait_for_maintenance().unwrap();
            store.put_document(&document("delta")).unwrap();
            store.wait_for_maintenance().unwrap();
        }
        let root_path = Path::new("corrupt-record-root.owl.generation.2");
        let mut root = fs
            .open_with_options(
                root_path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        root.seek(SeekFrom::Start(WAL_HEADER_LEN as u64)).unwrap();
        write_all(root.as_mut(), &[0xff]).unwrap();
        root.sync().unwrap();

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("manifest-referenced generation corruption must fail open");
        assert!(error.to_string().contains("flush generation"));
    }

    #[test]
    fn lazy_generation_record_root_wins_over_the_stale_base_after_reopen() {
        let fs = Arc::new(SimulatedFileSystem::new(41));
        let path = Path::new("lazy-generation-record.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            let mut current = document("current");
            store.put_document(&current).unwrap();
            store.checkpoint().unwrap();
            current.version = 2;
            current
                .fields
                .insert("message".into(), Value::String("from-generation".into()));
            store.put_document(&current).unwrap();
            store.checkpoint().unwrap();
            assert_eq!(store.state.read().unwrap().object_records.len(), 0);
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.state.read().unwrap().object_records.len(), 0);
        let layout = reopened.storage_layout_stats("events").unwrap();
        assert_eq!(layout.flush_generations, 1);
        assert!(layout.flush_generation_wal_bytes > 0);
        assert!(layout.flush_generation_record_bytes > 0);
        assert!(layout.flush_generation_index_bytes > 0);
        assert_eq!(
            reopened
                .get_document_projection("events", &"current".into(), &["message".into()])
                .unwrap()
                .unwrap()
                .fields["message"],
            Value::String("from-generation".into())
        );
    }

    #[test]
    fn manifest_referenced_generation_record_corruption_fails_closed() {
        let fs = Arc::new(SimulatedFileSystem::new(42));
        let path = Path::new("corrupt-generation-record.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("base")).unwrap();
            store.checkpoint().unwrap();
            store.put_document(&document("delta")).unwrap();
            store.checkpoint().unwrap();
        }
        let record_path = Path::new("corrupt-generation-record.owl.generation-record.2");
        let mut records = fs
            .open_with_options(
                record_path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        records.seek(SeekFrom::Start(8)).unwrap();
        write_all(records.as_mut(), &[0xff]).unwrap();
        records.sync().unwrap();

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("manifest-referenced generation record corruption must fail open");
        assert!(error.to_string().contains("record root"));
    }

    #[test]
    fn manifest_referenced_generation_index_corruption_fails_closed() {
        let fs = Arc::new(SimulatedFileSystem::new(44));
        let path = Path::new("corrupt-generation-index.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store.put_document(&document("base")).unwrap();
        store.checkpoint().unwrap();
        store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "by_message".into(),
                schema: "events".into(),
                fields: vec!["message".into()],
                unique: false,
            })
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);

        let index_path = Path::new("corrupt-generation-index.owl.generation-index.2");
        let mut indexes = fs
            .open_with_options(
                index_path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        indexes.seek(SeekFrom::Start(8)).unwrap();
        write_all(indexes.as_mut(), &[0xff]).unwrap();
        indexes.sync().unwrap();

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("manifest-referenced generation index corruption must fail open");
        assert!(error.to_string().contains("index root"));
    }

    #[test]
    fn integrity_audit_reads_generation_record_payload_pages() {
        let fs = Arc::new(SimulatedFileSystem::new(43));
        let path = Path::new("corrupt-generation-payload.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("base")).unwrap();
            store.checkpoint().unwrap();
            store.put_document(&document("delta")).unwrap();
            store.checkpoint().unwrap();
        }
        let record_path = Path::new("corrupt-generation-payload.owl.generation-record.2");
        let length = fs.file_size(record_path).unwrap();
        let mut records = fs
            .open_with_options(
                record_path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        records.seek(SeekFrom::Start(length - 1)).unwrap();
        write_all(records.as_mut(), &[0xff]).unwrap();
        records.sync().unwrap();

        let error = RustStore::check_integrity_with_fs(path, fs)
            .expect_err("integrity audit must read generation payload pages");
        assert!(error.to_string().contains("record root"));
    }

    #[test]
    fn immutable_generations_replay_index_and_catalog_changes() {
        let fs = Arc::new(SimulatedFileSystem::new(37));
        let path = Path::new("cumulative-index-root.owl");
        let definition = ScalarIndexDefinition {
            name: "by_code".into(),
            schema: "events".into(),
            fields: vec!["code".into()],
            unique: false,
        };
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            store.upsert_scalar_index(definition.clone()).unwrap();
            store.wait_for_maintenance().unwrap();

            let mut first = document("first");
            first.fields.insert("code".into(), Value::Integer(500));
            store.put_document(&first).unwrap();
            store.wait_for_maintenance().unwrap();
            let mut second = document("second");
            second.fields.insert("code".into(), Value::Integer(500));
            store.put_document(&second).unwrap();
            store.wait_for_maintenance().unwrap();

            first.version = 2;
            first.fields.insert("code".into(), Value::Integer(501));
            store.put_document(&first).unwrap();
            store.wait_for_maintenance().unwrap();
            assert!(store.delete_scalar_index("events", "by_code").unwrap());
            store.wait_for_maintenance().unwrap();
            store.upsert_scalar_index(definition.clone()).unwrap();
            store.wait_for_maintenance().unwrap();
            assert!(store
                .delete_document("events", &"second".to_string())
                .unwrap());
            store.wait_for_maintenance().unwrap();

            let manifest = Manifest::decode(&read_file(
                fs.as_ref(),
                Path::new("cumulative-index-root.owl.manifest.1"),
            ))
            .unwrap();
            assert_eq!(manifest.sequence, 7);
            assert!(!manifest.has_index_root);
            let root = GenerationRoot::decode(&read_file(
                fs.as_ref(),
                Path::new("cumulative-index-root.owl.generations.1"),
            ))
            .unwrap();
            assert_eq!(root.base_lsn, 1);
            assert_eq!(root.checkpoint_lsn, 7);
            assert_eq!(root.generations.len(), 6);
            for sequence in 2..=7 {
                assert!(fs.exists(Path::new(&format!(
                    "cumulative-index-root.owl.generation-index.{sequence}"
                ))));
            }
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(
            reopened.list_scalar_indexes(Some("events")).unwrap(),
            vec![definition]
        );
        assert!(reopened
            .lookup_scalar_index("events", "by_code", &[Value::Integer(500)])
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened
                .lookup_scalar_index("events", "by_code", &[Value::Integer(501)])
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn manifest_referenced_generation_root_corruption_fails_closed() {
        let fs = Arc::new(SimulatedFileSystem::new(38));
        let path = Path::new("corrupt-index-root.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            store
                .upsert_scalar_index(ScalarIndexDefinition {
                    name: "by_message".into(),
                    schema: "events".into(),
                    fields: vec!["message".into()],
                    unique: false,
                })
                .unwrap();
            store.wait_for_maintenance().unwrap();
            store.put_document(&document("delta")).unwrap();
            store.wait_for_maintenance().unwrap();
        }
        let root_path = Path::new("corrupt-index-root.owl.generations.0");
        let mut root = fs
            .open_with_options(
                root_path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        root.seek(SeekFrom::Start(20)).unwrap();
        write_all(root.as_mut(), &[0xff]).unwrap();
        root.sync().unwrap();

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("manifest-referenced generation-root corruption must fail open");
        assert!(error.to_string().contains("generation root"));
    }

    #[test]
    fn checksummed_but_noncontiguous_generation_root_fails_closed() {
        let fs = Arc::new(SimulatedFileSystem::new(43));
        let path = Path::new("incomplete-index-root.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            store
                .upsert_scalar_index(ScalarIndexDefinition {
                    name: "by_message".into(),
                    schema: "events".into(),
                    fields: vec!["message".into()],
                    unique: false,
                })
                .unwrap();
            store.wait_for_maintenance().unwrap();
            store.put_document(&document("delta")).unwrap();
            store.wait_for_maintenance().unwrap();
        }

        let root_path = Path::new("incomplete-index-root.owl.generations.0");
        let mut root = read_file(fs.as_ref(), root_path);
        let start = GENERATION_ROOT_HEADER_LEN + 8;
        let invalid = u64::from_le_bytes(root[start..start + 8].try_into().unwrap()) + 1;
        root[start..start + 8].copy_from_slice(&invalid.to_le_bytes());
        root[44..48].fill(0);
        let checksum = crc32fast::hash(&root);
        root[44..48].copy_from_slice(&checksum.to_le_bytes());
        write_synced_file(fs.as_ref(), root_path, &root).unwrap();

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("a checksummed but noncontiguous generation root must fail open");
        assert!(error.to_string().contains("contiguous"));
    }

    #[test]
    fn automatic_checkpoint_compacts_when_cumulative_delta_crosses_threshold() {
        let fs = Arc::new(SimulatedFileSystem::new(34));
        let path = Path::new("automatic-delta-compaction.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store
            .configure_auto_checkpoint(AutoCheckpointPolicy {
                wal_bytes: 0,
                commits: 1,
            })
            .unwrap();
        store.put_document(&document("base")).unwrap();
        store.wait_for_maintenance().unwrap();

        let large = Document::with_id(
            "large".to_string(),
            "events".to_string(),
            crate::document::Fields::from_iter([("message".to_string(), Value::String("x".repeat(70 * 1024)))]),
        );
        store.put_document(&large).unwrap();
        store.wait_for_maintenance().unwrap();

        let manifest = Manifest::decode(&read_file(
            fs.as_ref(),
            Path::new("automatic-delta-compaction.owl.manifest.0"),
        ))
        .unwrap();
        assert_eq!(manifest.sequence, 2);
        assert_eq!(manifest.segment_slot, 0);
        assert_eq!(manifest.delta_len, 0);
        assert!(!fs.exists(Path::new("automatic-delta-compaction.owl.wal-delta.0")));
        assert!(!fs.exists(Path::new("automatic-delta-compaction.owl.wal-delta.1")));
        assert_eq!(store.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn commits_continue_during_off_lock_snapshot_reconstruction() {
        let fs = Arc::new(CheckpointFailingFileSystem::new(29));
        let path = Path::new("background-checkpoint.owl");
        let store = Arc::new(RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap());
        store.put_document(&document("base")).unwrap();
        store.checkpoint().unwrap();
        store
            .configure_auto_checkpoint(AutoCheckpointPolicy {
                wal_bytes: 0,
                commits: 1,
            })
            .unwrap();
        fs.block_checkpoints();

        let (reply, response) = mpsc::channel();
        let writer = store.clone();
        let write = std::thread::spawn(move || {
            let result = writer.put_document(&document("a"));
            let _ = reply.send(result);
        });
        let result = response.recv_timeout(web_time::Duration::from_secs(2));
        if result.is_err() {
            fs.release_checkpoint();
        }
        result
            .expect("durable write waited for background checkpoint I/O")
            .unwrap();
        fs.wait_until_checkpoint_entered();
        assert_eq!(store.committed_lsn(), 2);
        assert!(store.maintenance_status().unwrap().wal_bytes > 0);

        let (second_reply, second_response) = mpsc::channel();
        let second_writer = store.clone();
        let second_write = std::thread::spawn(move || {
            let result = second_writer.put_document(&document("b"));
            let _ = second_reply.send(result);
        });
        let second_result = second_response.recv_timeout(web_time::Duration::from_secs(2));
        if second_result.is_err() {
            fs.release_checkpoint();
        }
        second_result
            .expect("new commit waited for sealed-WAL checkpoint I/O")
            .unwrap();
        assert_eq!(store.committed_lsn(), 3);

        fs.release_checkpoint();
        store.wait_for_maintenance().unwrap();
        write.join().unwrap();
        second_write.join().unwrap();
        assert_eq!(store.maintenance_status().unwrap().wal_bytes, 0);
        assert_eq!(store.scan_documents("events").unwrap().len(), 3);
    }

    #[test]
    fn automatic_checkpoint_failure_does_not_misreport_a_durable_write() {
        let fs = Arc::new(CheckpointFailingFileSystem::new(27));
        let path = Path::new("automatic-checkpoint-failure.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store
            .configure_auto_checkpoint(AutoCheckpointPolicy {
                wal_bytes: 0,
                commits: 1,
            })
            .unwrap();

        fs.set_checkpoint_failure(true);
        store.put_document(&document("a")).unwrap();
        store.wait_for_maintenance().unwrap();
        assert!(store
            .get_document("events", &"a".to_string())
            .unwrap()
            .is_some());
        let failed = store.maintenance_status().unwrap();
        assert_eq!(failed.checkpoint_sequence, 0);
        assert!(failed.wal_bytes > 0);
        assert!(failed.last_error.unwrap().contains("checkpoint"));

        fs.set_checkpoint_failure(false);
        store.put_document(&document("b")).unwrap();
        store.wait_for_maintenance().unwrap();
        let recovered = store.maintenance_status().unwrap();
        assert_eq!(recovered.checkpoint_sequence, 2);
        assert_eq!(recovered.wal_bytes, 0);
        assert!(recovered.last_error.is_none());

        drop(store);
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn recovery_replays_sealed_and_active_wal_generations() {
        let fs = Arc::new(CheckpointFailingFileSystem::new(30));
        let path = Path::new("sealed-wal-recovery.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy {
                    wal_bytes: 0,
                    commits: 1,
                })
                .unwrap();
            fs.set_checkpoint_failure(true);
            store.put_document(&document("sealed")).unwrap();
            store.wait_for_maintenance().unwrap();
            assert!(fs.exists(Path::new("sealed-wal-recovery.owl.wal.sealed")));

            store
                .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
                .unwrap();
            store.put_document(&document("active")).unwrap();
            store.wait_for_maintenance().unwrap();
            assert!(fs.file_size(path).unwrap() > 0);
        }

        fs.set_checkpoint_failure(false);
        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
        assert!(!fs.exists(Path::new("sealed-wal-recovery.owl.wal.sealed")));
        assert_eq!(fs.file_size(path).unwrap(), 0);
    }

    #[test]
    fn immutable_generations_are_reclaimed_after_full_compaction() {
        let fs = Arc::new(SimulatedFileSystem::new(26));
        let path = Path::new("automatic-reclamation.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store
            .configure_auto_checkpoint(AutoCheckpointPolicy {
                wal_bytes: 0,
                commits: 1,
            })
            .unwrap();

        for id in ["a", "b", "c", "d", "e"] {
            store.put_document(&document(id)).unwrap();
            store.wait_for_maintenance().unwrap();
            assert_eq!(fs.file_size(path).unwrap(), 0);
        }
        let slot_one_before = fs
            .file_size(Path::new("automatic-reclamation.owl.segment.1"))
            .unwrap();
        assert!(store.delete_document("events", &"a".to_string()).unwrap());
        store.wait_for_maintenance().unwrap();
        assert!(store.delete_document("events", &"b".to_string()).unwrap());
        store.wait_for_maintenance().unwrap();
        let slot_one_after = fs
            .file_size(Path::new("automatic-reclamation.owl.segment.1"))
            .unwrap();
        assert_eq!(slot_one_after, slot_one_before);

        let status = store.maintenance_status().unwrap();
        assert_eq!(status.checkpoint_sequence, 7);
        assert_eq!(status.last_checkpoint.unwrap().slot, 1);
        assert!(fs.exists(Path::new("automatic-reclamation.owl.generations.1")));
        for sequence in 2..=7 {
            assert!(fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.generation.{sequence}"
            ))));
            assert!(fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.generation-record.{sequence}"
            ))));
            assert!(fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.generation-index.{sequence}"
            ))));
        }
        assert!(!fs.exists(Path::new("automatic-reclamation.owl.segment.0")));

        let compacted = store.compact().unwrap();
        assert_eq!(compacted.sequence, 8);
        assert_eq!(compacted.live_objects, 3);
        assert!(fs.exists(Path::new("automatic-reclamation.owl.segment.0")));
        assert!(!fs.exists(Path::new("automatic-reclamation.owl.generations.0")));
        assert!(!fs.exists(Path::new("automatic-reclamation.owl.generations.1")));
        for sequence in 2..=7 {
            assert!(!fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.generation.{sequence}"
            ))));
            assert!(!fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.generation-record.{sequence}"
            ))));
            assert!(!fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.generation-index.{sequence}"
            ))));
        }
        for slot in 0..=1 {
            assert!(fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.manifest.{slot}"
            ))));
            assert!(fs.exists(Path::new(&format!(
                "automatic-reclamation.owl.segment.{slot}"
            ))));
        }
        assert_eq!(
            fs.file_size(Path::new("automatic-reclamation.owl.manifest.0"))
                .unwrap(),
            64
        );
        assert_eq!(
            fs.file_size(Path::new("automatic-reclamation.owl.manifest.1"))
                .unwrap(),
            64
        );
    }

    #[test]
    fn checkpoint_preserves_updates_and_excludes_deleted_records() {
        let fs = Arc::new(SimulatedFileSystem::new(18));
        let path = Path::new("checkpoint-mutations.owl");
        let stable_ref;
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            let mut current = document("current");
            store.put_document(&current).unwrap();
            stable_ref = store.get_objref("events", &current.id).unwrap().unwrap();
            store.put_document(&document("deleted")).unwrap();

            current.version = 2;
            current
                .fields
                .insert("message".to_string(), Value::String("updated".to_string()));
            store.put_document(&current).unwrap();
            assert!(store
                .delete_document("events", &"deleted".to_string())
                .unwrap());
            assert_eq!(store.checkpoint().unwrap().live_objects, 1);
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        let current = reopened
            .get_document("events", &"current".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(current.version, 2);
        assert_eq!(
            current.fields["message"],
            Value::String("updated".to_string())
        );
        assert_eq!(
            reopened
                .get_objref("events", &"current".to_string())
                .unwrap(),
            Some(stable_ref)
        );
        assert!(reopened
            .get_document("events", &"deleted".to_string())
            .unwrap()
            .is_none());
    }

    #[test]
    fn lazy_base_delete_publishes_a_tombstone_without_materialized_bytes() {
        let fs = Arc::new(SimulatedFileSystem::new(41));
        let path = Path::new("lazy-base-delete.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("deleted")).unwrap();
            store.put_document(&document("retained")).unwrap();
            store.checkpoint().unwrap();
            {
                let state = store.state.read().unwrap();
                assert_eq!(state.object_directory.len(), 2);
                assert_eq!(state.object_records.len(), 0);
            }
            assert!(store
                .delete_document("events", &"deleted".to_string())
                .unwrap());
            store.checkpoint().unwrap();
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert!(reopened
            .get_document("events", &"deleted".to_string())
            .unwrap()
            .is_none());
        assert!(reopened
            .get_document("events", &"retained".to_string())
            .unwrap()
            .is_some());
        assert_eq!(reopened.state.read().unwrap().object_records.len(), 0);
    }

    #[test]
    fn full_checkpoint_resets_record_root_to_its_compacted_base() {
        let fs = Arc::new(SimulatedFileSystem::new(31));
        let path = Path::new("record-delta.owl");
        let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
        store.put_document(&document("updated")).unwrap();
        store.put_document(&document("deleted")).unwrap();
        store.checkpoint().unwrap();

        let mut updated = document("updated");
        updated.version = 2;
        store.put_document(&updated).unwrap();
        store
            .delete_document("events", &"deleted".to_string())
            .unwrap();
        let checkpoint = store.compact().unwrap();
        assert_eq!(checkpoint.sequence, 2);

        let delta = RecordPageDelta::decode(&read_file(
            fs.as_ref(),
            Path::new("record-delta.owl.record-delta.0"),
        ))
        .unwrap();
        assert_eq!(delta.base_lsn, 4);
        assert_eq!(delta.target_lsn, 4);
        assert!(delta.puts.is_empty());
        assert!(delta.deletes.is_empty());
        let index_root = IndexRootDelta::decode(&read_file(
            fs.as_ref(),
            Path::new("record-delta.owl.index-delta.0"),
        ))
        .unwrap();
        assert_eq!(index_root.base_lsn, 4);
        assert_eq!(index_root.target_lsn, 4);
        assert!(index_root.indexes.is_empty());
    }

    #[test]
    fn recovery_applies_manifest_referenced_wal_delta_over_base_segment() {
        let fs = Arc::new(SimulatedFileSystem::new(32));
        let path = Path::new("manifest-delta-recovery.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store
                .configure_auto_checkpoint(AutoCheckpointPolicy::disabled())
                .unwrap();
            store.put_document(&document("base")).unwrap();
            let base = store.checkpoint().unwrap();
            assert_eq!(base.sequence, 1);
            store.put_document(&document("delta")).unwrap();

            let delta = read_file(fs.as_ref(), path);
            let delta_path = Path::new("manifest-delta-recovery.owl.wal-delta.0");
            let mut delta_file = fs.create(delta_path).unwrap();
            write_all(delta_file.as_mut(), &delta).unwrap();
            delta_file.sync().unwrap();

            let base_manifest = Manifest::decode(&read_file(
                fs.as_ref(),
                Path::new("manifest-delta-recovery.owl.manifest.1"),
            ))
            .unwrap();
            let incremental = Manifest {
                sequence: 2,
                checkpoint_lsn: 2,
                segment_len: base_manifest.segment_len,
                segment_checksum: base_manifest.segment_checksum,
                live_objects: 2,
                slot: 0,
                segment_slot: 1,
                delta_len: delta.len() as u64,
                delta_checksum: crc32fast::hash(&delta),
                has_record_root: false,
                has_index_root: false,
            };
            let mut manifest_file = fs
                .create(Path::new("manifest-delta-recovery.owl.manifest.0"))
                .unwrap();
            write_all(manifest_file.as_mut(), &incremental.encode()).unwrap();
            manifest_file.sync().unwrap();
        }

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn torn_inactive_manifest_keeps_previous_checkpoint_and_wal_replay() {
        let fs = Arc::new(SimulatedFileSystem::new(16));
        let path = Path::new("torn-checkpoint.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            store.checkpoint().unwrap();
            store.put_document(&document("b")).unwrap();
        }
        let mut torn = fs
            .create(Path::new("torn-checkpoint.owl.manifest.0"))
            .unwrap();
        write_all(torn.as_mut(), b"OWLM\x01").unwrap();
        torn.sync().unwrap();

        let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(reopened.committed_lsn(), 2);
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 2);
    }

    #[test]
    fn published_checkpoint_corruption_fails_closed() {
        let fs = Arc::new(SimulatedFileSystem::new(17));
        let path = Path::new("bad-checkpoint.owl");
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.put_document(&document("a")).unwrap();
            store.checkpoint().unwrap();
        }
        let segment_path = Path::new("bad-checkpoint.owl.segment.1");
        let mut segment = fs
            .open_with_options(
                segment_path,
                &FileOptions {
                    read: true,
                    write: true,
                    create: false,
                    truncate: false,
                    append: false,
                },
            )
            .unwrap();
        segment.seek(SeekFrom::Start(0)).unwrap();
        write_all(segment.as_mut(), &[0]).unwrap();
        segment.sync().unwrap();

        let error = RustStore::new_with_fs(path, 1024 * 1024, fs)
            .err()
            .expect("published checkpoint corruption must fail open");
        assert!(error.to_string().contains("manifest"));
    }

    #[test]
    fn scalar_postings_follow_mutations_and_checkpoint_recovery() {
        let fs = Arc::new(SimulatedFileSystem::new(19));
        let path = Path::new("scalar-index.owl");
        let definition = ScalarIndexDefinition {
            name: "by_code".into(),
            schema: "events".into(),
            fields: vec!["code".into()],
            unique: false,
        };
        {
            let store = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            store.upsert_scalar_index(definition.clone()).unwrap();
            let mut first = document("first");
            first.fields.insert("code".into(), Value::Integer(500));
            let mut second = document("second");
            second.fields.insert("code".into(), Value::Integer(500));
            store
                .write_batch(vec![
                    WriteOperation::PutDocument(first.clone()),
                    WriteOperation::PutDocument(second),
                ])
                .unwrap();
            assert_eq!(
                store
                    .lookup_scalar_index("events", "by_code", &[Value::Integer(500)])
                    .unwrap()
                    .len(),
                2
            );

            first.fields.insert("code".into(), Value::Integer(501));
            first.version = 2;
            store.put_document(&first).unwrap();
            assert_eq!(
                store
                    .lookup_scalar_index("events", "by_code", &[Value::Integer(500)])
                    .unwrap()
                    .len(),
                1
            );
            store
                .delete_document("events", &"second".to_string())
                .unwrap();
            assert!(store
                .lookup_scalar_index("events", "by_code", &[Value::Integer(500)])
                .unwrap()
                .is_empty());
            store.checkpoint().unwrap();
        }

        {
            let reopened = RustStore::new_with_fs(path, 1024 * 1024, fs.clone()).unwrap();
            assert_eq!(
                reopened.list_scalar_indexes(Some("events")).unwrap(),
                vec![definition]
            );
            let found = reopened
                .lookup_scalar_index("events", "by_code", &[Value::Integer(501)])
                .unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(
                reopened.navigate_reference(&found[0]).unwrap().unwrap().id,
                "first"
            );
            let mut after_checkpoint = document("after-checkpoint");
            after_checkpoint
                .fields
                .insert("code".into(), Value::Integer(501));
            reopened.put_document(&after_checkpoint).unwrap();
        }
        let recovered = RustStore::new_with_fs(path, 1024 * 1024, fs).unwrap();
        assert_eq!(
            recovered
                .lookup_scalar_index("events", "by_code", &[Value::Integer(501)])
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn unique_index_rejects_duplicate_batch_before_wal_commit() {
        let fs = Arc::new(SimulatedFileSystem::new(20));
        let store = RustStore::new_with_fs("unique-index.owl", 1024 * 1024, fs).unwrap();
        store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "unique_email".into(),
                schema: "users".into(),
                fields: vec!["email".into()],
                unique: true,
            })
            .unwrap();
        let user = |id: &str| {
            Document::with_id(
                id.to_string(),
                "users".into(),
                crate::document::Fields::from_iter([("email".into(), Value::String("same@example.com".into()))]),
            )
        };
        let error = store
            .write_batch(vec![
                WriteOperation::PutDocument(user("one")),
                WriteOperation::PutDocument(user("two")),
            ])
            .unwrap_err();
        // The refusal is the writer's to act on, so it speaks of their
        // collection, field and value and is not reported as a storage fault.
        assert!(matches!(error, OQLError::SchemaValidation { .. }), "{error}");
        let message = error.to_string();
        for part in ["'users'", "email \"same@example.com\"", "must be unique"] {
            assert!(message.contains(part), "{message}");
        }
        assert_eq!(store.committed_lsn(), 1);
        assert!(store.scan_documents("users").unwrap().is_empty());
    }

    #[test]
    fn typed_scalar_range_matches_scan_domain() {
        let fs = Arc::new(SimulatedFileSystem::new(21));
        let store = RustStore::new_with_fs("range-index.owl", 1024 * 1024, fs).unwrap();
        store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "by_score".into(),
                schema: "scores".into(),
                fields: vec!["score".into()],
                unique: false,
            })
            .unwrap();
        for (id, score) in [("low", -5), ("zero-a", 0), ("zero-b", 0), ("high", 10)] {
            store
                .put_document(&Document::with_id(
                    id.into(),
                    "scores".into(),
                    crate::document::Fields::from_iter([("score".into(), Value::Integer(score))]),
                ))
                .unwrap();
        }
        let found = store
            .range_scalar_index(
                "scores",
                "by_score",
                &[Value::Integer(-1)],
                &[Value::Integer(10)],
            )
            .unwrap();
        assert_eq!(found.len(), 3);
        let ids = found
            .iter()
            .map(|reference| store.navigate_reference(reference).unwrap().unwrap().id)
            .collect::<HashSet<_>>();
        assert_eq!(
            ids,
            HashSet::from(["zero-a".into(), "zero-b".into(), "high".into()])
        );
    }

    #[test]
    fn typed_index_ranges_equal_a_scan_oracle() {
        let fs = Arc::new(SimulatedFileSystem::new(22));
        let store = RustStore::new_with_fs("index-oracle.owl", 4 * 1024 * 1024, fs).unwrap();
        store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "by_value".into(),
                schema: "numbers".into(),
                fields: vec!["value".into()],
                unique: false,
            })
            .unwrap();
        for number in 0..100i64 {
            let value = ((number * 37) % 101) - 50;
            store
                .put_document(&Document::with_id(
                    number.to_string(),
                    "numbers".into(),
                    crate::document::Fields::from_iter([("value".into(), Value::Integer(value))]),
                ))
                .unwrap();
        }

        for (lower, upper) in [(-50, -25), (-7, 7), (0, 0), (12, 49), (-50, 50)] {
            let actual = store
                .range_scalar_index(
                    "numbers",
                    "by_value",
                    &[Value::Integer(lower)],
                    &[Value::Integer(upper)],
                )
                .unwrap()
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            let expected = store
                .scan_documents("numbers")
                .unwrap()
                .into_iter()
                .filter(|document| {
                    matches!(
                        document.fields.get("value"),
                        Some(Value::Integer(value)) if *value >= lower && *value <= upper
                    )
                })
                .map(|document| store.get_objref("numbers", &document.id).unwrap().unwrap())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(actual, expected, "range {lower}..={upper}");
        }
    }

    fn read_file(filesystem: &dyn FileSystem, path: &Path) -> Vec<u8> {
        let mut file = filesystem.open(path).unwrap();
        let mut bytes = vec![0; file.size().unwrap() as usize];
        assert_eq!(read_up_to(file.as_mut(), &mut bytes).unwrap(), bytes.len());
        bytes
    }

    fn count_bytes(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|window| *window == needle)
            .count()
    }
}
