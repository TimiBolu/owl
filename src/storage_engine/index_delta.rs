// The cumulative OWLI merge writer remains for format tests and legacy
// recovery while fresh checkpoints publish immutable OWGR generations.
#![allow(dead_code)]

use super::ObjRef;
use crate::error::{OQLError, OQLResult};
use crate::simulation::filesystem::{FileHandle, FileOptions, FileSystem};
#[cfg(test)]
use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::path::Path;

const MAGIC: [u8; 4] = *b"OWLI";
const LEGACY_VERSION: u16 = 1;
const VERSION: u16 = 2;
const HEADER_LEN: usize = 64;
const INDEX_HEADER_LEN: usize = 24;
const PAGE_HEADER_LEN: usize = 32;
const TARGET_PAGE_LEN: usize = 16 * 1024;
const PAGE_FIRST: u8 = 1;
const PAGE_LAST: u8 = 2;
const POSTINGS_TOMBSTONE: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexDeltaMode {
    Patch,
    Reset,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexDelta {
    pub schema: String,
    pub name: String,
    pub mode: IndexDeltaMode,
    pub changes: Vec<(Vec<u8>, Option<Vec<ObjRef>>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexRootDelta {
    pub base_lsn: u64,
    pub target_lsn: u64,
    pub indexes: Vec<IndexDelta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexRootMetadata {
    pub base_lsn: u64,
    pub target_lsn: u64,
}

pub(crate) struct IndexDeltaPage {
    pub schema: String,
    pub name: String,
    pub mode: IndexDeltaMode,
    pub first: bool,
    pub last: bool,
    pub changes: Vec<(Vec<u8>, Option<Vec<ObjRef>>)>,
}

impl IndexRootDelta {
    #[cfg(test)]
    pub(crate) fn merge(base: &Self, next: &Self) -> OQLResult<Self> {
        if base.target_lsn != next.base_lsn {
            return Err(storage_error("index root delta chain is not contiguous"));
        }
        let mut indexes = base
            .indexes
            .iter()
            .cloned()
            .map(|index| ((index.schema.clone(), index.name.clone()), index))
            .collect::<BTreeMap<_, _>>();
        for next_index in &next.indexes {
            let identity = (next_index.schema.clone(), next_index.name.clone());
            match next_index.mode {
                IndexDeltaMode::Delete | IndexDeltaMode::Reset => {
                    indexes.insert(identity, next_index.clone());
                }
                IndexDeltaMode::Patch => {
                    let entry = indexes.entry(identity).or_insert_with(|| IndexDelta {
                        schema: next_index.schema.clone(),
                        name: next_index.name.clone(),
                        mode: IndexDeltaMode::Patch,
                        changes: Vec::new(),
                    });
                    if entry.mode == IndexDeltaMode::Delete {
                        return Err(storage_error("cannot patch a deleted index root"));
                    }
                    let mut changes = entry.changes.iter().cloned().collect::<BTreeMap<_, _>>();
                    changes.extend(next_index.changes.iter().cloned());
                    entry.changes = changes.into_iter().collect();
                }
            }
        }
        Ok(Self {
            base_lsn: base.base_lsn,
            target_lsn: next.target_lsn,
            indexes: indexes.into_values().collect(),
        })
    }

    pub(crate) fn encode(&self) -> OQLResult<Vec<u8>> {
        validate_root(self)?;
        let layout = page_layout(self)?;
        let mut bytes = header_bytes(self, layout.page_count, layout.payload_len, 0)?;
        visit_encoded_pages(self, |page| {
            bytes.extend_from_slice(page);
            Ok(())
        })?;
        let checksum = checksum_with_zeroed_field(&bytes, 48);
        put_u32(&mut bytes, 48, checksum);
        Ok(bytes)
    }

    #[cfg(test)]
    pub(crate) fn decode(bytes: &[u8]) -> OQLResult<Self> {
        let mut indexes = BTreeMap::<(String, String), IndexDelta>::new();
        let metadata = visit_root_bytes(bytes, |page| collect_page(&mut indexes, page))?;
        finish_collected_root(metadata, indexes)
    }
}

pub(crate) fn write_index_root_file(
    filesystem: &dyn FileSystem,
    path: &Path,
    root: &IndexRootDelta,
) -> OQLResult<()> {
    validate_root(root)?;
    let layout = page_layout(root)?;
    let zero_header = header_bytes(root, layout.page_count, layout.payload_len, 0)?;
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&zero_header);
    visit_encoded_pages(root, |page| {
        hasher.update(page);
        Ok(())
    })?;
    let header = header_bytes(
        root,
        layout.page_count,
        layout.payload_len,
        hasher.finalize(),
    )?;
    let mut file = filesystem
        .create(path)
        .map_err(|_| storage_error("cannot create index root delta"))?;
    write_all(file.as_mut(), &header)?;
    visit_encoded_pages(root, |page| write_all(file.as_mut(), page))?;
    file.sync()
        .map_err(|_| storage_error("cannot sync index root delta"))
}

#[cfg(test)]
pub(crate) fn read_index_root_file(
    filesystem: &dyn FileSystem,
    path: &Path,
) -> OQLResult<IndexRootDelta> {
    let mut indexes = BTreeMap::<(String, String), IndexDelta>::new();
    let metadata =
        visit_index_root_file(filesystem, path, |page| collect_page(&mut indexes, page))?;
    finish_collected_root(metadata, indexes)
}

pub(crate) fn write_merged_index_root_file(
    filesystem: &dyn FileSystem,
    base_path: &Path,
    target_path: &Path,
    next: &IndexRootDelta,
) -> OQLResult<()> {
    validate_root(next)?;
    let mut file = filesystem
        .create(target_path)
        .map_err(|_| storage_error("cannot create merged index root delta"))?;
    write_all(file.as_mut(), &[0; HEADER_LEN])?;
    let mut merger = StreamingMerger::new(next, file);
    let base = visit_index_root_file(filesystem, base_path, |page| merger.accept_base_page(page))?;
    if base.target_lsn != next.base_lsn {
        return Err(storage_error("index root delta chain is not contiguous"));
    }
    merger.finish_remaining()?;
    let (mut file, page_count, payload_len) = merger.finish();
    let root = IndexRootDelta {
        base_lsn: base.base_lsn,
        target_lsn: next.target_lsn,
        indexes: Vec::new(),
    };
    let header = header_bytes(&root, page_count, payload_len, 0)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| storage_error("cannot seek merged index root delta"))?;
    write_all(file.as_mut(), &header)?;
    file.sync()
        .map_err(|_| storage_error("cannot sync merged index root delta"))?;
    drop(file);

    let checksum = checksum_file(filesystem, target_path)?;
    let mut file = filesystem
        .open_with_options(
            target_path,
            &FileOptions {
                read: false,
                write: true,
                create: false,
                truncate: false,
                append: false,
            },
        )
        .map_err(|_| storage_error("cannot reopen merged index root delta"))?;
    file.seek(SeekFrom::Start(48))
        .map_err(|_| storage_error("cannot seek merged index root checksum"))?;
    write_all(file.as_mut(), &checksum.to_le_bytes())?;
    file.sync()
        .map_err(|_| storage_error("cannot sync merged index root checksum"))
}

pub(crate) fn visit_index_root_file(
    filesystem: &dyn FileSystem,
    path: &Path,
    mut visitor: impl FnMut(IndexDeltaPage) -> OQLResult<()>,
) -> OQLResult<IndexRootMetadata> {
    let mut file = filesystem
        .open(path)
        .map_err(|_| storage_error("cannot open index root delta"))?;
    let size = file
        .size()
        .map_err(|_| storage_error("cannot inspect index root delta"))?;
    if size < HEADER_LEN as u64 {
        return Err(storage_error("index root delta is truncated"));
    }
    let mut header = [0; HEADER_LEN];
    read_exact(file.as_mut(), &mut header)?;
    let parsed = parse_header(&header, size)?;
    if parsed.version == LEGACY_VERSION {
        let mut bytes = vec![
            0;
            usize::try_from(size)
                .map_err(|_| storage_error("index root delta is too large"))?
        ];
        bytes[..HEADER_LEN].copy_from_slice(&header);
        read_exact(file.as_mut(), &mut bytes[HEADER_LEN..])?;
        return visit_root_bytes(&bytes, visitor);
    }
    let mut hasher = crc32fast::Hasher::new();
    let mut zero_header = header;
    put_u32(&mut zero_header, 48, 0);
    hasher.update(&zero_header);
    let mut chain = PageChain::default();
    for _ in 0..parsed.count {
        let mut page_header = [0; PAGE_HEADER_LEN];
        read_exact(file.as_mut(), &mut page_header)?;
        let page_len = get_u32(&page_header, 20)? as usize;
        if page_len < PAGE_HEADER_LEN || page_len as u64 > size {
            return Err(storage_error("index root delta page length is invalid"));
        }
        let mut page = vec![0; page_len];
        page[..PAGE_HEADER_LEN].copy_from_slice(&page_header);
        read_exact(file.as_mut(), &mut page[PAGE_HEADER_LEN..])?;
        hasher.update(&page);
        let decoded = decode_page(&page)?;
        validate_page_chain(&mut chain, &decoded)?;
        visitor(decoded)?;
    }
    if chain.open.is_some() || file.position() != size || hasher.finalize() != parsed.checksum {
        return Err(storage_error(
            "index root delta header or checksum is invalid",
        ));
    }
    Ok(parsed.metadata)
}

#[derive(Clone, Copy)]
struct RootHeader {
    version: u16,
    count: u32,
    checksum: u32,
    metadata: IndexRootMetadata,
}

#[derive(Default)]
struct PageChain {
    open: Option<(String, String, IndexDeltaMode, Option<Vec<u8>>)>,
    previous_identity: Option<(String, String)>,
}

struct PageLayout {
    page_count: u32,
    payload_len: u64,
}

struct StreamingPageOutput {
    file: Box<dyn FileHandle>,
    page_count: u32,
    payload_len: u64,
}

impl StreamingPageOutput {
    fn write(&mut self, page: &[u8]) -> OQLResult<()> {
        write_all(self.file.as_mut(), page)?;
        self.page_count = self
            .page_count
            .checked_add(1)
            .ok_or_else(|| storage_error("index root delta has too many pages"))?;
        self.payload_len = self
            .payload_len
            .checked_add(page.len() as u64)
            .ok_or_else(|| storage_error("index root delta length overflow"))?;
        Ok(())
    }
}

struct LogicalPageAccumulator {
    schema: String,
    name: String,
    mode: IndexDeltaMode,
    changes: Vec<(Vec<u8>, Option<Vec<ObjRef>>)>,
    encoded_len: usize,
    first: bool,
}

impl LogicalPageAccumulator {
    fn new(schema: String, name: String, mode: IndexDeltaMode) -> Self {
        Self {
            schema,
            name,
            mode,
            changes: Vec::new(),
            encoded_len: 0,
            first: true,
        }
    }

    fn push(
        &mut self,
        output: &mut StreamingPageOutput,
        change: (Vec<u8>, Option<Vec<ObjRef>>),
    ) -> OQLResult<()> {
        let change_len = encoded_change_len(&change)?;
        let capacity = TARGET_PAGE_LEN.saturating_sub(
            PAGE_HEADER_LEN
                .saturating_add(self.schema.len())
                .saturating_add(self.name.len()),
        );
        if !self.changes.is_empty() && self.encoded_len.saturating_add(change_len) > capacity {
            self.flush(output, false)?;
        }
        self.encoded_len = self
            .encoded_len
            .checked_add(change_len)
            .ok_or_else(|| storage_error("index root delta page length overflow"))?;
        self.changes.push(change);
        Ok(())
    }

    fn finish(mut self, output: &mut StreamingPageOutput) -> OQLResult<()> {
        self.flush(output, true)
    }

    fn flush(&mut self, output: &mut StreamingPageOutput, last: bool) -> OQLResult<()> {
        let index = IndexDelta {
            schema: self.schema.clone(),
            name: self.name.clone(),
            mode: self.mode,
            changes: Vec::new(),
        };
        let changes = self.changes.iter().collect::<Vec<_>>();
        let page = encode_page(&index, &changes, self.first, last)?;
        output.write(&page)?;
        self.changes.clear();
        self.encoded_len = 0;
        self.first = false;
        Ok(())
    }
}

struct ActiveBaseIndex<'a> {
    identity: (String, String),
    output: Option<LogicalPageAccumulator>,
    next: Option<&'a IndexDelta>,
    next_change: usize,
}

struct StreamingMerger<'a> {
    next: Vec<&'a IndexDelta>,
    next_index: usize,
    active: Option<ActiveBaseIndex<'a>>,
    output: StreamingPageOutput,
}

