// The cumulative OWLD writer remains for format tests and legacy recovery
// while fresh checkpoints publish immutable OWGR generations.
#![allow(dead_code)]

use super::ObjRef;
use crate::error::{OQLError, OQLResult};
use crate::simulation::filesystem::FileSystem;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DELTA_MAGIC: [u8; 4] = *b"OWLD";
const DELTA_VERSION: u16 = 3;
const DIRECTORY_PAGED_VERSION: u16 = 3;
const DELTA_HEADER_LEN: usize = 64;
const LEGACY_ENTRY_LEN: usize = 24;
const DIRECTORY_ENTRY_LEN: usize = 32;
const PAGE_DIRECTORY_ENTRY_LEN: usize = 32;
const TARGET_PAGE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordPageDelta {
    pub base_lsn: u64,
    pub target_lsn: u64,
    pub puts: Vec<(ObjRef, Arc<[u8]>)>,
    pub deletes: Vec<ObjRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Used by the next file-backed lazy record-root layer.
pub(crate) enum RecordDeltaLookup {
    Put(Arc<[u8]>),
    Delete,
    Missing,
}

pub(crate) struct RecordRootHandle {
    filesystem: Arc<dyn FileSystem>,
    path: PathBuf,
    prefix: Vec<u8>,
    file_len: usize,
    entry_count: usize,
    page_count: usize,
    page_directory_start: usize,
    data_start: usize,
    paged_entries: bool,
    base_lsn: u64,
    target_lsn: u64,
    authority_checksum: u32,
    legacy: Option<RecordPageDelta>,
}

impl RecordRootHandle {
    pub(crate) fn open(filesystem: Arc<dyn FileSystem>, path: &Path) -> OQLResult<Self> {
        let file_len = usize::try_from(filesystem.file_size(path).map_err(|error| {
            OQLError::storage_error(&format!("inspect record root: {error}"), None)
        })?)
        .map_err(|_| OQLError::storage_error("record root is too large to address", None))?;
        if file_len < DELTA_HEADER_LEN {
            return Err(OQLError::storage_error("record root is truncated", None));
        }
        let header = read_file_range(filesystem.as_ref(), path, 0, DELTA_HEADER_LEN)?;
        if header[0..4] != DELTA_MAGIC {
            return Err(OQLError::storage_error("invalid record delta header", None));
        }
        let version = get_u16(&header, 4)?;
        if version == 1 {
            let bytes = read_file_range(filesystem.as_ref(), path, 0, file_len)?;
            let legacy = RecordPageDelta::decode_v1(&bytes)?;
            let base_lsn = legacy.base_lsn;
            let target_lsn = legacy.target_lsn;
            return Ok(Self {
                filesystem,
                path: path.to_path_buf(),
                prefix: Vec::new(),
                file_len,
                entry_count: 0,
                page_count: 0,
                page_directory_start: 0,
                data_start: 0,
                paged_entries: false,
                base_lsn,
                target_lsn,
                authority_checksum: crc32fast::hash(&bytes),
                legacy: Some(legacy),
            });
        }
        let directory_len = usize::try_from(get_u64(&header, 32)?)
            .map_err(|_| OQLError::storage_error("record root directory is too large", None))?;
        let prefix_len = DELTA_HEADER_LEN
            .checked_add(directory_len)
            .filter(|length| *length <= file_len)
            .ok_or_else(|| OQLError::storage_error("record root directory is truncated", None))?;
        let prefix = read_file_range(filesystem.as_ref(), path, 0, prefix_len)?;
        let (entry_count, page_count, page_directory_start, data_start, paged_entries) =
            if version == 2 {
                let (entries, pages, page_start, data_start) =
                    validate_v2_directory(&prefix, file_len)?;
                (entries, pages, page_start, data_start, false)
            } else if version == DIRECTORY_PAGED_VERSION {
                let (entries, pages, data_start) = validate_v3_directory(&prefix, file_len)?;
                (entries, pages, DELTA_HEADER_LEN, data_start, true)
            } else {
                return Err(OQLError::storage_error(
                    "unsupported record delta format",
                    None,
                ));
            };
        let base_lsn = get_u64(&prefix, 8)?;
        let target_lsn = get_u64(&prefix, 16)?;
        let authority_checksum = crc32fast::hash(&prefix);
        Ok(Self {
            filesystem,
            path: path.to_path_buf(),
            prefix,
            file_len,
            entry_count,
            page_count,
            page_directory_start,
            data_start,
            paged_entries,
            base_lsn,
            target_lsn,
            authority_checksum,
            legacy: None,
        })
    }

    pub(crate) fn base_lsn(&self) -> u64 {
        self.base_lsn
    }

    pub(crate) fn target_lsn(&self) -> u64 {
        self.target_lsn
    }

    pub(crate) fn authority_checksum(&self) -> u32 {
        self.authority_checksum
    }

    pub(crate) fn lookup(&self, object_ref: ObjRef) -> OQLResult<RecordDeltaLookup> {
        if let Some(legacy) = &self.legacy {
            if let Some((_, record)) = legacy
                .puts
                .iter()
                .find(|(candidate, _)| *candidate == object_ref)
            {
                return Ok(RecordDeltaLookup::Put(record.clone()));
            }
            return Ok(if legacy.deletes.contains(&object_ref) {
                RecordDeltaLookup::Delete
            } else {
                RecordDeltaLookup::Missing
            });
        }
        if self.paged_entries {
            return self.lookup_v3(object_ref);
        }
        let Some(cursor) = find_object_entry(&self.prefix, self.entry_count, object_ref)? else {
            return Ok(RecordDeltaLookup::Missing);
        };
        let operation = self.prefix[cursor];
        let offset = usize::try_from(get_u64(&self.prefix, cursor + 16)?)
            .map_err(|_| OQLError::storage_error("record root offset overflow", None))?;
        let length = get_u32(&self.prefix, cursor + 24)? as usize;
        let checksum = get_u32(&self.prefix, cursor + 28)?;
        if operation == 1 {
            return if offset == 0 && length == 0 && checksum == 0 {
                Ok(RecordDeltaLookup::Delete)
            } else {
                Err(OQLError::storage_error(
                    "record root tombstone has a payload",
                    None,
                ))
            };
        }
        let (page_cursor, page_offset, page_end) = find_page_entry(
            &self.prefix,
            self.page_directory_start,
            self.page_count,
            object_ref,
        )?
        .ok_or_else(|| OQLError::storage_error("record root entry has no page", None))?;
        let end = offset
            .checked_add(length)
            .filter(|end| offset >= page_offset && *end <= page_end && *end <= self.file_len)
            .ok_or_else(|| {
                OQLError::storage_error("record root entry is outside its page", None)
            })?;
        if page_offset < self.data_start {
            return Err(OQLError::storage_error(
                "record root page overlaps its directory",
                None,
            ));
        }
        let page = read_file_range(
            self.filesystem.as_ref(),
            &self.path,
            page_offset,
            page_end - page_offset,
        )?;
        if crc32fast::hash(&page) != get_u32(&self.prefix, page_cursor + 28)? {
            return Err(OQLError::storage_error(
                "record root page checksum is invalid",
                None,
            ));
        }
        let relative_start = offset - page_offset;
        let relative_end = end - page_offset;
        let record = &page[relative_start..relative_end];
        if crc32fast::hash(record) != checksum {
            return Err(OQLError::storage_error(
                "record root entry checksum is invalid",
                None,
            ));
        }
        Ok(RecordDeltaLookup::Put(Arc::from(record)))
    }

    fn lookup_v3(&self, object_ref: ObjRef) -> OQLResult<RecordDeltaLookup> {
        let Some((page_cursor, page_offset, page_end)) = find_page_entry(
            &self.prefix,
            self.page_directory_start,
            self.page_count,
            object_ref,
        )?
        else {
            return Ok(RecordDeltaLookup::Missing);
        };
        if page_offset < self.data_start || page_end > self.file_len {
            return Err(OQLError::storage_error(
                "record root page is outside the file",
                None,
            ));
        }
        let page = read_file_range(
            self.filesystem.as_ref(),
            &self.path,
            page_offset,
            page_end - page_offset,
        )?;
        if crc32fast::hash(&page) != get_u32(&self.prefix, page_cursor + 28)? {
            return Err(OQLError::storage_error(
                "record root page checksum is invalid",
                None,
            ));
        }
        lookup_v3_page(&page, object_ref)
    }
}

impl RecordPageDelta {
    pub(crate) fn authority_checksum(bytes: &[u8]) -> OQLResult<u32> {
        if bytes.len() < DELTA_HEADER_LEN || bytes[0..4] != DELTA_MAGIC {
            return Err(OQLError::storage_error("invalid record delta header", None));
        }
        let version = get_u16(bytes, 4)?;
        if version == 1 {
            Self::decode_v1(bytes)?;
            return Ok(crc32fast::hash(bytes));
        }
        let prefix_len = DELTA_HEADER_LEN
            .checked_add(get_u64(bytes, 32)? as usize)
            .filter(|length| *length <= bytes.len())
            .ok_or_else(|| OQLError::storage_error("record root directory is truncated", None))?;
        match version {
            2 => {
                validate_v2_directory(&bytes[..prefix_len], bytes.len())?;
            }
            DIRECTORY_PAGED_VERSION => {
                validate_v3_directory(&bytes[..prefix_len], bytes.len())?;
            }
            _ => {
                return Err(OQLError::storage_error(
                    "unsupported record delta format",
                    None,
                ));
            }
        }
        Ok(crc32fast::hash(&bytes[..prefix_len]))
    }

    pub(crate) fn merge(base: &Self, next: &Self) -> OQLResult<Self> {
        if base.target_lsn != next.base_lsn {
            return Err(OQLError::storage_error(
                "record delta chain is not contiguous",
                None,
            ));
        }
        let mut puts = base.puts.iter().cloned().collect::<BTreeMap<_, _>>();
        let mut deletes = base.deletes.iter().copied().collect::<BTreeSet<_>>();
        for (object_ref, record) in &next.puts {
            deletes.remove(object_ref);
            puts.insert(*object_ref, record.clone());
        }
        for object_ref in &next.deletes {
            puts.remove(object_ref);
            deletes.insert(*object_ref);
        }
        Ok(Self {
            base_lsn: base.base_lsn,
            target_lsn: next.target_lsn,
            puts: puts.into_iter().collect(),
            deletes: deletes.into_iter().collect(),
        })
    }
    pub(crate) fn encode(&self) -> OQLResult<Vec<u8>> {
        if self.target_lsn < self.base_lsn {
            return Err(OQLError::storage_error(
                "record delta target LSN precedes its base",
                None,
            ));
        }
        let mut entries = self
            .puts
            .iter()
            .map(|(object_ref, record)| (*object_ref, Some(record.clone())))
            .chain(
                self.deletes
                    .iter()
                    .copied()
                    .map(|object_ref| (object_ref, None)),
            )
            .collect::<Vec<_>>();
        entries.sort_by_key(|(object_ref, _)| object_ref.packed());
        let mut seen = HashSet::with_capacity(entries.len());
        for (object_ref, _) in &entries {
            if object_ref.is_null() || !seen.insert(*object_ref) {
                return Err(OQLError::storage_error(
                    "record delta contains an invalid or duplicate object reference",
                    None,
                ));
            }
        }

        struct Page {
            first: ObjRef,
            last: ObjRef,
            entries: Vec<(ObjRef, Option<Arc<[u8]>>)>,
            payload: Vec<u8>,
        }
        let mut pages = Vec::<Page>::new();
        for (object_ref, record) in entries.iter().cloned() {
            let encoded_len = DIRECTORY_ENTRY_LEN
                .saturating_add(record.as_ref().map_or(0, |record| record.len()));
            if pages.last().is_none_or(|page| {
                !page.entries.is_empty()
                    && page.payload.len().saturating_add(encoded_len) > TARGET_PAGE_BYTES
            }) {
                pages.push(Page {
                    first: object_ref,
                    last: object_ref,
                    entries: Vec::new(),
                    payload: Vec::new(),
                });
            }
            let page = pages.last_mut().expect("record page was just created");
            page.payload
                .resize(page.payload.len().saturating_add(encoded_len), 0);
            page.entries.push((object_ref, record));
            page.last = object_ref;
        }
        for page in &mut pages {
            let directory_len = 8usize
                .checked_add(
                    page.entries
                        .len()
                        .checked_mul(DIRECTORY_ENTRY_LEN)
                        .ok_or_else(|| {
                            OQLError::storage_error("record delta page directory overflow", None)
                        })?,
                )
                .ok_or_else(|| {
                    OQLError::storage_error("record delta page directory overflow", None)
                })?;
            let record_bytes = page
                .entries
                .iter()
                .map(|(_, record)| record.as_ref().map_or(0, |record| record.len()))
                .sum::<usize>();
            page.payload = vec![0; directory_len.saturating_add(record_bytes)];
            put_u32(&mut page.payload, 0, page.entries.len() as u32);
            put_u32(&mut page.payload, 4, directory_len as u32);
            let mut record_cursor = directory_len;
            for (entry_index, (object_ref, record)) in page.entries.iter().enumerate() {
                let cursor = 8 + entry_index * DIRECTORY_ENTRY_LEN;
                page.payload[cursor] = u8::from(record.is_none());
                put_u64(&mut page.payload, cursor + 8, object_ref.packed());
                if let Some(record) = record {
                    put_u64(&mut page.payload, cursor + 16, record_cursor as u64);
                    put_u32(
                        &mut page.payload,
                        cursor + 24,
                        u32::try_from(record.len()).map_err(|_| {
                            OQLError::storage_error("delta record is too large", None)
                        })?,
                    );
                    put_u32(&mut page.payload, cursor + 28, crc32fast::hash(record));
                    page.payload[record_cursor..record_cursor + record.len()]
                        .copy_from_slice(record);
                    record_cursor += record.len();
                }
            }
        }
        let page_directory_len = pages
            .len()
            .checked_mul(PAGE_DIRECTORY_ENTRY_LEN)
            .ok_or_else(|| OQLError::storage_error("record delta page directory overflow", None))?;
        let directory_len = page_directory_len;
        let data_start = DELTA_HEADER_LEN
            .checked_add(directory_len)
            .ok_or_else(|| OQLError::storage_error("record delta length overflow", None))?;
        let total_len = pages.iter().try_fold(data_start, |length, page| {
            length
                .checked_add(page.payload.len())
                .ok_or_else(|| OQLError::storage_error("record delta length overflow", None))
        })?;
        let mut page_offsets = Vec::with_capacity(pages.len());
        let mut next_offset = data_start;
        for page in &pages {
            page_offsets.push(next_offset);
            next_offset += page.payload.len();
        }

        let mut bytes = vec![0u8; total_len];
        bytes[0..4].copy_from_slice(&DELTA_MAGIC);
        put_u16(&mut bytes, 4, DELTA_VERSION);
        put_u16(&mut bytes, 6, DELTA_HEADER_LEN as u16);
        put_u64(&mut bytes, 8, self.base_lsn);
        put_u64(&mut bytes, 16, self.target_lsn);
        put_u32(
            &mut bytes,
            24,
            u32::try_from(self.puts.len())
                .map_err(|_| OQLError::storage_error("too many record delta puts", None))?,
        );
        put_u32(
            &mut bytes,
            28,
            u32::try_from(self.deletes.len())
                .map_err(|_| OQLError::storage_error("too many record delta deletes", None))?,
        );
        put_u64(&mut bytes, 32, directory_len as u64);
        put_u64(&mut bytes, 40, total_len as u64);
        put_u32(
            &mut bytes,
            52,
            u32::try_from(pages.len())
                .map_err(|_| OQLError::storage_error("too many record delta pages", None))?,
        );
        put_u16(&mut bytes, 56, DIRECTORY_ENTRY_LEN as u16);
        put_u16(&mut bytes, 58, PAGE_DIRECTORY_ENTRY_LEN as u16);
        put_u32(&mut bytes, 60, TARGET_PAGE_BYTES as u32);

        let mut cursor = DELTA_HEADER_LEN;
        for (page, offset) in pages.iter().zip(&page_offsets) {
            put_u64(&mut bytes, cursor, page.first.packed());
            put_u64(&mut bytes, cursor + 8, page.last.packed());
            put_u64(&mut bytes, cursor + 16, *offset as u64);
            put_u32(
                &mut bytes,
                cursor + 24,
                u32::try_from(page.payload.len())
                    .map_err(|_| OQLError::storage_error("record delta page is too large", None))?,
            );
            put_u32(&mut bytes, cursor + 28, crc32fast::hash(&page.payload));
            cursor += PAGE_DIRECTORY_ENTRY_LEN;
        }
        for (page, offset) in pages.iter().zip(page_offsets) {
            bytes[offset..offset + page.payload.len()].copy_from_slice(&page.payload);
        }
        let checksum = checksum_with_zeroed_field(&bytes[..data_start], 48);
        put_u32(&mut bytes, 48, checksum);
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> OQLResult<Self> {
        if bytes.len() < DELTA_HEADER_LEN || bytes[0..4] != DELTA_MAGIC {
            return Err(OQLError::storage_error("invalid record delta header", None));
        }
        let version = get_u16(bytes, 4)?;
        if version == 1 {
            return Self::decode_v1(bytes);
        }
        match version {
            2 => Self::decode_v2(bytes),
            DIRECTORY_PAGED_VERSION => Self::decode_v3(bytes),
            _ => Err(OQLError::storage_error(
                "unsupported record delta format",
                None,
            )),
        }
    }

    /// Look up one object without checksumming or decoding unrelated pages.
    /// The header and complete directory remain authenticated before the
    /// selected page is accessed.
    #[allow(dead_code)] // Kept internal until Store retains a lazy root handle.
    pub(crate) fn lookup(bytes: &[u8], object_ref: ObjRef) -> OQLResult<RecordDeltaLookup> {
        if bytes.len() < DELTA_HEADER_LEN || bytes[0..4] != DELTA_MAGIC {
            return Err(OQLError::storage_error("invalid record delta header", None));
        }
        let version = get_u16(bytes, 4)?;
        if version == 1 {
            let delta = Self::decode_v1(bytes)?;
            if let Some((_, record)) = delta
                .puts
                .into_iter()
                .find(|(candidate, _)| *candidate == object_ref)
            {
                return Ok(RecordDeltaLookup::Put(record));
            }
            return Ok(if delta.deletes.contains(&object_ref) {
                RecordDeltaLookup::Delete
            } else {
                RecordDeltaLookup::Missing
            });
        }
        if version == DIRECTORY_PAGED_VERSION {
            let (_, page_count, _) = validate_v3_directory(bytes, bytes.len())?;
            let Some((page_cursor, page_offset, page_end)) =
                find_page_entry(bytes, DELTA_HEADER_LEN, page_count, object_ref)?
            else {
                return Ok(RecordDeltaLookup::Missing);
            };
            let page = &bytes[page_offset..page_end];
            if crc32fast::hash(page) != get_u32(bytes, page_cursor + 28)? {
                return Err(OQLError::storage_error(
                    "record root page checksum is invalid",
                    None,
                ));
            }
            return lookup_v3_page(page, object_ref);
        }
        let (entry_count, page_count, page_directory_start, data_start) =
            validate_v2_directory(bytes, bytes.len())?;
        let mut low = 0usize;
        let mut high = entry_count;
        while low < high {
            let middle = low + (high - low) / 2;
            let cursor = DELTA_HEADER_LEN + middle * DIRECTORY_ENTRY_LEN;
            let candidate = ObjRef::new(get_u64(bytes, cursor + 8)?);
            match candidate.cmp(&object_ref) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    let operation = bytes[cursor];
                    let offset = usize::try_from(get_u64(bytes, cursor + 16)?).map_err(|_| {
                        OQLError::storage_error("record delta offset overflow", None)
                    })?;
                    let length = get_u32(bytes, cursor + 24)? as usize;
                    let checksum = get_u32(bytes, cursor + 28)?;
                    if operation == 1 {
                        return if offset == 0 && length == 0 && checksum == 0 {
                            Ok(RecordDeltaLookup::Delete)
                        } else {
                            Err(OQLError::storage_error(
                                "record delta tombstone has a payload",
                                None,
                            ))
                        };
                    }
                    if operation != 0 {
                        return Err(OQLError::storage_error(
                            "invalid record delta operation",
                            None,
                        ));
                    }
                    let end = offset
                        .checked_add(length)
                        .filter(|end| *end <= bytes.len())
                        .ok_or_else(|| {
                            OQLError::storage_error("record delta payload is truncated", None)
                        })?;
                    let mut page_low = 0usize;
                    let mut page_high = page_count;
                    let mut page = None;
                    while page_low < page_high {
                        let page_id = page_low + (page_high - page_low) / 2;
                        let page_cursor = page_directory_start + page_id * PAGE_DIRECTORY_ENTRY_LEN;
                        let first = ObjRef::new(get_u64(bytes, page_cursor)?);
                        let last = ObjRef::new(get_u64(bytes, page_cursor + 8)?);
                        if object_ref < first {
                            page_high = page_id;
                        } else if object_ref > last {
                            page_low = page_id + 1;
                        } else {
                            let page_offset = usize::try_from(get_u64(bytes, page_cursor + 16)?)
                                .map_err(|_| {
                                    OQLError::storage_error(
                                        "record delta page offset overflow",
                                        None,
                                    )
                                })?;
                            let page_length = get_u32(bytes, page_cursor + 24)? as usize;
                            let page_end =
                                page_offset.checked_add(page_length).ok_or_else(|| {
                                    OQLError::storage_error(
                                        "record delta page length overflow",
                                        None,
                                    )
                                })?;
                            if offset >= page_offset && end <= page_end {
                                page = Some((page_cursor, page_offset, page_end));
                            }
                            break;
                        }
                    }
                    let Some((page_cursor, page_offset, page_end)) = page else {
                        return Err(OQLError::storage_error(
                            "record delta entry is outside its page",
                            None,
                        ));
                    };
                    if page_offset < data_start
                        || page_end > bytes.len()
                        || crc32fast::hash(&bytes[page_offset..page_end])
                            != get_u32(bytes, page_cursor + 28)?
                        || crc32fast::hash(&bytes[offset..end]) != checksum
                    {
                        return Err(OQLError::storage_error(
                            "record delta page or entry checksum is invalid",
                            None,
                        ));
                    }
                    return Ok(RecordDeltaLookup::Put(Arc::from(&bytes[offset..end])));
                }
            }
        }
        Ok(RecordDeltaLookup::Missing)
    }

    fn decode_v2(bytes: &[u8]) -> OQLResult<Self> {
        let base_lsn = get_u64(bytes, 8)?;
        let target_lsn = get_u64(bytes, 16)?;
        let put_count = get_u32(bytes, 24)? as usize;
        let delete_count = get_u32(bytes, 28)? as usize;
        let (entry_count, page_count, page_directory_start, data_start) =
            validate_v2_directory(bytes, bytes.len())?;
        let mut pages = Vec::with_capacity(page_count);
        let mut previous_last = None;
        let mut expected_offset = data_start;
        for page_id in 0..page_count {
            let cursor = page_directory_start + page_id * PAGE_DIRECTORY_ENTRY_LEN;
            let first = ObjRef::new(get_u64(bytes, cursor)?);
            let last = ObjRef::new(get_u64(bytes, cursor + 8)?);
            let offset = usize::try_from(get_u64(bytes, cursor + 16)?)
                .map_err(|_| OQLError::storage_error("record delta page offset overflow", None))?;
            let length = get_u32(bytes, cursor + 24)? as usize;
            let checksum = get_u32(bytes, cursor + 28)?;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| OQLError::storage_error("record delta page is truncated", None))?;
            if first.is_null()
                || last < first
                || previous_last.is_some_and(|previous| previous >= first)
                || offset != expected_offset
                || length == 0
                || crc32fast::hash(&bytes[offset..end]) != checksum
            {
                return Err(OQLError::storage_error(
                    "record delta page directory or checksum is invalid",
                    None,
                ));
            }
            pages.push((first, last, offset, end));
            previous_last = Some(last);
            expected_offset = end;
        }
        if expected_offset != bytes.len() {
            return Err(OQLError::storage_error(
                "record delta pages do not cover the payload",
                None,
            ));
        }
        let mut puts = Vec::with_capacity(put_count);
        let mut deletes = Vec::with_capacity(delete_count);
        let mut previous_ref = None;
        let mut cursor = DELTA_HEADER_LEN;
        for _ in 0..entry_count {
            let entry_end = cursor
                .checked_add(DIRECTORY_ENTRY_LEN)
                .filter(|end| *end <= page_directory_start)
                .ok_or_else(|| OQLError::storage_error("record delta is truncated", None))?;
            let operation = bytes[cursor];
            if operation > 1 || bytes[cursor + 1..cursor + 8].iter().any(|byte| *byte != 0) {
                return Err(OQLError::storage_error(
                    "invalid record delta operation",
                    None,
                ));
            }
            let object_ref = ObjRef::new(get_u64(bytes, cursor + 8)?);
            if object_ref.is_null() || previous_ref.is_some_and(|previous| previous >= object_ref) {
                return Err(OQLError::storage_error(
                    "record delta object directory is not strictly ordered",
                    None,
                ));
            }
            previous_ref = Some(object_ref);
            let offset = usize::try_from(get_u64(bytes, cursor + 16)?)
                .map_err(|_| OQLError::storage_error("record delta offset overflow", None))?;
            let length = get_u32(bytes, cursor + 24)? as usize;
            let checksum = get_u32(bytes, cursor + 28)?;
            cursor = entry_end;
            if operation == 1 {
                if offset != 0 || length != 0 || checksum != 0 {
                    return Err(OQLError::storage_error(
                        "record delta tombstone has a payload",
                        None,
                    ));
                }
                deletes.push(object_ref);
                continue;
            }
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| {
                    OQLError::storage_error("record delta payload is truncated", None)
                })?;
            let page = pages
                .iter()
                .find(|(first, last, page_start, page_end)| {
                    object_ref >= *first
                        && object_ref <= *last
                        && offset >= *page_start
                        && end <= *page_end
                })
                .ok_or_else(|| {
                    OQLError::storage_error("record delta entry is outside its page", None)
                })?;
            let _ = page;
            let record = &bytes[offset..end];
            if length == 0 || crc32fast::hash(record) != checksum {
                return Err(OQLError::storage_error(
                    "record delta entry checksum is invalid",
                    None,
                ));
            }
            puts.push((object_ref, Arc::from(record)));
        }
        if cursor != page_directory_start
            || puts.len() != put_count
            || deletes.len() != delete_count
        {
            return Err(OQLError::storage_error(
                "record delta counts or trailing bytes are invalid",
                None,
            ));
        }
        Ok(Self {
            base_lsn,
            target_lsn,
            puts,
            deletes,
        })
    }

    fn decode_v3(bytes: &[u8]) -> OQLResult<Self> {
        let (entry_count, page_count, data_start) = validate_v3_directory(bytes, bytes.len())?;
        let mut puts = Vec::with_capacity(get_u32(bytes, 24)? as usize);
        let mut deletes = Vec::with_capacity(get_u32(bytes, 28)? as usize);
        let mut expected_offset = data_start;
        for page_id in 0..page_count {
            let cursor = DELTA_HEADER_LEN + page_id * PAGE_DIRECTORY_ENTRY_LEN;
            let offset = usize::try_from(get_u64(bytes, cursor + 16)?)
                .map_err(|_| OQLError::storage_error("record root page offset overflow", None))?;
            let length = get_u32(bytes, cursor + 24)? as usize;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| OQLError::storage_error("record root page is truncated", None))?;
            if offset != expected_offset
                || crc32fast::hash(&bytes[offset..end]) != get_u32(bytes, cursor + 28)?
            {
                return Err(OQLError::storage_error(
                    "record root page layout or checksum is invalid",
                    None,
                ));
            }
            decode_v3_page(&bytes[offset..end], &mut puts, &mut deletes)?;
            expected_offset = end;
        }
        if expected_offset != bytes.len()
            || puts.len() != get_u32(bytes, 24)? as usize
            || deletes.len() != get_u32(bytes, 28)? as usize
            || puts.len().saturating_add(deletes.len()) != entry_count
        {
            return Err(OQLError::storage_error(
                "record root counts or trailing bytes are invalid",
                None,
            ));
        }
        Ok(Self {
            base_lsn: get_u64(bytes, 8)?,
            target_lsn: get_u64(bytes, 16)?,
            puts,
            deletes,
        })
    }

    fn decode_v1(bytes: &[u8]) -> OQLResult<Self> {
        if get_u16(bytes, 6)? as usize != DELTA_HEADER_LEN
            || get_u64(bytes, 40)? != bytes.len() as u64
            || checksum_with_zeroed_field(bytes, 48) != get_u32(bytes, 48)?
        {
            return Err(OQLError::storage_error(
                "record delta header or checksum is invalid",
                None,
            ));
        }
        let base_lsn = get_u64(bytes, 8)?;
        let target_lsn = get_u64(bytes, 16)?;
        if target_lsn < base_lsn || get_u64(bytes, 32)? as usize != bytes.len() - DELTA_HEADER_LEN {
            return Err(OQLError::storage_error(
                "record delta LSN or payload length is invalid",
                None,
            ));
        }
        let put_count = get_u32(bytes, 24)? as usize;
        let delete_count = get_u32(bytes, 28)? as usize;
        let entry_count = put_count
            .checked_add(delete_count)
            .ok_or_else(|| OQLError::storage_error("record delta entry count overflow", None))?;
        let mut puts = Vec::with_capacity(put_count);
        let mut deletes = Vec::with_capacity(delete_count);
        let mut seen = HashSet::with_capacity(entry_count);
        let mut cursor = DELTA_HEADER_LEN;
        for _ in 0..entry_count {
            let entry_end = cursor
                .checked_add(LEGACY_ENTRY_LEN)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| OQLError::storage_error("record delta is truncated", None))?;
            let operation = bytes[cursor];
            if operation > 1 || bytes[cursor + 1..cursor + 8].iter().any(|byte| *byte != 0) {
                return Err(OQLError::storage_error(
                    "invalid record delta operation",
                    None,
                ));
            }
            let object_ref = ObjRef::new(get_u64(bytes, cursor + 8)?);
            if object_ref.is_null() || !seen.insert(object_ref) {
                return Err(OQLError::storage_error(
                    "record delta contains an invalid or duplicate object reference",
                    None,
                ));
            }
            let length = get_u32(bytes, cursor + 16)? as usize;
            let checksum = get_u32(bytes, cursor + 20)?;
            cursor = entry_end;
            let end = cursor
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| {
                    OQLError::storage_error("record delta payload is truncated", None)
                })?;
            let record = &bytes[cursor..end];
            if crc32fast::hash(record) != checksum || (operation == 1 && !record.is_empty()) {
                return Err(OQLError::storage_error(
                    "record delta entry checksum or tombstone is invalid",
                    None,
                ));
            }
            if operation == 0 {
                puts.push((object_ref, Arc::from(record)));
            } else {
                deletes.push(object_ref);
            }
            cursor = end;
        }
        if cursor != bytes.len() || puts.len() != put_count || deletes.len() != delete_count {
            return Err(OQLError::storage_error(
                "record delta counts or trailing bytes are invalid",
                None,
            ));
        }
        Ok(Self {
            base_lsn,
            target_lsn,
            puts,
            deletes,
        })
    }
}

