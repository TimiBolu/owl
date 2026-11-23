//! A file system made of a fixed set of flat files.
//!
//! A browser hands out a file it can read and write without waiting only
//! after being waited on to open it, and the engine opens its files
//! without waiting. So the files are opened first, a pool of them, and this
//! keeps the engine's files in them: each file of the pool holds one of the
//! engine's files, under a header that says which. Making a file takes a
//! free one from the pool; removing a file gives it back; renaming a file
//! rewrites a header. None of that waits.
//!
//! What is under the pool is a [`SlotStorage`]: in a browser, a file of its
//! private file system; in the tests, bytes in memory.

use owl_db::simulation::filesystem::{FileHandle, FileOptions, FileSystem, MemoryMap};
use std::collections::HashMap;
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One flat file of the pool.
pub trait SlotStorage: Send + Sync {
    /// Read at an offset; fewer bytes than asked for at the end.
    fn read_at(&self, buffer: &mut [u8], at: u64) -> io::Result<usize>;
    fn write_at(&self, buffer: &[u8], at: u64) -> io::Result<usize>;
    fn truncate(&self, length: u64) -> io::Result<()>;
    fn size(&self) -> io::Result<u64>;
    /// Make what was written last.
    fn flush(&self) -> io::Result<()>;
}

/// The header is written twice over, each copy in its own half: a change
/// is written to the half that does not hold the copy in force, so a write
/// cut short spoils the new copy and leaves the old.
const HEADER: u64 = 4096;
const HALF: u64 = HEADER / 2;
const MAGIC: &[u8; 8] = b"OWLSLOT1";
/// Magic, stamp, length of the path, checksum.
const FIXED: usize = 8 + 8 + 4 + 4;

/// What one half of a header says: which file this is, and when it was
/// said. Of two that disagree the later is in force.
struct Named {
    stamp: u64,
    path: PathBuf,
}

fn encode(stamp: u64, path: &Path) -> io::Result<Vec<u8>> {
    let name = path.to_string_lossy();
    let name = name.as_bytes();
    if name.len() > HALF as usize - FIXED {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a file's name is too long for the pool"));
    }
    let mut half = vec![0u8; HALF as usize];
    half[0..8].copy_from_slice(MAGIC);
    half[8..16].copy_from_slice(&stamp.to_le_bytes());
    half[16..20].copy_from_slice(&(name.len() as u32).to_le_bytes());
    half[FIXED..FIXED + name.len()].copy_from_slice(name);
    let mut checked = crc32fast::Hasher::new();
    checked.update(&half[8..20]);
    checked.update(name);
    half[20..24].copy_from_slice(&checked.finalize().to_le_bytes());
    Ok(half)
}

fn decode(half: &[u8]) -> Option<Named> {
    if half.len() < HALF as usize || &half[0..8] != MAGIC {
        return None;
    }
    let stamp = u64::from_le_bytes(half[8..16].try_into().ok()?);
    let length = u32::from_le_bytes(half[16..20].try_into().ok()?) as usize;
    let checksum = u32::from_le_bytes(half[20..24].try_into().ok()?);
    let name = half.get(FIXED..FIXED + length)?;
    let mut checked = crc32fast::Hasher::new();
    checked.update(&half[8..20]);
    checked.update(name);
    if checked.finalize() != checksum || length == 0 {
        return None;
    }
    Some(Named { stamp, path: PathBuf::from(std::str::from_utf8(name).ok()?) })
}

struct Slot {
    storage: Box<dyn SlotStorage>,
    /// The engine's file held here, if one is.
    path: Option<PathBuf>,
    /// Which half of the header is in force: the next change is written to
    /// the other.
    half: u64,
    /// Moves when the file held here is removed, so that a handle on what
    /// was here cannot read or write what is here next.
    turn: u64,
}

impl Slot {
    /// Read both halves of the header and take the later that is whole.
    fn read(storage: Box<dyn SlotStorage>) -> io::Result<(Self, u64)> {
        let mut found: Option<(u64, Named)> = None;
        for half in [0, 1] {
            let mut bytes = vec![0u8; HALF as usize];
            let mut read = 0;
            while read < bytes.len() {
                match storage.read_at(&mut bytes[read..], half * HALF + read as u64)? {
                    0 => break,
                    count => read += count,
                }
            }
            if let Some(named) = decode(&bytes) {
                if found.as_ref().map_or(true, |(_, known)| named.stamp > known.stamp) {
                    found = Some((half, named));
                }
            }
        }
        let (half, path, stamp) = match found {
            Some((half, named)) => (half, Some(named.path), named.stamp),
            None => (1, None, 0),
        };
        Ok((Self { storage, path, half, turn: 0 }, stamp))
    }