impl<'a> StreamingMerger<'a> {
    fn new(next: &'a IndexRootDelta, file: Box<dyn FileHandle>) -> Self {
        let mut indexes = next.indexes.iter().collect::<Vec<_>>();
        indexes
            .sort_by(|left, right| (&left.schema, &left.name).cmp(&(&right.schema, &right.name)));
        Self {
            next: indexes,
            next_index: 0,
            active: None,
            output: StreamingPageOutput {
                file,
                page_count: 0,
                payload_len: 0,
            },
        }
    }

    fn accept_base_page(&mut self, page: IndexDeltaPage) -> OQLResult<()> {
        if page.first {
            self.start_base_index(&page)?;
        }
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| storage_error("index root delta merge has no active index"))?;
        if active.identity != (page.schema.clone(), page.name.clone()) {
            return Err(storage_error("index root delta merge identity changed"));
        }
        if let Some(accumulator) = active.output.as_mut() {
            if let Some(next) = active.next {
                for base_change in page.changes {
                    while active.next_change < next.changes.len()
                        && next.changes[active.next_change].0 < base_change.0
                    {
                        accumulator
                            .push(&mut self.output, next.changes[active.next_change].clone())?;
                        active.next_change += 1;
                    }
                    if active.next_change < next.changes.len()
                        && next.changes[active.next_change].0 == base_change.0
                    {
                        accumulator
                            .push(&mut self.output, next.changes[active.next_change].clone())?;
                        active.next_change += 1;
                    } else {
                        accumulator.push(&mut self.output, base_change)?;
                    }
                }
            } else {
                for change in page.changes {
                    accumulator.push(&mut self.output, change)?;
                }
            }
        }
        if page.last {
            let mut active = self
                .active
                .take()
                .ok_or_else(|| storage_error("index root delta merge lost its active index"))?;
            if let (Some(accumulator), Some(next)) = (active.output.as_mut(), active.next) {
                while active.next_change < next.changes.len() {
                    accumulator.push(&mut self.output, next.changes[active.next_change].clone())?;
                    active.next_change += 1;
                }
            }
            if let Some(accumulator) = active.output {
                accumulator.finish(&mut self.output)?;
            }
        }
        Ok(())
    }

    fn start_base_index(&mut self, page: &IndexDeltaPage) -> OQLResult<()> {
        if self.active.is_some() {
            return Err(storage_error("index root delta merge page chain overlaps"));
        }
        let identity = (page.schema.clone(), page.name.clone());
        while self.next_index < self.next.len()
            && compare_index_identity(self.next[self.next_index], &identity).is_lt()
        {
            self.emit_next_index(self.next[self.next_index])?;
            self.next_index += 1;
        }
        let equal = self.next_index < self.next.len()
            && compare_index_identity(self.next[self.next_index], &identity).is_eq();
        let (output, next) = if equal {
            let next = self.next[self.next_index];
            self.next_index += 1;
            match next.mode {
                IndexDeltaMode::Delete | IndexDeltaMode::Reset => {
                    self.emit_next_index(next)?;
                    (None, None)
                }
                IndexDeltaMode::Patch => {
                    if page.mode == IndexDeltaMode::Delete {
                        return Err(storage_error("cannot patch a deleted index root"));
                    }
                    (
                        Some(LogicalPageAccumulator::new(
                            page.schema.clone(),
                            page.name.clone(),
                            page.mode,
                        )),
                        Some(next),
                    )
                }
            }
        } else {
            (
                Some(LogicalPageAccumulator::new(
                    page.schema.clone(),
                    page.name.clone(),
                    page.mode,
                )),
                None,
            )
        };
        self.active = Some(ActiveBaseIndex {
            identity,
            output,
            next,
            next_change: 0,
        });
        Ok(())
    }

    fn emit_next_index(&mut self, index: &IndexDelta) -> OQLResult<()> {
        let mut accumulator =
            LogicalPageAccumulator::new(index.schema.clone(), index.name.clone(), index.mode);
        for change in &index.changes {
            accumulator.push(&mut self.output, change.clone())?;
        }
        accumulator.finish(&mut self.output)
    }

    fn finish_remaining(&mut self) -> OQLResult<()> {
        if self.active.is_some() {
            return Err(storage_error(
                "index root delta merge ended inside an index",
            ));
        }
        while self.next_index < self.next.len() {
            self.emit_next_index(self.next[self.next_index])?;
            self.next_index += 1;
        }
        Ok(())
    }

    fn finish(self) -> (Box<dyn FileHandle>, u32, u64) {
        (
            self.output.file,
            self.output.page_count,
            self.output.payload_len,
        )
    }
}

