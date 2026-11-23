//! A checkpoint kept as a small root over immutable part files.
//!
//! A checkpoint is one stretch of bytes: a header, the rows kept as
//! records, the index pages, the column segments. It used to be one file,
//! written whole by every compaction. Here the file at the checkpoint's
//! path is a *root*: a list of extents, each saying that a stretch of the
//! checkpoint's bytes is to be found at some offset of some *part* file.
//! A part is written once and never changed, so a later checkpoint can
//! list a stretch of an earlier one's part again instead of copying it.
//!
//! [`PartedFileSystem`] makes this invisible to whatever reads a
//! checkpoint: opening a root gives a handle that reads the checkpoint's
//! bytes as if they were one file, and asking a root's size gives the
//! checkpoint's length. A checkpoint written as one file before parts
//! existed is not a root, and is opened as the file it is.

use crate::simulation::filesystem::{FileHandle, FileOptions, FileSystem, MemoryMap};
use std::collections::HashMap;
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const ROOT_MAGIC: [u8; 4] = *b"OWLR";
const ROOT_VERSION: u16 = 1;
const ROOT_HEADER_LEN: usize = 40;
const ROOT_PART_LEN: usize = 16;
const ROOT_EXTENT_LEN: usize = 32;

/// How much a [`PartWriter`] gathers before it writes to its file.
const WRITE_BUFFER: usize = 1 << 20;

/// A stretch of a checkpoint's bytes and where a part holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Extent {
    /// Where the stretch starts in the checkpoint.
    pub start: u64,
    pub len: u64,
    /// The part that holds it, by its number.
    pub part: u64,
    /// Where it starts in that part.
    pub offset: u64,
    /// The CRC-32 of the stretch.
    pub checksum: u32,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("eight bytes"))
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
}

/// Whether a path is where a checkpoint is kept, and so may be a root.
/// Nothing else is looked into when it is opened.
fn may_be_root(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(".segment."))
}

/// The path of a part of the checkpoints kept beside `root`.
pub(crate) fn part_path(root: &Path, part: u64) -> PathBuf {
    let name = root.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = match name.rfind(".segment.") {
        Some(at) => &name[..at],
        None => name,
    };
    root.with_file_name(format!("{stem}.part.{part}"))
}

/// The number of the part a path names, when it is one of the parts kept
/// beside `root`.
fn part_number(root: &Path, candidate: &Path) -> Option<u64> {
    if candidate.parent() != root.parent() {
        return None;
    }
    let prefix = part_path(root, 0);
    let prefix = prefix.file_name()?.to_str()?.strip_suffix('0')?;
    candidate
        .file_name()?
        .to_str()?
        .strip_prefix(prefix)?
        .parse()
        .ok()
}

/// A root, read and checked.
struct RootMap {
    len: u64,
    /// Each part the extents name, and its handle once it has been opened.
    parts: Vec<(u64, OnceLock<Arc<dyn FileHandle>>)>,
    /// The extents in the order of the checkpoint's bytes, each with the
    /// place of its part in `parts`.
    extents: Vec<(Extent, usize)>,
    /// The root's own checksum, by which a root read again is known to be
    /// the same one.
    checksum: u32,
    path: PathBuf,
}