    /// Say which file this holds, in the half not in force, and make it
    /// last. Only then is it in force.
    fn name(&mut self, stamp: u64, path: &Path) -> io::Result<()> {
        let other = 1 - self.half;
        let bytes = encode(stamp, path)?;
        write_all(self.storage.as_ref(), &bytes, other * HALF)?;
        self.storage.flush()?;
        self.half = other;
        self.path = Some(path.to_path_buf());
        Ok(())
    }

    /// Hold nothing: both halves are wiped and what was held is cut away.
    fn clear(&mut self) -> io::Result<()> {
        self.storage.truncate(0)?;
        self.storage.flush()?;
        self.path = None;
        self.half = 1;
        self.turn += 1;
        Ok(())
    }
}

fn write_all(storage: &dyn SlotStorage, mut bytes: &[u8], mut at: u64) -> io::Result<()> {
    while !bytes.is_empty() {
        match storage.write_at(bytes, at)? {
            0 => return Err(io::Error::new(io::ErrorKind::WriteZero, "the pool's storage took no bytes")),
            count => {
                bytes = &bytes[count..];
                at += count as u64;
            }
        }
    }
    Ok(())
}

struct Pool {
    slots: Vec<Slot>,
    by_path: HashMap<PathBuf, usize>,
    /// The stamp of the last header written; the next is one more.
    stamp: u64,
}

impl Pool {
    fn take(&mut self, slot: Slot, stamp: u64) -> io::Result<()> {
        self.stamp = self.stamp.max(stamp);
        let at = self.slots.len();
        self.slots.push(slot);
        let Some(path) = self.slots[at].path.clone() else {
            return Ok(());
        };
        // Two that claim one name: a rename over a file was cut short
        // after the new was named and before the old was let go. The later
        // is the file; the earlier is let go now.
        match self.by_path.get(&path).copied() {
            Some(known) => {
                let (keep, drop) = match stamp > self.stamp_of(known)? {
                    true => (at, known),
                    false => (known, at),
                };
                self.slots[drop].clear()?;
                self.by_path.insert(path, keep);
            }
            None => {
                self.by_path.insert(path, at);
            }
        }
        Ok(())
    }

    fn stamp_of(&self, slot: usize) -> io::Result<u64> {
        let slot = &self.slots[slot];
        let mut bytes = vec![0u8; HALF as usize];
        let mut read = 0;
        while read < bytes.len() {
            match slot.storage.read_at(&mut bytes[read..], slot.half * HALF + read as u64)? {
                0 => break,
                count => read += count,
            }
        }
        Ok(decode(&bytes).map_or(0, |named| named.stamp))
    }

    fn next_stamp(&mut self) -> u64 {
        self.stamp += 1;
        self.stamp
    }

    /// A new, empty file under `path`, in a slot that held nothing.
    fn make(&mut self, path: &Path) -> io::Result<usize> {
        let Some(at) = self.slots.iter().position(|slot| slot.path.is_none()) else {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "the pool has no file left to give; more must be added before this is tried again",
            ));
        };
        let stamp = self.next_stamp();
        let slot = &mut self.slots[at];
        slot.storage.truncate(0)?;
        slot.half = 1;
        slot.name(stamp, path)?;
        // The header is all there is: the file itself is empty.
        slot.storage.truncate(HEADER)?;
        slot.storage.flush()?;
        self.by_path.insert(path.to_path_buf(), at);
        Ok(at)
    }

    fn remove(&mut self, path: &Path) -> io::Result<()> {
        match self.by_path.remove(path) {
            Some(at) => self.slots[at].clear(),
            None => Ok(()),
        }
    }
}

/// The engine's files, kept in a pool of flat files.
#[derive(Clone)]
pub struct PoolFileSystem {
    pool: Arc<Mutex<Pool>>,
}

impl PoolFileSystem {
    /// Take up what a pool holds: every file named by a whole header.
    pub fn open(slots: Vec<Box<dyn SlotStorage>>) -> io::Result<Self> {
        let files = Self { pool: Arc::new(Mutex::new(Pool { slots: Vec::new(), by_path: HashMap::new(), stamp: 0 })) };
        files.add(slots)?;
        Ok(files)
    }

    /// Add files to the pool. One that already holds a file brings it.
    pub fn add(&self, slots: Vec<Box<dyn SlotStorage>>) -> io::Result<()> {
        let mut pool = self.locked()?;
        for storage in slots {
            let (slot, stamp) = Slot::read(storage)?;
            pool.take(slot, stamp)?;
        }
        Ok(())
    }