fn compare_index_identity(index: &IndexDelta, identity: &(String, String)) -> std::cmp::Ordering {
    (index.schema.as_str(), index.name.as_str()).cmp(&(identity.0.as_str(), identity.1.as_str()))
}

fn page_layout(root: &IndexRootDelta) -> OQLResult<PageLayout> {
    let mut page_count = 0u32;
    let mut payload_len = 0u64;
    visit_encoded_pages(root, |page| {
        page_count = page_count
            .checked_add(1)
            .ok_or_else(|| storage_error("index root delta has too many pages"))?;
        payload_len = payload_len
            .checked_add(page.len() as u64)
            .ok_or_else(|| storage_error("index root delta length overflow"))?;
        Ok(())
    })?;
    Ok(PageLayout {
        page_count,
        payload_len,
    })
}

fn header_bytes(
    root: &IndexRootDelta,
    page_count: u32,
    payload_len: u64,
    checksum: u32,
) -> OQLResult<Vec<u8>> {
    let total_len = (HEADER_LEN as u64)
        .checked_add(payload_len)
        .ok_or_else(|| storage_error("index root delta length overflow"))?;
    let mut header = vec![0; HEADER_LEN];
    header[..4].copy_from_slice(&MAGIC);
    put_u16(&mut header, 4, VERSION);
    put_u16(&mut header, 6, HEADER_LEN as u16);
    put_u64(&mut header, 8, root.base_lsn);
    put_u64(&mut header, 16, root.target_lsn);
    put_u32(&mut header, 24, page_count);
    put_u64(&mut header, 32, payload_len);
    put_u64(&mut header, 40, total_len);
    put_u32(&mut header, 48, checksum);
    Ok(header)
}