fn validate_v3_directory(bytes: &[u8], file_len: usize) -> OQLResult<(usize, usize, usize)> {
    if get_u16(bytes, 4)? != DIRECTORY_PAGED_VERSION
        || get_u16(bytes, 6)? as usize != DELTA_HEADER_LEN
        || get_u64(bytes, 40)? != file_len as u64
        || get_u16(bytes, 56)? as usize != DIRECTORY_ENTRY_LEN
        || get_u16(bytes, 58)? as usize != PAGE_DIRECTORY_ENTRY_LEN
    {
        return Err(OQLError::storage_error(
            "record root header is invalid",
            None,
        ));
    }
    if get_u64(bytes, 16)? < get_u64(bytes, 8)? {
        return Err(OQLError::storage_error(
            "record root LSN range is invalid",
            None,
        ));
    }
    let entry_count = (get_u32(bytes, 24)? as usize)
        .checked_add(get_u32(bytes, 28)? as usize)
        .ok_or_else(|| OQLError::storage_error("record root entry count overflow", None))?;
    let page_count = get_u32(bytes, 52)? as usize;
    let directory_len = page_count
        .checked_mul(PAGE_DIRECTORY_ENTRY_LEN)
        .ok_or_else(|| OQLError::storage_error("record root directory overflow", None))?;
    let data_start = DELTA_HEADER_LEN
        .checked_add(directory_len)
        .filter(|start| *start <= bytes.len())
        .ok_or_else(|| OQLError::storage_error("record root directory is truncated", None))?;
    if get_u64(bytes, 32)? as usize != directory_len
        || checksum_with_zeroed_field(&bytes[..data_start], 48) != get_u32(bytes, 48)?
        || (entry_count == 0) != (page_count == 0)
    {
        return Err(OQLError::storage_error(
            "record root directory is invalid",
            None,
        ));
    }
    let mut previous_last = None;
    let mut expected_offset = data_start;
    for page_id in 0..page_count {
        let cursor = DELTA_HEADER_LEN + page_id * PAGE_DIRECTORY_ENTRY_LEN;
        let first = ObjRef::new(get_u64(bytes, cursor)?);
        let last = ObjRef::new(get_u64(bytes, cursor + 8)?);
        let offset = usize::try_from(get_u64(bytes, cursor + 16)?)
            .map_err(|_| OQLError::storage_error("record root page offset overflow", None))?;
        let length = get_u32(bytes, cursor + 24)? as usize;
        if first.is_null()
            || last < first
            || previous_last.is_some_and(|previous| previous >= first)
            || offset != expected_offset
            || length < 8
        {
            return Err(OQLError::storage_error(
                "record root page directory is invalid",
                None,
            ));
        }
        expected_offset = offset
            .checked_add(length)
            .filter(|end| *end <= file_len)
            .ok_or_else(|| OQLError::storage_error("record root page is truncated", None))?;
        previous_last = Some(last);
    }
    if expected_offset != file_len {
        return Err(OQLError::storage_error(
            "record root pages do not cover the file",
            None,
        ));
    }
    Ok((entry_count, page_count, data_start))
}