impl RootMap {
    fn decode(bytes: &[u8], path: &Path) -> io::Result<Self> {
        if bytes.len() < ROOT_HEADER_LEN || bytes[0..4] != ROOT_MAGIC {
            return Err(invalid("checkpoint root header is invalid"));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        let header_len = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        if version != ROOT_VERSION || header_len != ROOT_HEADER_LEN {
            return Err(invalid("checkpoint root is of a version this build cannot read"));
        }
        let len = u64_at(bytes, 8);
        let part_count = u32_at(bytes, 16) as usize;
        let extent_count = u32_at(bytes, 20) as usize;
        let checksum = u32_at(bytes, 24);
        let expected = part_count
            .checked_mul(ROOT_PART_LEN)
            .and_then(|parts| {
                extent_count
                    .checked_mul(ROOT_EXTENT_LEN)
                    .and_then(|extents| parts.checked_add(extents))
            })
            .and_then(|tables| tables.checked_add(ROOT_HEADER_LEN));
        if expected != Some(bytes.len()) {
            return Err(invalid("checkpoint root is not the length its header states"));
        }
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&bytes[..24]);
        hasher.update(&[0; 4]);
        hasher.update(&bytes[28..]);
        if hasher.finalize() != checksum {
            return Err(invalid("checkpoint root checksum mismatch"));
        }
        let mut parts = Vec::with_capacity(part_count);
        let mut part_lens = Vec::with_capacity(part_count);
        for index in 0..part_count {
            let at = ROOT_HEADER_LEN + index * ROOT_PART_LEN;
            parts.push((u64_at(bytes, at), OnceLock::new()));
            part_lens.push(u64_at(bytes, at + 8));
        }
        let mut extents = Vec::with_capacity(extent_count);
        let mut next = 0u64;
        for index in 0..extent_count {
            let at = ROOT_HEADER_LEN + part_count * ROOT_PART_LEN + index * ROOT_EXTENT_LEN;
            let start = u64_at(bytes, at);
            let extent_len = u64_at(bytes, at + 8);
            let part_index = u32_at(bytes, at + 16) as usize;
            let extent_checksum = u32_at(bytes, at + 20);
            let offset = u64_at(bytes, at + 24);
            let within = part_lens
                .get(part_index)
                .is_some_and(|part_len| offset.checked_add(extent_len).is_some_and(|end| end <= *part_len));
            if start != next || extent_len == 0 || !within {
                return Err(invalid("checkpoint root extents do not cover the checkpoint"));
            }
            next = start + extent_len;
            extents.push((
                Extent {
                    start,
                    len: extent_len,
                    part: parts[part_index].0,
                    offset,
                    checksum: extent_checksum,
                },
                part_index,
            ));
        }
        if next != len {
            return Err(invalid("checkpoint root extents do not cover the checkpoint"));
        }
        Ok(Self { len, parts, extents, checksum, path: path.to_path_buf() })
    }

    fn part(&self, index: usize, filesystem: &dyn FileSystem) -> io::Result<&Arc<dyn FileHandle>> {
        let (number, handle) = &self.parts[index];
        if let Some(handle) = handle.get() {
            return Ok(handle);
        }
        let opened: Arc<dyn FileHandle> = Arc::from(filesystem.open(&part_path(&self.path, *number))?);
        Ok(handle.get_or_init(|| opened))
    }

    /// Read from the checkpoint's bytes at `offset`, as much as `buffer`
    /// holds or the checkpoint has left.
    fn read_at(&self, buffer: &mut [u8], offset: u64, filesystem: &dyn FileSystem) -> io::Result<usize> {
        let mut done = 0usize;
        let mut offset = offset;
        // The extent that holds `offset`: the last that starts at or before it.
        let mut index = self
            .extents
            .partition_point(|(extent, _)| extent.start <= offset)
            .saturating_sub(1);
        while done < buffer.len() && offset < self.len {
            let Some((extent, part_index)) = self.extents.get(index) else {
                break;
            };
            let within = offset - extent.start;
            let wanted = ((extent.len - within) as usize).min(buffer.len() - done);
            let part = self.part(*part_index, filesystem)?;
            let mut read = 0usize;
            while read < wanted {
                let count = part.read_at(
                    &mut buffer[done + read..done + wanted],
                    extent.offset + within + read as u64,
                )?;
                if count == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "a checkpoint part is shorter than its root states",
                    ));
                }
                read += count;
            }
            done += wanted;
            offset += wanted as u64;
            index += 1;
        }
        Ok(done)
    }
}

/// A checkpoint read through its root as if it were one file.
struct PartedHandle {
    root: Arc<RootMap>,
    filesystem: Arc<dyn FileSystem>,
    position: u64,
}

fn read_only() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "a checkpoint read through its root is not written to")
}

impl FileHandle for PartedHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.root.read_at(buf, self.position, self.filesystem.as_ref())?;
        self.position += count as u64;
        Ok(count)
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.root.read_at(buf, offset, self.filesystem.as_ref())
    }

    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(read_only())
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> io::Result<usize> {
        Err(read_only())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.root.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        };
        self.position = target.ok_or_else(|| invalid("seek before the start of a checkpoint"))?;
        Ok(self.position)
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.root.len)
    }

    fn set_len(&mut self, _len: u64) -> io::Result<()> {
        Err(read_only())
    }
}