fn visit_encoded_pages(
    root: &IndexRootDelta,
    mut visitor: impl FnMut(&[u8]) -> OQLResult<()>,
) -> OQLResult<()> {
    let mut indexes = root.indexes.iter().collect::<Vec<_>>();
    indexes.sort_by(|left, right| (&left.schema, &left.name).cmp(&(&right.schema, &right.name)));
    for index in indexes {
        let mut changes = index.changes.iter().collect::<Vec<_>>();
        changes.sort_by(|left, right| left.0.cmp(&right.0));
        let identity_len = index.schema.len().saturating_add(index.name.len());
        let capacity = TARGET_PAGE_LEN.saturating_sub(PAGE_HEADER_LEN + identity_len);
        let mut start = 0;
        while start < changes.len() || (changes.is_empty() && start == 0) {
            let mut end = start;
            let mut used = 0usize;
            while end < changes.len() {
                let change_len = encoded_change_len(changes[end])?;
                if end > start && used.saturating_add(change_len) > capacity {
                    break;
                }
                used = used
                    .checked_add(change_len)
                    .ok_or_else(|| storage_error("index root delta page length overflow"))?;
                end += 1;
            }
            let first = start == 0;
            let last = end == changes.len();
            let page = encode_page(index, &changes[start..end], first, last)?;
            visitor(&page)?;
            if changes.is_empty() {
                break;
            }
            start = end;
        }
    }
    Ok(())
}

fn encoded_change_len(change: &(Vec<u8>, Option<Vec<ObjRef>>)) -> OQLResult<usize> {
    let postings = change.1.as_ref().map_or(0, Vec::len);
    8usize
        .checked_add(change.0.len())
        .and_then(|len| len.checked_add(postings.checked_mul(8)?))
        .ok_or_else(|| storage_error("index root delta change length overflow"))
}

fn encode_page(
    index: &IndexDelta,
    changes: &[&(Vec<u8>, Option<Vec<ObjRef>>)],
    first: bool,
    last: bool,
) -> OQLResult<Vec<u8>> {
    let mut page = vec![0; PAGE_HEADER_LEN];
    page[0] = mode_byte(index.mode);
    page[1] = u8::from(first) * PAGE_FIRST | u8::from(last) * PAGE_LAST;
    put_u32(&mut page, 8, as_u32(index.schema.len())?);
    put_u32(&mut page, 12, as_u32(index.name.len())?);
    put_u32(&mut page, 16, as_u32(changes.len())?);
    page.extend_from_slice(index.schema.as_bytes());
    page.extend_from_slice(index.name.as_bytes());
    for (key, postings) in changes {
        page.extend_from_slice(&as_u32(key.len())?.to_le_bytes());
        let count = match postings {
            Some(postings) => {
                let count = as_u32(postings.len())?;
                if count == POSTINGS_TOMBSTONE {
                    return Err(storage_error("index root delta posting list is too large"));
                }
                count
            }
            None => POSTINGS_TOMBSTONE,
        };
        page.extend_from_slice(&count.to_le_bytes());
        page.extend_from_slice(key);
        if let Some(postings) = postings {
            for object_ref in postings {
                page.extend_from_slice(&object_ref.packed().to_le_bytes());
            }
        }
    }
    let page_len = as_u32(page.len())?;
    put_u32(&mut page, 20, page_len);
    let checksum = checksum_with_zeroed_field(&page, 24);
    put_u32(&mut page, 24, checksum);
    Ok(page)
}