    /// How many files of the pool hold nothing.
    pub fn free(&self) -> usize {
        self.locked().map_or(0, |pool| pool.slots.iter().filter(|slot| slot.path.is_none()).count())
    }

    /// The engine's files and their sizes, by name.
    pub fn files(&self) -> Vec<(String, u64)> {
        let Ok(pool) = self.locked() else {
            return Vec::new();
        };
        let mut files: Vec<(String, u64)> = pool
            .by_path
            .iter()
            .map(|(path, at)| {
                let size = pool.slots[*at].storage.size().unwrap_or(HEADER).saturating_sub(HEADER);
                (path.to_string_lossy().into_owned(), size)
            })
            .collect();
        files.sort();
        files
    }

    fn locked(&self) -> io::Result<std::sync::MutexGuard<'_, Pool>> {
        self.pool.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "the pool's lock is poisoned"))
    }

    fn handle(&self, slot: usize, turn: u64, position: u64) -> Box<dyn FileHandle> {
        Box::new(PoolHandle { pool: self.pool.clone(), slot, turn, position })
    }
}

impl FileSystem for PoolFileSystem {
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        self.open_with_options(
            path,
            &FileOptions { read: false, write: true, create: true, truncate: true, append: false },
        )
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        self.open_with_options(path, &FileOptions::default())
    }

    fn open_with_options(&self, path: &Path, options: &FileOptions) -> io::Result<Box<dyn FileHandle>> {
        let mut pool = self.locked()?;
        let at = match pool.by_path.get(path).copied() {
            Some(at) => {
                if options.truncate {
                    pool.slots[at].storage.truncate(HEADER)?;
                }
                at
            }
            None if options.create => pool.make(path)?,
            None => return Err(io::Error::new(io::ErrorKind::NotFound, "File not found")),
        };
        let slot = &pool.slots[at];
        let position = match options.append {
            true => slot.storage.size()?.saturating_sub(HEADER),
            false => 0,
        };
        let turn = slot.turn;
        drop(pool);
        Ok(self.handle(at, turn, position))
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        self.locked()?.remove(path)
    }

    fn list_dir(&self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        let pool = self.locked()?;
        let mut listed: Vec<PathBuf> = pool
            .by_path
            .keys()
            .filter(|path| path.parent() == Some(directory))
            .cloned()
            .collect();
        listed.sort();
        Ok(listed)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut pool = self.locked()?;
        let Some(at) = pool.by_path.get(from).copied() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "File not found"));
        };
        if from == to {
            return Ok(());
        }
        // The file takes its new name first. If this stops before what had
        // the name is let go, two claim it, and the later is the file.
        let stamp = pool.next_stamp();
        pool.slots[at].name(stamp, to)?;
        pool.by_path.remove(from);
        if let Some(replaced) = pool.by_path.insert(to.to_path_buf(), at) {
            pool.slots[replaced].clear()?;
        }
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        self.locked().is_ok_and(|pool| pool.by_path.contains_key(path))
    }

    fn file_size(&self, path: &Path) -> io::Result<u64> {
        let pool = self.locked()?;
        match pool.by_path.get(path) {
            Some(at) => Ok(pool.slots[*at].storage.size()?.saturating_sub(HEADER)),
            None => Err(io::Error::new(io::ErrorKind::NotFound, "File not found")),
        }
    }

    fn sync_all(&self) -> io::Result<()> {
        // A name is made to last when it is given, and a file when it is
        // synced; there is no directory beside them to sync.
        Ok(())
    }

    fn mmap(&self, _path: &Path, _size: usize) -> io::Result<Box<dyn MemoryMap>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "the pool does not map files into memory"))
    }
}

/// One of the engine's files, open.
struct PoolHandle {
    pool: Arc<Mutex<Pool>>,
    slot: usize,
    turn: u64,
    position: u64,
}

impl PoolHandle {
    /// Do something with the file's storage, if the file is still the one
    /// this was opened on.
    fn with<T>(&self, work: impl FnOnce(&dyn SlotStorage) -> io::Result<T>) -> io::Result<T> {
        let pool = self.pool.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "the pool's lock is poisoned"))?;
        let slot = &pool.slots[self.slot];
        if slot.turn != self.turn {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the file was removed while it was open"));
        }
        work(slot.storage.as_ref())
    }
}