/// A file system in which a checkpoint kept as a root over parts reads as
/// one file. Everything else passes through to the one beneath.
pub(crate) struct PartedFileSystem {
    inner: Arc<dyn FileSystem>,
    /// Roots read lately, by path, so that the handles on their parts are
    /// opened once and not for every read of the checkpoint.
    roots: Mutex<HashMap<PathBuf, Arc<RootMap>>>,
}

impl PartedFileSystem {
    pub(crate) fn over(inner: Arc<dyn FileSystem>) -> Arc<dyn FileSystem> {
        Arc::new(Self { inner, roots: Mutex::new(HashMap::new()) })
    }

    /// The root at `path`, or `None` when the file there is not a root.
    fn root(&self, path: &Path) -> io::Result<Option<Arc<RootMap>>> {
        if !may_be_root(path) {
            return Ok(None);
        }
        let file = self.inner.open(path)?;
        let mut header = [0u8; ROOT_HEADER_LEN];
        let mut read = 0usize;
        while read < header.len() {
            let count = file.read_at(&mut header[read..], read as u64)?;
            if count == 0 {
                break;
            }
            read += count;
        }
        if read < 4 || header[0..4] != ROOT_MAGIC {
            return Ok(None);
        }
        if read < ROOT_HEADER_LEN {
            return Err(invalid("checkpoint root header is truncated"));
        }
        let checksum = u32_at(&header, 24);
        let size = file.size()?;
        if let Ok(roots) = self.roots.lock() {
            if let Some(known) = roots.get(path) {
                let tables = known.parts.len() * ROOT_PART_LEN + known.extents.len() * ROOT_EXTENT_LEN;
                if known.checksum == checksum && (ROOT_HEADER_LEN + tables) as u64 == size {
                    return Ok(Some(known.clone()));
                }
            }
        }
        let mut bytes = vec![0u8; size as usize];
        let mut read = 0usize;
        while read < bytes.len() {
            let count = file.read_at(&mut bytes[read..], read as u64)?;
            if count == 0 {
                return Err(invalid("checkpoint root is truncated"));
            }
            read += count;
        }
        let root = Arc::new(RootMap::decode(&bytes, path)?);
        if let Ok(mut roots) = self.roots.lock() {
            roots.insert(path.to_path_buf(), root.clone());
        }
        Ok(Some(root))
    }

    fn forget(&self, path: &Path) {
        if may_be_root(path) {
            if let Ok(mut roots) = self.roots.lock() {
                roots.remove(path);
            }
        }
    }
}