fn parse_header(header: &[u8], actual_len: u64) -> OQLResult<RootHeader> {
    if header.len() != HEADER_LEN
        || header[..4] != MAGIC
        || !matches!(get_u16(header, 4)?, LEGACY_VERSION | VERSION)
        || get_u16(header, 6)? as usize != HEADER_LEN
        || get_u64(header, 40)? != actual_len
        || get_u64(header, 32)? != actual_len - HEADER_LEN as u64
        || header[28..32].iter().any(|byte| *byte != 0)
        || header[52..].iter().any(|byte| *byte != 0)
    {
        return Err(storage_error(
            "index root delta header or checksum is invalid",
        ));
    }
    let base_lsn = get_u64(header, 8)?;
    let target_lsn = get_u64(header, 16)?;
    if target_lsn < base_lsn {
        return Err(storage_error("index root delta LSN range is invalid"));
    }
    Ok(RootHeader {
        version: get_u16(header, 4)?,
        count: get_u32(header, 24)?,
        checksum: get_u32(header, 48)?,
        metadata: IndexRootMetadata {
            base_lsn,
            target_lsn,
        },
    })
}

fn visit_root_bytes(
    bytes: &[u8],
    mut visitor: impl FnMut(IndexDeltaPage) -> OQLResult<()>,
) -> OQLResult<IndexRootMetadata> {
    if bytes.len() < HEADER_LEN {
        return Err(storage_error("index root delta is truncated"));
    }
    let header = parse_header(&bytes[..HEADER_LEN], bytes.len() as u64)?;
    if checksum_with_zeroed_field(bytes, 48) != header.checksum {
        return Err(storage_error(
            "index root delta header or checksum is invalid",
        ));
    }
    let mut cursor = HEADER_LEN;
    let mut chain = PageChain::default();
    for _ in 0..header.count {
        let page = if header.version == LEGACY_VERSION {
            decode_legacy_block(bytes, &mut cursor)?
        } else {
            let page_len = get_u32(bytes, cursor + 20)? as usize;
            let end = cursor
                .checked_add(page_len)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| storage_error("index root delta page length is invalid"))?;
            let page = decode_page(&bytes[cursor..end])?;
            cursor = end;
            page
        };
        validate_page_chain(&mut chain, &page)?;
        visitor(page)?;
    }
    if cursor != bytes.len() || chain.open.is_some() {
        return Err(storage_error(
            "index root delta has trailing or incomplete pages",
        ));
    }
    Ok(header.metadata)
}

fn decode_page(bytes: &[u8]) -> OQLResult<IndexDeltaPage> {
    if bytes.len() < PAGE_HEADER_LEN
        || get_u32(bytes, 20)? as usize != bytes.len()
        || checksum_with_zeroed_field(bytes, 24) != get_u32(bytes, 24)?
        || bytes[2..8].iter().any(|byte| *byte != 0)
        || bytes[28..32].iter().any(|byte| *byte != 0)
        || bytes[1] & !(PAGE_FIRST | PAGE_LAST) != 0
    {
        return Err(storage_error("index root delta page is invalid"));
    }
    let mode = decode_mode(bytes[0])?;
    let mut cursor = PAGE_HEADER_LEN;
    let schema = read_string(bytes, &mut cursor, get_u32(bytes, 8)? as usize, bytes.len())?;
    let name = read_string(
        bytes,
        &mut cursor,
        get_u32(bytes, 12)? as usize,
        bytes.len(),
    )?;
    let changes = decode_changes(
        bytes,
        &mut cursor,
        get_u32(bytes, 16)? as usize,
        bytes.len(),
    )?;
    if cursor != bytes.len() {
        return Err(storage_error("index root delta page has trailing bytes"));
    }
    Ok(IndexDeltaPage {
        schema,
        name,
        mode,
        first: bytes[1] & PAGE_FIRST != 0,
        last: bytes[1] & PAGE_LAST != 0,
        changes,
    })
}

fn decode_legacy_block(bytes: &[u8], cursor: &mut usize) -> OQLResult<IndexDeltaPage> {
    let start = *cursor;
    let header_end = start
        .checked_add(INDEX_HEADER_LEN)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| storage_error("index root delta is truncated"))?;
    if bytes[start + 1..start + 8].iter().any(|byte| *byte != 0) {
        return Err(storage_error("index root delta has invalid reserved bytes"));
    }
    let block_end = start
        .checked_add(get_u32(bytes, start + 20)? as usize)
        .filter(|end| *end >= header_end && *end <= bytes.len())
        .ok_or_else(|| storage_error("index root delta block length is invalid"))?;
    *cursor = header_end;
    let schema = read_string(
        bytes,
        cursor,
        get_u32(bytes, start + 8)? as usize,
        block_end,
    )?;
    let name = read_string(
        bytes,
        cursor,
        get_u32(bytes, start + 12)? as usize,
        block_end,
    )?;
    let changes = decode_changes(
        bytes,
        cursor,
        get_u32(bytes, start + 16)? as usize,
        block_end,
    )?;
    if *cursor != block_end {
        return Err(storage_error("index root delta block has trailing bytes"));
    }
    Ok(IndexDeltaPage {
        schema,
        name,
        mode: decode_mode(bytes[start])?,
        first: true,
        last: true,
        changes,
    })
}