fn lookup_v3_page(page: &[u8], object_ref: ObjRef) -> OQLResult<RecordDeltaLookup> {
    let entry_count = validate_v3_page_header(page)?;
    let mut low = 0usize;
    let mut high = entry_count;
    while low < high {
        let middle = low + (high - low) / 2;
        let cursor = 8 + middle * DIRECTORY_ENTRY_LEN;
        match ObjRef::new(get_u64(page, cursor + 8)?).cmp(&object_ref) {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
            std::cmp::Ordering::Equal => return decode_v3_page_entry(page, cursor),
        }
    }
    Ok(RecordDeltaLookup::Missing)
}

fn validate_v3_page_header(page: &[u8]) -> OQLResult<usize> {
    if page.len() < 8 {
        return Err(OQLError::storage_error(
            "record root page is truncated",
            None,
        ));
    }
    let entry_count = get_u32(page, 0)? as usize;
    let directory_len = 8usize
        .checked_add(
            entry_count
                .checked_mul(DIRECTORY_ENTRY_LEN)
                .ok_or_else(|| OQLError::storage_error("record page overflow", None))?,
        )
        .ok_or_else(|| OQLError::storage_error("record page overflow", None))?;
    if entry_count == 0 || get_u32(page, 4)? as usize != directory_len || directory_len > page.len()
    {
        return Err(OQLError::storage_error(
            "record root page directory is invalid",
            None,
        ));
    }
    let mut previous = None;
    for entry in 0..entry_count {
        let cursor = 8 + entry * DIRECTORY_ENTRY_LEN;
        if page[cursor] > 1 || page[cursor + 1..cursor + 8].iter().any(|byte| *byte != 0) {
            return Err(OQLError::storage_error(
                "record root page operation is invalid",
                None,
            ));
        }
        let object_ref = ObjRef::new(get_u64(page, cursor + 8)?);
        if object_ref.is_null() || previous.is_some_and(|value| value >= object_ref) {
            return Err(OQLError::storage_error(
                "record root page is not strictly ordered",
                None,
            ));
        }
        previous = Some(object_ref);
    }
    Ok(entry_count)
}