impl FileSystem for PartedFileSystem {
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        self.forget(path);
        self.inner.create(path)
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        match self.root(path)? {
            Some(root) => Ok(Box::new(PartedHandle {
                root,
                filesystem: self.inner.clone(),
                position: 0,
            })),
            None => self.inner.open(path),
        }
    }

    fn open_with_options(&self, path: &Path, options: &FileOptions) -> io::Result<Box<dyn FileHandle>> {
        self.forget(path);
        self.inner.open_with_options(path, options)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        self.forget(path);
        self.inner.remove(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.forget(from);
        self.forget(to);
        self.inner.rename(from, to)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn file_size(&self, path: &Path) -> io::Result<u64> {
        match self.root(path)? {
            Some(root) => Ok(root.len),
            None => self.inner.file_size(path),
        }
    }

    fn sync_all(&self) -> io::Result<()> {
        self.inner.sync_all()
    }

    fn mmap(&self, path: &Path, size: usize) -> io::Result<Box<dyn MemoryMap>> {
        self.inner.mmap(path, size)
    }

    fn list_dir(&self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.list_dir(directory)
    }
}

/// The CRC-32 of two stretches of bytes one after the other, from the
/// CRC-32 of each and the length of the second.
pub(crate) fn checksum_of_both(first: u32, second: u32, second_len: u64) -> u32 {
    let mut both = crc32fast::Hasher::new_with_initial(first);
    both.combine(&crc32fast::Hasher::new_with_initial_len(second, second_len));
    both.finalize()
}

/// The parts a root lists, each with its length, and its extents.
pub(crate) struct RootTables {
    pub parts: Vec<(u64, u64)>,
    pub extents: Vec<Extent>,
}

impl RootTables {
    /// Where a stretch of the checkpoint is held, when one extent holds
    /// all of it: the part, the offset in it, and the checksum of the
    /// stretch when it is exactly that extent.
    pub(crate) fn place(&self, start: u64, len: u64) -> Option<(u64, u64, Option<u32>)> {
        let index = self
            .extents
            .partition_point(|extent| extent.start <= start)
            .checked_sub(1)?;
        let extent = self.extents.get(index)?;
        let within = start - extent.start;
        if within.checked_add(len)? > extent.len {
            return None;
        }
        let whole = (within == 0 && len == extent.len).then_some(extent.checksum);
        Some((extent.part, extent.offset + within, whole))
    }

    pub(crate) fn part_len(&self, part: u64) -> Option<u64> {
        self.parts
            .iter()
            .find(|(number, _)| *number == part)
            .map(|(_, len)| *len)
    }
}

/// The tables of the root at `path`, or `None` when the file there is a
/// checkpoint kept as one file.
pub(crate) fn root_tables(filesystem: &dyn FileSystem, path: &Path) -> io::Result<Option<RootTables>> {
    let bytes = read_raw(filesystem, path)?;
    if bytes.len() < 4 || bytes[0..4] != ROOT_MAGIC {
        return Ok(None);
    }
    let root = RootMap::decode(&bytes, path)?;
    let part_lens = (0..root.parts.len())
        .map(|index| u64_at(&bytes, ROOT_HEADER_LEN + index * ROOT_PART_LEN + 8))
        .collect::<Vec<_>>();
    Ok(Some(RootTables {
        parts: root
            .parts
            .iter()
            .zip(part_lens)
            .map(|((part, _), len)| (*part, len))
            .collect(),
        extents: root.extents.iter().map(|(extent, _)| *extent).collect(),
    }))
}

/// Writes one part: the bytes of a checkpoint that no earlier part holds.
/// It notes each stretch it writes as an extent, with its checksum, and
/// gathers what it is given so that the file is written a megabyte at a
/// time and not a record at a time. A stretch an earlier part holds is
/// not written: it is noted as an extent of that part.
pub(crate) struct PartWriter {
    file: Box<dyn FileHandle>,
    part: u64,
    buffer: Vec<u8>,
    /// How many bytes have been handed to this writer, which is how long
    /// its part will be.
    written: u64,
    /// How far through the checkpoint's bytes this writer is: what it has
    /// written and what it has listed from earlier parts.
    at: u64,
    extents: Vec<Extent>,
    /// Where in the checkpoint and in the part the stretch being written
    /// starts, and its checksum so far.
    current: (u64, u64, crc32fast::Hasher),
    /// Whether the last extent noted is of an earlier part.
    last_listed: bool,
    /// Whether that extent may have the next stretch joined to it.
    last_joined: bool,
}

impl PartWriter {
    pub(crate) fn create(filesystem: &dyn FileSystem, root: &Path, part: u64) -> io::Result<Self> {
        Ok(Self {
            file: filesystem.create(&part_path(root, part))?,
            part,
            buffer: Vec::with_capacity(WRITE_BUFFER),
            written: 0,
            at: 0,
            extents: Vec::new(),
            current: (0, 0, crc32fast::Hasher::new()),
            last_listed: false,
            last_joined: false,
        })
    }

    /// End the stretch being written, so that what follows is an extent
    /// of its own.
    pub(crate) fn cut(&mut self) {
        let (start, offset, hasher) =
            std::mem::replace(&mut self.current, (self.at, self.written, crc32fast::Hasher::new()));
        if self.written > offset {
            self.extents.push(Extent {
                start,
                len: self.written - offset,
                part: self.part,
                offset,
                checksum: hasher.finalize(),
            });
            self.last_listed = false;
        }
    }

    /// Take the next `len` bytes of the checkpoint from an earlier part,
    /// where they are at `offset`, without writing them. With `joined`,
    /// stretches that follow one another in the same part are noted as
    /// one extent; without, each keeps an extent and a checksum of its
    /// own, and can be listed again later without being read.
    pub(crate) fn list(&mut self, part: u64, offset: u64, len: u64, checksum: u32, joined: bool) {
        if len == 0 {
            return;
        }
        self.cut();
        match self.extents.last_mut() {
            Some(last)
                if joined
                    && self.last_joined
                    && self.last_listed
                    && last.part == part
                    && last.offset + last.len == offset
                    && last.start + last.len == self.at =>
            {
                last.checksum = checksum_of_both(last.checksum, checksum, len);
                last.len += len;
            }
            _ => self.extents.push(Extent { start: self.at, len, part, offset, checksum }),
        }
        self.last_listed = true;
        self.last_joined = joined;
        self.at += len;
        self.current = (self.at, self.written, crc32fast::Hasher::new());
    }

    /// How far through the checkpoint's bytes this writer is.
    pub(crate) fn checkpoint_len(&self) -> u64 {
        self.at
    }

    fn write_out(&mut self) -> io::Result<()> {
        let mut done = 0usize;
        while done < self.buffer.len() {
            let count = self.file.write(&self.buffer[done..])?;
            if count == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "a checkpoint part write made no progress"));
            }
            done += count;
        }
        self.buffer.clear();
        Ok(())
    }

    /// Write what is left, put `first` in place of the first bytes
    /// written, which must have been cut off as an extent of their own and
    /// be of the same length, and make the part durable. Gives the part's
    /// length and its extents.
    pub(crate) fn finish(mut self, first: &[u8]) -> io::Result<(u64, Vec<Extent>)> {
        self.cut();
        self.write_out()?;
        match self.extents.first_mut() {
            Some(extent) if extent.len == first.len() as u64 => {
                extent.checksum = crc32fast::hash(first);
            }
            _ => return Err(invalid("a checkpoint part does not begin with its header")),
        }
        let mut done = 0usize;
        while done < first.len() {
            let count = self.file.write_at(&first[done..], done as u64)?;
            if count == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "a checkpoint part write made no progress"));
            }
            done += count;
        }
        self.file.flush()?;
        self.file.sync()?;
        Ok((self.written, self.extents))
    }
}