impl FileHandle for PoolHandle {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.read_at(buffer, self.position)?;
        self.position += read as u64;
        Ok(read)
    }

    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        self.with(|storage| storage.read_at(buffer, HEADER + offset))
    }

    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.write_at(buffer, self.position)?;
        self.position += written as u64;
        Ok(written)
    }

    fn write_at(&self, buffer: &[u8], offset: u64) -> io::Result<usize> {
        self.with(|storage| storage.write_at(buffer, HEADER + offset))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn sync(&mut self) -> io::Result<()> {
        self.with(|storage| storage.flush())
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let size = self.size()?;
        let moved = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => size.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        };
        self.position = moved.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek to before the file's start"))?;
        Ok(self.position)
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn size(&self) -> io::Result<u64> {
        self.with(|storage| Ok(storage.size()?.saturating_sub(HEADER)))
    }

    fn set_len(&mut self, length: u64) -> io::Result<()> {
        self.with(|storage| storage.truncate(HEADER + length))
    }
}

/// Bytes in memory, as one file of a pool: what the tests put under it. A
/// copy shares the bytes, as two handles on one file do.
#[derive(Clone, Default)]
pub struct MemorySlot {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl SlotStorage for MemorySlot {
    fn read_at(&self, buffer: &mut [u8], at: u64) -> io::Result<usize> {
        let bytes = self.bytes.lock().unwrap();
        let at = (at as usize).min(bytes.len());
        let count = buffer.len().min(bytes.len() - at);
        buffer[..count].copy_from_slice(&bytes[at..at + count]);
        Ok(count)
    }

    fn write_at(&self, buffer: &[u8], at: u64) -> io::Result<usize> {
        let mut bytes = self.bytes.lock().unwrap();
        let end = at as usize + buffer.len();
        if bytes.len() < end {
            bytes.resize(end, 0);
        }
        bytes[at as usize..end].copy_from_slice(buffer);
        Ok(buffer.len())
    }

    fn truncate(&self, length: u64) -> io::Result<()> {
        self.bytes.lock().unwrap().resize(length as usize, 0);
        Ok(())
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.bytes.lock().unwrap().len() as u64)
    }

    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::SeekFrom;

    fn pool(slots: &[MemorySlot]) -> PoolFileSystem {
        PoolFileSystem::open(slots.iter().map(|slot| Box::new(slot.clone()) as Box<dyn SlotStorage>).collect()).unwrap()
    }

    fn read(files: &PoolFileSystem, path: &str) -> Vec<u8> {
        let mut file = files.open(Path::new(path)).unwrap();
        let mut bytes = vec![0u8; file.size().unwrap() as usize];
        let mut read = 0;
        while read < bytes.len() {
            read += file.read(&mut bytes[read..]).unwrap();
        }
        bytes
    }

    #[test]
    fn files_are_made_written_read_and_outlast_the_pool_that_made_them() {
        let slots: Vec<MemorySlot> = (0..4).map(|_| MemorySlot::default()).collect();
        {
            let files = pool(&slots);
            assert_eq!(files.free(), 4);
            let mut one = files.create(Path::new("db.wal")).unwrap();
            one.write(b"hello, ").unwrap();
            one.write(b"owl").unwrap();
            one.sync().unwrap();
            assert_eq!(one.size().unwrap(), 10);
            let mut two = files.create(Path::new("db.segment.0")).unwrap();
            two.write(&[7u8; 5000]).unwrap();
            two.write_at(b"!!", 2).unwrap();
            two.set_len(4999).unwrap();
            assert!(files.exists(Path::new("db.wal")));
            assert!(!files.exists(Path::new("nothing")));
            assert_eq!(files.file_size(Path::new("db.segment.0")).unwrap(), 4999);
            assert_eq!(files.free(), 2);
            // Opened again with nothing asked for, it is read from its start.
            assert_eq!(read(&files, "db.wal"), b"hello, owl");
            // Asked to append, a write goes at its end.
            let mut more = files
                .open_with_options(
                    Path::new("db.wal"),
                    &FileOptions { read: true, write: true, create: false, truncate: false, append: true },
                )
                .unwrap();
            assert_eq!(more.position(), 10);
            more.write(b"s").unwrap();
            assert_eq!(more.seek(SeekFrom::End(-4)).unwrap(), 7);
            let mut tail = [0u8; 4];
            more.read(&mut tail).unwrap();
            assert_eq!(&tail, b"owls");
            assert!(files.open(Path::new("nothing")).is_err());
        }
        // A new pool over the same files finds what the first one kept.
        let files = pool(&slots);
        assert_eq!(files.free(), 2);
        assert_eq!(read(&files, "db.wal"), b"hello, owls");
        let segment = read(&files, "db.segment.0");
        assert_eq!(segment.len(), 4999);
        assert_eq!(&segment[..5], &[7, 7, b'!', b'!', 7]);
        assert_eq!(
            files.files(),
            vec![("db.segment.0".to_string(), 4999), ("db.wal".to_string(), 11)]
        );
    }