fn decode_v3_page_entry(page: &[u8], cursor: usize) -> OQLResult<RecordDeltaLookup> {
    let operation = page[cursor];
    let offset = usize::try_from(get_u64(page, cursor + 16)?)
        .map_err(|_| OQLError::storage_error("record page offset overflow", None))?;
    let length = get_u32(page, cursor + 24)? as usize;
    let checksum = get_u32(page, cursor + 28)?;
    if operation == 1 {
        return if offset == 0 && length == 0 && checksum == 0 {
            Ok(RecordDeltaLookup::Delete)
        } else {
            Err(OQLError::storage_error(
                "record root tombstone has a payload",
                None,
            ))
        };
    }
    let end = offset
        .checked_add(length)
        .filter(|end| length > 0 && *end <= page.len())
        .ok_or_else(|| OQLError::storage_error("record root entry is truncated", None))?;
    let record = &page[offset..end];
    if crc32fast::hash(record) != checksum {
        return Err(OQLError::storage_error(
            "record root entry checksum is invalid",
            None,
        ));
    }
    Ok(RecordDeltaLookup::Put(Arc::from(record)))
}

fn decode_v3_page(
    page: &[u8],
    puts: &mut Vec<(ObjRef, Arc<[u8]>)>,
    deletes: &mut Vec<ObjRef>,
) -> OQLResult<()> {
    let entry_count = validate_v3_page_header(page)?;
    let mut expected_record_offset = 8 + entry_count * DIRECTORY_ENTRY_LEN;
    for entry in 0..entry_count {
        let cursor = 8 + entry * DIRECTORY_ENTRY_LEN;
        let object_ref = ObjRef::new(get_u64(page, cursor + 8)?);
        match decode_v3_page_entry(page, cursor)? {
            RecordDeltaLookup::Put(record) => {
                if get_u64(page, cursor + 16)? as usize != expected_record_offset {
                    return Err(OQLError::storage_error(
                        "record root page payload is not contiguous",
                        None,
                    ));
                }
                expected_record_offset += record.len();
                puts.push((object_ref, record));
            }
            RecordDeltaLookup::Delete => deletes.push(object_ref),
            RecordDeltaLookup::Missing => unreachable!(),
        }
    }
    if expected_record_offset != page.len() {
        return Err(OQLError::storage_error(
            "record root page has trailing bytes",
            None,
        ));
    }
    Ok(())
}