fn write_only() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "a checkpoint part being written is only written to")
}

impl FileHandle for PartWriter {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(write_only())
    }

    fn read_at(&self, _buf: &mut [u8], _offset: u64) -> io::Result<usize> {
        Err(write_only())
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.current.2.update(buf);
        self.written += buf.len() as u64;
        self.at += buf.len() as u64;
        self.buffer.extend_from_slice(buf);
        if self.buffer.len() >= WRITE_BUFFER {
            self.write_out()?;
        }
        Ok(buf.len())
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> io::Result<usize> {
        Err(write_only())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.write_out()
    }

    fn sync(&mut self) -> io::Result<()> {
        Err(write_only())
    }

    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        Err(write_only())
    }

    fn position(&self) -> u64 {
        self.written
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.written)
    }

    fn set_len(&mut self, _len: u64) -> io::Result<()> {
        Err(write_only())
    }
}

/// Write the root of a checkpoint of `len` bytes held by `extents`, and
/// make it durable. `part_lens` gives the length of each part named.
pub(crate) fn write_root(
    filesystem: &dyn FileSystem,
    path: &Path,
    len: u64,
    part_lens: &[(u64, u64)],
    extents: &[Extent],
) -> io::Result<()> {
    let mut bytes = vec![0u8; ROOT_HEADER_LEN];
    bytes[0..4].copy_from_slice(&ROOT_MAGIC);
    bytes[4..6].copy_from_slice(&ROOT_VERSION.to_le_bytes());
    bytes[6..8].copy_from_slice(&(ROOT_HEADER_LEN as u16).to_le_bytes());
    bytes[8..16].copy_from_slice(&len.to_le_bytes());
    bytes[16..20].copy_from_slice(&(part_lens.len() as u32).to_le_bytes());
    bytes[20..24].copy_from_slice(&(extents.len() as u32).to_le_bytes());
    for (part, part_len) in part_lens {
        bytes.extend_from_slice(&part.to_le_bytes());
        bytes.extend_from_slice(&part_len.to_le_bytes());
    }
    for extent in extents {
        let part_index = part_lens
            .iter()
            .position(|(part, _)| *part == extent.part)
            .ok_or_else(|| invalid("a checkpoint extent names a part its root does not list"))?;
        bytes.extend_from_slice(&extent.start.to_le_bytes());
        bytes.extend_from_slice(&extent.len.to_le_bytes());
        bytes.extend_from_slice(&(part_index as u32).to_le_bytes());
        bytes.extend_from_slice(&extent.checksum.to_le_bytes());
        bytes.extend_from_slice(&extent.offset.to_le_bytes());
    }
    let checksum = crc32fast::hash(&bytes);
    bytes[24..28].copy_from_slice(&checksum.to_le_bytes());
    // Checked before it is written: a root that does not read back is
    // worse than no root.
    RootMap::decode(&bytes, path)?;
    let mut file = filesystem.create(path)?;
    let mut done = 0usize;
    while done < bytes.len() {
        let count = file.write(&bytes[done..])?;
        if count == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "a checkpoint root write made no progress"));
        }
        done += count;
    }
    file.flush()?;
    file.sync()
}

