use crate::document::ObjRef;
use crate::error::{OQLError, OQLResult};
use crate::schema::Schema;
use crate::simulation::filesystem::{FileHandle, FileSystem};
use crate::storage::{ScalarIndexDefinition, VectorIndexDefinition};
use crate::storage_engine::btree::OrderedIndexBase;
use crate::storage_engine::{
    CompiledSchema, TypedSegment, TypedSegmentDescriptor, TypedSegmentIdentity,
};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::SeekFrom;
use std::ops::{Bound, Range};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const SEGMENT_MAGIC: [u8; 4] = *b"OWLS";
const SEGMENT_VERSION_V3: u16 = 3;
const SEGMENT_VERSION_V4: u16 = 4;
const SEGMENT_VERSION_V5: u16 = 5;
const SEGMENT_VERSION: u16 = 6;
const SEGMENT_HEADER_LEN_V2: usize = 64;
const SEGMENT_HEADER_LEN_V4: usize = 72;
const SEGMENT_HEADER_LEN: usize = 80;
const DIRECTORY_ENTRY_LEN: usize = 32;
const INDEX_BLOCK_HEADER_LEN: usize = 32;
const INDEX_PAGE_TARGET_BYTES: usize = 16 * 1024;
const INDEX_PAGE_FORMAT: u8 = 0x80;
const INDEX_PAGE_ROUTING: u8 = 0x40;
const INDEX_PAGE_FIRST: u8 = 0x01;
const INDEX_PAGE_LAST: u8 = 0x02;
const INDEX_ROUTING_HEADER_LEN: usize = 16;
const LOCATOR_MAGIC: [u8; 4] = *b"OWLL";
const LOCATOR_VERSION_V1: u16 = 1;
const LOCATOR_VERSION_V2: u16 = 2;
const LOCATOR_VERSION_V3: u16 = 3;
const LOCATOR_VERSION: u16 = 4;
const LOCATOR_HEADER_LEN_LEGACY: usize = 32;
const LOCATOR_HEADER_LEN: usize = 40;
const LOCATOR_DIRECTORY_LEN: usize = 16;
const LOCATOR_DIRECTORY_LEN_V4: usize = 24;
const LOCATOR_PAGE_HEADER_LEN: usize = 8;
const LOCATOR_PAGE_SLOTS: usize = 4_096;
const LOCATOR_PAGE_CACHE_CAPACITY: usize = 64;
#[cfg(not(test))]
const LOCATOR_SORT_CHUNK_ENTRIES: usize = 1_048_576;
#[cfg(test)]
const LOCATOR_SORT_CHUNK_ENTRIES: usize = 128;
const LOCATOR_SORT_MERGE_FAN_IN: usize = 32;
const LOCATOR_RUN_ENTRY_LEN: usize = 10;
const LOCATOR_RUN_WRITE_BUFFER: usize = 256 * 1024;
const LOCATOR_BUILD_JOURNAL_MAGIC: [u8; 4] = *b"OWLJ";
const LOCATOR_BUILD_JOURNAL_VERSION: u16 = 1;
const LOCATOR_BUILD_JOURNAL_LEN: usize = 32;
const MAX_LOCATOR_BUILD_RUNS: usize = 10_000_000;