    #[test]
    fn a_file_removed_gives_its_place_back_and_cannot_be_read_through_an_old_handle() {
        let slots: Vec<MemorySlot> = (0..2).map(|_| MemorySlot::default()).collect();
        let files = pool(&slots);
        let mut old = files.create(Path::new("a")).unwrap();
        old.write(b"secret").unwrap();
        files.create(Path::new("b")).unwrap();
        // The pool is full.
        assert!(files.create(Path::new("c")).is_err());
        files.remove(Path::new("a")).unwrap();
        assert!(!files.exists(Path::new("a")));
        let mut new = files.create(Path::new("c")).unwrap();
        new.write(b"other").unwrap();
        // The handle on what was removed reads none of what took its place.
        let mut bytes = [0u8; 6];
        assert!(old.read_at(&mut bytes, 0).is_err());
        assert!(old.write(b"x").is_err());
        assert_eq!(read(&files, "c"), b"other");
        // Removing what is not there is not an error.
        files.remove(Path::new("a")).unwrap();
        // More files can be added to a pool that is full.
        assert!(files.create(Path::new("d")).is_err());
        files.add(vec![Box::new(MemorySlot::default())]).unwrap();
        files.create(Path::new("d")).unwrap();
    }

    #[test]
    fn a_rename_keeps_the_file_and_replaces_what_had_the_name() {
        let slots: Vec<MemorySlot> = (0..3).map(|_| MemorySlot::default()).collect();
        {
            let files = pool(&slots);
            files.create(Path::new("db.wal")).unwrap().write(b"new log").unwrap();
            files.create(Path::new("db.wal.sealed")).unwrap().write(b"old sealed").unwrap();
            files.rename(Path::new("db.wal"), Path::new("db.wal.sealed")).unwrap();
            assert!(!files.exists(Path::new("db.wal")));
            assert_eq!(read(&files, "db.wal.sealed"), b"new log");
            assert_eq!(files.free(), 2);
            assert!(files.rename(Path::new("db.wal"), Path::new("x")).is_err());
            // And back, more than once: each change goes to the other half.
            files.rename(Path::new("db.wal.sealed"), Path::new("db.wal")).unwrap();
            files.rename(Path::new("db.wal"), Path::new("db.wal.sealed")).unwrap();
        }
        let files = pool(&slots);
        assert_eq!(read(&files, "db.wal.sealed"), b"new log");
        assert!(!files.exists(Path::new("db.wal")));
        assert_eq!(files.free(), 2);
    }

    #[test]
    fn a_header_write_cut_short_leaves_the_name_it_had() {
        let slots: Vec<MemorySlot> = (0..2).map(|_| MemorySlot::default()).collect();
        {
            let files = pool(&slots);
            files.create(Path::new("db.wal")).unwrap().write(b"log").unwrap();
        }
        // A rename that got as far as spoiling the half it was writing.
        let slot = &slots[0];
        let in_force = {
            let bytes = slot.bytes.lock().unwrap();
            if decode(&bytes[..HALF as usize]).is_some() { 0 } else { 1 }
        };
        let other = (1 - in_force) * HALF as usize;
        slot.bytes.lock().unwrap()[other..other + 64].copy_from_slice(&[0xAB; 64]);
        let files = pool(&slots);
        assert_eq!(read(&files, "db.wal"), b"log");

        // Two files that claim one name, as when a rename over a file
        // stops before the old is let go: the later is the file.
        let slots: Vec<MemorySlot> = (0..2).map(|_| MemorySlot::default()).collect();
        {
            let files = pool(&slots);
            files.create(Path::new("a")).unwrap().write(b"earlier").unwrap();
            files.create(Path::new("b")).unwrap().write(b"later").unwrap();
        }
        let renamed = encode(99, Path::new("a")).unwrap();
        let later = slots
            .iter()
            .find(|slot| {
                let bytes = slot.bytes.lock().unwrap();
                [0usize, 1].iter().any(|half| {
                    decode(&bytes[half * HALF as usize..]).is_some_and(|named| named.path == Path::new("b"))
                })
            })
            .unwrap();
        {
            let mut bytes = later.bytes.lock().unwrap();
            let in_force = if decode(&bytes[..HALF as usize]).is_some() { 0 } else { 1 };
            let other = (1 - in_force) * HALF as usize;
            bytes[other..other + HALF as usize].copy_from_slice(&renamed);
        }
        let files = pool(&slots);
        assert_eq!(read(&files, "a"), b"later");
        assert!(!files.exists(Path::new("b")));
        assert_eq!(files.free(), 1);
    }
}