fn validate_v2_directory(bytes: &[u8], file_len: usize) -> OQLResult<(usize, usize, usize, usize)> {
    if get_u16(bytes, 4)? != 2
        || get_u16(bytes, 6)? as usize != DELTA_HEADER_LEN
        || get_u64(bytes, 40)? != file_len as u64
    {
        return Err(OQLError::storage_error(
            "record delta header is invalid",
            None,
        ));
    }
    if get_u64(bytes, 16)? < get_u64(bytes, 8)? {
        return Err(OQLError::storage_error(
            "record delta LSN range is invalid",
            None,
        ));
    }
    let entry_count = (get_u32(bytes, 24)? as usize)
        .checked_add(get_u32(bytes, 28)? as usize)
        .ok_or_else(|| OQLError::storage_error("record delta entry count overflow", None))?;
    let page_count = get_u32(bytes, 52)? as usize;
    let directory_len = get_u64(bytes, 32)? as usize;
    let expected_directory_len = entry_count
        .checked_mul(DIRECTORY_ENTRY_LEN)
        .and_then(|length| {
            page_count
                .checked_mul(PAGE_DIRECTORY_ENTRY_LEN)
                .and_then(|pages| length.checked_add(pages))
        })
        .ok_or_else(|| OQLError::storage_error("record delta directory overflow", None))?;
    let data_start = DELTA_HEADER_LEN
        .checked_add(directory_len)
        .filter(|start| *start <= bytes.len())
        .ok_or_else(|| OQLError::storage_error("record delta directory overflow", None))?;
    if directory_len != expected_directory_len
        || get_u16(bytes, 56)? as usize != DIRECTORY_ENTRY_LEN
        || get_u16(bytes, 58)? as usize != PAGE_DIRECTORY_ENTRY_LEN
        || checksum_with_zeroed_field(&bytes[..data_start], 48) != get_u32(bytes, 48)?
    {
        return Err(OQLError::storage_error(
            "record delta directory is invalid",
            None,
        ));
    }
    let mut previous = None;
    for entry in 0..entry_count {
        let cursor = DELTA_HEADER_LEN + entry * DIRECTORY_ENTRY_LEN;
        if bytes[cursor] > 1 || bytes[cursor + 1..cursor + 8].iter().any(|byte| *byte != 0) {
            return Err(OQLError::storage_error(
                "record delta operation is invalid",
                None,
            ));
        }
        let object_ref = ObjRef::new(get_u64(bytes, cursor + 8)?);
        if object_ref.is_null() || previous.is_some_and(|value| value >= object_ref) {
            return Err(OQLError::storage_error(
                "record delta object directory is not strictly ordered",
                None,
            ));
        }
        previous = Some(object_ref);
    }
    Ok((
        entry_count,
        page_count,
        DELTA_HEADER_LEN + entry_count * DIRECTORY_ENTRY_LEN,
        data_start,
    ))
}