/// What the file at a checkpoint's path says of the parts it needs.
pub(crate) enum PartsOf {
    /// There is no file there.
    Absent,
    /// A checkpoint written as one file, which needs no parts.
    OneFile,
    /// A root, and the parts its extents name.
    Root(Vec<u64>),
    /// A file that is neither, such as a root cut short by a crash.
    Unreadable,
}

/// Look at the file at a checkpoint's path.
pub(crate) fn parts_of(filesystem: &dyn FileSystem, path: &Path) -> PartsOf {
    if !filesystem.exists(path) {
        return PartsOf::Absent;
    }
    let Ok(bytes) = read_raw(filesystem, path) else {
        return PartsOf::Unreadable;
    };
    if bytes.len() >= 4 && bytes[0..4] != ROOT_MAGIC {
        return PartsOf::OneFile;
    }
    match RootMap::decode(&bytes, path) {
        Ok(root) => PartsOf::Root(root.parts.iter().map(|(part, _)| *part).collect()),
        Err(_) => PartsOf::Unreadable,
    }
}

/// The bytes of the file itself. Opening with options is never answered
/// through a root, so this sees a root as the file it is whichever file
/// system it is given.
fn read_raw(filesystem: &dyn FileSystem, path: &Path) -> io::Result<Vec<u8>> {
    let file = filesystem.open_with_options(path, &FileOptions::default())?;
    let mut bytes = vec![0u8; file.size()? as usize];
    let mut read = 0usize;
    while read < bytes.len() {
        let count = file.read_at(&mut bytes[read..], read as u64)?;
        if count == 0 {
            return Err(invalid("file is shorter than its size"));
        }
        read += count;
    }
    Ok(bytes)
}