fn decode_changes(
    bytes: &[u8],
    cursor: &mut usize,
    count: usize,
    limit: usize,
) -> OQLResult<Vec<(Vec<u8>, Option<Vec<ObjRef>>)>> {
    let remaining = limit
        .checked_sub(*cursor)
        .ok_or_else(|| storage_error("index root delta change offset is invalid"))?;
    if count > remaining / 8 {
        return Err(storage_error("index root delta change count is invalid"));
    }
    let mut changes = Vec::with_capacity(count);
    for _ in 0..count {
        let key_len = get_u32(bytes, *cursor)? as usize;
        let posting_count = get_u32(bytes, *cursor + 4)?;
        *cursor = cursor
            .checked_add(8)
            .ok_or_else(|| storage_error("index root delta offset overflow"))?;
        let key = read_bytes(bytes, cursor, key_len, limit)?.to_vec();
        let postings = if posting_count == POSTINGS_TOMBSTONE {
            None
        } else {
            let posting_count = posting_count as usize;
            let remaining = limit
                .checked_sub(*cursor)
                .ok_or_else(|| storage_error("index root delta posting offset is invalid"))?;
            if posting_count > remaining / 8 {
                return Err(storage_error("index root delta postings are truncated"));
            }
            let mut postings = Vec::with_capacity(posting_count);
            for _ in 0..posting_count {
                if cursor.checked_add(8).is_none_or(|end| end > limit) {
                    return Err(storage_error("index root delta postings are truncated"));
                }
                postings.push(ObjRef::new(get_u64(bytes, *cursor)?));
                *cursor += 8;
            }
            Some(postings)
        };
        changes.push((key, postings));
    }
    Ok(changes)
}

fn validate_page_chain(chain: &mut PageChain, page: &IndexDeltaPage) -> OQLResult<()> {
    if page.schema.is_empty() || page.name.is_empty() {
        return Err(storage_error("index root delta identity is empty"));
    }
    let identity = (page.schema.clone(), page.name.clone());
    if page.first {
        if chain.open.is_some()
            || chain
                .previous_identity
                .as_ref()
                .is_some_and(|previous| previous >= &identity)
        {
            return Err(storage_error("index root delta page order is invalid"));
        }
        chain.open = Some((page.schema.clone(), page.name.clone(), page.mode, None));
    }
    let Some((schema, name, mode, previous_key)) = chain.open.as_mut() else {
        return Err(storage_error(
            "index root delta page chain has no first page",
        ));
    };
    if schema != &page.schema || name != &page.name || *mode != page.mode {
        return Err(storage_error(
            "index root delta page chain identity changed",
        ));
    }
    if page.mode == IndexDeltaMode::Delete && !page.changes.is_empty() {
        return Err(storage_error("deleted index root contains key changes"));
    }
    for (key, postings) in &page.changes {
        if key.is_empty()
            || previous_key
                .as_ref()
                .is_some_and(|previous| previous >= key)
        {
            return Err(storage_error("index root delta contains an invalid key"));
        }
        validate_postings(postings)?;
        *previous_key = Some(key.clone());
    }
    if page.last {
        chain.previous_identity = Some(identity);
        chain.open = None;
    }
    Ok(())
}

#[cfg(test)]
fn collect_page(
    indexes: &mut BTreeMap<(String, String), IndexDelta>,
    page: IndexDeltaPage,
) -> OQLResult<()> {
    let identity = (page.schema.clone(), page.name.clone());
    let entry = indexes.entry(identity).or_insert_with(|| IndexDelta {
        schema: page.schema,
        name: page.name,
        mode: page.mode,
        changes: Vec::new(),
    });
    entry.changes.extend(page.changes);
    Ok(())
}

#[cfg(test)]
fn finish_collected_root(
    metadata: IndexRootMetadata,
    indexes: BTreeMap<(String, String), IndexDelta>,
) -> OQLResult<IndexRootDelta> {
    let root = IndexRootDelta {
        base_lsn: metadata.base_lsn,
        target_lsn: metadata.target_lsn,
        indexes: indexes.into_values().collect(),
    };
    validate_root(&root)?;
    Ok(root)
}

fn mode_byte(mode: IndexDeltaMode) -> u8 {
    match mode {
        IndexDeltaMode::Patch => 0,
        IndexDeltaMode::Reset => 1,
        IndexDeltaMode::Delete => 2,
    }
}

fn decode_mode(byte: u8) -> OQLResult<IndexDeltaMode> {
    match byte {
        0 => Ok(IndexDeltaMode::Patch),
        1 => Ok(IndexDeltaMode::Reset),
        2 => Ok(IndexDeltaMode::Delete),
        _ => Err(storage_error("index root delta operation is invalid")),
    }
}

fn validate_postings(postings: &Option<Vec<ObjRef>>) -> OQLResult<()> {
    if postings.as_ref().is_some_and(|postings| {
        postings.is_empty()
            || postings.iter().any(|object_ref| object_ref.is_null())
            || postings.windows(2).any(|pair| pair[0] >= pair[1])
    }) {
        return Err(storage_error("index root delta postings are invalid"));
    }
    Ok(())
}

fn checksum_file(filesystem: &dyn FileSystem, path: &Path) -> OQLResult<u32> {
    let mut file = filesystem
        .open(path)
        .map_err(|_| storage_error("cannot open merged index root for checksum"))?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0; TARGET_PAGE_LEN];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| storage_error("cannot checksum merged index root"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize())
}

fn read_exact(file: &mut dyn FileHandle, mut bytes: &mut [u8]) -> OQLResult<()> {
    while !bytes.is_empty() {
        let count = file
            .read(bytes)
            .map_err(|_| storage_error("cannot read index root delta"))?;
        if count == 0 {
            return Err(storage_error("index root delta is truncated"));
        }
        bytes = &mut bytes[count..];
    }
    Ok(())
}