fn find_object_entry(
    directory: &[u8],
    entry_count: usize,
    object_ref: ObjRef,
) -> OQLResult<Option<usize>> {
    let mut low = 0usize;
    let mut high = entry_count;
    while low < high {
        let middle = low + (high - low) / 2;
        let cursor = DELTA_HEADER_LEN + middle * DIRECTORY_ENTRY_LEN;
        match ObjRef::new(get_u64(directory, cursor + 8)?).cmp(&object_ref) {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
            std::cmp::Ordering::Equal => return Ok(Some(cursor)),
        }
    }
    Ok(None)
}

fn find_page_entry(
    directory: &[u8],
    page_directory_start: usize,
    page_count: usize,
    object_ref: ObjRef,
) -> OQLResult<Option<(usize, usize, usize)>> {
    let mut low = 0usize;
    let mut high = page_count;
    while low < high {
        let page_id = low + (high - low) / 2;
        let cursor = page_directory_start + page_id * PAGE_DIRECTORY_ENTRY_LEN;
        let first = ObjRef::new(get_u64(directory, cursor)?);
        let last = ObjRef::new(get_u64(directory, cursor + 8)?);
        if object_ref < first {
            high = page_id;
        } else if object_ref > last {
            low = page_id + 1;
        } else {
            let offset = usize::try_from(get_u64(directory, cursor + 16)?)
                .map_err(|_| OQLError::storage_error("record root page offset overflow", None))?;
            let length = get_u32(directory, cursor + 24)? as usize;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| OQLError::storage_error("record root page length overflow", None))?;
            return Ok(Some((cursor, offset, end)));
        }
    }
    Ok(None)
}