const MANIFEST_MAGIC: [u8; 4] = *b"OWLM";
const MANIFEST_VERSION: u16 = 2;
const MANIFEST_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointInfo {
    pub sequence: u64,
    pub checkpoint_lsn: u64,
    pub live_objects: usize,
    pub segment_bytes: u64,
    pub slot: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CheckpointMetadata {
    pub collection_ids: Vec<(String, u16)>,
    pub next_collection_id: u16,
    pub next_object_ids: Vec<(u16, u32)>,
    pub schemas: Vec<Schema>,
    pub compiled_schemas: Vec<CompiledSchema>,
    pub current_schema_versions: Vec<(String, u32)>,
    pub scalar_indexes: Vec<(String, Vec<ScalarIndexDefinition>)>,
    pub vector_indexes: Vec<(String, Vec<VectorIndexDefinition>)>,
}

#[derive(Debug, Clone)]
pub(crate) struct CheckpointImage {
    pub sequence: u64,
    pub checkpoint_lsn: u64,
    pub metadata: CheckpointMetadata,
    pub records: Vec<(ObjRef, Arc<[u8]>)>,
    pub indexes: Vec<CheckpointIndex>,
    pub typed_segments: Vec<Arc<[u8]>>,
}

pub(crate) struct DecodedCheckpoint {
    pub sequence: u64,
    pub checkpoint_lsn: u64,
    pub metadata: CheckpointMetadata,
    pub indexes: SegmentIndexReader,
    pub records: SegmentRecordReader,
    pub typed_segment_descriptors: Vec<TypedSegmentFileDescriptor>,
    pub typed_segments: Vec<Arc<TypedSegmentHandle>>,
    pub typed_locator: Option<TypedLocatorLeaf>,
}

#[derive(Clone)]
pub(crate) struct TypedLocatorLeaf {
    collections: HashMap<u16, (usize, usize)>,
    storage: TypedLocatorStorage,
    slot_width: usize,
    page_slots: usize,
    len: usize,
    fences: Vec<TypedLocatorFence>,
}

#[derive(Clone)]
enum TypedLocatorStorage {
    Resident(Arc<[u8]>),
    File {
        file: Arc<dyn FileHandle>,
        base_offset: usize,
        cache: Arc<Mutex<LocatorPageCache>>,
    },
}

#[derive(Default)]
struct LocatorPageCache {
    pages: HashMap<usize, Arc<[u8]>>,
    order: VecDeque<usize>,
}

#[derive(Clone)]
pub(crate) struct TypedLocatorFence {
    pub segment_index: usize,
    pub first: String,
    pub last: String,
}

impl TypedLocatorLeaf {
    pub(crate) fn get(&self, object_ref: ObjRef) -> OQLResult<Option<(usize, usize)>> {
        let Some((start, slots)) = self.collections.get(&object_ref.collection_id()).copied()
        else {
            return Ok(None);
        };
        let object_id = object_ref.object_id() as usize;
        if object_id >= slots {
            return Ok(None);
        }
        let packed = if self.page_slots == 0 {
            let offset = start
                .checked_add(object_id.checked_mul(self.slot_width).ok_or_else(|| {
                    OQLError::storage_error("typed locator offset overflow", None)
                })?)
                .ok_or_else(|| OQLError::storage_error("typed locator offset overflow", None))?;
            let TypedLocatorStorage::Resident(bytes) = &self.storage else {
                return Err(OQLError::storage_error(
                    "legacy typed locator cannot be file-backed",
                    None,
                ));
            };
            match self.slot_width {
                4 => u64::from(get_u32(bytes, offset)?),
                8 => get_u64(bytes, offset)?,
                _ => {
                    return Err(OQLError::storage_error(
                        "typed locator has an invalid slot width",
                        None,
                    ))
                }
            }
        } else {
            let page = object_id / self.page_slots;
            let within = object_id % self.page_slots;
            let page_start = start
                .checked_add(
                    page.checked_mul(LOCATOR_PAGE_HEADER_LEN + self.page_slots * self.slot_width)
                        .ok_or_else(|| {
                            OQLError::storage_error("typed locator page offset overflow", None)
                        })?,
                )
                .ok_or_else(|| OQLError::storage_error("typed locator offset overflow", None))?;
            let expected_slots = self.page_slots.min(slots - page * self.page_slots);
            let payload = self.locator_page(page_start, expected_slots)?;
            match self.slot_width {
                4 => u64::from(get_u32(&payload, within * 4)?),
                8 => get_u64(&payload, within * 8)?,
                _ => {
                    return Err(OQLError::storage_error(
                        "typed locator has an invalid slot width",
                        None,
                    ))
                }
            }
        };
        Ok(unpack_typed_location(packed))
    }

    fn locator_page(&self, page_start: usize, slots: usize) -> OQLResult<Arc<[u8]>> {
        let payload_len = slots
            .checked_mul(self.slot_width)
            .ok_or_else(|| OQLError::storage_error("typed locator page length overflow", None))?;
        match &self.storage {
            TypedLocatorStorage::Resident(bytes) => {
                let header_end = page_start + LOCATOR_PAGE_HEADER_LEN;
                let payload_end = header_end.checked_add(payload_len).ok_or_else(|| {
                    OQLError::storage_error("typed locator page length overflow", None)
                })?;
                let payload = bytes.get(header_end..payload_end).ok_or_else(|| {
                    OQLError::storage_error("typed locator page is truncated", None)
                })?;
                Ok(Arc::from(payload))
            }
            TypedLocatorStorage::File {
                file,
                base_offset,
                cache,
            } => {
                if let Some(page) = cache
                    .lock()
                    .map_err(|_| OQLError::generic("typed locator cache lock poisoned"))?
                    .pages
                    .get(&page_start)
                    .cloned()
                {
                    return Ok(page);
                }
                let absolute = base_offset.checked_add(page_start).ok_or_else(|| {
                    OQLError::storage_error("typed locator file offset overflow", None)
                })?;
                let encoded = read_exact_at(
                    file.as_ref(),
                    absolute,
                    LOCATOR_PAGE_HEADER_LEN + payload_len,
                )?;
                if get_u16(&encoded, 4)? as usize != slots
                    || crc32fast::hash(&encoded[LOCATOR_PAGE_HEADER_LEN..]) != get_u32(&encoded, 0)?
                {
                    return Err(OQLError::storage_error(
                        "typed locator page checksum is invalid",
                        None,
                    ));
                }
                let page: Arc<[u8]> = Arc::from(&encoded[LOCATOR_PAGE_HEADER_LEN..]);
                let mut cache = cache
                    .lock()
                    .map_err(|_| OQLError::generic("typed locator cache lock poisoned"))?;
                if cache.pages.len() == LOCATOR_PAGE_CACHE_CAPACITY {
                    if let Some(evicted) = cache.order.pop_front() {
                        cache.pages.remove(&evicted);
                    }
                }
                cache.order.push_back(page_start);
                cache.pages.insert(page_start, page.clone());
                Ok(page)
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn fences(&self) -> &[TypedLocatorFence] {
        &self.fences
    }

    pub(crate) fn cached_page_stats(&self) -> OQLResult<(usize, usize)> {
        match &self.storage {
            TypedLocatorStorage::Resident(bytes) => Ok((0, bytes.len())),
            TypedLocatorStorage::File { cache, .. } => {
                let cache = cache
                    .lock()
                    .map_err(|_| OQLError::generic("typed locator cache lock poisoned"))?;
                Ok((
                    cache.pages.len(),
                    cache.pages.values().map(|page| page.len()).sum(),
                ))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypedSegmentFileDescriptor {
    pub offset: usize,
    pub descriptor: TypedSegmentDescriptor,
}

pub(crate) struct TypedSegmentHandle {
    descriptor: TypedSegmentDescriptor,
    identity: Mutex<Option<TypedSegmentIdentity>>,
    decoded: Mutex<Option<Arc<TypedSegment>>>,
    file: Option<Arc<dyn FileHandle>>,
    offset: usize,
}

impl TypedSegmentHandle {
    pub(crate) fn resident(segment: TypedSegment) -> Self {
        let row_count = segment.row_count();
        Self {
            descriptor: TypedSegmentDescriptor {
                format_version: 0,
                authoritative: segment.authoritative,
                collection_id: segment.collection_id,
                schema_version: segment.schema_version,
                row_count,
                column_count: segment.columns.len(),
                lookup_len: 0,
                identity_len: 0,
                lookup_checksum: None,
                encoded_len: 0,
            },
            identity: Mutex::new(None),
            decoded: Mutex::new(Some(Arc::new(segment))),
            file: None,
            offset: 0,
        }
    }

    fn file_backed(
        file: Arc<dyn FileHandle>,
        location: TypedSegmentFileDescriptor,
        audit: bool,
    ) -> OQLResult<Self> {
        let identity = if audit {
            const TYPED_HEADER_LEN: usize = 64;
            let identity_bytes = read_exact_at(
                file.as_ref(),
                location.offset + TYPED_HEADER_LEN,
                location.descriptor.lookup_len,
            )?;
            Some(TypedSegment::decode_identity(
                location.descriptor,
                Arc::from(identity_bytes),
            )?)
        } else {
            None
        };
        Ok(Self {
            descriptor: location.descriptor,
            identity: Mutex::new(identity),
            decoded: Mutex::new(None),
            file: Some(file),
            offset: location.offset,
        })
    }

    pub(crate) fn authoritative(&self) -> bool {
        self.descriptor.authoritative
    }

    pub(crate) fn collection_id(&self) -> u16 {
        self.descriptor.collection_id
    }

    pub(crate) fn schema_version(&self) -> u32 {
        self.descriptor.schema_version
    }

    pub(crate) fn row_count(&self) -> usize {
        self.descriptor.row_count
    }

    pub(crate) fn column_count(&self) -> usize {
        self.descriptor.column_count
    }

    pub(crate) fn identity_bytes(&self) -> usize {
        self.identity.lock().map_or(0, |identity| {
            usize::from(identity.is_some()) * self.descriptor.lookup_len
        })
    }

    pub(crate) fn is_loaded(&self) -> OQLResult<bool> {
        self.decoded
            .lock()
            .map(|decoded| decoded.is_some())
            .map_err(|_| OQLError::generic("typed segment cache lock poisoned"))
    }

    pub(crate) fn object_ref(&self, row: usize) -> OQLResult<ObjRef> {
        if let Some(object_ref) = self.with_identity(|identity| identity.object_ref(row))? {
            return Ok(object_ref);
        }
        self.load()?
            .object_refs
            .get(row)
            .copied()
            .ok_or_else(|| OQLError::storage_error("typed row is out of bounds", None))
    }

    pub(crate) fn document_id(&self, row: usize) -> OQLResult<String> {
        if let Some(id) =
            self.with_identity(|identity| Ok(identity.document_id(row)?.to_owned()))?
        {
            return Ok(id);
        }
        self.load()?
            .document_ids
            .get(row)
            .cloned()
            .ok_or_else(|| OQLError::storage_error("typed row is out of bounds", None))
    }

    pub(crate) fn find_document_id(&self, id: &str) -> OQLResult<Option<usize>> {
        if let Some(row) = self.with_identity(|identity| identity.find_document_id(id))? {
            return Ok(row);
        }
        Ok(self
            .load()?
            .document_ids
            .binary_search_by(|candidate| candidate.as_str().cmp(id))
            .ok())
    }

    pub(crate) fn first_document_id(&self) -> OQLResult<Option<String>> {
        (self.row_count() > 0)
            .then(|| self.document_id(0))
            .transpose()
    }

    pub(crate) fn last_document_id(&self) -> OQLResult<Option<String>> {
        (self.row_count() > 0)
            .then(|| self.document_id(self.row_count() - 1))
            .transpose()
    }

    pub(crate) fn document_version(&self, row: usize) -> OQLResult<u64> {
        if self.identity_has_row_metadata() {
            if let Some(version) = self.with_identity(|identity| identity.document_version(row))? {
                return Ok(version);
            }
        }
        self.load()?
            .document_versions
            .get(row)
            .copied()
            .ok_or_else(|| OQLError::storage_error("typed row is out of bounds", None))
    }

    pub(crate) fn created_at(&self, row: usize) -> OQLResult<chrono::DateTime<chrono::Utc>> {
        if self.identity_has_row_metadata() {
            if let Some(created_at) = self.with_identity(|identity| identity.created_at(row))? {
                return Ok(created_at);
            }
        }
        self.load()?
            .created_at
            .get(row)
            .copied()
            .ok_or_else(|| OQLError::storage_error("typed row is out of bounds", None))
    }

    pub(crate) fn updated_at(&self, row: usize) -> OQLResult<chrono::DateTime<chrono::Utc>> {
        if self.identity_has_row_metadata() {
            if let Some(updated_at) = self.with_identity(|identity| identity.updated_at(row))? {
                return Ok(updated_at);
            }
        }
        self.load()?
            .updated_at
            .get(row)
            .copied()
            .ok_or_else(|| OQLError::storage_error("typed row is out of bounds", None))
    }

    fn identity_has_row_metadata(&self) -> bool {
        self.descriptor.format_version > 0
            && self.descriptor.lookup_len == self.descriptor.identity_len
    }

    fn with_identity<T>(
        &self,
        use_identity: impl FnOnce(&TypedSegmentIdentity) -> OQLResult<T>,
    ) -> OQLResult<Option<T>> {
        if self.file.is_none() {
            return Ok(None);
        }
        let mut identity = self
            .identity
            .lock()
            .map_err(|_| OQLError::generic("typed identity cache lock poisoned"))?;
        if identity.is_none() {
            const TYPED_HEADER_LEN: usize = 64;
            let file = self.file.as_ref().expect("file-backed typed segment");
            let bytes = read_exact_at(
                file.as_ref(),
                self.offset + TYPED_HEADER_LEN,
                self.descriptor.lookup_len,
            )?;
            *identity = Some(TypedSegment::open_identity(
                self.descriptor,
                Arc::from(bytes),
            )?);
        }
        use_identity(identity.as_ref().expect("typed identity initialized")).map(Some)
    }

    pub(crate) fn load(&self) -> OQLResult<Arc<TypedSegment>> {
        let mut decoded = self
            .decoded
            .lock()
            .map_err(|_| OQLError::generic("typed segment cache lock poisoned"))?;
        if let Some(segment) = decoded.as_ref() {
            return Ok(segment.clone());
        }
        let file = self.file.as_ref().ok_or_else(|| {
            OQLError::storage_error("typed segment has no resident or file source", None)
        })?;
        let bytes = read_exact_at(file.as_ref(), self.offset, self.descriptor.encoded_len)?;
        let segment = Arc::new(TypedSegment::decode(&bytes)?);
        *decoded = Some(segment.clone());
        Ok(segment)
    }
}

pub(crate) fn open_typed_segment_handles(
    filesystem: &dyn FileSystem,
    path: &Path,
    descriptors: &[TypedSegmentFileDescriptor],
    audit: bool,
) -> OQLResult<Vec<Arc<TypedSegmentHandle>>> {
    let shared_file = descriptors
        .iter()
        .any(|location| location.descriptor.lookup_checksum.is_some())
        .then(|| {
            filesystem
                .open(path)
                .map(Arc::<dyn FileHandle>::from)
                .map_err(|error| {
                    OQLError::storage_error(
                        &format!("open typed checkpoint segments: {error}"),
                        None,
                    )
                })
        })
        .transpose()?;
    descriptors
        .iter()
        .copied()
        .map(|location| {
            if location.descriptor.lookup_checksum.is_some() {
                TypedSegmentHandle::file_backed(
                    shared_file
                        .as_ref()
                        .expect("modern typed segments have a shared file")
                        .clone(),
                    location,
                    audit,
                )
                .map(Arc::new)
            } else {
                let bytes = read_file_range(
                    filesystem,
                    path,
                    location.offset,
                    location.descriptor.encoded_len,
                )?;
                Ok(Arc::new(TypedSegmentHandle::resident(
                    TypedSegment::decode(&bytes)?,
                )))
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SegmentVerification {
    /// Validate only the roots needed to open the checkpoint. Records, index
    /// pages, and typed segments retain checksums that are verified on decode.
    Open,
    /// Stream and validate the complete immutable segment.
    Audit,
}

pub(crate) struct SegmentRecordReader {
    path: PathBuf,
    directory: Vec<u8>,
    entry_count: usize,
    records_start: usize,
    records_len: usize,
}

pub(crate) struct SegmentIndexReader {
    path: PathBuf,
    start: usize,
    len: usize,
    count: usize,
}

impl SegmentIndexReader {
    pub(crate) fn for_each(
        self,
        filesystem: &dyn FileSystem,
        mut visitor: impl FnMut(CheckpointIndexPage) -> OQLResult<()>,
    ) -> OQLResult<()> {
        let mut file = filesystem.open(&self.path).map_err(|error| {
            OQLError::storage_error(&format!("open checkpoint indexes: {error}"), None)
        })?;
        file.seek(SeekFrom::Start(self.start as u64))
            .map_err(|error| {
                OQLError::storage_error(&format!("seek checkpoint indexes: {error}"), None)
            })?;
        let mut consumed = 0usize;
        let mut identities = HashSet::with_capacity(self.count);
        let mut open_identity: Option<(String, String, bool)> = None;
        for _ in 0..self.count {
            let header = read_exact_handle(file.as_mut(), INDEX_BLOCK_HEADER_LEN)?;
            let payload_len = usize::try_from(get_u64(&header, 8)?)
                .map_err(|_| OQLError::storage_error("index block length overflow", None))?;
            let block_len = INDEX_BLOCK_HEADER_LEN
                .checked_add(payload_len)
                .filter(|length| {
                    consumed
                        .checked_add(*length)
                        .is_some_and(|end| end <= self.len)
                })
                .ok_or_else(|| {
                    OQLError::storage_error("checkpoint index block is truncated", None)
                })?;
            let mut page = if header[21] & INDEX_PAGE_ROUTING != 0 {
                let identity_len = (get_u16(&header, 0)? as usize)
                    .checked_add(get_u16(&header, 2)? as usize)
                    .and_then(|length| length.checked_add(INDEX_ROUTING_HEADER_LEN))
                    .filter(|length| *length <= payload_len)
                    .ok_or_else(|| {
                        OQLError::storage_error("index routing prefix length overflow", None)
                    })?;
                let mut routing = read_exact_handle(file.as_mut(), identity_len)?;
                let first_len = get_u32(&routing, identity_len - 16)? as usize;
                let last_len = get_u32(&routing, identity_len - 12)? as usize;
                let key_len = first_len.checked_add(last_len).ok_or_else(|| {
                    OQLError::storage_error("index routing key length overflow", None)
                })?;
                routing.extend_from_slice(&read_exact_handle(file.as_mut(), key_len)?);
                let mut page = decode_index_page_routing(&header, &routing)?;
                let remaining = payload_len.checked_sub(routing.len()).ok_or_else(|| {
                    OQLError::storage_error("checkpoint index routing prefix is truncated", None)
                })?;
                file.seek(SeekFrom::Current(i64::try_from(remaining).map_err(
                    |_| OQLError::storage_error("index page seek length overflow", None),
                )?))
                .map_err(|error| {
                    OQLError::storage_error(&format!("seek checkpoint index page: {error}"), None)
                })?;
                page.routing_only = true;
                page
            } else {
                let payload = read_exact_handle(file.as_mut(), payload_len)?;
                decode_index_page(&header, &payload)?
            };
            page.location = Some(IndexPageLocation {
                path: self.path.clone(),
                offset: self.start + consumed,
                len: block_len,
            });
            let identity = (page.schema.clone(), page.name.clone(), page.unique);
            if page.first {
                if open_identity.is_some()
                    || !identities.insert((page.schema.clone(), page.name.clone()))
                {
                    return Err(OQLError::storage_error(
                        "duplicate or interrupted checkpoint index page chain",
                        None,
                    ));
                }
            } else if open_identity.as_ref() != Some(&identity) {
                return Err(OQLError::storage_error(
                    "checkpoint index page chain changes identity",
                    None,
                ));
            }
            open_identity = (!page.last).then_some(identity);
            visitor(page)?;
            consumed += block_len;
        }
        if consumed != self.len || open_identity.is_some() {
            return Err(OQLError::storage_error(
                "checkpoint index region has trailing bytes",
                None,
            ));
        }
        Ok(())
    }
}

impl SegmentRecordReader {
    pub(crate) fn for_each_chunk(
        self,
        filesystem: &dyn FileSystem,
        chunk_size: usize,
        mut visitor: impl FnMut(Vec<(ObjRef, Arc<[u8]>)>) -> OQLResult<()>,
    ) -> OQLResult<()> {
        if chunk_size == 0 {
            return Err(OQLError::storage_error(
                "checkpoint record chunk size must be positive",
                None,
            ));
        }
        let mut previous = None;
        let mut expected_record_offset = 0usize;
        let mut chunk = Vec::with_capacity(chunk_size.min(self.entry_count));
        let mut record_file = filesystem.open(&self.path).map_err(|error| {
            OQLError::storage_error(&format!("open checkpoint records: {error}"), None)
        })?;
        record_file
            .seek(SeekFrom::Start(self.records_start as u64))
            .map_err(|error| {
                OQLError::storage_error(&format!("seek checkpoint records: {error}"), None)
            })?;
        for index in 0..self.entry_count {
            let entry = index * DIRECTORY_ENTRY_LEN;
            let object_ref = ObjRef::new(get_u64(&self.directory, entry)?);
            let offset = usize::try_from(get_u64(&self.directory, entry + 8)?)
                .map_err(|_| OQLError::storage_error("checkpoint record offset overflow", None))?;
            let length = get_u32(&self.directory, entry + 16)? as usize;
            let record_end = offset
                .checked_add(length)
                .filter(|end| *end <= self.records_len)
                .ok_or_else(|| OQLError::storage_error("checkpoint record is truncated", None))?;
            if object_ref.is_null()
                || previous.is_some_and(|candidate| candidate >= object_ref)
                || offset != expected_record_offset
            {
                return Err(OQLError::storage_error(
                    "checkpoint directory is not ordered or contiguous",
                    None,
                ));
            }
            let record = read_exact_handle(record_file.as_mut(), record_end - offset)?;
            if crc32fast::hash(&record) != get_u32(&self.directory, entry + 20)? {
                return Err(OQLError::storage_error(
                    "checkpoint object record checksum mismatch",
                    None,
                ));
            }
            chunk.push((object_ref, Arc::from(record)));
            if chunk.len() == chunk_size {
                visitor(std::mem::replace(
                    &mut chunk,
                    Vec::with_capacity(chunk_size),
                ))?;
            }
            previous = Some(object_ref);
            expected_record_offset = record_end;
        }
        if !chunk.is_empty() {
            visitor(chunk)?;
        }
        if expected_record_offset != self.records_len {
            return Err(OQLError::storage_error(
                "checkpoint records do not cover their region",
                None,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CheckpointIndex {
    pub schema: String,
    pub name: String,
    pub unique: bool,
    pub entries: Vec<(Vec<u8>, Vec<ObjRef>)>,
}

pub(crate) struct CheckpointIndexPage {
    pub schema: String,
    pub name: String,
    pub unique: bool,
    pub first: bool,
    pub last: bool,
    pub entries: Vec<(Vec<u8>, Vec<ObjRef>)>,
    pub first_key: Option<Vec<u8>>,
    pub last_key: Option<Vec<u8>>,
    pub posting_count: usize,
    routing_only: bool,
    pub location: Option<IndexPageLocation>,
}

impl CheckpointIndexPage {
    pub(crate) fn descriptor(self) -> OQLResult<IndexPageDescriptor> {
        let location = self.location.clone().ok_or_else(|| {
            OQLError::storage_error("checkpoint index page has no file location", None)
        })?;
        Ok(IndexPageDescriptor {
            location,
            first_key: self.first_key,
            last_key: self.last_key,
            cached_entries: (!self.routing_only).then(|| Arc::new(self.entries)),
        })
    }
}

#[derive(Clone)]
pub(crate) struct IndexPageLocation {
    path: PathBuf,
    offset: usize,
    len: usize,
}

#[derive(Clone)]
pub(crate) struct IndexPageDescriptor {
    location: IndexPageLocation,
    first_key: Option<Vec<u8>>,
    last_key: Option<Vec<u8>>,
    cached_entries: Option<Arc<Vec<(Vec<u8>, Vec<ObjRef>)>>>,
}

pub(crate) struct FileIndexBase {
    filesystem: Arc<dyn FileSystem>,
    schema: String,
    name: String,
    unique: bool,
    pages: Vec<IndexPageDescriptor>,
    page_cache: Mutex<HashMap<usize, Arc<Vec<(Vec<u8>, Vec<ObjRef>)>>>>,
}

impl FileIndexBase {
    pub(crate) fn new(
        filesystem: Arc<dyn FileSystem>,
        schema: String,
        name: String,
        unique: bool,
        mut pages: Vec<IndexPageDescriptor>,
    ) -> Self {
        let page_cache = pages
            .iter_mut()
            .enumerate()
            .filter_map(|(index, page)| page.cached_entries.take().map(|entries| (index, entries)))
            .collect();
        Self {
            filesystem,
            schema,
            name,
            unique,
            pages,
            page_cache: Mutex::new(page_cache),
        }
    }

    fn read_page(&self, page_index: usize) -> Result<Arc<Vec<(Vec<u8>, Vec<ObjRef>)>>, String> {
        if let Some(entries) = self
            .page_cache
            .lock()
            .map_err(|_| "checkpoint index page cache lock poisoned".to_string())?
            .get(&page_index)
            .cloned()
        {
            return Ok(entries);
        }
        let descriptor = self
            .pages
            .get(page_index)
            .ok_or_else(|| "checkpoint index page number is invalid".to_string())?;
        let bytes = read_file_range(
            self.filesystem.as_ref(),
            &descriptor.location.path,
            descriptor.location.offset,
            descriptor.location.len,
        )
        .map_err(|error| error.to_string())?;
        if bytes.len() < INDEX_BLOCK_HEADER_LEN {
            return Err("checkpoint index page is truncated".into());
        }
        let page = decode_index_page(
            &bytes[..INDEX_BLOCK_HEADER_LEN],
            &bytes[INDEX_BLOCK_HEADER_LEN..],
        )
        .map_err(|error| error.to_string())?;
        if page.schema != self.schema || page.name != self.name || page.unique != self.unique {
            return Err("checkpoint index page identity changed after recovery".into());
        }
        let entries = Arc::new(page.entries);
        self.page_cache
            .lock()
            .map_err(|_| "checkpoint index page cache lock poisoned".to_string())?
            .insert(page_index, entries.clone());
        Ok(entries)
    }
}

impl OrderedIndexBase for FileIndexBase {
    fn search_key(&self, key: &[u8]) -> Result<Vec<ObjRef>, String> {
        let Some(page_index) = self.pages.iter().position(|page| {
            page.first_key
                .as_deref()
                .zip(page.last_key.as_deref())
                .is_some_and(|(first, last)| first <= key && key <= last)
        }) else {
            return Ok(Vec::new());
        };
        Ok(self
            .read_page(page_index)?
            .iter()
            .find_map(|(candidate, postings)| (candidate == key).then(|| postings.clone()))
            .unwrap_or_default())
    }

    fn range_entries(
        &self,
        start: Bound<Vec<u8>>,
        end: Bound<Vec<u8>>,
        limit: Option<usize>,
        descending: bool,
    ) -> Result<Vec<(Vec<u8>, Vec<ObjRef>)>, String> {
        let mut entries = Vec::new();
        let mut page_indexes = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(index, descriptor)| {
                descriptor
                    .first_key
                    .as_deref()
                    .zip(descriptor.last_key.as_deref())
                    .filter(|(first, last)| page_intersects(first, last, &start, &end))
                    .map(|_| index)
            })
            .collect::<Vec<_>>();
        if descending {
            page_indexes.reverse();
        }
        let mut posting_count = 0usize;
        for page_index in page_indexes {
            let descriptor = &self.pages[page_index];
            let Some((first, last)) = descriptor
                .first_key
                .as_deref()
                .zip(descriptor.last_key.as_deref())
            else {
                continue;
            };
            if !page_intersects(first, last, &start, &end) {
                continue;
            }
            let page_entries = self
                .read_page(page_index)?
                .iter()
                .filter(|(key, _)| key_in_bounds(key, &start, &end))
                .cloned()
                .collect::<Vec<_>>();
            let mut page_entries = page_entries;
            if descending {
                page_entries.reverse();
            }
            posting_count += page_entries
                .iter()
                .map(|(_, postings)| postings.len())
                .sum::<usize>();
            entries.extend(page_entries);
            if limit.is_some_and(|limit| posting_count >= limit) {
                break;
            }
        }
        Ok(entries)
    }

    fn visit_entries(
        &self,
        visitor: &mut dyn FnMut(&[u8], &[ObjRef]) -> Result<(), String>,
    ) -> Result<(), String> {
        for page_index in 0..self.pages.len() {
            for (key, postings) in self.read_page(page_index)?.iter() {
                visitor(key, postings)?;
            }
        }
        Ok(())
    }
}

fn key_in_bounds(key: &[u8], start: &Bound<Vec<u8>>, end: &Bound<Vec<u8>>) -> bool {
    let after_start = match start {
        Bound::Included(value) => key >= value.as_slice(),
        Bound::Excluded(value) => key > value.as_slice(),
        Bound::Unbounded => true,
    };
    let before_end = match end {
        Bound::Included(value) => key <= value.as_slice(),
        Bound::Excluded(value) => key < value.as_slice(),
        Bound::Unbounded => true,
    };
    after_start && before_end
}

fn page_intersects(
    first: &[u8],
    last: &[u8],
    start: &Bound<Vec<u8>>,
    end: &Bound<Vec<u8>>,
) -> bool {
    let after_start = match start {
        Bound::Included(value) => last >= value.as_slice(),
        Bound::Excluded(value) => last > value.as_slice(),
        Bound::Unbounded => true,
    };
    let before_end = match end {
        Bound::Included(value) => first <= value.as_slice(),
        Bound::Excluded(value) => first < value.as_slice(),
        Bound::Unbounded => true,
    };
    after_start && before_end
}

pub(crate) struct SegmentFileInfo {
    pub len: u64,
    pub checksum: u32,
    pub indexes: Vec<PublishedIndexPages>,
    pub typed_locator: TypedLocatorLeaf,
    pub typed_segments: Vec<TypedSegmentFileDescriptor>,
}

pub(crate) struct PublishedIndexPages {
    pub schema: String,
    pub name: String,
    pub unique: bool,
    pub pages: Vec<IndexPageDescriptor>,
}

fn pack_typed_location(segment: usize, row: usize) -> OQLResult<u32> {
    let segment = u16::try_from(segment)
        .map_err(|_| OQLError::storage_error("typed segment index exceeds u16", None))?;
    let row = u16::try_from(row)
        .map_err(|_| OQLError::storage_error("typed segment row exceeds u16", None))?;
    ((u32::from(segment) << 16) | u32::from(row))
        .checked_add(1)
        .ok_or_else(|| OQLError::storage_error("typed locator uses its reserved sentinel", None))
}

fn unpack_typed_location(packed: u64) -> Option<(usize, usize)> {
    let packed = packed.checked_sub(1)?;
    if packed <= u64::from(u32::MAX) {
        Some((
            ((packed as u32) >> 16) as usize,
            ((packed as u32) & 0xffff) as usize,
        ))
    } else {
        Some(((packed >> 32) as usize, (packed as u32) as usize))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct LocatorSortEntry {
    collection_id: u16,
    object_id: u32,
    packed: u32,
}

struct LocatorRunReader {
    file: Box<dyn FileHandle>,
    remaining: usize,
}

impl LocatorRunReader {
    fn open(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<Self> {
        let len = usize::try_from(filesystem.file_size(path).map_err(|error| {
            OQLError::storage_error(&format!("inspect locator run: {error}"), None)
        })?)
        .map_err(|_| OQLError::storage_error("locator run is too large", None))?;
        if len % LOCATOR_RUN_ENTRY_LEN != 0 {
            return Err(OQLError::storage_error(
                "locator run has a truncated entry",
                None,
            ));
        }
        let file = filesystem.open(path).map_err(|error| {
            OQLError::storage_error(&format!("open locator run: {error}"), None)
        })?;
        Ok(Self {
            file,
            remaining: len / LOCATOR_RUN_ENTRY_LEN,
        })
    }

    fn next(&mut self) -> OQLResult<Option<LocatorSortEntry>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let bytes = read_exact_handle(self.file.as_mut(), LOCATOR_RUN_ENTRY_LEN)?;
        self.remaining -= 1;
        Ok(Some(LocatorSortEntry {
            collection_id: get_u16(&bytes, 0)?,
            object_id: get_u32(&bytes, 2)?,
            packed: get_u32(&bytes, 6)?,
        }))
    }
}

struct LocatorRunWriter {
    file: Box<dyn FileHandle>,
    buffer: Vec<u8>,
}

struct LocatorRunCleanup<'a> {
    filesystem: &'a dyn FileSystem,
    paths: Vec<PathBuf>,
}

impl Drop for LocatorRunCleanup<'_> {
    fn drop(&mut self) {
        for path in &self.paths {
            if self.filesystem.exists(path) {
                let _ = self.filesystem.remove(path);
            }
        }
    }
}

impl LocatorRunWriter {
    fn create(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<Self> {
        let file = filesystem.create(path).map_err(|error| {
            OQLError::storage_error(&format!("create locator run: {error}"), None)
        })?;
        Ok(Self {
            file,
            buffer: Vec::with_capacity(LOCATOR_RUN_WRITE_BUFFER),
        })
    }

    fn push(&mut self, entry: LocatorSortEntry) -> OQLResult<()> {
        self.buffer
            .extend_from_slice(&entry.collection_id.to_le_bytes());
        self.buffer
            .extend_from_slice(&entry.object_id.to_le_bytes());
        self.buffer.extend_from_slice(&entry.packed.to_le_bytes());
        if self.buffer.len() >= LOCATOR_RUN_WRITE_BUFFER {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> OQLResult<()> {
        if !self.buffer.is_empty() {
            write_all_handle(self.file.as_mut(), &self.buffer)?;
            self.buffer.clear();
        }
        Ok(())
    }

    fn finish(mut self) -> OQLResult<()> {
        self.flush()?;
        self.file
            .sync()
            .map_err(|error| OQLError::storage_error(&format!("sync locator run: {error}"), None))
    }
}

fn locator_temp_path(path: &Path, sequence: u64, kind: &str, ordinal: usize) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(format!(".locator-{sequence}-{kind}-{ordinal}"));
    PathBuf::from(name)
}

fn locator_journal_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(".locator-journal");
    PathBuf::from(name)
}

fn write_locator_build_journal(
    filesystem: &dyn FileSystem,
    path: &Path,
    sequence: u64,
    initial_runs: usize,
) -> OQLResult<()> {
    if initial_runs > MAX_LOCATOR_BUILD_RUNS {
        return Err(OQLError::storage_error(
            "typed locator build has too many sort runs",
            None,
        ));
    }
    let mut bytes = [0u8; LOCATOR_BUILD_JOURNAL_LEN];
    bytes[0..4].copy_from_slice(&LOCATOR_BUILD_JOURNAL_MAGIC);
    put_u16(&mut bytes, 4, LOCATOR_BUILD_JOURNAL_VERSION);
    put_u16(&mut bytes, 6, LOCATOR_BUILD_JOURNAL_LEN as u16);
    put_u64(&mut bytes, 8, sequence);
    put_u64(&mut bytes, 16, initial_runs as u64);
    let checksum = crc32fast::hash(&bytes[..28]);
    put_u32(&mut bytes, 28, checksum);
    let journal = locator_journal_path(path);
    let mut file = filesystem.create(&journal).map_err(|error| {
        OQLError::storage_error(&format!("create locator build journal: {error}"), None)
    })?;
    write_all_handle(file.as_mut(), &bytes)?;
    file.sync().map_err(|error| {
        OQLError::storage_error(&format!("sync locator build journal: {error}"), None)
    })?;
    filesystem.sync_all().map_err(|error| {
        OQLError::storage_error(
            &format!("sync locator build journal directory: {error}"),
            None,
        )
    })
}

pub(crate) fn cleanup_typed_locator_spools(
    filesystem: &dyn FileSystem,
    path: &Path,
) -> OQLResult<()> {
    let journal = locator_journal_path(path);
    if !filesystem.exists(&journal) {
        return Ok(());
    }
    let len = usize::try_from(filesystem.file_size(&journal).map_err(|error| {
        OQLError::storage_error(&format!("inspect locator build journal: {error}"), None)
    })?)
    .map_err(|_| OQLError::storage_error("locator build journal is too large", None))?;
    if len != LOCATOR_BUILD_JOURNAL_LEN {
        return Err(OQLError::storage_error(
            "locator build journal is truncated",
            None,
        ));
    }
    let mut file = filesystem.open(&journal).map_err(|error| {
        OQLError::storage_error(&format!("open locator build journal: {error}"), None)
    })?;
    let bytes = read_exact_handle(file.as_mut(), len)?;
    if bytes[0..4] != LOCATOR_BUILD_JOURNAL_MAGIC
        || get_u16(&bytes, 4)? != LOCATOR_BUILD_JOURNAL_VERSION
        || get_u16(&bytes, 6)? as usize != LOCATOR_BUILD_JOURNAL_LEN
        || crc32fast::hash(&bytes[..28]) != get_u32(&bytes, 28)?
    {
        return Err(OQLError::storage_error(
            "locator build journal header or checksum is invalid",
            None,
        ));
    }
    let sequence = get_u64(&bytes, 8)?;
    let initial_runs = usize::try_from(get_u64(&bytes, 16)?)
        .map_err(|_| OQLError::storage_error("locator build run count is too large", None))?;
    if initial_runs > MAX_LOCATOR_BUILD_RUNS {
        return Err(OQLError::storage_error(
            "locator build run count exceeds the recovery limit",
            None,
        ));
    }
    for ordinal in 0..initial_runs {
        let run = locator_temp_path(path, sequence, "run", ordinal);
        if filesystem.exists(&run) {
            filesystem.remove(&run).map_err(|error| {
                OQLError::storage_error(&format!("remove locator build run: {error}"), None)
            })?;
        }
    }
    let mut pass = 0usize;
    let mut pass_inputs = initial_runs;
    while pass_inputs > 1 {
        let outputs = pass_inputs.div_ceil(LOCATOR_SORT_MERGE_FAN_IN);
        for group in 0..outputs {
            let run = locator_temp_path(path, sequence, &format!("merge-{pass}"), group);
            if filesystem.exists(&run) {
                filesystem.remove(&run).map_err(|error| {
                    OQLError::storage_error(&format!("remove locator merge run: {error}"), None)
                })?;
            }
        }
        pass_inputs = outputs;
        pass += 1;
    }
    let pages = locator_temp_path(path, sequence, "pages", 0);
    if filesystem.exists(&pages) {
        filesystem.remove(&pages).map_err(|error| {
            OQLError::storage_error(&format!("remove locator page spool: {error}"), None)
        })?;
    }
    filesystem.remove(&journal).map_err(|error| {
        OQLError::storage_error(&format!("remove locator build journal: {error}"), None)
    })?;
    filesystem.sync_all().map_err(|error| {
        OQLError::storage_error(
            &format!("sync reclaimed locator build artifacts: {error}"),
            None,
        )
    })
}

fn write_locator_run(
    filesystem: &dyn FileSystem,
    path: &Path,
    entries: &mut Vec<LocatorSortEntry>,
) -> OQLResult<()> {
    entries.sort_unstable();
    let mut writer = LocatorRunWriter::create(filesystem, path)?;
    for entry in entries.drain(..) {
        writer.push(entry)?;
    }
    writer.finish()
}

fn merge_locator_runs(
    filesystem: &dyn FileSystem,
    inputs: &[PathBuf],
    output: &Path,
) -> OQLResult<()> {
    let mut readers = inputs
        .iter()
        .map(|path| LocatorRunReader::open(filesystem, path))
        .collect::<OQLResult<Vec<_>>>()?;
    let mut heap = BinaryHeap::<Reverse<(LocatorSortEntry, usize)>>::new();
    for (index, reader) in readers.iter_mut().enumerate() {
        if let Some(entry) = reader.next()? {
            heap.push(Reverse((entry, index)));
        }
    }
    let mut writer = LocatorRunWriter::create(filesystem, output)?;
    while let Some(Reverse((entry, index))) = heap.pop() {
        writer.push(entry)?;
        if let Some(next) = readers[index].next()? {
            heap.push(Reverse((next, index)));
        }
    }
    writer.finish()
}

fn consolidate_locator_runs(
    filesystem: &dyn FileSystem,
    path: &Path,
    sequence: u64,
    mut runs: Vec<PathBuf>,
) -> OQLResult<PathBuf> {
    let mut pass = 0usize;
    while runs.len() > 1 {
        let mut outputs =
            Vec::<PathBuf>::with_capacity(runs.len().div_ceil(LOCATOR_SORT_MERGE_FAN_IN));
        for (group, inputs) in runs.chunks(LOCATOR_SORT_MERGE_FAN_IN).enumerate() {
            let output = locator_temp_path(path, sequence, &format!("merge-{pass}"), group);
            if let Err(error) = merge_locator_runs(filesystem, inputs, &output) {
                let _ = filesystem.remove(&output);
                for completed in &outputs {
                    let _ = filesystem.remove(completed);
                }
                for input in &runs {
                    if filesystem.exists(input) {
                        let _ = filesystem.remove(input);
                    }
                }
                return Err(error);
            }
            outputs.push(output);
        }
        for input in &runs {
            if let Err(error) = filesystem.remove(input) {
                for output in &outputs {
                    let _ = filesystem.remove(output);
                }
                return Err(OQLError::storage_error(
                    &format!("remove locator run: {error}"),
                    None,
                ));
            }
        }
        runs = outputs;
        pass += 1;
    }
    runs.pop()
        .ok_or_else(|| OQLError::storage_error("locator sort produced no run", None))
}

struct PreparedTypedLocator<'a> {
    filesystem: &'a dyn FileSystem,
    root: Vec<u8>,
    pages_path: PathBuf,
    journal_path: PathBuf,
    pages_len: usize,
    fences: Vec<u8>,
}

impl PreparedTypedLocator<'_> {
    fn len(&self) -> usize {
        self.root.len() + self.pages_len + self.fences.len()
    }

    fn write_to(
        &self,
        output: &mut dyn FileHandle,
        checksum: &mut crc32fast::Hasher,
    ) -> OQLResult<()> {
        write_all_handle(output, &self.root)?;
        checksum.update(&self.root);
        let mut pages = self.filesystem.open(&self.pages_path).map_err(|error| {
            OQLError::storage_error(&format!("open locator page spool: {error}"), None)
        })?;
        let mut remaining = self.pages_len;
        let buffer = vec![0u8; 64 * 1024];
        while remaining > 0 {
            let wanted = remaining.min(buffer.len());
            let bytes = read_exact_handle(pages.as_mut(), wanted)?;
            write_all_handle(output, &bytes)?;
            checksum.update(&bytes);
            remaining -= wanted;
        }
        write_all_handle(output, &self.fences)?;
        checksum.update(&self.fences);
        Ok(())
    }
}

impl Drop for PreparedTypedLocator<'_> {
    fn drop(&mut self) {
        let _ = self.filesystem.remove(&self.pages_path);
        let _ = self.filesystem.remove(&self.journal_path);
        let _ = self.filesystem.sync_all();
    }
}

#[cfg(test)]
fn encode_typed_locator(segments: &[Arc<[u8]>]) -> OQLResult<Vec<u8>> {
    let mut collections = BTreeMap::<u16, Vec<u32>>::new();
    let mut fences = Vec::<(usize, String, String)>::new();
    for (segment_index, encoded) in segments.iter().enumerate() {
        let segment = TypedSegment::decode(encoded)?;
        if !segment.authoritative {
            continue;
        }
        if let (Some(first), Some(last)) =
            (segment.document_ids.first(), segment.document_ids.last())
        {
            fences.push((segment_index, first.clone(), last.clone()));
        }
        let slots = collections.entry(segment.collection_id).or_default();
        for (row, object_ref) in segment.object_refs.iter().copied().enumerate() {
            let object_id = object_ref.object_id() as usize;
            if slots.len() <= object_id {
                slots.resize(object_id + 1, 0);
            }
            if slots[object_id] != 0 {
                return Err(OQLError::storage_error(
                    "typed locator contains a duplicate object reference",
                    None,
                ));
            }
            slots[object_id] = pack_typed_location(segment_index, row)?;
        }
    }
    let directory_len = collections
        .len()
        .checked_mul(LOCATOR_DIRECTORY_LEN_V4)
        .ok_or_else(|| OQLError::storage_error("typed locator directory overflow", None))?;
    let slot_count = collections.values().try_fold(0usize, |total, slots| {
        total
            .checked_add(slots.len())
            .ok_or_else(|| OQLError::storage_error("typed locator slot count overflow", None))
    })?;
    let fences_len = fences.iter().try_fold(0usize, |total, (_, first, last)| {
        total
            .checked_add(16)
            .and_then(|length| length.checked_add(first.len()))
            .and_then(|length| length.checked_add(last.len()))
            .ok_or_else(|| OQLError::storage_error("typed locator fence overflow", None))
    })?;
    let mut page_region = Vec::new();
    let mut directory = vec![0u8; directory_len];
    for (index, (collection_id, slots)) in collections.iter().enumerate() {
        let entry = index * LOCATOR_DIRECTORY_LEN_V4;
        put_u16(&mut directory, entry, *collection_id);
        put_u32(
            &mut directory,
            entry + 4,
            as_u32(slots.len(), "typed locator collection slots")?,
        );
        put_u64(&mut directory, entry + 8, page_region.len() as u64);
        put_u32(
            &mut directory,
            entry + 16,
            as_u32(
                slots.len().div_ceil(LOCATOR_PAGE_SLOTS),
                "typed locator collection pages",
            )?,
        );
        for page in slots.chunks(LOCATOR_PAGE_SLOTS) {
            let header = page_region.len();
            page_region.resize(header + LOCATOR_PAGE_HEADER_LEN, 0);
            put_u16(
                &mut page_region,
                header + 4,
                u16::try_from(page.len()).map_err(|_| {
                    OQLError::storage_error("typed locator page has too many slots", None)
                })?,
            );
            let payload = page_region.len();
            page_region.resize(payload + page.len() * 4, 0);
            for (slot, packed) in page.iter().copied().enumerate() {
                put_u32(&mut page_region, payload + slot * 4, packed);
            }
            let checksum = crc32fast::hash(&page_region[payload..]);
            put_u32(&mut page_region, header, checksum);
        }
    }
    let total_len = LOCATOR_HEADER_LEN
        .checked_add(directory_len)
        .and_then(|length| length.checked_add(page_region.len()))
        .and_then(|length| length.checked_add(fences_len))
        .ok_or_else(|| OQLError::storage_error("typed locator length overflow", None))?;
    let mut encoded = vec![0u8; LOCATOR_HEADER_LEN];
    encoded[0..4].copy_from_slice(&LOCATOR_MAGIC);
    put_u16(&mut encoded, 4, LOCATOR_VERSION);
    put_u16(&mut encoded, 6, LOCATOR_HEADER_LEN as u16);
    put_u32(
        &mut encoded,
        8,
        as_u32(collections.len(), "typed locator collections")?,
    );
    let live_count = collections
        .values()
        .map(|slots| slots.iter().filter(|packed| **packed != 0).count())
        .sum::<usize>();
    put_u32(
        &mut encoded,
        12,
        as_u32(live_count, "typed locator live rows")?,
    );
    put_u64(&mut encoded, 16, slot_count as u64);
    put_u32(
        &mut encoded,
        28,
        as_u32(fences.len(), "typed locator fences")?,
    );
    put_u64(&mut encoded, 32, page_region.len() as u64);
    encoded.extend_from_slice(&directory);
    encoded.extend_from_slice(&page_region);
    for (segment_index, first, last) in fences {
        let fence_offset = encoded.len();
        encoded.resize(fence_offset + 16, 0);
        put_u32(
            &mut encoded,
            fence_offset,
            as_u32(segment_index, "typed locator fence segment")?,
        );
        put_u32(
            &mut encoded,
            fence_offset + 4,
            as_u32(first.len(), "typed locator first fence")?,
        );
        put_u32(
            &mut encoded,
            fence_offset + 8,
            as_u32(last.len(), "typed locator last fence")?,
        );
        encoded.extend_from_slice(first.as_bytes());
        encoded.extend_from_slice(last.as_bytes());
    }
    debug_assert_eq!(encoded.len(), total_len);
    let mut root_checksum = crc32fast::Hasher::new();
    root_checksum.update(&encoded[LOCATOR_HEADER_LEN..LOCATOR_HEADER_LEN + directory_len]);
    root_checksum.update(&encoded[LOCATOR_HEADER_LEN + directory_len + page_region.len()..]);
    let checksum = root_checksum.finalize();
    put_u32(&mut encoded, 24, checksum);
    Ok(encoded)
}

fn prepare_typed_locator<'a>(
    filesystem: &'a dyn FileSystem,
    path: &Path,
    sequence: u64,
    segments: &[Arc<[u8]>],
) -> OQLResult<PreparedTypedLocator<'a>> {
    let mut collection_slots = BTreeMap::<u16, usize>::new();
    let mut fences = Vec::<(usize, String, String)>::new();
    let mut live_count = 0usize;
    for (segment_index, encoded) in segments.iter().enumerate() {
        let segment = TypedSegment::decode(encoded)?;
        if !segment.authoritative {
            continue;
        }
        if let (Some(first), Some(last)) =
            (segment.document_ids.first(), segment.document_ids.last())
        {
            fences.push((segment_index, first.clone(), last.clone()));
        }
        for object_ref in segment.object_refs.iter().copied() {
            let slots = collection_slots.entry(segment.collection_id).or_default();
            *slots = (*slots).max(object_ref.object_id() as usize + 1);
            live_count = live_count
                .checked_add(1)
                .ok_or_else(|| OQLError::storage_error("typed locator live rows overflow", None))?;
        }
    }
    let initial_run_count = live_count.div_ceil(LOCATOR_SORT_CHUNK_ENTRIES);
    write_locator_build_journal(filesystem, path, sequence, initial_run_count)?;
    let journal_path = locator_journal_path(path);
    let pages_path = locator_temp_path(path, sequence, "pages", 0);
    let mut run_cleanup = LocatorRunCleanup {
        filesystem,
        paths: vec![journal_path.clone(), pages_path.clone()],
    };
    let mut chunk = Vec::<LocatorSortEntry>::with_capacity(LOCATOR_SORT_CHUNK_ENTRIES);
    let mut runs = Vec::<PathBuf>::with_capacity(initial_run_count);
    for (segment_index, encoded) in segments.iter().enumerate() {
        let segment = TypedSegment::decode(encoded)?;
        if !segment.authoritative {
            continue;
        }
        for (row, object_ref) in segment.object_refs.iter().copied().enumerate() {
            chunk.push(LocatorSortEntry {
                collection_id: segment.collection_id,
                object_id: object_ref.object_id(),
                packed: pack_typed_location(segment_index, row)?,
            });
            if chunk.len() == LOCATOR_SORT_CHUNK_ENTRIES {
                let run = locator_temp_path(path, sequence, "run", runs.len());
                if let Err(error) = write_locator_run(filesystem, &run, &mut chunk) {
                    for previous in &runs {
                        let _ = filesystem.remove(previous);
                    }
                    let _ = filesystem.remove(&run);
                    return Err(error);
                }
                run_cleanup.paths.push(run.clone());
                runs.push(run);
            }
        }
    }
    if !chunk.is_empty() {
        let run = locator_temp_path(path, sequence, "run", runs.len());
        if let Err(error) = write_locator_run(filesystem, &run, &mut chunk) {
            for previous in &runs {
                let _ = filesystem.remove(previous);
            }
            let _ = filesystem.remove(&run);
            return Err(error);
        }
        run_cleanup.paths.push(run.clone());
        runs.push(run);
    }
    debug_assert_eq!(initial_run_count, runs.len());
    let final_run = if runs.is_empty() {
        None
    } else {
        match consolidate_locator_runs(filesystem, path, sequence, runs.clone()) {
            Ok(run) => {
                run_cleanup.paths.push(run.clone());
                Some(run)
            }
            Err(error) => {
                for run in runs {
                    if filesystem.exists(&run) {
                        let _ = filesystem.remove(&run);
                    }
                }
                return Err(error);
            }
        }
    };

    let directory_len = collection_slots
        .len()
        .checked_mul(LOCATOR_DIRECTORY_LEN_V4)
        .ok_or_else(|| OQLError::storage_error("typed locator directory overflow", None))?;
    let mut directory = vec![0u8; directory_len];
    let build_pages = (|| -> OQLResult<usize> {
        let mut writer = filesystem.create(&pages_path).map_err(|error| {
            OQLError::storage_error(&format!("create locator page spool: {error}"), None)
        })?;
        let mut reader = final_run
            .as_ref()
            .map(|run| LocatorRunReader::open(filesystem, run))
            .transpose()?;
        let mut next = reader
            .as_mut()
            .map(LocatorRunReader::next)
            .transpose()?
            .flatten();
        let mut last_key = None::<(u16, u32)>;
        let mut pages_len = 0usize;
        for (index, (collection_id, slots)) in collection_slots.iter().enumerate() {
            let entry = index * LOCATOR_DIRECTORY_LEN_V4;
            put_u16(&mut directory, entry, *collection_id);
            put_u32(
                &mut directory,
                entry + 4,
                as_u32(*slots, "typed locator collection slots")?,
            );
            put_u64(&mut directory, entry + 8, pages_len as u64);
            put_u32(
                &mut directory,
                entry + 16,
                as_u32(
                    slots.div_ceil(LOCATOR_PAGE_SLOTS),
                    "typed locator collection pages",
                )?,
            );
            for page_index in 0..slots.div_ceil(LOCATOR_PAGE_SLOTS) {
                let first_object = page_index * LOCATOR_PAGE_SLOTS;
                let page_slots = LOCATOR_PAGE_SLOTS.min(slots - first_object);
                let mut page = vec![0u8; page_slots * 4];
                while next.is_some_and(|candidate| {
                    candidate.collection_id == *collection_id
                        && (candidate.object_id as usize) < first_object + page_slots
                }) {
                    let candidate = next.take().expect("locator candidate exists");
                    if (candidate.object_id as usize) < first_object
                        || last_key == Some((candidate.collection_id, candidate.object_id))
                    {
                        return Err(OQLError::storage_error(
                            "typed locator contains a duplicate or unordered object reference",
                            None,
                        ));
                    }
                    put_u32(
                        &mut page,
                        (candidate.object_id as usize - first_object) * 4,
                        candidate.packed,
                    );
                    last_key = Some((candidate.collection_id, candidate.object_id));
                    next = reader
                        .as_mut()
                        .map(LocatorRunReader::next)
                        .transpose()?
                        .flatten();
                }
                let mut header = [0u8; LOCATOR_PAGE_HEADER_LEN];
                put_u32(&mut header, 0, crc32fast::hash(&page));
                put_u16(
                    &mut header,
                    4,
                    u16::try_from(page_slots).map_err(|_| {
                        OQLError::storage_error("typed locator page has too many slots", None)
                    })?,
                );
                write_all_handle(writer.as_mut(), &header)?;
                write_all_handle(writer.as_mut(), &page)?;
                pages_len = pages_len
                    .checked_add(header.len() + page.len())
                    .ok_or_else(|| {
                        OQLError::storage_error("typed locator page region overflow", None)
                    })?;
            }
        }
        if next.is_some() {
            return Err(OQLError::storage_error(
                "typed locator sort contains an unknown collection",
                None,
            ));
        }
        writer.sync().map_err(|error| {
            OQLError::storage_error(&format!("sync locator page spool: {error}"), None)
        })?;
        Ok(pages_len)
    })();
    if let Some(run) = &final_run {
        let _ = filesystem.remove(run);
    }
    let pages_len = match build_pages {
        Ok(length) => length,
        Err(error) => {
            let _ = filesystem.remove(&pages_path);
            return Err(error);
        }
    };

    let mut fence_bytes = Vec::new();
    for (segment_index, first, last) in &fences {
        let start = fence_bytes.len();
        fence_bytes.resize(start + 16, 0);
        put_u32(
            &mut fence_bytes,
            start,
            as_u32(*segment_index, "typed locator fence segment")?,
        );
        put_u32(
            &mut fence_bytes,
            start + 4,
            as_u32(first.len(), "typed locator first fence")?,
        );
        put_u32(
            &mut fence_bytes,
            start + 8,
            as_u32(last.len(), "typed locator last fence")?,
        );
        fence_bytes.extend_from_slice(first.as_bytes());
        fence_bytes.extend_from_slice(last.as_bytes());
    }
    let slot_count = collection_slots.values().try_fold(0usize, |total, slots| {
        total
            .checked_add(*slots)
            .ok_or_else(|| OQLError::storage_error("typed locator slot count overflow", None))
    })?;
    let mut root = vec![0u8; LOCATOR_HEADER_LEN];
    root[0..4].copy_from_slice(&LOCATOR_MAGIC);
    put_u16(&mut root, 4, LOCATOR_VERSION);
    put_u16(&mut root, 6, LOCATOR_HEADER_LEN as u16);
    put_u32(
        &mut root,
        8,
        as_u32(collection_slots.len(), "typed locator collections")?,
    );
    put_u32(
        &mut root,
        12,
        as_u32(live_count, "typed locator live rows")?,
    );
    put_u64(&mut root, 16, slot_count as u64);
    put_u32(&mut root, 28, as_u32(fences.len(), "typed locator fences")?);
    put_u64(&mut root, 32, pages_len as u64);
    let mut root_checksum = crc32fast::Hasher::new();
    root_checksum.update(&directory);
    root_checksum.update(&fence_bytes);
    put_u32(&mut root, 24, root_checksum.finalize());
    root.extend_from_slice(&directory);
    tracing::debug!(
        target: "owl_db::storage_engine::checkpoint",
        live_rows = live_count,
        locator_slots = slot_count,
        initial_runs = initial_run_count,
        sort_chunk_entries = LOCATOR_SORT_CHUNK_ENTRIES,
        merge_fan_in = LOCATOR_SORT_MERGE_FAN_IN,
        page_spool_bytes = pages_len,
        "prepared bounded typed locator"
    );
    run_cleanup
        .paths
        .retain(|candidate| candidate != &pages_path && candidate != &journal_path);
    Ok(PreparedTypedLocator {
        filesystem,
        root,
        pages_path,
        journal_path,
        pages_len,
        fences: fence_bytes,
    })
}

fn decode_typed_locator(bytes: Arc<[u8]>) -> OQLResult<TypedLocatorLeaf> {
    if bytes.len() < LOCATOR_HEADER_LEN_LEGACY || bytes[0..4] != LOCATOR_MAGIC {
        return Err(OQLError::storage_error(
            "typed locator header or checksum is invalid",
            None,
        ));
    }
    let version = get_u16(&bytes, 4)?;
    let header_len = if version >= LOCATOR_VERSION {
        LOCATOR_HEADER_LEN
    } else {
        LOCATOR_HEADER_LEN_LEGACY
    };
    if !matches!(
        version,
        LOCATOR_VERSION_V1 | LOCATOR_VERSION_V2 | LOCATOR_VERSION_V3 | LOCATOR_VERSION
    ) || get_u16(&bytes, 6)? as usize != header_len
        || (version < LOCATOR_VERSION
            && crc32fast::hash(&bytes[header_len..]) != get_u32(&bytes, 24)?)
    {
        return Err(OQLError::storage_error(
            "typed locator header or checksum is invalid",
            None,
        ));
    }
    let collection_count = get_u32(&bytes, 8)? as usize;
    let slot_width = if version >= LOCATOR_VERSION_V3 { 4 } else { 8 };
    let page_slots = if version >= LOCATOR_VERSION {
        LOCATOR_PAGE_SLOTS
    } else {
        0
    };
    let live = get_u32(&bytes, 12)? as usize;
    let slot_count = usize::try_from(get_u64(&bytes, 16)?)
        .map_err(|_| OQLError::storage_error("typed locator slot count overflows", None))?;
    if live > slot_count {
        return Err(OQLError::storage_error(
            "typed locator live count is invalid",
            None,
        ));
    }
    let directory_len = if version >= LOCATOR_VERSION {
        LOCATOR_DIRECTORY_LEN_V4
    } else {
        LOCATOR_DIRECTORY_LEN
    };
    let slots_start = header_len
        .checked_add(
            collection_count
                .checked_mul(directory_len)
                .ok_or_else(|| OQLError::storage_error("typed locator directory overflow", None))?,
        )
        .ok_or_else(|| OQLError::storage_error("typed locator directory overflow", None))?;
    let page_region_len = if version >= LOCATOR_VERSION {
        usize::try_from(get_u64(&bytes, 32)?)
            .map_err(|_| OQLError::storage_error("typed locator page region overflows", None))?
    } else {
        slot_count.checked_mul(slot_width).unwrap_or(usize::MAX)
    };
    let fences_start = slots_start
        .checked_add(page_region_len)
        .ok_or_else(|| OQLError::storage_error("typed locator slot region overflows", None))?;
    if fences_start > bytes.len() || (version == LOCATOR_VERSION_V1 && fences_start != bytes.len())
    {
        return Err(OQLError::storage_error(
            "typed locator length is invalid",
            None,
        ));
    }
    if version >= LOCATOR_VERSION {
        let mut root_checksum = crc32fast::Hasher::new();
        root_checksum.update(&bytes[header_len..slots_start]);
        root_checksum.update(&bytes[fences_start..]);
        if root_checksum.finalize() != get_u32(&bytes, 24)? {
            return Err(OQLError::storage_error(
                "typed locator root checksum is invalid",
                None,
            ));
        }
    }
    let mut collections = HashMap::with_capacity(collection_count);
    let mut previous = None;
    for index in 0..collection_count {
        let entry = header_len + index * directory_len;
        let collection_id = get_u16(&bytes, entry)?;
        let slots = get_u32(&bytes, entry + 4)? as usize;
        let relative = usize::try_from(get_u64(&bytes, entry + 8)?)
            .map_err(|_| OQLError::storage_error("typed locator offset overflows", None))?;
        let byte_offset = if version >= LOCATOR_VERSION {
            let pages = get_u32(&bytes, entry + 16)? as usize;
            if pages != slots.div_ceil(LOCATOR_PAGE_SLOTS) || relative > page_region_len {
                return Err(OQLError::storage_error(
                    "typed locator page directory is invalid",
                    None,
                ));
            }
            slots_start + relative
        } else {
            if relative
                .checked_add(slots)
                .is_none_or(|end| end > slot_count)
            {
                return Err(OQLError::storage_error(
                    "typed locator collection directory is invalid",
                    None,
                ));
            }
            slots_start + relative * slot_width
        };
        if collection_id == 0
            || previous.is_some_and(|value| value >= collection_id)
            || collections
                .insert(collection_id, (byte_offset, slots))
                .is_some()
        {
            return Err(OQLError::storage_error(
                "typed locator collection directory is invalid",
                None,
            ));
        }
        previous = Some(collection_id);
    }
    if version >= LOCATOR_VERSION {
        for (start, slots) in collections.values().copied() {
            for page in 0..slots.div_ceil(LOCATOR_PAGE_SLOTS) {
                let expected_slots = LOCATOR_PAGE_SLOTS.min(slots - page * LOCATOR_PAGE_SLOTS);
                let header = start + page * (LOCATOR_PAGE_HEADER_LEN + LOCATOR_PAGE_SLOTS * 4);
                let payload = header + LOCATOR_PAGE_HEADER_LEN;
                let payload_end = payload
                    .checked_add(expected_slots * 4)
                    .filter(|end| *end <= fences_start)
                    .ok_or_else(|| {
                        OQLError::storage_error("typed locator page is truncated", None)
                    })?;
                if get_u16(&bytes, header + 4)? as usize != expected_slots
                    || crc32fast::hash(&bytes[payload..payload_end]) != get_u32(&bytes, header)?
                {
                    return Err(OQLError::storage_error(
                        "typed locator page checksum is invalid",
                        None,
                    ));
                }
            }
        }
    }
    let fence_count = if version >= LOCATOR_VERSION_V2 {
        get_u32(&bytes, 28)? as usize
    } else {
        0
    };
    let mut fences = Vec::with_capacity(fence_count);
    let mut cursor = fences_start;
    let mut previous_segment = None;
    for _ in 0..fence_count {
        let segment_index = get_u32(&bytes, cursor)? as usize;
        let first_len = get_u32(&bytes, cursor + 4)? as usize;
        let last_len = get_u32(&bytes, cursor + 8)? as usize;
        cursor = cursor
            .checked_add(16)
            .ok_or_else(|| OQLError::storage_error("typed locator fence overflows", None))?;
        let first_end = cursor
            .checked_add(first_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                OQLError::storage_error("typed locator first fence is truncated", None)
            })?;
        let last_end = first_end
            .checked_add(last_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                OQLError::storage_error("typed locator last fence is truncated", None)
            })?;
        let first = std::str::from_utf8(&bytes[cursor..first_end])
            .map_err(|_| OQLError::storage_error("typed locator first fence is not UTF-8", None))?
            .to_owned();
        let last = std::str::from_utf8(&bytes[first_end..last_end])
            .map_err(|_| OQLError::storage_error("typed locator last fence is not UTF-8", None))?
            .to_owned();
        if first.is_empty()
            || first > last
            || previous_segment.is_some_and(|previous| previous >= segment_index)
        {
            return Err(OQLError::storage_error(
                "typed locator fence directory is invalid",
                None,
            ));
        }
        fences.push(TypedLocatorFence {
            segment_index,
            first,
            last,
        });
        previous_segment = Some(segment_index);
        cursor = last_end;
    }
    if cursor != bytes.len() {
        return Err(OQLError::storage_error(
            "typed locator has trailing fence bytes",
            None,
        ));
    }
    Ok(TypedLocatorLeaf {
        collections,
        storage: TypedLocatorStorage::Resident(bytes),
        slot_width,
        page_slots,
        len: live,
        fences,
    })
}

fn open_typed_locator(
    filesystem: &dyn FileSystem,
    path: &Path,
    offset: usize,
    length: usize,
) -> OQLResult<TypedLocatorLeaf> {
    let header_prefix = read_file_range(
        filesystem,
        path,
        offset,
        LOCATOR_HEADER_LEN_LEGACY.min(length),
    )?;
    if header_prefix.len() < LOCATOR_HEADER_LEN_LEGACY || header_prefix[0..4] != LOCATOR_MAGIC {
        return Err(OQLError::storage_error(
            "typed locator header is invalid",
            None,
        ));
    }
    let version = get_u16(&header_prefix, 4)?;
    if version < LOCATOR_VERSION {
        return decode_typed_locator(Arc::from(read_file_range(
            filesystem, path, offset, length,
        )?));
    }
    let header = read_file_range(filesystem, path, offset, LOCATOR_HEADER_LEN)?;
    if version != LOCATOR_VERSION
        || get_u16(&header, 6)? as usize != LOCATOR_HEADER_LEN
        || length < LOCATOR_HEADER_LEN
    {
        return Err(OQLError::storage_error(
            "typed locator header is invalid",
            None,
        ));
    }
    let collection_count = get_u32(&header, 8)? as usize;
    let live = get_u32(&header, 12)? as usize;
    let slot_count = usize::try_from(get_u64(&header, 16)?)
        .map_err(|_| OQLError::storage_error("typed locator slot count overflows", None))?;
    if live > slot_count {
        return Err(OQLError::storage_error(
            "typed locator live count is invalid",
            None,
        ));
    }
    let directory_len = collection_count
        .checked_mul(LOCATOR_DIRECTORY_LEN_V4)
        .ok_or_else(|| OQLError::storage_error("typed locator directory overflow", None))?;
    let page_region_len = usize::try_from(get_u64(&header, 32)?)
        .map_err(|_| OQLError::storage_error("typed locator page region overflows", None))?;
    let pages_start = LOCATOR_HEADER_LEN
        .checked_add(directory_len)
        .ok_or_else(|| OQLError::storage_error("typed locator directory overflow", None))?;
    let fences_start = pages_start
        .checked_add(page_region_len)
        .filter(|start| *start <= length)
        .ok_or_else(|| OQLError::storage_error("typed locator page region is truncated", None))?;
    let directory = read_file_range(filesystem, path, offset + LOCATOR_HEADER_LEN, directory_len)?;
    let fence_bytes = read_file_range(
        filesystem,
        path,
        offset + fences_start,
        length - fences_start,
    )?;
    let mut root_checksum = crc32fast::Hasher::new();
    root_checksum.update(&directory);
    root_checksum.update(&fence_bytes);
    if root_checksum.finalize() != get_u32(&header, 24)? {
        return Err(OQLError::storage_error(
            "typed locator root checksum is invalid",
            None,
        ));
    }

    let mut collections = HashMap::with_capacity(collection_count);
    let mut previous = None;
    let mut described_slots = 0usize;
    for index in 0..collection_count {
        let entry = index * LOCATOR_DIRECTORY_LEN_V4;
        let collection_id = get_u16(&directory, entry)?;
        let slots = get_u32(&directory, entry + 4)? as usize;
        let relative = usize::try_from(get_u64(&directory, entry + 8)?)
            .map_err(|_| OQLError::storage_error("typed locator offset overflows", None))?;
        let pages = get_u32(&directory, entry + 16)? as usize;
        let expected_pages = slots.div_ceil(LOCATOR_PAGE_SLOTS);
        let last_slots = if slots == 0 {
            0
        } else {
            slots - (expected_pages - 1) * LOCATOR_PAGE_SLOTS
        };
        let encoded_len = if expected_pages == 0 {
            0
        } else {
            (expected_pages - 1)
                .checked_mul(LOCATOR_PAGE_HEADER_LEN + LOCATOR_PAGE_SLOTS * 4)
                .and_then(|bytes| bytes.checked_add(LOCATOR_PAGE_HEADER_LEN + last_slots * 4))
                .ok_or_else(|| OQLError::storage_error("typed locator pages overflow", None))?
        };
        if collection_id == 0
            || previous.is_some_and(|value| value >= collection_id)
            || pages != expected_pages
            || relative
                .checked_add(encoded_len)
                .is_none_or(|end| end > page_region_len)
            || collections
                .insert(collection_id, (pages_start + relative, slots))
                .is_some()
        {
            return Err(OQLError::storage_error(
                "typed locator collection directory is invalid",
                None,
            ));
        }
        described_slots = described_slots
            .checked_add(slots)
            .ok_or_else(|| OQLError::storage_error("typed locator slot count overflow", None))?;
        previous = Some(collection_id);
    }
    if described_slots != slot_count {
        return Err(OQLError::storage_error(
            "typed locator slot count conflicts with its directory",
            None,
        ));
    }

    let fence_count = get_u32(&header, 28)? as usize;
    let mut fences = Vec::with_capacity(fence_count);
    let mut cursor = 0usize;
    let mut previous_segment = None;
    for _ in 0..fence_count {
        let segment_index = get_u32(&fence_bytes, cursor)? as usize;
        let first_len = get_u32(&fence_bytes, cursor + 4)? as usize;
        let last_len = get_u32(&fence_bytes, cursor + 8)? as usize;
        cursor = cursor
            .checked_add(16)
            .ok_or_else(|| OQLError::storage_error("typed locator fence overflows", None))?;
        let first_end = cursor
            .checked_add(first_len)
            .filter(|end| *end <= fence_bytes.len())
            .ok_or_else(|| {
                OQLError::storage_error("typed locator first fence is truncated", None)
            })?;
        let last_end = first_end
            .checked_add(last_len)
            .filter(|end| *end <= fence_bytes.len())
            .ok_or_else(|| {
                OQLError::storage_error("typed locator last fence is truncated", None)
            })?;
        let first = std::str::from_utf8(&fence_bytes[cursor..first_end])
            .map_err(|_| OQLError::storage_error("typed locator first fence is not UTF-8", None))?
            .to_owned();
        let last = std::str::from_utf8(&fence_bytes[first_end..last_end])
            .map_err(|_| OQLError::storage_error("typed locator last fence is not UTF-8", None))?
            .to_owned();
        if first.is_empty()
            || first > last
            || previous_segment.is_some_and(|previous| previous >= segment_index)
        {
            return Err(OQLError::storage_error(
                "typed locator fence directory is invalid",
                None,
            ));
        }
        fences.push(TypedLocatorFence {
            segment_index,
            first,
            last,
        });
        previous_segment = Some(segment_index);
        cursor = last_end;
    }
    if cursor != fence_bytes.len() {
        return Err(OQLError::storage_error(
            "typed locator has trailing fence bytes",
            None,
        ));
    }
    let file = filesystem
        .open(path)
        .map(Arc::<dyn FileHandle>::from)
        .map_err(|error| {
            OQLError::storage_error(&format!("open typed locator pages: {error}"), None)
        })?;
    Ok(TypedLocatorLeaf {
        collections,
        storage: TypedLocatorStorage::File {
            file,
            base_offset: offset,
            cache: Arc::new(Mutex::new(LocatorPageCache::default())),
        },
        slot_width: 4,
        page_slots: LOCATOR_PAGE_SLOTS,
        len: live,
        fences,
    })
}

pub(crate) fn write_segment_file(
    filesystem: &dyn FileSystem,
    path: &Path,
    image: &CheckpointImage,
    max_size_bytes: usize,
) -> OQLResult<SegmentFileInfo> {
    let metadata = bincode::serialize(&image.metadata).map_err(|error| {
        OQLError::serialization_error(&format!("cannot encode checkpoint metadata: {error}"), None)
    })?;
    let mut records = image.records.iter().collect::<Vec<_>>();
    records.sort_by_key(|(object_ref, _)| object_ref.packed());
    let records_len = records.iter().try_fold(0usize, |total, (_, record)| {
        total
            .checked_add(record.len())
            .ok_or_else(|| OQLError::storage_error("checkpoint record region overflow", None))
    })?;
    let directory_len = records
        .len()
        .checked_mul(DIRECTORY_ENTRY_LEN)
        .ok_or_else(|| OQLError::storage_error("checkpoint directory overflow", None))?;
    let mut ordered_indexes = image.indexes.iter().collect::<Vec<_>>();
    ordered_indexes
        .sort_by(|left, right| (&left.schema, &left.name).cmp(&(&right.schema, &right.name)));
    let mut index_pages = Vec::new();
    let mut indexes_len = 0usize;
    for index in ordered_indexes {
        let ranges = plan_index_pages(index)?;
        let last_page = ranges.len() - 1;
        for (page, range) in ranges.into_iter().enumerate() {
            indexes_len = indexes_len
                .checked_add(encoded_index_page_len(index, range.clone())?)
                .ok_or_else(|| OQLError::storage_error("checkpoint index region overflow", None))?;
            index_pages.push((index, range, page == 0, page == last_page));
        }
    }
    let typed_segments_len = image
        .typed_segments
        .iter()
        .try_fold(0usize, |total, segment| total.checked_add(segment.len()))
        .ok_or_else(|| OQLError::storage_error("typed segment region overflow", None))?;
    let typed_locator =
        prepare_typed_locator(filesystem, path, image.sequence, &image.typed_segments)?;
    let total_len = SEGMENT_HEADER_LEN
        .checked_add(metadata.len())
        .and_then(|length| length.checked_add(directory_len))
        .and_then(|length| length.checked_add(records_len))
        .and_then(|length| length.checked_add(indexes_len))
        .and_then(|length| length.checked_add(typed_locator.len()))
        .and_then(|length| length.checked_add(typed_segments_len))
        .ok_or_else(|| OQLError::storage_error("checkpoint segment length overflow", None))?;
    let typed_segments_start = total_len - typed_segments_len;
    let mut typed_offset = typed_segments_start;
    let typed_segment_descriptors = image
        .typed_segments
        .iter()
        .map(|segment| {
            let descriptor = TypedSegment::descriptor(segment)?;
            let location = TypedSegmentFileDescriptor {
                offset: typed_offset,
                descriptor,
            };
            typed_offset = typed_offset
                .checked_add(descriptor.encoded_len)
                .ok_or_else(|| {
                    OQLError::storage_error("typed segment descriptor offset overflow", None)
                })?;
            Ok(location)
        })
        .collect::<OQLResult<Vec<_>>>()?;
    if typed_offset != total_len {
        return Err(OQLError::storage_error(
            "typed segment descriptors do not cover their output region",
            None,
        ));
    }
    if max_size_bytes > 0 && total_len > max_size_bytes {
        return Err(OQLError::storage_error(
            "checkpoint exceeds configured storage size limit",
            None,
        ));
    }
    let mut header = [0; SEGMENT_HEADER_LEN];
    header[0..4].copy_from_slice(&SEGMENT_MAGIC);
    put_u16(&mut header, 4, SEGMENT_VERSION);
    put_u16(&mut header, 6, SEGMENT_HEADER_LEN as u16);
    put_u64(&mut header, 8, image.sequence);
    put_u64(&mut header, 16, image.checkpoint_lsn);
    put_u32(
        &mut header,
        24,
        as_u32(metadata.len(), "checkpoint metadata")?,
    );
    put_u32(
        &mut header,
        28,
        as_u32(records.len(), "checkpoint object count")?,
    );
    put_u32(
        &mut header,
        32,
        as_u32(directory_len, "checkpoint directory")?,
    );
    put_u32(
        &mut header,
        36,
        as_u32(index_pages.len(), "checkpoint index page count")?,
    );
    put_u64(&mut header, 40, records_len as u64);
    put_u64(&mut header, 48, total_len as u64);
    put_u64(&mut header, 60, typed_segments_len as u64);
    put_u64(&mut header, 72, typed_locator.len() as u64);

    let mut directory = vec![0; directory_len];
    let mut record_offset = 0usize;
    for (index, (object_ref, record)) in records.iter().enumerate() {
        let entry = index * DIRECTORY_ENTRY_LEN;
        put_u64(&mut directory, entry, object_ref.packed());
        put_u64(&mut directory, entry + 8, record_offset as u64);
        put_u32(
            &mut directory,
            entry + 16,
            as_u32(record.len(), "object record")?,
        );
        put_u32(&mut directory, entry + 20, crc32fast::hash(record));
        record_offset += record.len();
    }
    let root_checksum = checkpoint_root_checksum(&header, &[&metadata, &directory]);
    put_u32(&mut header, 68, root_checksum);

    let mut file = filesystem.create(path).map_err(|error| {
        OQLError::storage_error(&format!("create checkpoint segment: {error}"), None)
    })?;
    let mut internal = crc32fast::Hasher::new();
    for bytes in [&header[..], &metadata, &directory] {
        write_all_handle(file.as_mut(), bytes)?;
        internal.update(bytes);
    }
    for (_, record) in records {
        write_all_handle(file.as_mut(), record)?;
        internal.update(record);
    }
    let mut published_indexes = Vec::<PublishedIndexPages>::new();
    let mut index_offset = SEGMENT_HEADER_LEN + metadata.len() + directory_len + records_len;
    for (index, range, first, last) in index_pages {
        let first_key = index.entries[range.clone()]
            .first()
            .map(|entry| entry.0.clone());
        let last_key = index.entries[range.clone()]
            .last()
            .map(|entry| entry.0.clone());
        let block = encode_index_page(index, range, first, last)?;
        write_all_handle(file.as_mut(), &block)?;
        internal.update(&block);
        if first {
            published_indexes.push(PublishedIndexPages {
                schema: index.schema.clone(),
                name: index.name.clone(),
                unique: index.unique,
                pages: Vec::new(),
            });
        }
        published_indexes
            .last_mut()
            .ok_or_else(|| OQLError::storage_error("index page has no published chain", None))?
            .pages
            .push(IndexPageDescriptor {
                location: IndexPageLocation {
                    path: path.to_path_buf(),
                    offset: index_offset,
                    len: block.len(),
                },
                first_key,
                last_key,
                // A newly published base must read through the durable file
                // at least once so post-publication corruption is not hidden
                // by the writer's in-memory source entries.
                cached_entries: None,
            });
        index_offset += block.len();
    }
    typed_locator.write_to(file.as_mut(), &mut internal)?;
    for segment in &image.typed_segments {
        write_all_handle(file.as_mut(), segment)?;
        internal.update(segment);
    }
    let internal_checksum = internal.finalize();
    file.seek(SeekFrom::Start(56)).map_err(|error| {
        OQLError::storage_error(&format!("seek checkpoint checksum: {error}"), None)
    })?;
    write_all_handle(file.as_mut(), &internal_checksum.to_le_bytes())?;
    file.flush().and_then(|_| file.sync()).map_err(|error| {
        OQLError::storage_error(&format!("sync checkpoint segment: {error}"), None)
    })?;
    filesystem.sync_all().map_err(|error| {
        OQLError::storage_error(&format!("sync checkpoint directory: {error}"), None)
    })?;
    Ok(SegmentFileInfo {
        len: total_len as u64,
        // Format 6 manifests authenticate the same checksum embedded in the
        // segment header. The writer computed it while streaming output, so
        // publication does not reread the complete immutable file.
        checksum: internal_checksum,
        indexes: published_indexes,
        typed_locator: open_typed_locator(
            filesystem,
            path,
            typed_segments_start - typed_locator.len(),
            typed_locator.len(),
        )?,
        typed_segments: typed_segment_descriptors,
    })
}

pub(crate) fn decode_segment_file(
    filesystem: &dyn FileSystem,
    path: &Path,
    expected_len: u64,
    expected_checksum: u32,
    verification: SegmentVerification,
) -> OQLResult<DecodedCheckpoint> {
    let file_len = usize::try_from(expected_len)
        .map_err(|_| OQLError::storage_error("checkpoint segment is too large", None))?;
    if filesystem.file_size(path).map_err(|error| {
        OQLError::storage_error(&format!("inspect checkpoint segment: {error}"), None)
    })? != expected_len
    {
        return Err(OQLError::storage_error(
            "published checkpoint segment length does not match its manifest",
            None,
        ));
    }
    let header_prefix = read_file_range(filesystem, path, 0, 8)?;
    let segment_version = get_u16(&header_prefix, 4)?;
    let header_len = get_u16(&header_prefix, 6)? as usize;
    let expected_header_len = match segment_version {
        2 => SEGMENT_HEADER_LEN_V2,
        SEGMENT_VERSION_V3 | SEGMENT_VERSION_V4 => SEGMENT_HEADER_LEN_V4,
        SEGMENT_VERSION_V5 | SEGMENT_VERSION => SEGMENT_HEADER_LEN,
        _ => 0,
    };
    if header_prefix[0..4] != SEGMENT_MAGIC || header_len != expected_header_len {
        return Err(OQLError::storage_error(
            "published checkpoint segment header conflicts with its manifest",
            None,
        ));
    }
    let header = read_file_range(filesystem, path, 0, header_len)?;
    if header[0..4] != SEGMENT_MAGIC || get_u64(&header, 48)? != expected_len {
        return Err(OQLError::storage_error(
            "published checkpoint segment header conflicts with its manifest",
            None,
        ));
    }
    if verification == SegmentVerification::Audit {
        let (checksum, internal_checksum) = stream_segment_checksums(filesystem, path, file_len)?;
        let manifest_checksum = if segment_version >= SEGMENT_VERSION {
            internal_checksum
        } else {
            checksum
        };
        if manifest_checksum != expected_checksum || internal_checksum != get_u32(&header, 56)? {
            return Err(OQLError::storage_error(
                "published checkpoint segment checksum mismatch",
                None,
            ));
        }
    }
    let metadata_len = get_u32(&header, 24)? as usize;
    let entry_count = get_u32(&header, 28)? as usize;
    let directory_len = get_u32(&header, 32)? as usize;
    let index_count = get_u32(&header, 36)? as usize;
    let typed_segments_len = if segment_version >= 3 {
        usize::try_from(get_u64(&header, 60)?)
            .map_err(|_| OQLError::storage_error("typed segment region is too large", None))?
    } else {
        0
    };
    let typed_locator_len = if segment_version >= SEGMENT_VERSION_V5 {
        usize::try_from(get_u64(&header, 72)?)
            .map_err(|_| OQLError::storage_error("typed locator region is too large", None))?
    } else {
        0
    };
    let records_len = usize::try_from(get_u64(&header, 40)?)
        .map_err(|_| OQLError::storage_error("checkpoint record length overflow", None))?;
    if directory_len
        != entry_count
            .checked_mul(DIRECTORY_ENTRY_LEN)
            .ok_or_else(|| OQLError::storage_error("checkpoint directory length overflow", None))?
    {
        return Err(OQLError::storage_error(
            "checkpoint directory length is invalid",
            None,
        ));
    }
    let metadata_start = header_len;
    let directory_start = metadata_start
        .checked_add(metadata_len)
        .ok_or_else(|| OQLError::storage_error("checkpoint metadata overflow", None))?;
    let records_start = directory_start
        .checked_add(directory_len)
        .ok_or_else(|| OQLError::storage_error("checkpoint directory overflow", None))?;
    let records_end = records_start
        .checked_add(records_len)
        .filter(|end| *end <= file_len)
        .ok_or_else(|| OQLError::storage_error("checkpoint record region is truncated", None))?;
    if typed_segments_len > file_len.saturating_sub(records_end) {
        return Err(OQLError::storage_error(
            "checkpoint typed segment region is truncated",
            None,
        ));
    }
    let typed_segments_start = file_len - typed_segments_len;
    let typed_locator_start = typed_segments_start
        .checked_sub(typed_locator_len)
        .filter(|start| *start >= records_end)
        .ok_or_else(|| {
            OQLError::storage_error("checkpoint typed locator region is truncated", None)
        })?;
    let typed_locator = if typed_locator_len == 0 {
        None
    } else {
        Some(open_typed_locator(
            filesystem,
            path,
            typed_locator_start,
            typed_locator_len,
        )?)
    };
    let typed_segment_descriptors =
        discover_typed_segments(filesystem, path, typed_segments_start, typed_segments_len)?;
    let typed_segments = open_typed_segment_handles(
        filesystem,
        path,
        &typed_segment_descriptors,
        verification == SegmentVerification::Audit,
    )?;
    let mut prefix = read_file_range(
        filesystem,
        path,
        metadata_start,
        metadata_len + directory_len,
    )?;
    if segment_version >= SEGMENT_VERSION_V5
        && checkpoint_root_checksum(&header, &[&prefix]) != get_u32(&header, 68)?
    {
        return Err(OQLError::storage_error(
            "checkpoint root checksum mismatch",
            None,
        ));
    }
    let metadata: CheckpointMetadata =
        bincode::deserialize(&prefix[..metadata_len]).map_err(|error| {
            OQLError::serialization_error(
                &format!("cannot decode checkpoint metadata: {error}"),
                None,
            )
        })?;
    let directory = prefix.split_off(metadata_len);
    Ok(DecodedCheckpoint {
        sequence: get_u64(&header, 8)?,
        checkpoint_lsn: get_u64(&header, 16)?,
        metadata,
        indexes: SegmentIndexReader {
            path: path.to_path_buf(),
            start: records_end,
            len: typed_locator_start - records_end,
            count: index_count,
        },
        records: SegmentRecordReader {
            path: path.to_path_buf(),
            directory,
            entry_count,
            records_start,
            records_len,
        },
        typed_segment_descriptors,
        typed_segments,
        typed_locator,
    })
}

fn discover_typed_segments(
    filesystem: &dyn FileSystem,
    path: &Path,
    start: usize,
    len: usize,
) -> OQLResult<Vec<TypedSegmentFileDescriptor>> {
    const TYPED_HEADER_LEN: usize = 64;
    let end = start
        .checked_add(len)
        .ok_or_else(|| OQLError::storage_error("typed segment region overflow", None))?;
    let mut descriptors = Vec::new();
    let mut offset = start;
    while offset < end {
        let remaining = end - offset;
        if remaining < TYPED_HEADER_LEN {
            return Err(OQLError::storage_error(
                "typed segment region has a truncated descriptor",
                None,
            ));
        }
        let header = read_file_range(filesystem, path, offset, TYPED_HEADER_LEN)?;
        let descriptor = TypedSegment::descriptor(&header)?;
        let next = offset
            .checked_add(descriptor.encoded_len)
            .filter(|next| *next <= end)
            .ok_or_else(|| {
                OQLError::storage_error("typed segment descriptor exceeds its region", None)
            })?;
        descriptors.push(TypedSegmentFileDescriptor { offset, descriptor });
        offset = next;
    }
    Ok(descriptors)
}

pub(crate) struct BaseSegmentHandle {
    file: Mutex<Box<dyn FileHandle>>,
    prefix: Vec<u8>,
    entry_count: usize,
    directory_start: usize,
    records_start: usize,
    records_len: usize,
}

impl BaseSegmentHandle {
    pub(crate) fn open(
        filesystem: Arc<dyn FileSystem>,
        path: &Path,
        manifest: Manifest,
    ) -> OQLResult<Self> {
        let file_len = usize::try_from(filesystem.file_size(path).map_err(|error| {
            OQLError::storage_error(&format!("inspect base segment: {error}"), None)
        })?)
        .map_err(|_| OQLError::storage_error("base segment is too large to address", None))?;
        let manifest_len = usize::try_from(manifest.segment_len)
            .map_err(|_| OQLError::storage_error("manifest segment length overflow", None))?;
        if file_len != manifest_len || file_len < SEGMENT_HEADER_LEN_V2 {
            return Err(OQLError::storage_error(
                "base segment length conflicts with its manifest",
                None,
            ));
        }
        let header_prefix = read_file_range(filesystem.as_ref(), path, 0, 8)?;
        let version = get_u16(&header_prefix, 4)?;
        let header_len = get_u16(&header_prefix, 6)? as usize;
        let expected_header_len = match version {
            2 => SEGMENT_HEADER_LEN_V2,
            SEGMENT_VERSION_V3 | SEGMENT_VERSION_V4 => SEGMENT_HEADER_LEN_V4,
            SEGMENT_VERSION_V5 | SEGMENT_VERSION => SEGMENT_HEADER_LEN,
            _ => 0,
        };
        if header_prefix[0..4] != SEGMENT_MAGIC || header_len != expected_header_len {
            return Err(OQLError::storage_error(
                "base segment header is invalid",
                None,
            ));
        }
        let header = read_file_range(filesystem.as_ref(), path, 0, header_len)?;
        if get_u64(&header, 48)? as usize != file_len {
            return Err(OQLError::storage_error(
                "base segment header is invalid",
                None,
            ));
        }
        let metadata_len = get_u32(&header, 24)? as usize;
        let entry_count = get_u32(&header, 28)? as usize;
        let directory_len = get_u32(&header, 32)? as usize;
        let records_len = usize::try_from(get_u64(&header, 40)?)
            .map_err(|_| OQLError::storage_error("base record region is too large", None))?;
        if directory_len
            != entry_count
                .checked_mul(DIRECTORY_ENTRY_LEN)
                .ok_or_else(|| {
                    OQLError::storage_error("base segment directory length overflow", None)
                })?
        {
            return Err(OQLError::storage_error(
                "base segment directory length is invalid",
                None,
            ));
        }
        let directory_start = header_len
            .checked_add(metadata_len)
            .ok_or_else(|| OQLError::storage_error("base segment metadata overflow", None))?;
        let records_start = directory_start
            .checked_add(directory_len)
            .filter(|start| *start <= file_len)
            .ok_or_else(|| OQLError::storage_error("base segment directory is truncated", None))?;
        records_start
            .checked_add(records_len)
            .filter(|end| *end <= file_len)
            .ok_or_else(|| OQLError::storage_error("base record region is truncated", None))?;
        let prefix = read_file_range(filesystem.as_ref(), path, 0, records_start)?;
        let mut previous = None;
        for index in 0..entry_count {
            let entry = directory_start + index * DIRECTORY_ENTRY_LEN;
            let object_ref = ObjRef::new(get_u64(&prefix, entry)?);
            let offset = usize::try_from(get_u64(&prefix, entry + 8)?)
                .map_err(|_| OQLError::storage_error("base record offset overflow", None))?;
            let length = get_u32(&prefix, entry + 16)? as usize;
            if object_ref.is_null()
                || previous.is_some_and(|candidate| candidate >= object_ref)
                || offset
                    .checked_add(length)
                    .is_none_or(|end| end > records_len)
            {
                return Err(OQLError::storage_error(
                    "base segment object directory is invalid",
                    None,
                ));
            }
            previous = Some(object_ref);
        }
        let file = filesystem.open(path).map_err(|error| {
            OQLError::storage_error(&format!("open base segment records: {error}"), None)
        })?;
        Ok(Self {
            file: Mutex::new(file),
            prefix,
            entry_count,
            directory_start,
            records_start,
            records_len,
        })
    }

    pub(crate) fn lookup(&self, object_ref: ObjRef) -> OQLResult<Option<Arc<[u8]>>> {
        let mut low = 0usize;
        let mut high = self.entry_count;
        while low < high {
            let middle = low + (high - low) / 2;
            let entry = self.directory_start + middle * DIRECTORY_ENTRY_LEN;
            match ObjRef::new(get_u64(&self.prefix, entry)?).cmp(&object_ref) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    let offset =
                        usize::try_from(get_u64(&self.prefix, entry + 8)?).map_err(|_| {
                            OQLError::storage_error("base record offset overflow", None)
                        })?;
                    let length = get_u32(&self.prefix, entry + 16)? as usize;
                    let checksum = get_u32(&self.prefix, entry + 20)?;
                    let end = offset
                        .checked_add(length)
                        .filter(|end| *end <= self.records_len)
                        .ok_or_else(|| {
                            OQLError::storage_error("base record lies outside its region", None)
                        })?;
                    let mut file = self.file.lock().map_err(|_| {
                        OQLError::storage_error("base segment read handle lock poisoned", None)
                    })?;
                    file.seek(SeekFrom::Start((self.records_start + offset) as u64))
                        .map_err(|error| {
                            OQLError::storage_error(
                                &format!("seek base object record: {error}"),
                                None,
                            )
                        })?;
                    let record = read_exact_handle(file.as_mut(), end - offset)?;
                    if crc32fast::hash(&record) != checksum {
                        return Err(OQLError::storage_error(
                            "base object record checksum mismatch",
                            None,
                        ));
                    }
                    return Ok(Some(Arc::from(record)));
                }
            }
        }
        Ok(None)
    }

    /// Resolve a scan batch with one contiguous base-segment read. Object
    /// records are physically ordered by `ObjRef`, so rows from one collection
    /// normally occupy one compact span even when logical scan order differs.
    pub(crate) fn lookup_many(&self, object_refs: &[ObjRef]) -> OQLResult<Vec<Option<Arc<[u8]>>>> {
        #[derive(Clone, Copy)]
        struct Located {
            offset: usize,
            length: usize,
            checksum: u32,
        }

        let locate = |object_ref: ObjRef| -> OQLResult<Option<Located>> {
            let mut low = 0usize;
            let mut high = self.entry_count;
            while low < high {
                let middle = low + (high - low) / 2;
                let entry = self.directory_start + middle * DIRECTORY_ENTRY_LEN;
                match ObjRef::new(get_u64(&self.prefix, entry)?).cmp(&object_ref) {
                    std::cmp::Ordering::Less => low = middle + 1,
                    std::cmp::Ordering::Greater => high = middle,
                    std::cmp::Ordering::Equal => {
                        let offset =
                            usize::try_from(get_u64(&self.prefix, entry + 8)?).map_err(|_| {
                                OQLError::storage_error("base record offset overflow", None)
                            })?;
                        let length = get_u32(&self.prefix, entry + 16)? as usize;
                        let checksum = get_u32(&self.prefix, entry + 20)?;
                        offset
                            .checked_add(length)
                            .filter(|end| *end <= self.records_len)
                            .ok_or_else(|| {
                                OQLError::storage_error("base record lies outside its region", None)
                            })?;
                        return Ok(Some(Located {
                            offset,
                            length,
                            checksum,
                        }));
                    }
                }
            }
            Ok(None)
        };

        let located = object_refs
            .iter()
            .copied()
            .map(locate)
            .collect::<OQLResult<Vec<_>>>()?;
        // Read the records in runs of nearby bytes. Records requested
        // together may be adjacent (a scan) or scattered across the whole
        // segment (an index order); reading one span from the first to the
        // last would then read almost the entire file for every batch.
        // Records closer than this are worth one read; farther apart, the
        // bytes between them are skipped.
        const MAX_GAP: usize = 16 * 1024;
        let mut by_offset: Vec<usize> = (0..located.len())
            .filter(|index| located[*index].is_some())
            .collect();
        by_offset.sort_unstable_by_key(|index| located[*index].map(|entry| entry.offset));

        let mut records: Vec<Option<Arc<[u8]>>> = vec![None; object_refs.len()];
        let mut file = self
            .file
            .lock()
            .map_err(|_| OQLError::storage_error("base segment read handle lock poisoned", None))?;
        let mut run_start = 0;
        while run_start < by_offset.len() {
            let first = located[by_offset[run_start]].expect("only located records are queued");
            let mut run_end = run_start + 1;
            let mut span_end = first.offset + first.length;
            while let Some(next) = by_offset
                .get(run_end)
                .and_then(|index| located[*index])
                .filter(|next| next.offset <= span_end + MAX_GAP)
            {
                span_end = span_end.max(next.offset + next.length);
                run_end += 1;
            }

            file.seek(SeekFrom::Start((self.records_start + first.offset) as u64))
                .map_err(|error| {
                    OQLError::storage_error(&format!("seek base record batch: {error}"), None)
                })?;
            let span = read_exact_handle(file.as_mut(), span_end - first.offset)?;
            for index in &by_offset[run_start..run_end] {
                let entry = located[*index].expect("only located records are queued");
                let start = entry.offset - first.offset;
                let record = &span[start..start + entry.length];
                if crc32fast::hash(record) != entry.checksum {
                    return Err(OQLError::storage_error(
                        "base object record checksum mismatch",
                        None,
                    ));
                }
                records[*index] = Some(Arc::from(record));
            }
            run_start = run_end;
        }
        Ok(records)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub sequence: u64,
    pub checkpoint_lsn: u64,
    pub segment_len: u64,
    pub segment_checksum: u32,
    pub live_objects: u64,
    /// Slot containing this manifest copy.
    pub slot: u8,
    /// Slot containing the full base segment. Format 1 used `slot` for both.
    pub segment_slot: u8,
    /// Optional cumulative WAL delta layered over the base segment.
    pub delta_len: u64,
    pub delta_checksum: u32,
    /// Whether this manifest publishes a matching cumulative OWLD record root.
    pub has_record_root: bool,
    /// Whether this manifest publishes a matching cumulative OWLI index root.
    pub has_index_root: bool,
}

#[cfg(test)]
pub(crate) fn encode_segment(image: &CheckpointImage) -> OQLResult<Vec<u8>> {
    let metadata = bincode::serialize(&image.metadata).map_err(|error| {
        OQLError::serialization_error(&format!("cannot encode checkpoint metadata: {error}"), None)
    })?;
    let metadata_len = u32::try_from(metadata.len())
        .map_err(|_| OQLError::storage_error("checkpoint metadata is too large", None))?;
    let entry_count = u32::try_from(image.records.len())
        .map_err(|_| OQLError::storage_error("checkpoint has too many objects", None))?;
    let directory_len = image
        .records
        .len()
        .checked_mul(DIRECTORY_ENTRY_LEN)
        .ok_or_else(|| OQLError::storage_error("checkpoint directory overflow", None))?;
    let directory_len_u32 = u32::try_from(directory_len)
        .map_err(|_| OQLError::storage_error("checkpoint directory is too large", None))?;

    let mut records = image.records.iter().collect::<Vec<_>>();
    records.sort_by_key(|(object_ref, _)| object_ref.packed());
    let records_len = records.iter().try_fold(0usize, |total, (_, record)| {
        total
            .checked_add(record.len())
            .ok_or_else(|| OQLError::storage_error("checkpoint record region overflow", None))
    })?;
    let (indexes, index_page_count) = encode_indexes(&image.indexes)?;
    let typed_segments_len = image
        .typed_segments
        .iter()
        .try_fold(0usize, |total, segment| total.checked_add(segment.len()))
        .ok_or_else(|| OQLError::storage_error("typed segment region overflow", None))?;
    let typed_locator = encode_typed_locator(&image.typed_segments)?;
    let index_count = u32::try_from(index_page_count)
        .map_err(|_| OQLError::storage_error("checkpoint has too many index pages", None))?;
    let total_len = SEGMENT_HEADER_LEN
        .checked_add(metadata.len())
        .and_then(|length| length.checked_add(directory_len))
        .and_then(|length| length.checked_add(records_len))
        .and_then(|length| length.checked_add(indexes.len()))
        .and_then(|length| length.checked_add(typed_locator.len()))
        .and_then(|length| length.checked_add(typed_segments_len))
        .ok_or_else(|| OQLError::storage_error("checkpoint segment length overflow", None))?;
    let total_len_u64 = u64::try_from(total_len)
        .map_err(|_| OQLError::storage_error("checkpoint segment is too large", None))?;

    let mut segment = vec![0u8; total_len];
    segment[0..4].copy_from_slice(&SEGMENT_MAGIC);
    put_u16(&mut segment, 4, SEGMENT_VERSION);
    put_u16(&mut segment, 6, SEGMENT_HEADER_LEN as u16);
    put_u64(&mut segment, 8, image.sequence);
    put_u64(&mut segment, 16, image.checkpoint_lsn);
    put_u32(&mut segment, 24, metadata_len);
    put_u32(&mut segment, 28, entry_count);
    put_u32(&mut segment, 32, directory_len_u32);
    put_u32(&mut segment, 36, index_count);
    put_u64(&mut segment, 40, records_len as u64);
    put_u64(&mut segment, 48, total_len_u64);
    put_u64(&mut segment, 60, typed_segments_len as u64);
    put_u64(&mut segment, 72, typed_locator.len() as u64);

    let metadata_start = SEGMENT_HEADER_LEN;
    let directory_start = metadata_start + metadata.len();
    let records_start = directory_start + directory_len;
    segment[metadata_start..directory_start].copy_from_slice(&metadata);

    let mut record_offset = 0usize;
    for (index, (object_ref, record)) in records.into_iter().enumerate() {
        let entry = directory_start + index * DIRECTORY_ENTRY_LEN;
        put_u64(&mut segment, entry, object_ref.packed());
        put_u64(&mut segment, entry + 8, record_offset as u64);
        put_u32(
            &mut segment,
            entry + 16,
            u32::try_from(record.len())
                .map_err(|_| OQLError::storage_error("object record is too large", None))?,
        );
        put_u32(&mut segment, entry + 20, crc32fast::hash(record));
        let destination = records_start + record_offset;
        segment[destination..destination + record.len()].copy_from_slice(record);
        record_offset += record.len();
    }
    let indexes_start = records_start + records_len;
    let typed_locator_start = indexes_start + indexes.len();
    let typed_segments_start = typed_locator_start + typed_locator.len();
    segment[indexes_start..typed_locator_start].copy_from_slice(&indexes);
    segment[typed_locator_start..typed_segments_start].copy_from_slice(&typed_locator);
    let mut typed_cursor = typed_segments_start;
    for typed in &image.typed_segments {
        segment[typed_cursor..typed_cursor + typed.len()].copy_from_slice(typed);
        typed_cursor += typed.len();
    }
    let root_checksum = checkpoint_root_checksum(
        &segment[..SEGMENT_HEADER_LEN],
        &[&segment[metadata_start..records_start]],
    );
    put_u32(&mut segment, 68, root_checksum);
    let checksum = checksum_with_zeroed_field(&segment, 56);
    put_u32(&mut segment, 56, checksum);
    Ok(segment)
}

#[cfg(test)]
pub(crate) fn decode_segment(segment: &[u8]) -> OQLResult<CheckpointImage> {
    if segment.len() < SEGMENT_HEADER_LEN || segment[0..4] != SEGMENT_MAGIC {
        return Err(OQLError::storage_error(
            "invalid checkpoint segment header",
            None,
        ));
    }
    if get_u16(segment, 4)? != SEGMENT_VERSION
        || get_u16(segment, 6)? as usize != SEGMENT_HEADER_LEN
    {
        return Err(OQLError::storage_error(
            "unsupported checkpoint segment format",
            None,
        ));
    }
    let metadata_len = get_u32(segment, 24)? as usize;
    let directory_len = get_u32(segment, 32)? as usize;
    let root_end = SEGMENT_HEADER_LEN
        .checked_add(metadata_len)
        .and_then(|end| end.checked_add(directory_len))
        .filter(|end| *end <= segment.len())
        .ok_or_else(|| OQLError::storage_error("checkpoint root is truncated", None))?;
    if checkpoint_root_checksum(
        &segment[..SEGMENT_HEADER_LEN],
        &[&segment[SEGMENT_HEADER_LEN..root_end]],
    ) != get_u32(segment, 68)?
    {
        return Err(OQLError::storage_error(
            "checkpoint root checksum mismatch",
            None,
        ));
    }
    if get_u64(segment, 48)? != segment.len() as u64 {
        return Err(OQLError::storage_error(
            "checkpoint segment length does not match header",
            None,
        ));
    }
    if checksum_with_zeroed_field(segment, 56) != get_u32(segment, 56)? {
        return Err(OQLError::storage_error(
            "checkpoint segment checksum mismatch",
            None,
        ));
    }

    let sequence = get_u64(segment, 8)?;
    let checkpoint_lsn = get_u64(segment, 16)?;
    let entry_count = get_u32(segment, 28)? as usize;
    let index_count = get_u32(segment, 36)? as usize;
    let typed_segments_len = usize::try_from(get_u64(segment, 60)?)
        .map_err(|_| OQLError::storage_error("typed segment region is too large", None))?;
    let typed_locator_len = usize::try_from(get_u64(segment, 72)?)
        .map_err(|_| OQLError::storage_error("typed locator region is too large", None))?;
    let records_len = usize::try_from(get_u64(segment, 40)?)
        .map_err(|_| OQLError::storage_error("checkpoint record length overflow", None))?;
    if directory_len
        != entry_count
            .checked_mul(DIRECTORY_ENTRY_LEN)
            .ok_or_else(|| OQLError::storage_error("checkpoint directory length overflow", None))?
    {
        return Err(OQLError::storage_error(
            "checkpoint directory length is invalid",
            None,
        ));
    }
    let metadata_start = SEGMENT_HEADER_LEN;
    let directory_start = checked_end(metadata_start, metadata_len, segment.len())?;
    let records_start = checked_end(directory_start, directory_len, segment.len())?;
    let records_end = checked_end(records_start, records_len, segment.len())?;
    let typed_segments_start = segment
        .len()
        .checked_sub(typed_segments_len)
        .filter(|start| *start >= records_end)
        .ok_or_else(|| {
            OQLError::storage_error("checkpoint typed segment region is truncated", None)
        })?;
    let typed_locator_start = typed_segments_start
        .checked_sub(typed_locator_len)
        .filter(|start| *start >= records_end)
        .ok_or_else(|| OQLError::storage_error("typed locator region is truncated", None))?;
    let metadata: CheckpointMetadata =
        bincode::deserialize(&segment[metadata_start..directory_start]).map_err(|error| {
            OQLError::serialization_error(
                &format!("cannot decode checkpoint metadata: {error}"),
                None,
            )
        })?;

    let mut seen = HashSet::with_capacity(entry_count);
    let mut records = Vec::with_capacity(entry_count);
    for index in 0..entry_count {
        let entry = directory_start + index * DIRECTORY_ENTRY_LEN;
        let object_ref = ObjRef::new(get_u64(segment, entry)?);
        if object_ref.is_null() || !seen.insert(object_ref) {
            return Err(OQLError::storage_error(
                "checkpoint directory contains an invalid object reference",
                None,
            ));
        }
        let offset = usize::try_from(get_u64(segment, entry + 8)?)
            .map_err(|_| OQLError::storage_error("checkpoint record offset overflow", None))?;
        let length = get_u32(segment, entry + 16)? as usize;
        let expected_checksum = get_u32(segment, entry + 20)?;
        let end = checked_end(offset, length, records_len)?;
        let record = Arc::<[u8]>::from(&segment[records_start + offset..records_start + end]);
        if crc32fast::hash(&record) != expected_checksum {
            return Err(OQLError::storage_error(
                "checkpoint object record checksum mismatch",
                None,
            ));
        }
        records.push((object_ref, record));
    }
    decode_typed_locator(Arc::from(
        &segment[typed_locator_start..typed_segments_start],
    ))?;
    let indexes = decode_indexes(&segment[records_end..typed_locator_start], index_count)?;
    let typed_segments = TypedSegment::decode_all(&segment[typed_segments_start..])?
        .into_iter()
        .map(|segment| segment.encode().map(Arc::from))
        .collect::<OQLResult<Vec<_>>>()?;
    Ok(CheckpointImage {
        sequence,
        checkpoint_lsn,
        metadata,
        records,
        indexes,
        typed_segments,
    })
}

#[cfg(test)]
fn encode_indexes(indexes: &[CheckpointIndex]) -> OQLResult<(Vec<u8>, usize)> {
    let mut ordered = indexes.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| (&left.schema, &left.name).cmp(&(&right.schema, &right.name)));
    let mut output = Vec::new();
    let mut page_count = 0usize;
    for index in ordered {
        let ranges = plan_index_pages(index)?;
        let last_page = ranges.len() - 1;
        for (page, range) in ranges.into_iter().enumerate() {
            output.extend_from_slice(&encode_index_page(
                index,
                range,
                page == 0,
                page == last_page,
            )?);
            page_count += 1;
        }
    }
    Ok((output, page_count))
}

fn index_entry_len(key: &[u8], postings: &[ObjRef]) -> OQLResult<usize> {
    if key.is_empty() || postings.is_empty() {
        return Err(OQLError::storage_error(
            "index snapshot contains an empty key or posting list",
            None,
        ));
    }
    let _ = u32::try_from(key.len())
        .map_err(|_| OQLError::storage_error("index key is too large", None))?;
    let _ = u32::try_from(postings.len())
        .map_err(|_| OQLError::storage_error("index posting list is too large", None))?;
    8usize
        .checked_add(key.len())
        .and_then(|length| length.checked_add(postings.len().checked_mul(8)?))
        .ok_or_else(|| OQLError::storage_error("index entry length overflow", None))
}

fn plan_index_pages(index: &CheckpointIndex) -> OQLResult<Vec<Range<usize>>> {
    let _ = u16::try_from(index.schema.len())
        .map_err(|_| OQLError::storage_error("index schema name is too long", None))?;
    let _ = u16::try_from(index.name.len())
        .map_err(|_| OQLError::storage_error("index name is too long", None))?;
    let _ = u32::try_from(index.entries.len())
        .map_err(|_| OQLError::storage_error("index has too many keys", None))?;
    let fixed = INDEX_BLOCK_HEADER_LEN
        .checked_add(index.schema.len())
        .and_then(|length| length.checked_add(index.name.len()))
        .and_then(|length| length.checked_add(INDEX_ROUTING_HEADER_LEN))
        .ok_or_else(|| OQLError::storage_error("index page length overflow", None))?;
    if index.entries.is_empty() {
        return Ok(vec![0..0]);
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < index.entries.len() {
        let mut end = start;
        let mut length = fixed;
        while end < index.entries.len() {
            let (key, postings) = &index.entries[end];
            let entry_len = index_entry_len(key, postings)?;
            let first_key_len = index.entries[start].0.len();
            let candidate_len = length
                .checked_add(entry_len)
                .and_then(|length| length.checked_add(first_key_len))
                .and_then(|length| length.checked_add(key.len()))
                .ok_or_else(|| OQLError::storage_error("index page length overflow", None))?;
            if end > start && candidate_len > INDEX_PAGE_TARGET_BYTES {
                break;
            }
            length = length
                .checked_add(entry_len)
                .ok_or_else(|| OQLError::storage_error("index page length overflow", None))?;
            end += 1;
        }
        ranges.push(start..end);
        start = end;
    }
    Ok(ranges)
}

fn encoded_index_page_len(index: &CheckpointIndex, range: Range<usize>) -> OQLResult<usize> {
    range.clone().try_fold(
        INDEX_BLOCK_HEADER_LEN
            .checked_add(index.schema.len())
            .and_then(|length| length.checked_add(index.name.len()))
            .and_then(|length| length.checked_add(INDEX_ROUTING_HEADER_LEN))
            .and_then(|length| {
                index.entries[range.clone()]
                    .first()
                    .map(|entry| entry.0.len())
                    .unwrap_or(0)
                    .checked_add(
                        index.entries[range.clone()]
                            .last()
                            .map(|entry| entry.0.len())
                            .unwrap_or(0),
                    )
                    .and_then(|keys| length.checked_add(keys))
            })
            .ok_or_else(|| OQLError::storage_error("index page length overflow", None))?,
        |length, entry| {
            let (key, postings) = &index.entries[entry];
            length
                .checked_add(index_entry_len(key, postings)?)
                .ok_or_else(|| OQLError::storage_error("index page length overflow", None))
        },
    )
}

fn encode_index_page(
    index: &CheckpointIndex,
    range: Range<usize>,
    first: bool,
    last: bool,
) -> OQLResult<Vec<u8>> {
    let total_len = encoded_index_page_len(index, range.clone())?;
    let mut output = vec![0; INDEX_BLOCK_HEADER_LEN];
    put_u16(&mut output, 0, index.schema.len() as u16);
    put_u16(&mut output, 2, index.name.len() as u16);
    put_u32(&mut output, 4, (range.end - range.start) as u32);
    put_u64(&mut output, 8, (total_len - INDEX_BLOCK_HEADER_LEN) as u64);
    output[20] = u8::from(index.unique);
    output[21] = INDEX_PAGE_FORMAT
        | INDEX_PAGE_ROUTING
        | if first { INDEX_PAGE_FIRST } else { 0 }
        | if last { INDEX_PAGE_LAST } else { 0 };
    output.extend_from_slice(index.schema.as_bytes());
    output.extend_from_slice(index.name.as_bytes());
    let first_key = index.entries[range.clone()]
        .first()
        .map(|entry| entry.0.as_slice())
        .unwrap_or_default();
    let last_key = index.entries[range.clone()]
        .last()
        .map(|entry| entry.0.as_slice())
        .unwrap_or_default();
    let posting_count =
        index.entries[range.clone()]
            .iter()
            .try_fold(0usize, |count, (_, postings)| {
                count.checked_add(postings.len()).ok_or_else(|| {
                    OQLError::storage_error("index page posting count overflow", None)
                })
            })?;
    output.extend_from_slice(&(first_key.len() as u32).to_le_bytes());
    output.extend_from_slice(&(last_key.len() as u32).to_le_bytes());
    output.extend_from_slice(&(posting_count as u64).to_le_bytes());
    output.extend_from_slice(first_key);
    output.extend_from_slice(last_key);
    let routing_checksum = crc32fast::hash(&output[INDEX_BLOCK_HEADER_LEN..]);
    put_u32(&mut output, 24, routing_checksum);
    for (key, postings) in &index.entries[range] {
        output.extend_from_slice(&(key.len() as u32).to_le_bytes());
        output.extend_from_slice(&(postings.len() as u32).to_le_bytes());
        output.extend_from_slice(key);
        for object_ref in postings {
            output.extend_from_slice(&object_ref.packed().to_le_bytes());
        }
    }
    let checksum = checksum_with_zeroed_field(&output, 16);
    put_u32(&mut output, 16, checksum);
    Ok(output)
}

#[cfg(test)]
fn decode_indexes(bytes: &[u8], count: usize) -> OQLResult<Vec<CheckpointIndex>> {
    let mut cursor = 0usize;
    let mut indexes = Vec::with_capacity(count);
    let mut identities = HashSet::with_capacity(count);
    let mut open_identity: Option<(String, String, bool)> = None;
    for _ in 0..count {
        let header_end = checked_end(cursor, INDEX_BLOCK_HEADER_LEN, bytes.len())?;
        let payload_len = usize::try_from(get_u64(bytes, cursor + 8)?)
            .map_err(|_| OQLError::storage_error("index block length overflow", None))?;
        let block_end = checked_end(header_end, payload_len, bytes.len())?;
        let page = decode_index_page(&bytes[cursor..header_end], &bytes[header_end..block_end])?;
        let identity = (page.schema.clone(), page.name.clone(), page.unique);
        if page.first {
            if open_identity.is_some()
                || !identities.insert((page.schema.clone(), page.name.clone()))
            {
                return Err(OQLError::storage_error(
                    "duplicate or interrupted checkpoint index page chain",
                    None,
                ));
            }
            indexes.push(CheckpointIndex {
                schema: page.schema,
                name: page.name,
                unique: page.unique,
                entries: page.entries,
            });
        } else if open_identity.as_ref() == Some(&identity) {
            let index = indexes.last_mut().ok_or_else(|| {
                OQLError::storage_error("checkpoint index page chain has no first page", None)
            })?;
            index.entries.extend(page.entries);
        } else {
            return Err(OQLError::storage_error(
                "checkpoint index page chain changes identity",
                None,
            ));
        }
        open_identity = (!page.last).then_some(identity);
        cursor = block_end;
    }
    if cursor != bytes.len() || open_identity.is_some() {
        return Err(OQLError::storage_error(
            "checkpoint index region has trailing bytes",
            None,
        ));
    }
    Ok(indexes)
}

fn decode_index_page(header: &[u8], payload: &[u8]) -> OQLResult<CheckpointIndexPage> {
    if header.len() != INDEX_BLOCK_HEADER_LEN
        || get_u64(header, 8)? != payload.len() as u64
        || header[22..24].iter().any(|byte| *byte != 0)
        || header[28..].iter().any(|byte| *byte != 0)
    {
        return Err(OQLError::storage_error(
            "checkpoint index block header is invalid",
            None,
        ));
    }
    let mut checksum_header = [0; INDEX_BLOCK_HEADER_LEN];
    checksum_header.copy_from_slice(header);
    checksum_header[16..20].fill(0);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&checksum_header);
    hasher.update(payload);
    if hasher.finalize() != get_u32(header, 16)? {
        return Err(OQLError::storage_error(
            "checkpoint index block checksum mismatch",
            None,
        ));
    }
    let schema_len = get_u16(header, 0)? as usize;
    let name_len = get_u16(header, 2)? as usize;
    let entry_count = get_u32(header, 4)? as usize;
    let flags = header[21];
    let (first, last) = if flags == 0 {
        (true, true)
    } else if flags & !(INDEX_PAGE_FORMAT | INDEX_PAGE_ROUTING | INDEX_PAGE_FIRST | INDEX_PAGE_LAST)
        != 0
        || flags & INDEX_PAGE_FORMAT == 0
    {
        return Err(OQLError::storage_error(
            "checkpoint index page flags are invalid",
            None,
        ));
    } else {
        (flags & INDEX_PAGE_FIRST != 0, flags & INDEX_PAGE_LAST != 0)
    };
    let unique = match header[20] {
        0 => false,
        1 => true,
        _ => {
            return Err(OQLError::storage_error(
                "invalid index uniqueness flag",
                None,
            ))
        }
    };
    let mut cursor = 0usize;
    let schema_end = checked_end(cursor, schema_len, payload.len())?;
    let schema = std::str::from_utf8(&payload[cursor..schema_end])
        .map_err(|_| OQLError::storage_error("index schema is not UTF-8", None))?
        .to_string();
    cursor = schema_end;
    let name_end = checked_end(cursor, name_len, payload.len())?;
    let name = std::str::from_utf8(&payload[cursor..name_end])
        .map_err(|_| OQLError::storage_error("index name is not UTF-8", None))?
        .to_string();
    cursor = name_end;
    let (first_key, last_key, persisted_posting_count) = if flags & INDEX_PAGE_ROUTING != 0 {
        let routing_end = checked_end(cursor, INDEX_ROUTING_HEADER_LEN, payload.len())?;
        let first_len = get_u32(payload, cursor)? as usize;
        let last_len = get_u32(payload, cursor + 4)? as usize;
        let posting_count = usize::try_from(get_u64(payload, cursor + 8)?)
            .map_err(|_| OQLError::storage_error("index posting count overflow", None))?;
        let first_end = checked_end(routing_end, first_len, payload.len())?;
        let last_end = checked_end(first_end, last_len, payload.len())?;
        let routing_checksum = crc32fast::hash(&payload[..last_end]);
        if routing_checksum != get_u32(header, 24)? {
            return Err(OQLError::storage_error(
                "checkpoint index routing checksum mismatch",
                None,
            ));
        }
        cursor = last_end;
        (
            (first_len != 0).then(|| payload[routing_end..first_end].to_vec()),
            (last_len != 0).then(|| payload[first_end..last_end].to_vec()),
            Some(posting_count),
        )
    } else {
        (None, None, None)
    };
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let entry_header_end = checked_end(cursor, 8, payload.len())?;
        let key_len = get_u32(payload, cursor)? as usize;
        let posting_count = get_u32(payload, cursor + 4)? as usize;
        if key_len == 0 || posting_count == 0 {
            return Err(OQLError::storage_error(
                "empty checkpoint index entry",
                None,
            ));
        }
        let key_end = checked_end(entry_header_end, key_len, payload.len())?;
        let postings_end = checked_end(
            key_end,
            posting_count
                .checked_mul(8)
                .ok_or_else(|| OQLError::storage_error("index posting length overflow", None))?,
            payload.len(),
        )?;
        let key = payload[entry_header_end..key_end].to_vec();
        let mut postings = Vec::with_capacity(posting_count);
        let mut seen = HashSet::with_capacity(posting_count);
        for chunk in payload[key_end..postings_end].chunks_exact(8) {
            let object_ref = ObjRef::new(u64::from_le_bytes(chunk.try_into().unwrap()));
            if object_ref.is_null() || !seen.insert(object_ref) {
                return Err(OQLError::storage_error(
                    "invalid checkpoint index posting",
                    None,
                ));
            }
            postings.push(object_ref);
        }
        if entries.last().is_some_and(|(previous, _)| previous >= &key)
            || postings.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(OQLError::storage_error(
                "checkpoint index entries are not strictly ordered",
                None,
            ));
        }
        entries.push((key, postings));
        cursor = postings_end;
    }
    if cursor != payload.len() {
        return Err(OQLError::storage_error(
            "index block has trailing bytes",
            None,
        ));
    }
    let decoded_posting_count = entries.iter().try_fold(0usize, |count, (_, postings)| {
        count
            .checked_add(postings.len())
            .ok_or_else(|| OQLError::storage_error("index posting count overflow", None))
    })?;
    let decoded_first_key = entries.first().map(|entry| entry.0.clone());
    let decoded_last_key = entries.last().map(|entry| entry.0.clone());
    if persisted_posting_count.is_some_and(|count| count != decoded_posting_count)
        || first_key
            .as_ref()
            .is_some_and(|key| Some(key) != decoded_first_key.as_ref())
        || last_key
            .as_ref()
            .is_some_and(|key| Some(key) != decoded_last_key.as_ref())
        || (entry_count == 0 && (first_key.is_some() || last_key.is_some()))
    {
        return Err(OQLError::storage_error(
            "checkpoint index routing metadata does not match its entries",
            None,
        ));
    }
    Ok(CheckpointIndexPage {
        schema,
        name,
        unique,
        first,
        last,
        entries,
        first_key: first_key.or(decoded_first_key),
        last_key: last_key.or(decoded_last_key),
        posting_count: decoded_posting_count,
        routing_only: false,
        location: None,
    })
}

fn decode_index_page_routing(header: &[u8], routing: &[u8]) -> OQLResult<CheckpointIndexPage> {
    if header.len() != INDEX_BLOCK_HEADER_LEN
        || get_u64(header, 8)? < routing.len() as u64
        || header[22..24].iter().any(|byte| *byte != 0)
        || header[28..].iter().any(|byte| *byte != 0)
    {
        return Err(OQLError::storage_error(
            "checkpoint index routing header is invalid",
            None,
        ));
    }
    let flags = header[21];
    if flags & INDEX_PAGE_ROUTING == 0
        || flags & !(INDEX_PAGE_FORMAT | INDEX_PAGE_ROUTING | INDEX_PAGE_FIRST | INDEX_PAGE_LAST)
            != 0
        || flags & INDEX_PAGE_FORMAT == 0
    {
        return Err(OQLError::storage_error(
            "checkpoint index routing flags are invalid",
            None,
        ));
    }
    let unique = match header[20] {
        0 => false,
        1 => true,
        _ => {
            return Err(OQLError::storage_error(
                "invalid index uniqueness flag",
                None,
            ))
        }
    };
    let schema_len = get_u16(header, 0)? as usize;
    let name_len = get_u16(header, 2)? as usize;
    let entry_count = get_u32(header, 4)? as usize;
    let identity_end = schema_len
        .checked_add(name_len)
        .ok_or_else(|| OQLError::storage_error("index identity length overflow", None))?;
    let routing_header_end = checked_end(identity_end, INDEX_ROUTING_HEADER_LEN, routing.len())?;
    let first_len = get_u32(routing, identity_end)? as usize;
    let last_len = get_u32(routing, identity_end + 4)? as usize;
    let posting_count = usize::try_from(get_u64(routing, identity_end + 8)?)
        .map_err(|_| OQLError::storage_error("index posting count overflow", None))?;
    let first_end = checked_end(routing_header_end, first_len, routing.len())?;
    let last_end = checked_end(first_end, last_len, routing.len())?;
    if last_end != routing.len()
        || crc32fast::hash(routing) != get_u32(header, 24)?
        || (entry_count == 0 && (first_len != 0 || last_len != 0 || posting_count != 0))
        || (entry_count != 0 && (first_len == 0 || last_len == 0 || posting_count < entry_count))
        || routing[routing_header_end..first_end] > routing[first_end..last_end]
    {
        return Err(OQLError::storage_error(
            "checkpoint index routing metadata is invalid",
            None,
        ));
    }
    let schema = std::str::from_utf8(&routing[..schema_len])
        .map_err(|_| OQLError::storage_error("index schema is not UTF-8", None))?
        .to_string();
    let name = std::str::from_utf8(&routing[schema_len..identity_end])
        .map_err(|_| OQLError::storage_error("index name is not UTF-8", None))?
        .to_string();
    Ok(CheckpointIndexPage {
        schema,
        name,
        unique,
        first: flags & INDEX_PAGE_FIRST != 0,
        last: flags & INDEX_PAGE_LAST != 0,
        entries: Vec::new(),
        first_key: (first_len != 0).then(|| routing[routing_header_end..first_end].to_vec()),
        last_key: (last_len != 0).then(|| routing[first_end..last_end].to_vec()),
        posting_count,
        routing_only: true,
        location: None,
    })
}

impl Manifest {
    pub(crate) fn encode(self) -> [u8; MANIFEST_LEN] {
        let mut manifest = [0u8; MANIFEST_LEN];
        manifest[0..4].copy_from_slice(&MANIFEST_MAGIC);
        put_u16(&mut manifest, 4, MANIFEST_VERSION);
        put_u16(&mut manifest, 6, MANIFEST_LEN as u16);
        put_u64(&mut manifest, 8, self.sequence);
        put_u64(&mut manifest, 16, self.checkpoint_lsn);
        put_u64(&mut manifest, 24, self.segment_len);
        put_u32(&mut manifest, 32, self.segment_checksum);
        let locations = u32::from(self.slot)
            | (u32::from(self.segment_slot) << 8)
            | (u32::from(self.has_record_root) << 16)
            | (u32::from(self.has_index_root) << 17);
        put_u32(&mut manifest, 36, locations);
        put_u64(&mut manifest, 40, self.live_objects);
        put_u64(&mut manifest, 52, self.delta_len);
        put_u32(&mut manifest, 60, self.delta_checksum);
        let checksum = checksum_with_zeroed_field(&manifest, 48);
        put_u32(&mut manifest, 48, checksum);
        manifest
    }

    pub(crate) fn decode(bytes: &[u8]) -> OQLResult<Self> {
        if bytes.len() != MANIFEST_LEN || bytes[0..4] != MANIFEST_MAGIC {
            return Err(OQLError::storage_error(
                "invalid checkpoint manifest header",
                None,
            ));
        }
        let version = get_u16(bytes, 4)?;
        if !matches!(version, 1 | MANIFEST_VERSION) || get_u16(bytes, 6)? as usize != MANIFEST_LEN {
            return Err(OQLError::storage_error(
                "unsupported checkpoint manifest format",
                None,
            ));
        }
        if checksum_with_zeroed_field(bytes, 48) != get_u32(bytes, 48)? {
            return Err(OQLError::storage_error(
                "checkpoint manifest checksum mismatch",
                None,
            ));
        }
        let locations = get_u32(bytes, 36)?;
        let slot = (locations & 0xff) as u8;
        let segment_slot = if version == 1 {
            slot
        } else {
            ((locations >> 8) & 0xff) as u8
        };
        let has_record_root = version != 1 && ((locations >> 16) & 1) != 0;
        let has_index_root = version != 1 && ((locations >> 17) & 1) != 0;
        let known_location_bits = if version == 1 {
            0x0000_0001
        } else {
            0x0003_0101
        };
        if slot > 1 || segment_slot > 1 || locations & !known_location_bits != 0 {
            return Err(OQLError::storage_error(
                "checkpoint manifest slot is invalid",
                None,
            ));
        }
        Ok(Self {
            sequence: get_u64(bytes, 8)?,
            checkpoint_lsn: get_u64(bytes, 16)?,
            segment_len: get_u64(bytes, 24)?,
            segment_checksum: get_u32(bytes, 32)?,
            live_objects: get_u64(bytes, 40)?,
            slot,
            segment_slot,
            delta_len: if version == 1 { 0 } else { get_u64(bytes, 52)? },
            delta_checksum: if version == 1 { 0 } else { get_u32(bytes, 60)? },
            has_record_root,
            has_index_root,
        })
    }
}

fn checksum_with_zeroed_field(bytes: &[u8], checksum_offset: usize) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes[..checksum_offset]);
    hasher.update(&[0; 4]);
    hasher.update(&bytes[checksum_offset + 4..]);
    hasher.finalize()
}

fn checkpoint_root_checksum(header: &[u8], roots: &[&[u8]]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&header[..56]);
    hasher.update(&[0; 4]);
    hasher.update(&header[60..68]);
    hasher.update(&[0; 4]);
    for root in roots {
        hasher.update(root);
    }
    hasher.finalize()
}

fn checked_end(start: usize, length: usize, limit: usize) -> OQLResult<usize> {
    let end = start
        .checked_add(length)
        .ok_or_else(|| OQLError::storage_error("checkpoint region length overflow", None))?;
    if end > limit {
        return Err(OQLError::storage_error(
            "checkpoint region is truncated",
            None,
        ));
    }
    Ok(end)
}

fn put_u16(output: &mut [u8], offset: usize, value: u16) {
    output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(output: &mut [u8], offset: usize, value: u32) {
    output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(output: &mut [u8], offset: usize, value: u64) {
    output[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn get_u16(input: &[u8], offset: usize) -> OQLResult<u16> {
    Ok(u16::from_le_bytes(
        input
            .get(offset..offset + 2)
            .ok_or_else(|| OQLError::storage_error("truncated checkpoint integer", None))?
            .try_into()
            .unwrap(),
    ))
}
fn get_u32(input: &[u8], offset: usize) -> OQLResult<u32> {
    Ok(u32::from_le_bytes(
        input
            .get(offset..offset + 4)
            .ok_or_else(|| OQLError::storage_error("truncated checkpoint integer", None))?
            .try_into()
            .unwrap(),
    ))
}
fn get_u64(input: &[u8], offset: usize) -> OQLResult<u64> {
    Ok(u64::from_le_bytes(
        input
            .get(offset..offset + 8)
            .ok_or_else(|| OQLError::storage_error("truncated checkpoint integer", None))?
            .try_into()
            .unwrap(),
    ))
}

fn read_file_range(
    filesystem: &dyn FileSystem,
    path: &Path,
    offset: usize,
    length: usize,
) -> OQLResult<Vec<u8>> {
    let mut file = filesystem
        .open(path)
        .map_err(|error| OQLError::storage_error(&format!("open base segment: {error}"), None))?;
    file.seek(SeekFrom::Start(offset as u64))
        .map_err(|error| OQLError::storage_error(&format!("seek base segment: {error}"), None))?;
    let mut bytes = vec![0; length];
    let mut read = 0;
    while read < length {
        let count = file.read(&mut bytes[read..]).map_err(|error| {
            OQLError::storage_error(&format!("read base segment: {error}"), None)
        })?;
        if count == 0 {
            return Err(OQLError::storage_error("base segment is truncated", None));
        }
        read += count;
    }
    Ok(bytes)
}

fn write_all_handle(file: &mut dyn FileHandle, bytes: &[u8]) -> OQLResult<()> {
    let mut written = 0;
    while written < bytes.len() {
        let count = file.write(&bytes[written..]).map_err(|error| {
            OQLError::storage_error(&format!("write checkpoint segment: {error}"), None)
        })?;
        if count == 0 {
            return Err(OQLError::storage_error(
                "checkpoint segment write made no progress",
                None,
            ));
        }
        written += count;
    }
    Ok(())
}

fn read_exact_handle(file: &mut dyn FileHandle, length: usize) -> OQLResult<Vec<u8>> {
    let mut bytes = vec![0; length];
    let mut read = 0;
    while read < length {
        let count = file.read(&mut bytes[read..]).map_err(|error| {
            OQLError::storage_error(&format!("read checkpoint record: {error}"), None)
        })?;
        if count == 0 {
            return Err(OQLError::storage_error(
                "checkpoint record is truncated",
                None,
            ));
        }
        read += count;
    }
    Ok(bytes)
}

fn read_exact_at(file: &dyn FileHandle, offset: usize, length: usize) -> OQLResult<Vec<u8>> {
    let mut bytes = vec![0; length];
    let mut read = 0usize;
    while read < length {
        let absolute = offset
            .checked_add(read)
            .ok_or_else(|| OQLError::storage_error("checkpoint read offset overflow", None))?;
        let count = file
            .read_at(&mut bytes[read..], absolute as u64)
            .map_err(|error| {
                OQLError::storage_error(&format!("read checkpoint segment: {error}"), None)
            })?;
        if count == 0 {
            return Err(OQLError::storage_error(
                "checkpoint segment is truncated",
                None,
            ));
        }
        read += count;
    }
    Ok(bytes)
}

fn stream_segment_checksums(
    filesystem: &dyn FileSystem,
    path: &Path,
    expected_len: usize,
) -> OQLResult<(u32, u32)> {
    let mut file = filesystem.open(path).map_err(|error| {
        OQLError::storage_error(&format!("open checkpoint segment: {error}"), None)
    })?;
    let mut regular = crc32fast::Hasher::new();
    let mut internal = crc32fast::Hasher::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut offset = 0usize;
    while offset < expected_len {
        let wanted = buffer.len().min(expected_len - offset);
        let mut read = 0;
        while read < wanted {
            let count = file.read(&mut buffer[read..wanted]).map_err(|error| {
                OQLError::storage_error(&format!("read checkpoint segment: {error}"), None)
            })?;
            if count == 0 {
                return Err(OQLError::storage_error(
                    "checkpoint segment is truncated",
                    None,
                ));
            }
            read += count;
        }
        let chunk = &buffer[..wanted];
        regular.update(chunk);
        let checksum_start = 56usize.saturating_sub(offset).min(wanted);
        let checksum_end = 60usize.saturating_sub(offset).min(wanted);
        if checksum_start < checksum_end && offset < 60 && offset + wanted > 56 {
            internal.update(&chunk[..checksum_start]);
            internal.update(&[0; 4][..checksum_end - checksum_start]);
            internal.update(&chunk[checksum_end..]);
        } else {
            internal.update(chunk);
        }
        offset += wanted;
    }
    Ok((regular.finalize(), internal.finalize()))
}

fn as_u32(value: usize, field: &str) -> OQLResult<u32> {
    u32::try_from(value)
        .map_err(|_| OQLError::storage_error(&format!("{field} is too large"), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::filesystem::SimulatedFileSystem;
    use std::path::Path;

    #[test]
    fn empty_checkpoint_and_manifest_round_trip() {
        let image = CheckpointImage {
            sequence: 9,
            checkpoint_lsn: 42,
            metadata: CheckpointMetadata {
                collection_ids: Vec::new(),
                next_collection_id: 1,
                next_object_ids: Vec::new(),
                schemas: Vec::new(),
                compiled_schemas: Vec::new(),
                current_schema_versions: Vec::new(),
                scalar_indexes: Vec::new(),
                vector_indexes: Vec::new(),
            },
            records: Vec::new(),
            indexes: Vec::new(),
            typed_segments: Vec::new(),
        };
        let encoded = encode_segment(&image).unwrap();
        let decoded = decode_segment(&encoded).unwrap();
        assert_eq!(decoded.sequence, 9);
        assert_eq!(decoded.checkpoint_lsn, 42);
        let manifest = Manifest {
            sequence: 9,
            checkpoint_lsn: 42,
            segment_len: encoded.len() as u64,
            segment_checksum: crc32fast::hash(&encoded),
            live_objects: 0,
            slot: 1,
            segment_slot: 0,
            delta_len: 4096,
            delta_checksum: 0x1234_5678,
            has_record_root: true,
            has_index_root: true,
        };
        assert_eq!(Manifest::decode(&manifest.encode()).unwrap(), manifest);
    }

    #[test]
    fn production_reader_accepts_format_two_segment_header() {
        let image = CheckpointImage {
            sequence: 3,
            checkpoint_lsn: 8,
            metadata: CheckpointMetadata {
                collection_ids: Vec::new(),
                next_collection_id: 1,
                next_object_ids: Vec::new(),
                schemas: Vec::new(),
                compiled_schemas: Vec::new(),
                current_schema_versions: Vec::new(),
                scalar_indexes: Vec::new(),
                vector_indexes: Vec::new(),
            },
            records: Vec::new(),
            indexes: Vec::new(),
            typed_segments: Vec::new(),
        };
        let mut legacy = encode_segment(&image).unwrap();
        legacy.drain(SEGMENT_HEADER_LEN_V2..SEGMENT_HEADER_LEN);
        put_u16(&mut legacy, 4, 2);
        put_u16(&mut legacy, 6, SEGMENT_HEADER_LEN_V2 as u16);
        let legacy_len = legacy.len() as u64;
        put_u64(&mut legacy, 48, legacy_len);
        put_u32(&mut legacy, 56, 0);
        let internal = checksum_with_zeroed_field(&legacy, 56);
        put_u32(&mut legacy, 56, internal);

        let filesystem = SimulatedFileSystem::new(63);
        let path = Path::new("format-two-segment.owl");
        let mut file = filesystem.create(path).unwrap();
        write_all_handle(file.as_mut(), &legacy).unwrap();
        file.sync().unwrap();
        let decoded = decode_segment_file(
            &filesystem,
            path,
            legacy.len() as u64,
            crc32fast::hash(&legacy),
            SegmentVerification::Audit,
        )
        .unwrap();
        assert_eq!(decoded.sequence, 3);
        assert_eq!(decoded.checkpoint_lsn, 8);
        assert!(decoded.typed_segments.is_empty());
    }

    #[test]
    fn streamed_segment_matches_slice_codec_and_decodes_by_range() {
        let object_ref = ObjRef::from_parts(1, 1, 1);
        let large_index_entries = (0u32..4_096)
            .map(|value| {
                (
                    value.to_be_bytes().to_vec(),
                    vec![ObjRef::from_parts(1, value + 1, 1)],
                )
            })
            .collect();
        let image = CheckpointImage {
            sequence: 7,
            checkpoint_lsn: 11,
            metadata: CheckpointMetadata {
                collection_ids: Vec::new(),
                next_collection_id: 1,
                next_object_ids: Vec::new(),
                schemas: Vec::new(),
                compiled_schemas: Vec::new(),
                current_schema_versions: Vec::new(),
                scalar_indexes: Vec::new(),
                vector_indexes: Vec::new(),
            },
            records: vec![(object_ref, Arc::from(vec![9; 128 * 1024]))],
            indexes: vec![
                CheckpointIndex {
                    schema: "logs".into(),
                    name: "by_timestamp".into(),
                    unique: true,
                    entries: large_index_entries,
                },
                CheckpointIndex {
                    schema: "logs".into(),
                    name: "by_level".into(),
                    unique: false,
                    entries: vec![(b"error".to_vec(), vec![object_ref])],
                },
            ],
            typed_segments: Vec::new(),
        };
        let expected = encode_segment(&image).unwrap();
        let timestamp_ranges = plan_index_pages(&image.indexes[0]).unwrap();
        assert!(timestamp_ranges.len() > 1);
        for (page, range) in timestamp_ranges.iter().cloned().enumerate() {
            assert!(
                encode_index_page(
                    &image.indexes[0],
                    range,
                    page == 0,
                    page + 1 == timestamp_ranges.len(),
                )
                .unwrap()
                .len()
                    <= INDEX_PAGE_TARGET_BYTES
            );
        }
        let filesystem = Arc::new(SimulatedFileSystem::new(44));
        let path = Path::new("streamed-segment.owl");
        let info = write_segment_file(filesystem.as_ref(), path, &image, usize::MAX).unwrap();
        assert_eq!(info.len, expected.len() as u64);
        assert_eq!(info.checksum, get_u32(&expected, 56).unwrap());
        assert_eq!(
            read_file_range(filesystem.as_ref(), path, 0, expected.len()).unwrap(),
            expected
        );
        let decoded = decode_segment_file(
            filesystem.as_ref(),
            path,
            info.len,
            info.checksum,
            SegmentVerification::Audit,
        )
        .unwrap();
        assert_eq!(decoded.sequence, 7);
        assert_eq!(decoded.checkpoint_lsn, 11);
        let mut records = Vec::new();
        decoded
            .records
            .for_each_chunk(filesystem.as_ref(), 1, |chunk| {
                records.extend(chunk);
                Ok(())
            })
            .unwrap();
        assert_eq!(records[0].0, object_ref);
        assert_eq!(records[0].1.len(), 128 * 1024);
        let mut pages = Vec::new();
        decoded
            .indexes
            .for_each(filesystem.as_ref(), |page| {
                pages.push(page);
                Ok(())
            })
            .unwrap();
        assert!(pages.len() > 2);
        assert_eq!(pages[0].name, "by_level");
        assert!(pages[0].first && pages[0].last);
        let timestamp_pages = pages
            .iter()
            .filter(|page| page.name == "by_timestamp")
            .collect::<Vec<_>>();
        assert!(timestamp_pages.first().unwrap().first);
        assert!(timestamp_pages.last().unwrap().last);
        assert_eq!(
            timestamp_pages
                .iter()
                .map(|page| page.posting_count)
                .sum::<usize>(),
            4_096
        );
        assert!(timestamp_pages.iter().all(|page| page.entries.is_empty()));

        let published = info
            .indexes
            .into_iter()
            .find(|index| index.name == "by_timestamp")
            .unwrap();
        let target_key = image.indexes[0].entries[0].0.clone();
        let damaged_page = published.pages.last().unwrap();
        let damaged_offset = damaged_page.location.offset + damaged_page.location.len - 1;
        let damaged_byte =
            read_file_range(filesystem.as_ref(), path, damaged_offset, 1).unwrap()[0];
        let mut file = filesystem.open(path).unwrap();
        file.seek(SeekFrom::Start(damaged_offset as u64)).unwrap();
        write_all_handle(file.as_mut(), &[damaged_byte ^ 0xff]).unwrap();
        file.sync().unwrap();
        let base = FileIndexBase::new(
            filesystem,
            published.schema,
            published.name,
            published.unique,
            published.pages,
        );
        assert_eq!(base.search_key(&target_key).unwrap().len(), 1);
        assert!(!base
            .range_entries(Bound::Unbounded, Bound::Unbounded, Some(1), false)
            .unwrap()
            .is_empty());
        assert!(base
            .range_entries(Bound::Unbounded, Bound::Unbounded, Some(1), true)
            .unwrap_err()
            .contains("checksum"));
        assert!(base
            .search_key(image.indexes[0].entries.last().unwrap().0.as_slice())
            .unwrap_err()
            .contains("checksum"));
    }

    #[test]
    fn legacy_whole_index_block_decodes_as_one_page() {
        let index = CheckpointIndex {
            schema: "events".into(),
            name: "by_level".into(),
            unique: false,
            entries: vec![(b"error".to_vec(), vec![ObjRef::from_parts(1, 1, 1)])],
        };
        let mut block = vec![0; INDEX_BLOCK_HEADER_LEN];
        let payload_len = index.schema.len()
            + index.name.len()
            + index_entry_len(&index.entries[0].0, &index.entries[0].1).unwrap();
        put_u16(&mut block, 0, index.schema.len() as u16);
        put_u16(&mut block, 2, index.name.len() as u16);
        put_u32(&mut block, 4, 1);
        put_u64(&mut block, 8, payload_len as u64);
        block.extend_from_slice(index.schema.as_bytes());
        block.extend_from_slice(index.name.as_bytes());
        block.extend_from_slice(&(index.entries[0].0.len() as u32).to_le_bytes());
        block.extend_from_slice(&(index.entries[0].1.len() as u32).to_le_bytes());
        block.extend_from_slice(&index.entries[0].0);
        block.extend_from_slice(&index.entries[0].1[0].packed().to_le_bytes());
        let checksum = checksum_with_zeroed_field(&block, 16);
        put_u32(&mut block, 16, checksum);
        let page = decode_index_page(
            &block[..INDEX_BLOCK_HEADER_LEN],
            &block[INDEX_BLOCK_HEADER_LEN..],
        )
        .unwrap();
        assert!(page.first && page.last);
        assert_eq!(page.entries, index.entries);
    }

    #[test]
    fn routing_prefix_is_authenticated_without_reading_entry_payload() {
        let index = CheckpointIndex {
            schema: "events".into(),
            name: "incoming".into(),
            unique: false,
            entries: vec![
                (b"first".to_vec(), vec![ObjRef::from_parts(1, 1, 1)]),
                (b"last".to_vec(), vec![ObjRef::from_parts(1, 2, 1)]),
            ],
        };
        let block = encode_index_page(&index, 0..2, true, true).unwrap();
        let routing_len = index.schema.len()
            + index.name.len()
            + INDEX_ROUTING_HEADER_LEN
            + index.entries.first().unwrap().0.len()
            + index.entries.last().unwrap().0.len();
        let header = &block[..INDEX_BLOCK_HEADER_LEN];
        let routing = &block[INDEX_BLOCK_HEADER_LEN..INDEX_BLOCK_HEADER_LEN + routing_len];
        let page = decode_index_page_routing(header, routing).unwrap();
        assert!(page.routing_only);
        assert!(page.entries.is_empty());
        assert_eq!(page.posting_count, 2);

        let mut corrupt_routing = routing.to_vec();
        *corrupt_routing.last_mut().unwrap() ^= 1;
        assert!(decode_index_page_routing(header, &corrupt_routing)
            .err()
            .unwrap()
            .to_string()
            .contains("routing"));

        let mut corrupt_payload = block.clone();
        *corrupt_payload.last_mut().unwrap() ^= 1;
        assert!(decode_index_page_routing(
            &corrupt_payload[..INDEX_BLOCK_HEADER_LEN],
            &corrupt_payload[INDEX_BLOCK_HEADER_LEN..INDEX_BLOCK_HEADER_LEN + routing_len],
        )
        .is_ok());
        assert!(decode_index_page(
            &corrupt_payload[..INDEX_BLOCK_HEADER_LEN],
            &corrupt_payload[INDEX_BLOCK_HEADER_LEN..],
        )
        .err()
        .unwrap()
        .to_string()
        .contains("checksum"));
    }

    #[test]
    fn format_two_manifest_decodes_format_one_roots() {
        let mut legacy = Manifest {
            sequence: 3,
            checkpoint_lsn: 9,
            segment_len: 100,
            segment_checksum: 200,
            live_objects: 4,
            slot: 1,
            segment_slot: 1,
            delta_len: 0,
            delta_checksum: 0,
            has_record_root: true,
            has_index_root: true,
        }
        .encode();
        put_u16(&mut legacy, 4, 1);
        put_u32(&mut legacy, 36, 1);
        put_u64(&mut legacy, 52, 0);
        put_u32(&mut legacy, 60, 0);
        put_u32(&mut legacy, 48, 0);
        let checksum = checksum_with_zeroed_field(&legacy, 48);
        put_u32(&mut legacy, 48, checksum);

        let decoded = Manifest::decode(&legacy).unwrap();
        assert_eq!(decoded.sequence, 3);
        assert_eq!(decoded.slot, 1);
        assert_eq!(decoded.segment_slot, 1);
        assert_eq!(decoded.delta_len, 0);
        assert!(!decoded.has_record_root);
        assert!(!decoded.has_index_root);
    }

    #[test]
    fn segment_and_manifest_corruption_are_detected() {
        let manifest = Manifest {
            sequence: 1,
            checkpoint_lsn: 2,
            segment_len: 3,
            segment_checksum: 4,
            live_objects: 5,
            slot: 1,
            segment_slot: 1,
            delta_len: 0,
            delta_checksum: 0,
            has_record_root: true,
            has_index_root: true,
        };
        let mut bytes = manifest.encode();
        bytes[24] ^= 1;
        assert!(Manifest::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("checksum"));
    }

    #[test]
    fn persisted_index_blocks_round_trip_and_have_independent_checksums() {
        let image = CheckpointImage {
            sequence: 3,
            checkpoint_lsn: 7,
            metadata: CheckpointMetadata {
                collection_ids: Vec::new(),
                next_collection_id: 1,
                next_object_ids: Vec::new(),
                schemas: Vec::new(),
                compiled_schemas: Vec::new(),
                current_schema_versions: Vec::new(),
                scalar_indexes: Vec::new(),
                vector_indexes: Vec::new(),
            },
            records: Vec::new(),
            indexes: vec![CheckpointIndex {
                schema: "events".into(),
                name: "by_level".into(),
                unique: false,
                entries: vec![(
                    vec![1, 2, 3],
                    vec![ObjRef::from_parts(1, 1, 1), ObjRef::from_parts(1, 2, 1)],
                )],
            }],
            typed_segments: Vec::new(),
        };
        let encoded = encode_segment(&image).unwrap();
        let decoded = decode_segment(&encoded).unwrap();
        assert_eq!(decoded.indexes.len(), 1);
        assert_eq!(decoded.indexes[0].entries[0].1.len(), 2);

        let mut corrupted_root = encoded.clone();
        corrupted_root[SEGMENT_HEADER_LEN] ^= 0x20;
        let checksum = checksum_with_zeroed_field(&corrupted_root, 56);
        put_u32(&mut corrupted_root, 56, checksum);
        let error = decode_segment(&corrupted_root).unwrap_err();
        assert!(error.to_string().contains("root checksum"));

        let mut corrupted = encoded;
        let locator_len = get_u64(&corrupted, 72).unwrap() as usize;
        let last = corrupted.len() - locator_len - 1;
        corrupted[last] ^= 0x40;
        let checksum = checksum_with_zeroed_field(&corrupted, 56);
        put_u32(&mut corrupted, 56, checksum);
        let error = decode_segment(&corrupted).unwrap_err();
        assert!(error.to_string().contains("index block checksum"));
    }

    #[test]
    fn packed_typed_locator_round_trips_and_rejects_corruption() {
        let object_ref = ObjRef::from_parts(3, 7, 1);
        let now = chrono::Utc::now();
        let segment = TypedSegment {
            authoritative: true,
            collection_id: 3,
            schema_version: 1,
            object_refs: vec![object_ref],
            document_ids: vec!["event-7".into()],
            document_versions: vec![1],
            created_at: vec![now],
            updated_at: vec![now],
            columns: Vec::new(),
        }
        .encode()
        .unwrap();
        let encoded = encode_typed_locator(&[Arc::from(segment)]).unwrap();
        let locator = decode_typed_locator(Arc::from(encoded.clone())).unwrap();
        assert_eq!(locator.len(), 1);
        assert_eq!(locator.get(object_ref).unwrap(), Some((0, 0)));
        assert_eq!(locator.get(ObjRef::from_parts(3, 6, 1)).unwrap(), None);

        let first_page_payload =
            LOCATOR_HEADER_LEN + LOCATOR_DIRECTORY_LEN_V4 + LOCATOR_PAGE_HEADER_LEN;
        let filesystem = SimulatedFileSystem::new(149);
        let path = Path::new("lazy-locator.owl");
        let mut file = filesystem.create(path).unwrap();
        write_all_handle(file.as_mut(), &encoded).unwrap();
        file.sync().unwrap();
        let lazy = open_typed_locator(&filesystem, path, 0, encoded.len()).unwrap();
        assert_eq!(lazy.cached_page_stats().unwrap(), (0, 0));
        let file = filesystem.open(path).unwrap();
        file.write_at(
            &[encoded[first_page_payload] ^ 0x20],
            first_page_payload as u64,
        )
        .unwrap();
        assert!(lazy
            .get(object_ref)
            .err()
            .unwrap()
            .to_string()
            .contains("page checksum"));

        let mut corrupted_page = encoded.clone();
        corrupted_page[first_page_payload] ^= 0x20;
        assert!(decode_typed_locator(Arc::from(corrupted_page))
            .err()
            .unwrap()
            .to_string()
            .contains("page checksum"));

        let mut corrupted = encoded;
        *corrupted.last_mut().unwrap() ^= 0x40;
        assert!(decode_typed_locator(Arc::from(corrupted)).is_err());
    }

    #[test]
    fn spillable_locator_builder_matches_the_slice_codec_across_runs() {
        let now = chrono::Utc::now();
        let rows = 300usize;
        let object_refs = (1..=rows)
            .rev()
            .map(|object_id| ObjRef::from_parts(7, object_id as u32, 1))
            .collect::<Vec<_>>();
        let segment = TypedSegment {
            authoritative: true,
            collection_id: 7,
            schema_version: 1,
            object_refs,
            document_ids: (0..rows).map(|row| format!("event-{row:04}")).collect(),
            document_versions: vec![1; rows],
            created_at: vec![now; rows],
            updated_at: vec![now; rows],
            columns: Vec::new(),
        }
        .encode()
        .unwrap();
        let segments = vec![Arc::from(segment)];
        let expected = encode_typed_locator(&segments).unwrap();
        let filesystem = SimulatedFileSystem::new(151);
        let base = Path::new("spillable-locator.owl");
        let prepared = prepare_typed_locator(&filesystem, base, 77, &segments).unwrap();
        assert_eq!(prepared.len(), expected.len());
        let pages_path = prepared.pages_path.clone();
        let journal_path = prepared.journal_path.clone();
        assert!(filesystem.exists(&journal_path));
        let output = Path::new("spillable-locator-output.owl");
        let mut file = filesystem.create(output).unwrap();
        let mut checksum = crc32fast::Hasher::new();
        prepared.write_to(file.as_mut(), &mut checksum).unwrap();
        file.sync().unwrap();
        drop(prepared);
        assert!(!filesystem.exists(&pages_path));
        assert!(!filesystem.exists(&journal_path));
        assert_eq!(
            read_file_range(&filesystem, output, 0, expected.len()).unwrap(),
            expected
        );
    }

    #[test]
    fn interrupted_locator_build_artifacts_are_reclaimed_from_the_journal() {
        let filesystem = SimulatedFileSystem::new(157);
        let base = Path::new("interrupted-locator.owl");
        let sequence = 91;
        let initial_runs = LOCATOR_SORT_MERGE_FAN_IN + 1;
        write_locator_build_journal(&filesystem, base, sequence, initial_runs).unwrap();

        let artifacts = [
            locator_temp_path(base, sequence, "run", 0),
            locator_temp_path(base, sequence, "run", initial_runs - 1),
            locator_temp_path(base, sequence, "merge-0", 0),
            locator_temp_path(base, sequence, "merge-0", 1),
            locator_temp_path(base, sequence, "merge-1", 0),
            locator_temp_path(base, sequence, "pages", 0),
        ];
        for artifact in &artifacts {
            filesystem.create(artifact).unwrap().sync().unwrap();
        }

        cleanup_typed_locator_spools(&filesystem, base).unwrap();
        assert!(!filesystem.exists(&locator_journal_path(base)));
        for artifact in artifacts {
            assert!(!filesystem.exists(&artifact));
        }
    }
}