fn write_all(file: &mut dyn FileHandle, mut bytes: &[u8]) -> OQLResult<()> {
    while !bytes.is_empty() {
        let count = file
            .write(bytes)
            .map_err(|_| storage_error("cannot write index root delta"))?;
        if count == 0 {
            return Err(storage_error("cannot write index root delta"));
        }
        bytes = &bytes[count..];
    }
    Ok(())
}

fn validate_root(root: &IndexRootDelta) -> OQLResult<()> {
    if root.target_lsn < root.base_lsn {
        return Err(storage_error(
            "index root delta target LSN precedes its base",
        ));
    }
    let mut previous_identity: Option<(&str, &str)> = None;
    let mut indexes = root.indexes.iter().collect::<Vec<_>>();
    indexes.sort_by(|left, right| (&left.schema, &left.name).cmp(&(&right.schema, &right.name)));
    for index in indexes {
        if index.schema.is_empty() || index.name.is_empty() {
            return Err(storage_error("index root delta identity is empty"));
        }
        let identity = (index.schema.as_str(), index.name.as_str());
        if previous_identity == Some(identity) {
            return Err(storage_error("index root delta contains a duplicate index"));
        }
        previous_identity = Some(identity);
        if index.mode == IndexDeltaMode::Delete && !index.changes.is_empty() {
            return Err(storage_error("deleted index root contains key changes"));
        }
        let mut previous_key: Option<&[u8]> = None;
        let mut changes = index.changes.iter().collect::<Vec<_>>();
        changes.sort_by(|left, right| left.0.cmp(&right.0));
        for (key, postings) in changes {
            if key.is_empty() || previous_key == Some(key.as_slice()) {
                return Err(storage_error("index root delta contains an invalid key"));
            }
            if let Some(postings) = postings {
                if postings.is_empty()
                    || postings.iter().any(|object_ref| object_ref.is_null())
                    || postings.windows(2).any(|pair| pair[0] >= pair[1])
                {
                    return Err(storage_error("index root delta postings are invalid"));
                }
            }
            previous_key = Some(key);
        }
    }
    Ok(())
}

fn read_string(bytes: &[u8], cursor: &mut usize, len: usize, limit: usize) -> OQLResult<String> {
    String::from_utf8(read_bytes(bytes, cursor, len, limit)?.to_vec())
        .map_err(|_| storage_error("index root delta identity is not UTF-8"))
}

fn read_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
    limit: usize,
) -> OQLResult<&'a [u8]> {
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= limit && *end <= bytes.len())
        .ok_or_else(|| storage_error("index root delta is truncated"))?;
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}

fn as_u32(value: usize) -> OQLResult<u32> {
    u32::try_from(value).map_err(|_| storage_error("index root delta field is too large"))
}