fn read_file_range(
    filesystem: &dyn FileSystem,
    path: &Path,
    offset: usize,
    length: usize,
) -> OQLResult<Vec<u8>> {
    let mut file = filesystem
        .open(path)
        .map_err(|error| OQLError::storage_error(&format!("open record root: {error}"), None))?;
    file.seek(SeekFrom::Start(offset as u64))
        .map_err(|error| OQLError::storage_error(&format!("seek record root: {error}"), None))?;
    let mut bytes = vec![0; length];
    let mut read = 0;
    while read < length {
        let count = file.read(&mut bytes[read..]).map_err(|error| {
            OQLError::storage_error(&format!("read record root: {error}"), None)
        })?;
        if count == 0 {
            return Err(OQLError::storage_error("record root is truncated", None));
        }
        read += count;
    }
    Ok(bytes)
}

fn checksum_with_zeroed_field(bytes: &[u8], offset: usize) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes[..offset]);
    hasher.update(&[0; 4]);
    hasher.update(&bytes[offset + 4..]);
    hasher.finalize()
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn get_u16(bytes: &[u8], offset: usize) -> OQLResult<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|value| u16::from_le_bytes(value.try_into().unwrap()))
        .ok_or_else(|| OQLError::storage_error("record delta is truncated", None))
}
fn get_u32(bytes: &[u8], offset: usize) -> OQLResult<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|value| u32::from_le_bytes(value.try_into().unwrap()))
        .ok_or_else(|| OQLError::storage_error("record delta is truncated", None))
}
fn get_u64(bytes: &[u8], offset: usize) -> OQLResult<u64> {
    bytes
        .get(offset..offset + 8)
        .map(|value| u64::from_le_bytes(value.try_into().unwrap()))
        .ok_or_else(|| OQLError::storage_error("record delta is truncated", None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::filesystem::RealFileSystem;

    #[test]
    fn record_delta_round_trips_puts_and_tombstones() {
        let delta = RecordPageDelta {
            base_lsn: 10,
            target_lsn: 12,
            puts: vec![(ObjRef::from_parts(1, 2, 1), Arc::from(&b"changed"[..]))],
            deletes: vec![ObjRef::from_parts(1, 1, 1)],
        };
        assert_eq!(
            RecordPageDelta::decode(&delta.encode().unwrap()).unwrap(),
            delta
        );
    }

    #[test]
    fn record_delta_detects_corruption_and_duplicate_identity() {
        let object_ref = ObjRef::from_parts(1, 1, 1);
        let duplicate = RecordPageDelta {
            base_lsn: 1,
            target_lsn: 2,
            puts: vec![(object_ref, Arc::from(&b"value"[..]))],
            deletes: vec![object_ref],
        };
        assert!(duplicate.encode().is_err());

        let mut bytes = RecordPageDelta {
            base_lsn: 1,
            target_lsn: 2,
            puts: vec![(object_ref, Arc::from(&b"value"[..]))],
            deletes: vec![],
        }
        .encode()
        .unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        assert!(RecordPageDelta::decode(&bytes).is_err());
    }

    #[test]
    fn record_delta_merge_coalesces_a_contiguous_chain() {
        let first = ObjRef::from_parts(1, 1, 1);
        let second = ObjRef::from_parts(1, 2, 1);
        let merged = RecordPageDelta::merge(
            &RecordPageDelta {
                base_lsn: 1,
                target_lsn: 3,
                puts: vec![(first, Arc::from(&b"old"[..]))],
                deletes: vec![second],
            },
            &RecordPageDelta {
                base_lsn: 3,
                target_lsn: 5,
                puts: vec![(second, Arc::from(&b"restored"[..]))],
                deletes: vec![first],
            },
        )
        .unwrap();
        assert_eq!(merged.base_lsn, 1);
        assert_eq!(merged.target_lsn, 5);
        assert_eq!(merged.puts, vec![(second, Arc::from(&b"restored"[..]))]);
        assert_eq!(merged.deletes, vec![first]);
    }

    #[test]
    fn paged_lookup_does_not_read_an_unrelated_corrupt_page() {
        let first = ObjRef::from_parts(1, 1, 1);
        let second = ObjRef::from_parts(1, 2, 1);
        let first_record = Arc::<[u8]>::from(vec![1; 10 * 1024]);
        let second_record = Arc::<[u8]>::from(vec![2; 10 * 1024]);
        let mut bytes = RecordPageDelta {
            base_lsn: 1,
            target_lsn: 2,
            puts: vec![(first, first_record.clone()), (second, second_record)],
            deletes: vec![],
        }
        .encode()
        .unwrap();
        assert_eq!(get_u16(&bytes, 4).unwrap(), DIRECTORY_PAGED_VERSION);
        assert_eq!(get_u32(&bytes, 52).unwrap(), 2);

        let second_page_directory = DELTA_HEADER_LEN + PAGE_DIRECTORY_ENTRY_LEN;
        let second_page_offset = get_u64(&bytes, second_page_directory + 16).unwrap() as usize;
        bytes[second_page_offset] ^= 0xff;

        assert_eq!(
            RecordPageDelta::lookup(&bytes, first).unwrap(),
            RecordDeltaLookup::Put(first_record)
        );
        assert!(RecordPageDelta::lookup(&bytes, second).is_err());
        assert!(RecordPageDelta::decode(&bytes).is_err());
    }

    #[test]
    fn file_backed_root_retains_page_fences_not_per_object_entries() {
        let puts = (1..=10_000)
            .map(|object_id| {
                (
                    ObjRef::from_parts(1, object_id, 1),
                    Arc::<[u8]>::from(&b"x"[..]),
                )
            })
            .collect();
        let bytes = RecordPageDelta {
            base_lsn: 1,
            target_lsn: 2,
            puts,
            deletes: Vec::new(),
        }
        .encode()
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("record-root.owld");
        std::fs::write(&path, &bytes).unwrap();

        let root = RecordRootHandle::open(Arc::new(RealFileSystem::new()), &path).unwrap();
        assert!(root.prefix.len() < 2_000);
        assert_eq!(root.entry_count, 10_000);
        assert_eq!(
            root.authority_checksum(),
            RecordPageDelta::authority_checksum(&bytes).unwrap()
        );
        assert_eq!(
            root.lookup(ObjRef::from_parts(1, 9_999, 1)).unwrap(),
            RecordDeltaLookup::Put(Arc::from(&b"x"[..]))
        );
    }

    #[test]
    fn flat_format_one_remains_readable() {
        let object_ref = ObjRef::from_parts(1, 1, 1);
        let record = b"legacy";
        let payload_len = LEGACY_ENTRY_LEN + record.len();
        let mut bytes = vec![0; DELTA_HEADER_LEN + payload_len];
        bytes[0..4].copy_from_slice(&DELTA_MAGIC);
        put_u16(&mut bytes, 4, 1);
        put_u16(&mut bytes, 6, DELTA_HEADER_LEN as u16);
        put_u64(&mut bytes, 8, 3);
        put_u64(&mut bytes, 16, 4);
        put_u32(&mut bytes, 24, 1);
        put_u64(&mut bytes, 32, payload_len as u64);
        let total_len = bytes.len() as u64;
        put_u64(&mut bytes, 40, total_len);
        put_u64(&mut bytes, DELTA_HEADER_LEN + 8, object_ref.packed());
        put_u32(&mut bytes, DELTA_HEADER_LEN + 16, record.len() as u32);
        put_u32(&mut bytes, DELTA_HEADER_LEN + 20, crc32fast::hash(record));
        bytes[DELTA_HEADER_LEN + LEGACY_ENTRY_LEN..].copy_from_slice(record);
        let checksum = checksum_with_zeroed_field(&bytes, 48);
        put_u32(&mut bytes, 48, checksum);

        let decoded = RecordPageDelta::decode(&bytes).unwrap();
        assert_eq!(decoded.base_lsn, 3);
        assert_eq!(decoded.puts[0].1.as_ref(), record);
    }
}