/// Remove every part beside `roots` that none of them lists. Nothing is
/// removed unless every root can be read, so that a root that cannot be
/// made sense of never costs a part another needs. Gives the number of
/// parts removed and their bytes.
pub(crate) fn remove_unlisted_parts(filesystem: &dyn FileSystem, roots: &[PathBuf]) -> io::Result<(usize, u64)> {
    let mut listed = std::collections::HashSet::new();
    for root in roots {
        match parts_of(filesystem, root) {
            PartsOf::Absent | PartsOf::OneFile => {}
            PartsOf::Root(parts) => listed.extend(parts),
            PartsOf::Unreadable => return Ok((0, 0)),
        }
    }
    let Some(first) = roots.first() else {
        return Ok((0, 0));
    };
    let directory = first.parent().unwrap_or_else(|| Path::new("."));
    let mut removed = (0usize, 0u64);
    for candidate in filesystem.list_dir(directory)? {
        let Some(part) = part_number(first, &candidate) else {
            continue;
        };
        if !listed.contains(&part) {
            let bytes = filesystem.file_size(&candidate).unwrap_or(0);
            // A part is not itself a root, so its size is its own.
            filesystem.remove(&candidate)?;
            removed.0 += 1;
            removed.1 += bytes;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::filesystem::SimulatedFileSystem;

    fn write_part(filesystem: &dyn FileSystem, root: &Path, part: u64, bytes: &[u8]) {
        let mut file = filesystem.create(&part_path(root, part)).unwrap();
        file.write(bytes).unwrap();
        file.sync().unwrap();
    }

    #[test]
    fn a_root_reads_as_the_bytes_its_extents_name() {
        let inner: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(1));
        let root = PathBuf::from("/db/data.owl.segment.0");
        write_part(inner.as_ref(), &root, 3, b"0123456789");
        write_part(inner.as_ref(), &root, 7, b"abcdefghij");
        let extents = [
            Extent { start: 0, len: 4, part: 7, offset: 2, checksum: crc32fast::hash(b"cdef") },
            Extent { start: 4, len: 3, part: 3, offset: 7, checksum: crc32fast::hash(b"789") },
            Extent { start: 7, len: 2, part: 7, offset: 0, checksum: crc32fast::hash(b"ab") },
        ];
        write_root(inner.as_ref(), &root, 9, &[(3, 10), (7, 10)], &extents).unwrap();

        let parted = PartedFileSystem::over(inner.clone());
        assert_eq!(parted.file_size(&root).unwrap(), 9);
        let mut file = parted.open(&root).unwrap();
        let mut all = [0u8; 9];
        assert_eq!(file.read(&mut all).unwrap(), 9);
        assert_eq!(&all, b"cdef789ab");
        let mut middle = [0u8; 4];
        assert_eq!(file.read_at(&mut middle, 3).unwrap(), 4);
        assert_eq!(&middle, b"f789");
        let mut past = [0u8; 4];
        assert_eq!(file.read_at(&mut past, 7).unwrap(), 2);
        assert_eq!(&past[..2], b"ab");
    }

    #[test]
    fn a_file_that_is_not_a_root_is_opened_as_it_is() {
        let inner: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(1));
        let path = PathBuf::from("/db/data.owl.segment.1");
        let mut file = inner.create(&path).unwrap();
        file.write(b"OWLS and the rest").unwrap();
        drop(file);
        let parted = PartedFileSystem::over(inner);
        assert_eq!(parted.file_size(&path).unwrap(), 17);
        let mut read = [0u8; 4];
        parted.open(&path).unwrap().read(&mut read).unwrap();
        assert_eq!(&read, b"OWLS");
    }

    #[test]
    fn a_root_that_does_not_cover_its_checkpoint_is_refused() {
        let inner: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(1));
        let root = PathBuf::from("/db/data.owl.segment.0");
        write_part(inner.as_ref(), &root, 1, b"0123456789");
        let gap = [
            Extent { start: 0, len: 4, part: 1, offset: 0, checksum: 0 },
            Extent { start: 5, len: 4, part: 1, offset: 4, checksum: 0 },
        ];
        assert!(write_root(inner.as_ref(), &root, 9, &[(1, 10)], &gap).is_err());
        let beyond = [Extent { start: 0, len: 11, part: 1, offset: 0, checksum: 0 }];
        assert!(write_root(inner.as_ref(), &root, 11, &[(1, 10)], &beyond).is_err());
    }

    #[test]
    fn parts_no_root_lists_are_removed_and_the_rest_kept() {
        let inner: Arc<dyn FileSystem> = Arc::new(SimulatedFileSystem::new(1));
        let roots = [
            PathBuf::from("/db/data.owl.segment.0"),
            PathBuf::from("/db/data.owl.segment.1"),
        ];
        for part in [1, 2, 3] {
            write_part(inner.as_ref(), &roots[0], part, b"0123456789");
        }
        let whole = |part| [Extent { start: 0, len: 10, part, offset: 0, checksum: 0 }];
        write_root(inner.as_ref(), &roots[0], 10, &[(1, 10)], &whole(1)).unwrap();
        write_root(inner.as_ref(), &roots[1], 10, &[(3, 10)], &whole(3)).unwrap();
        assert_eq!(remove_unlisted_parts(inner.as_ref(), &roots).unwrap(), (1, 10));
        assert!(inner.exists(&part_path(&roots[0], 1)));
        assert!(!inner.exists(&part_path(&roots[0], 2)));
        assert!(inner.exists(&part_path(&roots[0], 3)));

        // A root that cannot be read stops anything being removed.
        let mut torn = inner.create(&roots[1]).unwrap();
        torn.write(b"OWLR").unwrap();
        drop(torn);
        assert_eq!(remove_unlisted_parts(inner.as_ref(), &roots).unwrap(), (0, 0));
        assert!(inner.exists(&part_path(&roots[0], 3)));
    }
}