fn storage_error(message: &str) -> OQLError {
    OQLError::storage_error(message, None)
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
    Ok(u16::from_le_bytes(read_array(bytes, offset)?))
}
fn get_u32(bytes: &[u8], offset: usize) -> OQLResult<u32> {
    Ok(u32::from_le_bytes(read_array(bytes, offset)?))
}
fn get_u64(bytes: &[u8], offset: usize) -> OQLResult<u64> {
    Ok(u64::from_le_bytes(read_array(bytes, offset)?))
}
fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> OQLResult<[u8; N]> {
    bytes
        .get(offset..offset + N)
        .ok_or_else(|| storage_error("index root delta is truncated"))?
        .try_into()
        .map_err(|_| storage_error("index root delta field is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::filesystem::SimulatedFileSystem;

    fn object(id: u32) -> ObjRef {
        ObjRef::from_parts(1, id, 1)
    }

    #[test]
    fn index_root_round_trips_and_detects_corruption() {
        let root = IndexRootDelta {
            base_lsn: 2,
            target_lsn: 4,
            indexes: vec![IndexDelta {
                schema: "events".into(),
                name: "by_time".into(),
                mode: IndexDeltaMode::Patch,
                changes: vec![(vec![1], Some(vec![object(1), object(2)])), (vec![2], None)],
            }],
        };
        let bytes = root.encode().unwrap();
        assert_eq!(IndexRootDelta::decode(&bytes).unwrap(), root);
        let mut corrupt = bytes;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(IndexRootDelta::decode(&corrupt).is_err());
    }

    #[test]
    fn paged_file_streams_multiple_independently_checksummed_pages() {
        let root = IndexRootDelta {
            base_lsn: 2,
            target_lsn: 4,
            indexes: vec![IndexDelta {
                schema: "events".into(),
                name: "by_time".into(),
                mode: IndexDeltaMode::Patch,
                changes: (0..2_000u32)
                    .map(|key| (key.to_be_bytes().to_vec(), Some(vec![object(key + 1)])))
                    .collect(),
            }],
        };
        let filesystem = SimulatedFileSystem::new(1);
        let path = Path::new("paged.owli");
        write_index_root_file(&filesystem, path, &root).unwrap();
        let mut pages = 0;
        let metadata = visit_index_root_file(&filesystem, path, |page| {
            pages += 1;
            assert!(!page.changes.is_empty());
            Ok(())
        })
        .unwrap();
        assert!(pages > 1);
        assert_eq!(metadata.base_lsn, 2);
        assert_eq!(read_index_root_file(&filesystem, path).unwrap(), root);

        let mut bytes = root.encode().unwrap();
        let first_page_len = get_u32(&bytes, HEADER_LEN + 20).unwrap() as usize;
        bytes[HEADER_LEN + first_page_len + PAGE_HEADER_LEN] ^= 1;
        let checksum = checksum_with_zeroed_field(&bytes, 48);
        put_u32(&mut bytes, 48, checksum);
        assert!(IndexRootDelta::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("page"));
    }

    #[test]
    fn streaming_merge_matches_logical_merge_across_pages_and_modes() {
        let base = IndexRootDelta {
            base_lsn: 1,
            target_lsn: 2,
            indexes: vec![
                IndexDelta {
                    schema: "events".into(),
                    name: "by_code".into(),
                    mode: IndexDeltaMode::Patch,
                    changes: (0..2_000u32)
                        .map(|key| (key.to_be_bytes().to_vec(), Some(vec![object(key + 1)])))
                        .collect(),
                },
                IndexDelta {
                    schema: "events".into(),
                    name: "by_message".into(),
                    mode: IndexDeltaMode::Reset,
                    changes: vec![(b"old".to_vec(), Some(vec![object(1)]))],
                },
                IndexDelta {
                    schema: "old".into(),
                    name: "deleted".into(),
                    mode: IndexDeltaMode::Delete,
                    changes: Vec::new(),
                },
            ],
        };
        let next = IndexRootDelta {
            base_lsn: 2,
            target_lsn: 3,
            indexes: vec![
                IndexDelta {
                    schema: "alpha".into(),
                    name: "new".into(),
                    mode: IndexDeltaMode::Reset,
                    changes: vec![(b"first".to_vec(), Some(vec![object(4)]))],
                },
                IndexDelta {
                    schema: "events".into(),
                    name: "by_code".into(),
                    mode: IndexDeltaMode::Patch,
                    changes: vec![
                        (10u32.to_be_bytes().to_vec(), None),
                        (2_500u32.to_be_bytes().to_vec(), Some(vec![object(2_501)])),
                    ],
                },
                IndexDelta {
                    schema: "events".into(),
                    name: "by_message".into(),
                    mode: IndexDeltaMode::Reset,
                    changes: vec![(b"new".to_vec(), Some(vec![object(2)]))],
                },
                IndexDelta {
                    schema: "zeta".into(),
                    name: "last".into(),
                    mode: IndexDeltaMode::Patch,
                    changes: vec![(b"last".to_vec(), Some(vec![object(3)]))],
                },
            ],
        };
        let filesystem = SimulatedFileSystem::new(2);
        let base_path = Path::new("base.owli");
        let merged_path = Path::new("merged.owli");
        write_index_root_file(&filesystem, base_path, &base).unwrap();
        write_merged_index_root_file(&filesystem, base_path, merged_path, &next).unwrap();
        assert_eq!(
            read_index_root_file(&filesystem, merged_path).unwrap(),
            IndexRootDelta::merge(&base, &next).unwrap()
        );
    }

    #[test]
    fn legacy_v1_root_remains_readable() {
        let root = IndexRootDelta {
            base_lsn: 2,
            target_lsn: 4,
            indexes: vec![IndexDelta {
                schema: "events".into(),
                name: "by_time".into(),
                mode: IndexDeltaMode::Reset,
                changes: vec![(vec![1], Some(vec![object(1)])), (vec![2], None)],
            }],
        };
        let mut payload = vec![0; INDEX_HEADER_LEN];
        payload[0] = mode_byte(root.indexes[0].mode);
        put_u32(&mut payload, 8, 6);
        put_u32(&mut payload, 12, 7);
        put_u32(&mut payload, 16, 2);
        payload.extend_from_slice(b"eventsby_time");
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(1);
        payload.extend_from_slice(&object(1).packed().to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&POSTINGS_TOMBSTONE.to_le_bytes());
        payload.push(2);
        let block_len = payload.len() as u32;
        put_u32(&mut payload, 20, block_len);
        let mut bytes = vec![0; HEADER_LEN];
        bytes[..4].copy_from_slice(&MAGIC);
        put_u16(&mut bytes, 4, LEGACY_VERSION);
        put_u16(&mut bytes, 6, HEADER_LEN as u16);
        put_u64(&mut bytes, 8, root.base_lsn);
        put_u64(&mut bytes, 16, root.target_lsn);
        put_u32(&mut bytes, 24, 1);
        put_u64(&mut bytes, 32, payload.len() as u64);
        put_u64(&mut bytes, 40, (HEADER_LEN + payload.len()) as u64);
        bytes.extend_from_slice(&payload);
        let checksum = checksum_with_zeroed_field(&bytes, 48);
        put_u32(&mut bytes, 48, checksum);
        assert_eq!(IndexRootDelta::decode(&bytes).unwrap(), root);
    }

    #[test]
    fn merge_coalesces_keys_and_honors_reset_and_delete() {
        let base = IndexRootDelta {
            base_lsn: 1,
            target_lsn: 2,
            indexes: vec![IndexDelta {
                schema: "events".into(),
                name: "by_time".into(),
                mode: IndexDeltaMode::Patch,
                changes: vec![(vec![1], Some(vec![object(1)]))],
            }],
        };
        let next = IndexRootDelta {
            base_lsn: 2,
            target_lsn: 3,
            indexes: vec![IndexDelta {
                schema: "events".into(),
                name: "by_time".into(),
                mode: IndexDeltaMode::Patch,
                changes: vec![(vec![1], None), (vec![2], Some(vec![object(2)]))],
            }],
        };
        let merged = IndexRootDelta::merge(&base, &next).unwrap();
        assert_eq!(merged.base_lsn, 1);
        assert_eq!(merged.target_lsn, 3);
        assert_eq!(merged.indexes[0].changes.len(), 2);
    }
}
