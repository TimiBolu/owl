//! Checkpoint parts kept in object storage.
//!
//! A checkpoint is a small root over part files, and a part is written
//! once and never changed (see `parts`). That makes a part the thing to
//! keep in an object store: put once, read by range, deleted when no root
//! lists it.
//!
//! [`ObjectFileSystem`] sits under the store as its file system. To the
//! store a part is still a file in the database's directory:
//!
//! - when a part is made durable it is also put in the object store, and
//!   the sync does not return until it is there, so a root never names a
//!   part the object store lacks;
//! - a part that is not on the local disk is read from the object store,
//!   a block at a time, through a cache on the local disk that holds no
//!   more than its budget;
//! - removing a part removes it from the object store too.
//!
//! Everything that is not a part (the log, the roots, the manifests) lives
//! on the local file system. For another machine to read the database,
//! the writer *publishes* it: [`RustStore::publish`] puts a copy of those
//! files, as they stand at one commit, in the object store under numbered
//! names, and then moves a pointer object to the new number. A reader
//! ([`ObjectFileSystem::reader`]) fetches the files the pointer names into
//! a directory of its own and opens them there, reading parts from the
//! object store. It holds nothing the writer needs and puts and deletes
//! nothing, so any number of them can come and go.
//!
//! The publication before the newest is kept, and so are the parts it
//! lists, so a reader that opened one publication goes on working through
//! the next. After that it must open again.
//!
//! One process writes a database's objects at a time. A writer claims the
//! database ([`ObjectFileSystem::writer`]), and one that has been replaced
//! can change nothing its successor relies on: see `Fence`.

use super::RustStore;
use crate::error::{OQLError, OQLResult};
use crate::simulation::filesystem::{FileHandle, FileOptions, FileSystem, MemoryMap};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{self, Read, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A place that keeps whole objects under names.
///
/// What the store needs of S3 and its like, and no more: an object is put
/// whole, read by range, listed and deleted. An implementation must make a
/// put all or nothing: a reader sees the old object or the new one.
pub trait ObjectStore: Send + Sync {
    /// Keep the `len` bytes `source` gives under `key`, in place of
    /// anything already there.
    fn put(&self, key: &str, source: &mut dyn Read, len: u64) -> io::Result<()>;

    /// Read the object's bytes from `offset` into `buffer`. Gives how many
    /// were read, which is fewer than the buffer holds only at the
    /// object's end. An object that is not there is `NotFound`.
    fn read_at(&self, key: &str, offset: u64, buffer: &mut [u8]) -> io::Result<usize>;

    /// The object's length, or `None` when it is not there.
    fn len(&self, key: &str) -> io::Result<Option<u64>>;

    /// Remove the object. One that is not there is not an error.
    fn delete(&self, key: &str) -> io::Result<()>;

    /// The keys that begin with `prefix`, in order.
    fn list(&self, prefix: &str) -> io::Result<Vec<String>>;

    /// A mark that is different for every object ever put under `key`,
    /// or `None` when nothing is there. Two reads that give the same mark
    /// saw the same object.
    fn version(&self, key: &str) -> io::Result<Option<String>>;

    /// Put as [`ObjectStore::put`] does, but only if what is under `key`
    /// is still what the caller last saw: nothing, for `None`, or the
    /// object with this mark. Gives the new object's mark, or `None` when
    /// something else was there and nothing was changed. The check and
    /// the put are one step: of two callers that saw the same thing, one
    /// succeeds.
    fn put_if(
        &self,
        key: &str,
        source: &mut dyn Read,
        len: u64,
        expected: Option<&str>,
    ) -> io::Result<Option<String>>;

    /// Remove the object only if it is still the one with this mark.
    /// Says whether it was removed.
    fn delete_if(&self, key: &str, expected: &str) -> io::Result<bool>;
}

fn not_found(key: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("no object named '{key}'"))
}

/// An object store that is a directory: one file for each object.
///
/// It is what the tests and a single machine use, and it serves any object
/// store that is mounted as a directory.
pub struct DirectoryObjectStore {
    directory: PathBuf,
    next_temporary: AtomicU64,
}

impl DirectoryObjectStore {
    pub fn new(directory: impl Into<PathBuf>) -> io::Result<Self> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            next_temporary: AtomicU64::new(0),
        })
    }

    /// Hold the directory's lock while `work` runs, so that a check and
    /// the change that depends on it are one step for every process that
    /// uses this directory.
    fn locked<T>(&self, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        let lock = self.directory.join(LOCK);
        let started = std::time::Instant::now();
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if started.elapsed() > std::time::Duration::from_secs(10) {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("{} is held; remove it if no process is using the directory", lock.display()),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(error) => return Err(error),
            }
        }
        let outcome = work();
        let _ = std::fs::remove_file(&lock);
        outcome
    }

    /// The mark of the file at `path`: its length, when it was written
    /// and, where there is one, the number the file system knows it by. A
    /// put makes a new file, so all three change.
    fn mark(path: &Path) -> io::Result<Option<String>> {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let written = metadata
            .modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        #[cfg(unix)]
        let number = std::os::unix::fs::MetadataExt::ino(&metadata);
        #[cfg(not(unix))]
        let number = 0u64;
        Ok(Some(format!("{}-{written}-{number}", metadata.len())))
    }

    fn path(&self, key: &str) -> io::Result<PathBuf> {
        // A key is a file name. Anything that could leave the directory
        // is refused.
        if key.is_empty() || key.contains(['/', '\\']) || key == "." || key == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("'{key}' is not a name an object can have here"),
            ));
        }
        Ok(self.directory.join(key))
    }
}

/// What a put in progress is called until it is whole.
const TEMPORARY: &str = ".putting.";

/// The file that is held while a check and a change are made together.
const LOCK: &str = ".lock";

impl ObjectStore for DirectoryObjectStore {
    fn put(&self, key: &str, source: &mut dyn Read, len: u64) -> io::Result<()> {
        let path = self.path(key)?;
        let temporary = self.directory.join(format!(
            "{TEMPORARY}{}.{}",
            std::process::id(),
            self.next_temporary.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            let mut file = std::fs::File::create(&temporary)?;
            let copied = io::copy(&mut source.take(len), &mut file)?;
            if copied != len {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("'{key}' was to be {len} bytes and {copied} were given"),
                ));
            }
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            // The name is not durable until its directory is.
            if let Ok(directory) = std::fs::File::open(&self.directory) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written
    }

    fn read_at(&self, key: &str, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        use std::io::Seek;
        let mut file = std::fs::File::open(self.path(key)?).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => not_found(key),
            _ => error,
        })?;
        file.seek(SeekFrom::Start(offset))?;
        let mut read = 0;
        while read < buffer.len() {
            match file.read(&mut buffer[read..]) {
                Ok(0) => break,
                Ok(count) => read += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(read)
    }

    fn len(&self, key: &str) -> io::Result<Option<u64>> {
        match std::fs::metadata(self.path(key)?) {
            Ok(metadata) => Ok(Some(metadata.len())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(key)?) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn list(&self, prefix: &str) -> io::Result<Vec<String>> {
        let mut keys = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let name = entry?.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(prefix) && !name.starts_with(TEMPORARY) && name != LOCK {
                keys.push(name.to_string());
            }
        }
        keys.sort();
        Ok(keys)
    }

    fn version(&self, key: &str) -> io::Result<Option<String>> {
        Self::mark(&self.path(key)?)
    }

    fn put_if(
        &self,
        key: &str,
        source: &mut dyn Read,
        len: u64,
        expected: Option<&str>,
    ) -> io::Result<Option<String>> {
        let path = self.path(key)?;
        self.locked(|| {
            if Self::mark(&path)?.as_deref() != expected {
                return Ok(None);
            }
            self.put(key, source, len)?;
            Self::mark(&path)
        })
    }

    fn delete_if(&self, key: &str, expected: &str) -> io::Result<bool> {
        let path = self.path(key)?;
        self.locked(|| {
            if Self::mark(&path)?.as_deref() != Some(expected) {
                return Ok(false);
            }
            std::fs::remove_file(&path)?;
            Ok(true)
        })
    }
}

/// An object store held in memory, which counts what is asked of it and
/// can be told to fail. For tests.
#[derive(Default)]
pub struct MemoryObjectStore {
    objects: Mutex<BTreeMap<String, (Arc<Vec<u8>>, u64)>>,
    /// The mark the next object put will have.
    next_mark: AtomicU64,
    puts: AtomicU64,
    reads: AtomicU64,
    bytes_read: AtomicU64,
    failing: AtomicBool,
}

impl MemoryObjectStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many objects have been put.
    pub fn puts(&self) -> u64 {
        self.puts.load(Ordering::Relaxed)
    }

    /// How many ranges have been read.
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    /// How many bytes those reads gave.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Make every call fail, or stop doing so.
    pub fn fail(&self, failing: bool) {
        self.failing.store(failing, Ordering::Relaxed);
    }

    fn check(&self) -> io::Result<()> {
        if self.failing.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Other, "the object store is unreachable"));
        }
        Ok(())
    }
}

impl ObjectStore for MemoryObjectStore {
    fn put(&self, key: &str, source: &mut dyn Read, len: u64) -> io::Result<()> {
        self.check()?;
        let mut bytes = Vec::with_capacity(len as usize);
        source.take(len).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a put was given too few bytes"));
        }
        let mark = self.next_mark.fetch_add(1, Ordering::Relaxed) + 1;
        self.objects.lock().unwrap().insert(key.to_string(), (Arc::new(bytes), mark));
        self.puts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn read_at(&self, key: &str, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        let (object, _) = self.objects.lock().unwrap().get(key).cloned().ok_or_else(|| not_found(key))?;
        let from = (offset as usize).min(object.len());
        let count = buffer.len().min(object.len() - from);
        buffer[..count].copy_from_slice(&object[from..from + count]);
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }

    fn len(&self, key: &str) -> io::Result<Option<u64>> {
        self.check()?;
        Ok(self.objects.lock().unwrap().get(key).map(|(object, _)| object.len() as u64))
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        self.check()?;
        self.objects.lock().unwrap().remove(key);
        Ok(())
    }

    fn list(&self, prefix: &str) -> io::Result<Vec<String>> {
        self.check()?;
        Ok(self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn version(&self, key: &str) -> io::Result<Option<String>> {
        self.check()?;
        Ok(self.objects.lock().unwrap().get(key).map(|(_, mark)| mark.to_string()))
    }

    fn put_if(
        &self,
        key: &str,
        source: &mut dyn Read,
        len: u64,
        expected: Option<&str>,
    ) -> io::Result<Option<String>> {
        self.check()?;
        let mut bytes = Vec::with_capacity(len as usize);
        source.take(len).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a put was given too few bytes"));
        }
        let mut objects = self.objects.lock().unwrap();
        if objects.get(key).map(|(_, mark)| mark.to_string()).as_deref() != expected {
            return Ok(None);
        }
        let mark = self.next_mark.fetch_add(1, Ordering::Relaxed) + 1;
        objects.insert(key.to_string(), (Arc::new(bytes), mark));
        self.puts.fetch_add(1, Ordering::Relaxed);
        Ok(Some(mark.to_string()))
    }

    fn delete_if(&self, key: &str, expected: &str) -> io::Result<bool> {
        self.check()?;
        let mut objects = self.objects.lock().unwrap();
        if objects.get(key).map(|(_, mark)| mark.to_string()).as_deref() != Some(expected) {
            return Ok(false);
        }
        objects.remove(key);
        Ok(true)
    }
}

/// How much of a part is fetched and kept at a time.
const BLOCK: u64 = 1 << 20;

/// What a cached block's file is called: the part's name, this, and the
/// block's number.
const CACHED: &str = ".cached.";

/// Blocks of parts fetched from the object store, kept as files on the
/// local disk up to a budget, the least recently read dropped first.
struct BlockCache {
    local: Arc<dyn FileSystem>,
    directory: PathBuf,
    budget: u64,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    blocks: HashMap<(String, u64), (Arc<dyn FileHandle>, u64)>,
    /// Least recently read first. A block read again is moved to the end.
    order: VecDeque<(String, u64)>,
    bytes: u64,
}

impl BlockCache {
    fn path(&self, key: &str, block: u64) -> PathBuf {
        self.directory.join(format!("{key}{CACHED}{block}"))
    }

    fn get(&self, key: &str, block: u64) -> Option<Arc<dyn FileHandle>> {
        let mut held = self.held.lock().unwrap();
        let name = (key.to_string(), block);
        let handle = held.blocks.get(&name)?.0.clone();
        if let Some(at) = held.order.iter().position(|other| *other == name) {
            held.order.remove(at);
        }
        held.order.push_back(name);
        Some(handle)
    }

    /// Keep a block. A cache that cannot write to its disk keeps nothing
    /// and says nothing: the bytes were read, and will be read again.
    fn keep(&self, key: &str, block: u64, bytes: &[u8]) {
        if bytes.len() as u64 > self.budget {
            return;
        }
        let path = self.path(key, block);
        let options = FileOptions {
            read: true,
            write: true,
            create: true,
            truncate: true,
            append: false,
        };
        let Ok(mut file) = self.local.open_with_options(&path, &options) else {
            return;
        };
        let mut done = 0;
        while done < bytes.len() {
            match file.write(&bytes[done..]) {
                Ok(count) if count > 0 => done += count,
                _ => {
                    let _ = self.local.remove(&path);
                    return;
                }
            }
        }
        let _ = file.flush();

        let mut dropped = Vec::new();
        {
            let mut held = self.held.lock().unwrap();
            let name = (key.to_string(), block);
            if held.blocks.insert(name.clone(), (Arc::from(file), bytes.len() as u64)).is_none() {
                held.order.push_back(name);
                held.bytes += bytes.len() as u64;
            }
            while held.bytes > self.budget {
                let Some(oldest) = held.order.pop_front() else { break };
                if let Some((_, len)) = held.blocks.remove(&oldest) {
                    held.bytes -= len;
                    dropped.push(oldest);
                }
            }
        }
        for (key, block) in dropped {
            let _ = self.local.remove(&self.path(&key, block));
        }
    }

    /// Drop every block of one part.
    fn forget(&self, key: &str) {
        let dropped: Vec<_> = {
            let mut held = self.held.lock().unwrap();
            let names: Vec<_> = held.blocks.keys().filter(|(other, _)| other == key).cloned().collect();
            for name in &names {
                if let Some((_, len)) = held.blocks.remove(name) {
                    held.bytes -= len;
                }
            }
            held.order.retain(|(other, _)| other != key);
            names
        };
        for (key, block) in dropped {
            let _ = self.local.remove(&self.path(&key, block));
        }
    }

    fn bytes(&self) -> u64 {
        self.held.lock().unwrap().bytes
    }
}

/// The name of the part a path is, when it is one in `directory`.
fn part_key(directory: &Path, path: &Path) -> Option<String> {
    if path.parent() != Some(directory) {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let (_, number) = name.rsplit_once(".part.")?;
    (!number.is_empty() && number.bytes().all(|digit| digit.is_ascii_digit())).then(|| name.to_string())
}

fn read_only() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "a part read from the object store is not written to")
}

/// A part that is not on the local disk, read from the object store
/// through the block cache.
struct RemotePart {
    key: String,
    len: u64,
    objects: Arc<dyn ObjectStore>,
    cache: Arc<BlockCache>,
    position: u64,
}

impl RemotePart {
    /// Read from one block, fetching it if it is not kept.
    fn read_block(&self, block: u64, within: u64, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(file) = self.cache.get(&self.key, block) {
            let mut done = 0;
            while done < buffer.len() {
                match file.read_at(&mut buffer[done..], within + done as u64) {
                    Ok(0) => break,
                    Ok(count) => done += count,
                    // A cached block that will not read is fetched again.
                    Err(_) => {
                        done = 0;
                        break;
                    }
                }
            }
            if done == buffer.len() {
                return Ok(done);
            }
        }
        let start = block * BLOCK;
        let mut bytes = vec![0u8; BLOCK.min(self.len - start) as usize];
        let mut fetched = 0;
        while fetched < bytes.len() {
            let count = self.objects.read_at(&self.key, start + fetched as u64, &mut bytes[fetched..])?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("'{}' is shorter in the object store than it was", self.key),
                ));
            }
            fetched += count;
        }
        let count = buffer.len().min(bytes.len() - within as usize);
        buffer[..count].copy_from_slice(&bytes[within as usize..within as usize + count]);
        self.cache.keep(&self.key, block, &bytes);
        Ok(count)
    }
}

impl FileHandle for RemotePart {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.read_at(buf, self.position)?;
        self.position += count as u64;
        Ok(count)
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() && offset + (done as u64) < self.len {
            let at = offset + done as u64;
            let (block, within) = (at / BLOCK, at % BLOCK);
            let room = ((BLOCK - within).min(self.len - at) as usize).min(buf.len() - done);
            let count = self.read_block(block, within, &mut buf[done..done + room])?;
            if count == 0 {
                break;
            }
            done += count;
        }
        Ok(done)
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
            SeekFrom::End(delta) => self.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        };
        self.position =
            target.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start of a part"))?;
        Ok(self.position)
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.len)
    }

    fn set_len(&mut self, _len: u64) -> io::Result<()> {
        Err(read_only())
    }
}

/// Open a file to read it by position, leaving alone any cursor another
/// handle on it has. The log is read this way while it is being appended
/// to. (The simulated file system keeps one cursor for a file, and its
/// plain `open` sets that cursor back to the start.)
fn open_to_read(filesystem: &dyn FileSystem, path: &Path) -> io::Result<Box<dyn FileHandle>> {
    filesystem.open_with_options(path, &FileOptions::default())
}

/// A file's bytes from its start, for a put.
struct FromStart<'a> {
    file: &'a dyn FileHandle,
    at: u64,
}

impl Read for FromStart<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read_at(buffer, self.at)?;
        self.at += count as u64;
        Ok(count)
    }
}

/// A writer's claim on a database.
///
/// One process may write a database's objects at a time. A writer claims
/// the database by writing its number, one more than the last writer's,
/// into the pointer object, on the condition that the pointer is still as
/// it read it; of two that try together one succeeds. From then on every
/// change it makes to the object store is on the condition that the
/// object is as this writer last left it, and the parts it puts carry its
/// number in their names. So a writer that has been replaced and does not
/// know it cannot change the pointer, cannot overwrite or delete what its
/// successor put, and is told so the first time it tries.
struct Fence {
    database: String,
    /// This writer's number.
    writer: u64,
    /// The pointer's mark as this writer last wrote it.
    pointer: Mutex<String>,
    /// Set once the object store has shown that another writer has taken
    /// the database. Nothing more is written.
    lost: AtomicBool,
}

impl Fence {
    fn taken(&self) -> io::Error {
        self.lost.store(true, Ordering::Release);
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("another writer has taken over '{}'; this one has stopped writing to the object store", self.database),
        )
    }

    fn held(&self) -> io::Result<()> {
        match self.lost.load(Ordering::Acquire) {
            true => Err(self.taken()),
            false => Ok(()),
        }
    }
}

/// The object each part is kept as, by the part's file name, with the
/// object's mark where the holder is a writer.
type Parts = Arc<Mutex<HashMap<String, (String, Option<String>)>>>;

/// An object that was a part until the store removed the part, kept for
/// the readers of a publication that may list it. It is the object that
/// waits, not the part's name: a part of the same name may be made again.
struct Retired {
    object: String,
    /// Its mark, where the holder is a writer with a claim.
    mark: Option<String>,
    /// The number of the newest publication when it was removed.
    when: u64,
}

type RetiredObjects = Arc<Mutex<Vec<Retired>>>;

/// The object a part put by writer `writer` is kept as.
fn part_object(name: &str, writer: u64) -> String {
    format!("{name}~{writer}")
}

/// A part being written on the local disk, put in the object store when
/// it is made durable.
struct PartUpload {
    file: Box<dyn FileHandle>,
    path: PathBuf,
    name: String,
    local: Arc<dyn FileSystem>,
    objects: Arc<dyn ObjectStore>,
    fence: Option<Arc<Fence>>,
    parts: Parts,
    retired: RetiredObjects,
    published: Arc<AtomicU64>,
}

impl FileHandle for PartUpload {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.file.read_at(buf, offset)
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.file.write_at(buf, offset)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    /// Durable means on the local disk and in the object store. A part is
    /// synced once, when it is finished, and a root is only written after.
    fn sync(&mut self) -> io::Result<()> {
        self.file.sync()?;
        // The handle that wrote the part may not be one that reads.
        let written = open_to_read(self.local.as_ref(), &self.path)?;
        let len = written.size()?;
        let mut source = FromStart {
            file: written.as_ref(),
            at: 0,
        };
        let Some(fence) = &self.fence else {
            self.objects.put(&self.name, &mut source, len)?;
            // The object is the part again, and no longer waits to go.
            self.retired.lock().unwrap().retain(|retired| retired.object != self.name);
            return Ok(());
        };
        fence.held()?;
        let key = part_object(&self.name, fence.writer);
        let before = self.parts.lock().unwrap().get(&self.name).cloned();
        // The object is this writer's own name for the part, so nothing
        // is there unless this writer put it: as the part, or as a part
        // since removed and waiting to go. Either way it is as this
        // writer left it.
        let waiting = {
            let retired = self.retired.lock().unwrap();
            retired.iter().find(|retired| retired.object == key).and_then(|retired| retired.mark.clone())
        };
        let expected = match &before {
            Some((object, mark)) if *object == key => mark.clone(),
            _ => waiting,
        };
        let Some(mark) = self.objects.put_if(&key, &mut source, len, expected.as_deref())? else {
            return Err(fence.taken());
        };
        self.retired.lock().unwrap().retain(|retired| retired.object != key);
        self.parts.lock().unwrap().insert(self.name.clone(), (key.clone(), Some(mark)));
        // The same part under an earlier writer's name is no longer the
        // part. A publication may list it, so it waits like any other.
        if let Some((object, mark)) = before {
            if object != key {
                let when = self.published.load(Ordering::Acquire);
                match (when, &mark) {
                    (0, Some(mark)) => {
                        let _ = self.objects.delete_if(&object, mark);
                    }
                    (0, None) => {}
                    _ => self.retired.lock().unwrap().push(Retired { object, mark, when }),
                }
            }
        }
        Ok(())
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }

    fn position(&self) -> u64 {
        self.file.position()
    }

    fn size(&self) -> io::Result<u64> {
        self.file.size()
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }
}

/// A file system whose checkpoint parts are also kept in an object store.
///
/// It is for one database: the one whose files are in `directory`. Give
/// it to the store as its file system.
///
/// There are three ways to have one:
///
/// - [`ObjectFileSystem::writer`] claims the database, so that only one
///   process writes its objects. This is the one to write with.
/// - [`ObjectFileSystem::reader`] reads the last publication and changes
///   nothing.
/// - [`ObjectFileSystem::new`] writes without a claim. Nothing stops a
///   second writer, so it is for a database only one process can reach.
pub struct ObjectFileSystem {
    local: Arc<dyn FileSystem>,
    objects: Arc<dyn ObjectStore>,
    directory: PathBuf,
    cache: Arc<BlockCache>,
    /// A reader's: nothing is put in the object store or deleted from it.
    reads_only: bool,
    /// A writer's claim, when it has made one.
    fence: Option<Arc<Fence>>,
    /// The object each part is kept as. Without a claim or a publication
    /// that says otherwise, a part is kept under its own name.
    parts: Parts,
    /// The number of the newest publication this writer knows of.
    published: Arc<AtomicU64>,
    /// Objects of parts removed since a publication that may list them.
    /// They are deleted once no kept publication can.
    retired: RetiredObjects,
}

/// Remove every local file of the database called `database`.
fn remove_local(local: &dyn FileSystem, directory: &Path, database: &str) -> io::Result<()> {
    for path in local.list_dir(directory)? {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if belongs_to(database, name) {
            local.remove(&path)?;
        }
    }
    Ok(())
}

/// The file in which a directory remembers the claim it last made: the
/// writer's number, and the number of the first writer in its line.
fn record_path(directory: &Path, database: &str) -> PathBuf {
    directory.join(format!("{database}.writer"))
}

fn read_record(local: &dyn FileSystem, directory: &Path, database: &str) -> io::Result<Option<(u64, u64)>> {
    let path = record_path(directory, database);
    if !local.exists(&path) {
        return Ok(None);
    }
    let file = open_to_read(local, &path)?;
    let mut text = vec![0u8; file.size()? as usize];
    let read = file.read_at(&mut text, 0)?;
    let text = String::from_utf8_lossy(&text[..read]).into_owned();
    let mut numbers = text.split_whitespace().map(str::parse::<u64>);
    match (numbers.next(), numbers.next()) {
        (Some(Ok(writer)), Some(Ok(first))) => Ok(Some((writer, first))),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} cannot be read", path.display()))),
    }
}

/// What the record is called while it is being written. It takes its
/// name only once whole, so that a crash leaves the old record or the new.
fn record_being_written(directory: &Path, database: &str) -> PathBuf {
    directory.join(format!("{database}.writer.new"))
}

fn write_record(local: &dyn FileSystem, directory: &Path, database: &str, writer: u64, first: u64) -> io::Result<()> {
    let staged = record_being_written(directory, database);
    let mut file = local.create(&staged)?;
    let text = format!("{writer} {first}\n");
    let mut done = 0;
    while done < text.len() {
        done += match file.write(&text.as_bytes()[done..])? {
            0 => return Err(io::Error::new(io::ErrorKind::WriteZero, "the writer's record could not be written")),
            written => written,
        };
    }
    file.flush()?;
    file.sync()?;
    drop(file);
    local.rename(&staged, &record_path(directory, database))?;
    local.sync_all()
}

impl ObjectFileSystem {
    fn build(
        local: Arc<dyn FileSystem>,
        objects: Arc<dyn ObjectStore>,
        directory: PathBuf,
        cache_bytes: u64,
    ) -> io::Result<Self> {
        local.create_dir_all(&directory)?;
        // Blocks kept by an earlier run are not known to this one. They
        // are dropped, so that the cache holds what it counts.
        for path in local.list_dir(&directory)? {
            let cached = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.contains(CACHED));
            if cached {
                let _ = local.remove(&path);
            }
        }
        Ok(Self {
            cache: Arc::new(BlockCache {
                local: local.clone(),
                directory: directory.clone(),
                budget: cache_bytes,
                held: Mutex::new(Held::default()),
            }),
            local,
            objects,
            directory,
            reads_only: false,
            fence: None,
            parts: Arc::new(Mutex::new(HashMap::new())),
            published: Arc::new(AtomicU64::new(0)),
            retired: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Keep the parts of the database in `directory` in `objects` as well
    /// as on `local`, and read a part that `local` lacks from `objects`
    /// through a cache in `directory` of at most `cache_bytes`.
    ///
    /// This makes no claim. Use [`ObjectFileSystem::writer`] where a
    /// second process could reach the same object store.
    pub fn new(
        local: Arc<dyn FileSystem>,
        objects: Arc<dyn ObjectStore>,
        directory: impl Into<PathBuf>,
        cache_bytes: u64,
    ) -> io::Result<Arc<Self>> {
        let filesystem = Self::build(local, objects, directory.into(), cache_bytes)?;
        let newest = newest_publication(filesystem.objects.as_ref())?;
        filesystem.published.store(newest, Ordering::Release);
        Ok(Arc::new(filesystem))
    }

    /// Claim the database called `database` and give the file system to
    /// write it with.
    ///
    /// The claim is refused if another writer has claimed the database
    /// since this directory last did: what is here is then behind, and
    /// writing from it would undo the other's work. To carry on from what
    /// that writer last published, use [`ObjectFileSystem::take_over`].
    pub fn writer(
        local: Arc<dyn FileSystem>,
        objects: Arc<dyn ObjectStore>,
        directory: impl Into<PathBuf>,
        database: &str,
        cache_bytes: u64,
    ) -> io::Result<Arc<Self>> {
        Self::claim(local, objects, directory.into(), database, cache_bytes, false)
    }

    /// Become the database's writer on this machine, starting from what
    /// was last published: fetch that publication into `directory`, in
    /// place of any files of the database there, and claim the database.
    ///
    /// Whatever the writer before had committed and not published is not
    /// carried over. If that writer is still running it is stopped from
    /// writing to the object store the next time it tries.
    pub fn take_over(
        local: Arc<dyn FileSystem>,
        objects: Arc<dyn ObjectStore>,
        directory: impl Into<PathBuf>,
        database: &str,
        cache_bytes: u64,
    ) -> io::Result<Arc<Self>> {
        Self::claim(local, objects, directory.into(), database, cache_bytes, true)
    }

    fn claim(
        local: Arc<dyn FileSystem>,
        objects: Arc<dyn ObjectStore>,
        directory: PathBuf,
        database: &str,
        cache_bytes: u64,
        take_over: bool,
    ) -> io::Result<Arc<Self>> {
        local.create_dir_all(&directory)?;
        let pointer = Published::pointer(database);
        let mut attempts = 0;
        let (writer, first, claimed, mark) = loop {
            let current = Published::with_mark(objects.as_ref(), database)?;
            let last = current.as_ref().map_or(0, |(published, _)| published.writer);
            let first = if take_over {
                let Some((published, _)) = current.as_ref().filter(|(published, _)| published.number > 0) else {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("'{database}' has not been published, so there is nothing to take over from"),
                    ));
                };
                remove_local(local.as_ref(), &directory, database)?;
                match published.fetch(local.as_ref(), objects.as_ref(), &directory) {
                    Ok(()) => {}
                    // Replaced while it was being fetched. Look again.
                    Err(error) if error.kind() == io::ErrorKind::NotFound && attempts < 5 => {
                        attempts += 1;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
                // Parts put by writers before this one are this line's
                // only where the publication lists them.
                last + 1
            } else {
                let record = read_record(local.as_ref(), &directory, database)?;
                // The record is written before the claim, so a directory
                // that died between the two holds the next number.
                let ours = record.is_some_and(|(writer, _)| writer == last || writer == last + 1);
                if last != 0 && !ours {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "'{database}' has been claimed by another writer (number {last}) since this \
                             directory last wrote it. What is here is behind. Take over to carry on from \
                             the last publication."
                        ),
                    ));
                }
                record.map_or(1, |(_, first)| first)
            };
            let writer = last + 1;
            write_record(local.as_ref(), &directory, database, writer, first)?;
            let mut claimed = current.as_ref().map(|(published, _)| published.clone()).unwrap_or_default();
            claimed.writer = writer;
            let encoded = serde_json::to_vec(&claimed).map_err(io::Error::other)?;
            let expected = current.as_ref().map(|(_, mark)| mark.as_str());
            match objects.put_if(&pointer, &mut &encoded[..], encoded.len() as u64, expected)? {
                Some(mark) => break (writer, first, claimed, mark),
                // Another claim or a publication came between. Look again.
                None if attempts < 5 => attempts += 1,
                None => {
                    return Err(io::Error::other(format!("'{database}' could not be claimed: it keeps changing")));
                }
            }
        };
        local.sync_all()?;

        // The parts that are this line's: those the publication lists,
        // and those put by this line's writers, the newest of each name.
        let mut parts: HashMap<String, (String, Option<String>)> = HashMap::new();
        for (name, object) in &claimed.parts {
            if let Some(mark) = objects.version(object)? {
                parts.insert(name.clone(), (object.clone(), Some(mark)));
            }
        }
        let mut newest: HashMap<String, (u64, String)> = HashMap::new();
        for object in objects.list(database)? {
            let (name, by) = match object.rsplit_once('~') {
                Some((name, by)) => (name, by.parse::<u64>().ok()),
                // A part put without a claim is kept under its own name.
                None => (object.as_str(), Some(0)),
            };
            let Some(by) = by else { continue };
            if part_key(&directory, &directory.join(name)).is_none() || (by != 0 && by < first) {
                continue;
            }
            if newest.get(name).is_none_or(|(other, _)| by > *other) {
                newest.insert(name.to_string(), (by, object.clone()));
            }
        }
        for (name, (by, object)) in newest {
            if by == 0 && parts.contains_key(&name) {
                continue;
            }
            if let Some(mark) = objects.version(&object)? {
                parts.insert(name, (object, Some(mark)));
            }
        }

        let mut filesystem = Self::build(local, objects, directory, cache_bytes)?;
        filesystem.published.store(claimed.number, Ordering::Release);
        filesystem.parts = Arc::new(Mutex::new(parts));
        filesystem.fence = Some(Arc::new(Fence {
            database: database.to_string(),
            writer,
            pointer: Mutex::new(mark),
            lost: AtomicBool::new(false),
        }));
        Ok(Arc::new(filesystem))
    }

    /// Make ready to read the database called `database` as it was last
    /// published to `objects`: fetch its files into `directory` on
    /// `local`, in place of any files of that database already there, and
    /// give the file system to open it with. The database is then opened
    /// at `directory/database`.
    ///
    /// What is opened is the reader's own. It may write to its directory,
    /// as any open database does, and nothing it does reaches the object
    /// store. To see a later publication, call this again.
    pub fn reader(
        local: Arc<dyn FileSystem>,
        objects: Arc<dyn ObjectStore>,
        directory: impl Into<PathBuf>,
        database: &str,
        cache_bytes: u64,
    ) -> io::Result<(Arc<Self>, Published)> {
        let directory = directory.into();
        local.create_dir_all(&directory)?;
        // A publication can be replaced while it is being fetched. Its
        // files are then gone, and the newer one is fetched instead.
        let mut attempts = 0;
        let published = loop {
            let published = Published::newest(objects.as_ref(), database)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("'{database}' has not been published"))
            })?;
            remove_local(local.as_ref(), &directory, database)?;
            match published.fetch(local.as_ref(), objects.as_ref(), &directory) {
                Ok(()) => break published,
                Err(error) if error.kind() == io::ErrorKind::NotFound && attempts < 3 => attempts += 1,
                Err(error) => return Err(error),
            }
        };
        local.sync_all()?;
        let mut filesystem = Self::build(local, objects, directory, cache_bytes)?;
        filesystem.reads_only = true;
        filesystem.parts = Arc::new(Mutex::new(
            published.parts.iter().map(|(name, object)| (name.clone(), (object.clone(), None))).collect(),
        ));
        Ok((Arc::new(filesystem), published))
    }

    /// Remove the local copy of every part that is in the object store,
    /// leaving it to be read from there. Gives how many were removed.
    pub fn drop_local_parts(&self) -> io::Result<usize> {
        let mut dropped = 0;
        for path in self.local.list_dir(&self.directory)? {
            let Some(name) = part_key(&self.directory, &path) else { continue };
            if self.objects.len(&self.object_of(&name))?.is_some() {
                self.local.remove(&path)?;
                dropped += 1;
            }
        }
        Ok(dropped)
    }

    /// How many bytes of parts the local cache holds.
    pub fn cached_bytes(&self) -> u64 {
        self.cache.bytes()
    }

    /// The object the part of this name is kept as.
    fn object_of(&self, name: &str) -> String {
        match self.parts.lock().unwrap().get(name) {
            Some((object, _)) => object.clone(),
            None => name.to_string(),
        }
    }

    /// The part a path names, when the local disk does not have it and
    /// the object store does: its object and its length.
    fn remote(&self, path: &Path) -> io::Result<Option<(String, u64)>> {
        let Some(name) = part_key(&self.directory, path) else {
            return Ok(None);
        };
        if self.local.exists(path) {
            return Ok(None);
        }
        // Without a claim a part is its own object, and one that waits to
        // be deleted is, to the store, gone.
        if self.fence.is_none() && self.retired.lock().unwrap().iter().any(|retired| retired.object == name) {
            return Ok(None);
        }
        // A writer with a claim sees only its own line's parts.
        if self.fence.is_some() && !self.parts.lock().unwrap().contains_key(&name) {
            return Ok(None);
        }
        let object = self.object_of(&name);
        Ok(self.objects.len(&object)?.map(|len| (object, len)))
    }

    fn open_remote(&self, key: String, len: u64) -> Box<dyn FileHandle> {
        Box::new(RemotePart {
            key,
            len,
            objects: self.objects.clone(),
            cache: self.cache.clone(),
            position: 0,
        })
    }

    /// Delete the object a part is kept as. With a claim, only if it is
    /// as this writer left it.
    fn delete_object(&self, name: &str) -> io::Result<()> {
        let Some(fence) = &self.fence else {
            return self.objects.delete(name);
        };
        fence.held()?;
        let Some((object, mark)) = self.parts.lock().unwrap().get(name).cloned() else {
            return Ok(());
        };
        if let Some(mark) = mark {
            if !self.objects.delete_if(&object, &mark)? && self.objects.version(&object)?.is_some() {
                return Err(fence.taken());
            }
        }
        self.parts.lock().unwrap().remove(name);
        Ok(())
    }
}

impl FileSystem for ObjectFileSystem {
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        let file = self.local.create(path)?;
        let Some(name) = part_key(&self.directory, path) else {
            return Ok(file);
        };
        if self.reads_only {
            // A reader's own part stays in its directory.
            return Ok(file);
        }
        // A part of this name made before, and never listed by a root,
        // may have left blocks behind.
        self.cache.forget(&self.object_of(&name));
        Ok(Box::new(PartUpload {
            file,
            path: path.to_path_buf(),
            name,
            local: self.local.clone(),
            objects: self.objects.clone(),
            fence: self.fence.clone(),
            parts: self.parts.clone(),
            retired: self.retired.clone(),
            published: self.published.clone(),
        }))
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        match self.remote(path)? {
            Some((key, len)) => Ok(self.open_remote(key, len)),
            None => self.local.open(path),
        }
    }

    fn open_with_options(&self, path: &Path, options: &FileOptions) -> io::Result<Box<dyn FileHandle>> {
        let reads_only = !(options.write || options.create || options.truncate || options.append);
        if reads_only {
            if let Some((key, len)) = self.remote(path)? {
                return Ok(self.open_remote(key, len));
            }
        }
        self.local.open_with_options(path, options)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        let Some(name) = part_key(&self.directory, path) else {
            return self.local.remove(path);
        };
        if self.reads_only {
            // A reader removes what is its own and leaves the rest.
            return match self.local.exists(path) {
                true => self.local.remove(path),
                false => Ok(()),
            };
        }
        let published = self.published.load(Ordering::Acquire);
        if published == 0 {
            // The object goes first. If that fails the part is still
            // whole in both places, and the removal can be tried again.
            self.cache.forget(&self.object_of(&name));
            self.delete_object(&name)?;
        } else {
            // A publication may still list it. Its object waits, and
            // the name is free for a part to be made under again.
            self.cache.forget(&self.object_of(&name));
            let waiting = match &self.fence {
                Some(fence) => {
                    fence.held()?;
                    self.parts.lock().unwrap().remove(&name)
                }
                None => Some((name.clone(), None)),
            };
            if let Some((object, mark)) = waiting {
                let mut retired = self.retired.lock().unwrap();
                if !retired.iter().any(|retired| retired.object == object) {
                    retired.push(Retired {
                        object,
                        mark,
                        when: published,
                    });
                }
            }
        }
        if self.local.exists(path) {
            self.local.remove(path)?;
        }
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.local.create_dir_all(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.local.rename(from, to)
    }

    fn exists(&self, path: &Path) -> bool {
        self.local.exists(path) || matches!(self.remote(path), Ok(Some(_)))
    }

    fn file_size(&self, path: &Path) -> io::Result<u64> {
        match self.remote(path)? {
            Some((_, len)) => Ok(len),
            None => self.local.file_size(path),
        }
    }

    fn sync_all(&self) -> io::Result<()> {
        self.local.sync_all()
    }

    fn mmap(&self, path: &Path, size: usize) -> io::Result<Box<dyn MemoryMap>> {
        self.local.mmap(path, size)
    }

    /// The directory as the store should see it: the local files, without
    /// the cache's own, and the parts that are only in the object store.
    fn list_dir(&self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        let mut listed = self.local.list_dir(directory)?;
        if directory != self.directory {
            return Ok(listed);
        }
        listed.retain(|path| {
            !path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.contains(CACHED))
        });
        if self.reads_only {
            return Ok(listed);
        }
        // With a claim, the parts are this line's. Without one, they are
        // whatever parts the object store holds.
        let names: Vec<String> = match &self.fence {
            Some(_) => self.parts.lock().unwrap().keys().cloned().collect(),
            None => self.objects.list("")?,
        };
        let retired = self.retired.lock().unwrap();
        for name in names {
            // A part waiting to be deleted is, to the store, gone.
            if self.fence.is_none() && retired.iter().any(|retired| retired.object == name) {
                continue;
            }
            let path = directory.join(&name);
            if part_key(directory, &path).is_some() && !listed.contains(&path) {
                listed.push(path);
            }
        }
        listed.sort();
        Ok(listed)
    }
}

/// The number of the newest publication in an object store, or zero when
/// nothing has been published there. A writer that starts again learns
/// from it that readers may be about, and that removed parts must wait.
fn newest_publication(objects: &dyn ObjectStore) -> io::Result<u64> {
    let mut newest = 0;
    for key in objects.list("")? {
        if let Some(database) = key.strip_suffix(".published") {
            if let Some(published) = Published::newest(objects, database)? {
                newest = newest.max(published.number);
            }
        }
    }
    Ok(newest)
}

/// Whether a file of this name is one of the database called `database`:
/// the file itself, or anything named `<database>.<something>`.
fn belongs_to(database: &str, name: &str) -> bool {
    name == database || name.strip_prefix(database).is_some_and(|rest| rest.starts_with('.'))
}

/// One file of a publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedFile {
    pub name: String,
    pub bytes: u64,
    pub crc32: u32,
}

/// A database as it stood at one commit, in the object store for readers.
///
/// The object that holds the newest of these is the pointer. It is also
/// where a writer's claim is kept, so that a claim and a publication
/// cannot pass each other.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    /// Which publication this is. Each is one more than the last, and
    /// zero is a database that has been claimed and never published.
    pub number: u64,
    /// The last commit it contains.
    pub committed_lsn: u64,
    /// Its files, other than parts.
    pub files: Vec<PublishedFile>,
    /// The number of the writer that holds the database, or zero when no
    /// writer has claimed it.
    #[serde(default)]
    pub writer: u64,
    /// The number of the writer that made this publication, which its
    /// files' objects are named with.
    #[serde(default)]
    pub by: u64,
    /// The object each part it lists is kept as, by the part's file name.
    /// A part not named here is kept under its own name.
    #[serde(default)]
    pub parts: BTreeMap<String, String>,
}

impl Published {
    /// The object that says which publication is the newest.
    fn pointer(database: &str) -> String {
        format!("{database}.published")
    }

    /// The object a file of publication `number`, made by writer `by`,
    /// is kept as.
    fn key(name: &str, number: u64, by: u64) -> String {
        match by {
            0 => format!("{name}@{number}"),
            by => format!("{name}@{number}.{by}"),
        }
    }

    /// The publication a key is a file of, when it is one of `database`'s.
    fn number_of(database: &str, key: &str) -> Option<u64> {
        let (name, number) = key.rsplit_once('@')?;
        let number = number.split('.').next()?;
        belongs_to(database, name).then(|| number.parse().ok())?
    }

    /// The pointer as it is now, with its mark.
    fn with_mark(objects: &dyn ObjectStore, database: &str) -> io::Result<Option<(Self, String)>> {
        let key = Self::pointer(database);
        loop {
            let Some(mark) = objects.version(&key)? else {
                return Ok(None);
            };
            let Some(len) = objects.len(&key)? else {
                continue;
            };
            let mut encoded = vec![0u8; len as usize];
            let read = objects.read_at(&key, 0, &mut encoded)?;
            // The mark is of what was read only if it has not moved.
            if objects.version(&key)?.as_deref() != Some(mark.as_str()) {
                continue;
            }
            let published = serde_json::from_slice(&encoded[..read]).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("'{key}' cannot be read: {error}"))
            })?;
            return Ok(Some((published, mark)));
        }
    }

    /// The newest publication of `database`, if it has been published.
    pub fn newest(objects: &dyn ObjectStore, database: &str) -> io::Result<Option<Self>> {
        Ok(Self::with_mark(objects, database)?
            .map(|(published, _)| published)
            .filter(|published| published.number > 0))
    }

    /// Bring the publication's files to `directory`, each checked against
    /// its size and checksum.
    fn fetch(&self, local: &dyn FileSystem, objects: &dyn ObjectStore, directory: &Path) -> io::Result<()> {
        let mut buffer = vec![0u8; BLOCK as usize];
        for file in &self.files {
            let key = Self::key(&file.name, self.number, self.by);
            let mut target = local.create(&directory.join(&file.name))?;
            let mut checksum = crc32fast::Hasher::new();
            let mut at = 0u64;
            while at < file.bytes {
                let wanted = (file.bytes - at).min(BLOCK) as usize;
                let count = objects.read_at(&key, at, &mut buffer[..wanted])?;
                if count == 0 {
                    break;
                }
                checksum.update(&buffer[..count]);
                let mut done = 0;
                while done < count {
                    done += match target.write(&buffer[done..count])? {
                        0 => return Err(io::Error::new(io::ErrorKind::WriteZero, "a fetched file could not be written")),
                        written => written,
                    };
                }
                at += count as u64;
            }
            if at != file.bytes || checksum.finalize() != file.crc32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("'{key}' is not the file that was published"),
                ));
            }
            target.flush()?;
            target.sync()?;
        }
        Ok(())
    }
}

/// A reader over a file that keeps the checksum of what it has given.
struct Summed<'a> {
    file: &'a dyn FileHandle,
    at: u64,
    checksum: crc32fast::Hasher,
}

impl Read for Summed<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read_at(buffer, self.at)?;
        self.checksum.update(&buffer[..count]);
        self.at += count as u64;
        Ok(count)
    }
}

impl RustStore {
    /// Publish the database, as of its last commit, for readers.
    ///
    /// `filesystem` is the one this store was opened with. Every file of
    /// the database other than its parts is put in the object store under
    /// a name of this publication, and then the pointer readers follow is
    /// moved to it. Commits wait while the files are put; reads carry on.
    ///
    /// The publication before this one is kept, with the parts it lists.
    /// Older ones, and parts only they listed, are deleted.
    ///
    /// A writer that has been replaced cannot publish: the pointer is no
    /// longer as it left it.
    pub fn publish(&self, filesystem: &ObjectFileSystem) -> OQLResult<Published> {
        let failed = |what: &str, error: io::Error| {
            OQLError::storage_error(&format!("publish: cannot {what}: {error}"), None)
        };
        if filesystem.reads_only {
            return Err(OQLError::storage_error("publish: a reader does not publish", None));
        }
        let (local, objects) = (filesystem.local.as_ref(), filesystem.objects.as_ref());
        let fence = filesystem.fence.as_deref();
        if let Some(fence) = fence {
            fence.held().map_err(|error| failed("publish", error))?;
        }
        let by = fence.map_or(0, |fence| fence.writer);
        let published = self.with_files_frozen(|database, committed_lsn| {
            let name = database
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| OQLError::storage_error("publish: the database has no name", None))?;
            let number = match fence {
                Some(_) => filesystem.published.load(Ordering::Acquire) + 1,
                None => {
                    Published::newest(objects, name)
                        .map_err(|error| failed("read the last publication", error))?
                        .map_or(0, |newest| newest.number)
                        .max(filesystem.published.load(Ordering::Acquire))
                        + 1
                }
            };
            let record = record_path(&filesystem.directory, name);
            let mut files = Vec::new();
            let listed = local
                .list_dir(&filesystem.directory)
                .map_err(|error| failed("list the database's files", error))?;
            for path in listed {
                let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !belongs_to(name, file_name)
                    || file_name.contains(CACHED)
                    || path == record
                    || path == record_being_written(&filesystem.directory, name)
                    || part_key(&filesystem.directory, &path).is_some()
                {
                    continue;
                }
                let file = open_to_read(local, &path).map_err(|error| failed(&format!("read {file_name}"), error))?;
                let bytes = file.size().map_err(|error| failed(&format!("measure {file_name}"), error))?;
                let mut source = Summed {
                    file: file.as_ref(),
                    at: 0,
                    checksum: crc32fast::Hasher::new(),
                };
                objects
                    .put(&Published::key(file_name, number, by), &mut source, bytes)
                    .map_err(|error| failed(&format!("put {file_name}"), error))?;
                files.push(PublishedFile {
                    name: file_name.to_string(),
                    bytes,
                    crc32: source.checksum.finalize(),
                });
            }
            let parts = {
                let parts = filesystem.parts.lock().unwrap();
                parts.iter().map(|(part, (object, _))| (part.clone(), object.clone())).collect()
            };
            let published = Published {
                number,
                committed_lsn,
                files,
                writer: by,
                by,
                parts,
            };
            // The pointer goes last. Until it moves, readers see the
            // publication before, whole.
            let encoded = serde_json::to_vec(&published)
                .map_err(|error| OQLError::storage_error(&format!("publish: cannot encode: {error}"), None))?;
            let key = Published::pointer(name);
            match fence {
                None => objects
                    .put(&key, &mut &encoded[..], encoded.len() as u64)
                    .map_err(|error| failed("move the pointer", error))?,
                Some(fence) => {
                    // Only if it is as this writer left it: a writer that
                    // has been replaced stops here.
                    let mut mark = fence.pointer.lock().unwrap();
                    let moved = objects
                        .put_if(&key, &mut &encoded[..], encoded.len() as u64, Some(mark.as_str()))
                        .map_err(|error| failed("move the pointer", error))?;
                    match moved {
                        Some(moved) => *mark = moved,
                        None => return Err(failed("move the pointer", fence.taken())),
                    }
                }
            }
            filesystem.published.store(number, Ordering::Release);
            Ok((name.to_string(), published))
        });
        let (name, published) = published?;

        // What no kept publication can need is deleted. A failure here
        // leaves objects behind and loses nothing; the next publication
        // tries again.
        let keep_from = published.number.saturating_sub(1);
        if let Ok(keys) = objects.list(&name) {
            for key in keys {
                if Published::number_of(&name, &key).is_some_and(|number| number < keep_from) {
                    let _ = objects.delete(&key);
                }
            }
        }
        // Removed after publication `when`, so `when` may list it and no
        // later one does.
        let mut retired = filesystem.retired.lock().unwrap();
        retired.retain(|retired| {
            if retired.when >= keep_from {
                return true;
            }
            let gone = match &retired.mark {
                // With a claim, only if it is as this writer left it.
                Some(mark) => {
                    objects.delete_if(&retired.object, mark).unwrap_or(false)
                        || matches!(objects.version(&retired.object), Ok(None))
                }
                None => objects.delete(&retired.object).is_ok(),
            };
            !gone
        });
        Ok(published)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, Fields, Value};
    use crate::simulation::filesystem::SimulatedFileSystem;
    use crate::storage::DocumentStore;
    use crate::storage_engine::RustStore;

    fn event(number: i64) -> Document {
        let mut event = Document::with_id(format!("event-{number:05}"), "events".to_string(), Fields::default());
        event.fields.insert("code".into(), Value::Integer(number));
        event.fields.insert("note".into(), Value::String(format!("note {number} {}", "x".repeat(200))));
        event
    }

    fn layered(
        local: &Arc<SimulatedFileSystem>,
        objects: &Arc<MemoryObjectStore>,
        cache_bytes: u64,
    ) -> Arc<ObjectFileSystem> {
        ObjectFileSystem::new(local.clone(), objects.clone(), "", cache_bytes).unwrap()
    }

    fn codes(store: &RustStore) -> Vec<i64> {
        let mut codes: Vec<i64> = store
            .scan_documents("events")
            .unwrap()
            .iter()
            .map(|document| match document.fields.get("code") {
                Some(Value::Integer(code)) => *code,
                other => panic!("an event without a code: {other:?}"),
            })
            .collect();
        codes.sort();
        codes
    }

    #[test]
    fn a_directory_keeps_objects_whole() {
        let directory = tempfile::TempDir::new().unwrap();
        let store = DirectoryObjectStore::new(directory.path().join("bucket")).unwrap();
        assert_eq!(store.len("a.part.1").unwrap(), None);
        store.put("a.part.1", &mut &b"hello, owl"[..], 10).unwrap();
        store.put("a.part.2", &mut &b"second"[..], 6).unwrap();
        assert_eq!(store.len("a.part.1").unwrap(), Some(10));
        let mut buffer = [0u8; 8];
        assert_eq!(store.read_at("a.part.1", 7, &mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], b"owl");
        assert_eq!(store.list("a.part.").unwrap(), ["a.part.1", "a.part.2"]);

        // A put given too little leaves what was there.
        assert!(store.put("a.part.1", &mut &b"short"[..], 9).is_err());
        assert_eq!(store.len("a.part.1").unwrap(), Some(10));
        assert_eq!(store.list("").unwrap().len(), 2);

        store.delete("a.part.1").unwrap();
        store.delete("a.part.1").unwrap();
        assert_eq!(store.read_at("a.part.1", 0, &mut buffer).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(store.put("../escape", &mut &b"x"[..], 1).is_err());
    }

    #[test]
    fn a_checkpoint_puts_its_part_in_the_object_store() {
        let local = Arc::new(SimulatedFileSystem::new(801));
        let objects = Arc::new(MemoryObjectStore::new());
        let path = Path::new("shop.owl");
        let store = RustStore::new_with_fs(path, 64 << 20, layered(&local, &objects, 8 << 20)).unwrap();
        for number in 0..300 {
            store.put_document(&event(number)).unwrap();
        }
        assert_eq!(objects.puts(), 0, "nothing is put before a checkpoint");
        store.checkpoint().unwrap();

        let keys = objects.list("").unwrap();
        assert!(!keys.is_empty(), "a checkpoint makes a part");
        for key in &keys {
            assert!(key.contains(".part."), "only parts are objects, and '{key}' is not one");
            assert_eq!(
                objects.len(key).unwrap(),
                Some(local.file_size(Path::new(key)).unwrap()),
                "'{key}' is whole in the object store"
            );
        }
    }

    #[test]
    fn a_database_whose_parts_are_only_in_the_object_store_answers_the_same() {
        let local = Arc::new(SimulatedFileSystem::new(802));
        let objects = Arc::new(MemoryObjectStore::new());
        let path = Path::new("shop.owl");
        {
            let store = RustStore::new_with_fs(path, 64 << 20, layered(&local, &objects, 8 << 20)).unwrap();
            for number in 0..300 {
                store.put_document(&event(number)).unwrap();
            }
            store.checkpoint().unwrap();
            // Written after the checkpoint, and so only in the log.
            store.put_document(&event(300)).unwrap();
        }

        let filesystem = layered(&local, &objects, 8 << 20);
        assert!(filesystem.drop_local_parts().unwrap() > 0);
        let before = objects.reads();
        let reopened = RustStore::new_with_fs(path, 64 << 20, filesystem.clone()).unwrap();
        assert_eq!(codes(&reopened), (0..=300).collect::<Vec<_>>());
        assert!(objects.reads() > before, "the rows came from the object store");
        assert!(filesystem.cached_bytes() > 0);

        // What was fetched is kept: reading again asks for nothing more.
        let fetched = objects.reads();
        assert_eq!(codes(&reopened), (0..=300).collect::<Vec<_>>());
        assert_eq!(objects.reads(), fetched);
    }

    #[test]
    fn the_local_cache_holds_no_more_than_its_budget() {
        let local = Arc::new(SimulatedFileSystem::new(803));
        let objects = Arc::new(MemoryObjectStore::new());
        // Four blocks, each of its own byte.
        let part: Vec<u8> = (0..4 * BLOCK).map(|at| (at / BLOCK) as u8 + 1).collect();
        objects.put("shop.part.7", &mut &part[..], part.len() as u64).unwrap();

        let filesystem = layered(&local, &objects, 2 * BLOCK);
        let file = filesystem.open(Path::new("shop.part.7")).unwrap();
        assert_eq!(file.size().unwrap(), 4 * BLOCK);
        let mut byte = [0u8; 1];
        for block in 0..4 {
            assert_eq!(file.read_at(&mut byte, block * BLOCK + 5).unwrap(), 1);
            assert_eq!(byte[0], block as u8 + 1);
            assert!(filesystem.cached_bytes() <= 2 * BLOCK);
        }
        assert_eq!(filesystem.cached_bytes(), 2 * BLOCK);
        let cached = local.list_dir(Path::new("")).unwrap().len();
        assert_eq!(cached, 2, "a dropped block's file is removed");

        // The two read last are kept. The first is fetched again.
        let fetched = objects.reads();
        file.read_at(&mut byte, 3 * BLOCK).unwrap();
        file.read_at(&mut byte, 2 * BLOCK).unwrap();
        assert_eq!(objects.reads(), fetched);
        file.read_at(&mut byte, 0).unwrap();
        assert_eq!(objects.reads(), fetched + 1);

        // A read across the edge of two blocks gives the bytes of both.
        let mut across = [0u8; 4];
        assert_eq!(file.read_at(&mut across, 2 * BLOCK - 2).unwrap(), 4);
        assert_eq!(across, [2, 2, 3, 3]);
    }

    #[test]
    fn a_part_that_cannot_be_put_fails_the_checkpoint_and_loses_nothing() {
        let local = Arc::new(SimulatedFileSystem::new(804));
        let objects = Arc::new(MemoryObjectStore::new());
        let path = Path::new("shop.owl");
        let store = RustStore::new_with_fs(path, 64 << 20, layered(&local, &objects, 8 << 20)).unwrap();
        for number in 0..50 {
            store.put_document(&event(number)).unwrap();
        }
        objects.fail(true);
        assert!(store.checkpoint().is_err(), "a part that is not in the object store is not durable");
        objects.fail(false);
        drop(store);

        let reopened = RustStore::new_with_fs(path, 64 << 20, layered(&local, &objects, 8 << 20)).unwrap();
        assert_eq!(codes(&reopened), (0..50).collect::<Vec<_>>());
        reopened.checkpoint().unwrap();
        assert!(objects.puts() > 0);
    }

    #[test]
    fn a_part_no_root_lists_is_removed_from_the_object_store() {
        let local = Arc::new(SimulatedFileSystem::new(805));
        let objects = Arc::new(MemoryObjectStore::new());
        objects.put("shop.owl.part.900", &mut &b"left behind"[..], 11).unwrap();
        let filesystem = layered(&local, &objects, 8 << 20);

        let listed = filesystem.list_dir(Path::new("")).unwrap();
        assert_eq!(listed, [PathBuf::from("shop.owl.part.900")]);
        assert!(filesystem.exists(Path::new("shop.owl.part.900")));
        assert_eq!(filesystem.file_size(Path::new("shop.owl.part.900")).unwrap(), 11);
        filesystem.remove(Path::new("shop.owl.part.900")).unwrap();
        assert!(objects.list("").unwrap().is_empty());
        assert!(!filesystem.exists(Path::new("shop.owl.part.900")));
    }

    /// A writer's database with rows 0..300 checkpointed and 300 in the
    /// log, and the file system it was opened with.
    fn written(seed: u64, objects: &Arc<MemoryObjectStore>) -> (RustStore, Arc<ObjectFileSystem>) {
        let local = Arc::new(SimulatedFileSystem::new(seed));
        let filesystem = layered(&local, objects, 8 << 20);
        let store = RustStore::new_with_fs(Path::new("shop.owl"), 64 << 20, filesystem.clone()).unwrap();
        for number in 0..300 {
            store.put_document(&event(number)).unwrap();
        }
        store.checkpoint().unwrap();
        store.put_document(&event(300)).unwrap();
        (store, filesystem)
    }

    /// Open the database as last published, on a machine of its own.
    fn read(seed: u64, objects: &Arc<MemoryObjectStore>) -> (RustStore, Arc<SimulatedFileSystem>) {
        let machine = Arc::new(SimulatedFileSystem::new(seed));
        let (filesystem, _) =
            ObjectFileSystem::reader(machine.clone(), objects.clone(), "", "shop.owl", 8 << 20).unwrap();
        let store = RustStore::new_with_fs(Path::new("shop.owl"), 64 << 20, filesystem).unwrap();
        (store, machine)
    }

    #[test]
    fn a_reader_opens_from_the_object_store_alone() {
        let objects = Arc::new(MemoryObjectStore::new());
        let (writer, filesystem) = written(810, &objects);
        let published = writer.publish(&filesystem).unwrap();
        assert_eq!(published.number, 1);
        assert_eq!(published.committed_lsn, writer.committed_lsn());

        let before = objects.list("").unwrap();
        let (reader, machine) = read(811, &objects);
        // Checkpointed rows and the one that was only in the log.
        assert_eq!(codes(&reader), (0..=300).collect::<Vec<_>>());
        let fetched_parts = machine
            .list_dir(Path::new(""))
            .unwrap()
            .iter()
            .filter(|path| part_key(Path::new(""), path).is_some())
            .count();
        assert_eq!(fetched_parts, 0, "a reader fetches blocks of a part, not the part");

        // Whatever the reader does stays with the reader.
        reader.put_document(&event(999)).unwrap();
        reader.checkpoint().unwrap();
        drop(reader);
        assert_eq!(objects.list("").unwrap(), before);
        assert_eq!(codes(&writer), (0..=300).collect::<Vec<_>>());
    }

    #[test]
    fn a_reader_sees_what_was_published_and_no_later() {
        let objects = Arc::new(MemoryObjectStore::new());
        let (writer, filesystem) = written(812, &objects);
        writer.publish(&filesystem).unwrap();
        let (first, _) = read(813, &objects);

        for number in 301..400 {
            writer.put_document(&event(number)).unwrap();
        }
        writer.checkpoint().unwrap();
        assert_eq!(codes(&first), (0..=300).collect::<Vec<_>>(), "unpublished writes are not seen");

        assert_eq!(writer.publish(&filesystem).unwrap().number, 2);
        let (second, _) = read(814, &objects);
        assert_eq!(codes(&second), (0..400).collect::<Vec<_>>());
        // The reader of the publication before goes on working.
        assert_eq!(codes(&first), (0..=300).collect::<Vec<_>>());
    }

    #[test]
    fn publications_before_the_last_two_are_deleted_with_their_parts() {
        let objects = Arc::new(MemoryObjectStore::new());
        let (writer, filesystem) = written(815, &objects);
        writer.publish(&filesystem).unwrap();
        let first_parts: Vec<String> =
            objects.list("").unwrap().into_iter().filter(|key| !key.contains('@') && key.contains(".part.")).collect();
        assert!(!first_parts.is_empty());

        // Rewrite every row twice over, so that the first part is listed
        // by no root, and publish each time.
        for round in 0..3 {
            for number in 0..=300 {
                let mut changed = event(number);
                changed.fields.insert("round".into(), Value::Integer(round));
                writer.put_document(&changed).unwrap();
            }
            writer.checkpoint().unwrap();
            writer.wait_for_maintenance().unwrap();
            writer.reclaim_obsolete_files().unwrap();
            writer.publish(&filesystem).unwrap();
        }

        let keys = objects.list("").unwrap();
        let numbers: std::collections::BTreeSet<u64> =
            keys.iter().filter_map(|key| Published::number_of("shop.owl", key)).collect();
        assert_eq!(numbers.into_iter().collect::<Vec<_>>(), [3, 4], "the last two publications are kept");
        for part in &first_parts {
            assert!(!keys.contains(part), "'{part}' is listed by no kept publication and is still there");
        }
        // And a reader of the newest has every part it needs.
        let (reader, _) = read(816, &objects);
        assert_eq!(codes(&reader), (0..=300).collect::<Vec<_>>());
    }

    #[test]
    fn a_database_that_was_never_published_cannot_be_read() {
        let objects = Arc::new(MemoryObjectStore::new());
        let (_writer, _filesystem) = written(817, &objects);
        let machine = Arc::new(SimulatedFileSystem::new(818));
        let error = ObjectFileSystem::reader(machine, objects.clone(), "", "shop.owl", 8 << 20).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_writer_that_starts_again_keeps_parts_a_reader_may_need() {
        let objects = Arc::new(MemoryObjectStore::new());
        let local = Arc::new(SimulatedFileSystem::new(819));
        {
            let filesystem = layered(&local, &objects, 8 << 20);
            let store = RustStore::new_with_fs(Path::new("shop.owl"), 64 << 20, filesystem.clone()).unwrap();
            for number in 0..300 {
                store.put_document(&event(number)).unwrap();
            }
            store.checkpoint().unwrap();
            store.publish(&filesystem).unwrap();
        }
        let (reader, _) = read(820, &objects);

        // A new process: it has published nothing itself.
        let filesystem = layered(&local, &objects, 8 << 20);
        let part = objects.list("").unwrap().into_iter().find(|key| key.ends_with(".part.1") || key.contains(".part.")).unwrap();
        let part = part.split('@').next().unwrap().to_string();
        filesystem.remove(Path::new(&part)).unwrap();
        assert!(objects.len(&part).unwrap().is_some(), "the published part waits for a later publication");
        assert!(!filesystem.exists(Path::new(&part)), "to the writer it is gone");
        assert_eq!(codes(&reader), (0..300).collect::<Vec<_>>());
    }

    fn conditional_changes_are_one_step(store: &dyn ObjectStore) {
        assert_eq!(store.version("a").unwrap(), None);
        // Only if nothing is there.
        let first = store.put_if("a", &mut &b"one"[..], 3, None).unwrap().expect("nothing was there");
        assert_eq!(store.put_if("a", &mut &b"two"[..], 3, None).unwrap(), None);
        assert_eq!(store.version("a").unwrap().as_deref(), Some(first.as_str()));

        // Only if it is the object last seen.
        let second = store.put_if("a", &mut &b"two"[..], 3, Some(&first)).unwrap().expect("it was as seen");
        assert_ne!(first, second);
        assert_eq!(store.put_if("a", &mut &b"six"[..], 3, Some(&first)).unwrap(), None);
        let mut held = [0u8; 3];
        store.read_at("a", 0, &mut held).unwrap();
        assert_eq!(&held, b"two");

        assert!(!store.delete_if("a", &first).unwrap());
        assert!(store.delete_if("a", &second).unwrap());
        assert_eq!(store.version("a").unwrap(), None);
        assert!(!store.delete_if("a", &second).unwrap());
    }

    #[test]
    fn both_stores_change_an_object_only_if_it_is_as_last_seen() {
        conditional_changes_are_one_step(&MemoryObjectStore::new());
        let directory = tempfile::TempDir::new().unwrap();
        let store = DirectoryObjectStore::new(directory.path()).unwrap();
        conditional_changes_are_one_step(&store);
        assert!(store.list("").unwrap().is_empty(), "the lock is not an object");
    }

    /// Open the database as its writer on the machine `local`.
    fn write(local: &Arc<SimulatedFileSystem>, objects: &Arc<MemoryObjectStore>) -> (RustStore, Arc<ObjectFileSystem>) {
        let filesystem = ObjectFileSystem::writer(local.clone(), objects.clone(), "", "shop.owl", 8 << 20).unwrap();
        let store = RustStore::new_with_fs(Path::new("shop.owl"), 64 << 20, filesystem.clone()).unwrap();
        (store, filesystem)
    }

    #[test]
    fn a_database_has_one_writer() {
        let objects = Arc::new(MemoryObjectStore::new());
        let first = Arc::new(SimulatedFileSystem::new(830));
        let (store, filesystem) = write(&first, &objects);
        for number in 0..100 {
            store.put_document(&event(number)).unwrap();
        }
        store.checkpoint().unwrap();
        store.publish(&filesystem).unwrap();
        drop(store);

        // Another machine cannot simply start writing it.
        let second = Arc::new(SimulatedFileSystem::new(831));
        let refused = ObjectFileSystem::writer(second, objects.clone(), "", "shop.owl", 8 << 20).err().unwrap();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);

        // The machine that wrote it carries on, under a new number, and
        // finds the parts it put under the old one.
        let (store, filesystem) = write(&first, &objects);
        assert_eq!(codes(&store), (0..100).collect::<Vec<_>>());
        assert!(filesystem.drop_local_parts().unwrap() > 0);
        drop(store);
        let (store, _) = write(&first, &objects);
        assert_eq!(codes(&store), (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn a_writer_that_was_replaced_cannot_harm_the_one_that_replaced_it() {
        let objects = Arc::new(MemoryObjectStore::new());
        let old_machine = Arc::new(SimulatedFileSystem::new(832));
        let (old, old_filesystem) = write(&old_machine, &objects);
        for number in 0..100 {
            old.put_document(&event(number)).unwrap();
        }
        old.checkpoint().unwrap();
        old.publish(&old_filesystem).unwrap();
        // Committed and never published: it does not cross.
        old.put_document(&event(100)).unwrap();

        // A second machine takes the database over from the publication.
        let new_machine = Arc::new(SimulatedFileSystem::new(833));
        let new_filesystem =
            ObjectFileSystem::take_over(new_machine.clone(), objects.clone(), "", "shop.owl", 8 << 20).unwrap();
        let new = RustStore::new_with_fs(Path::new("shop.owl"), 64 << 20, new_filesystem.clone()).unwrap();
        assert_eq!(codes(&new), (0..100).collect::<Vec<_>>());
        for number in 200..300 {
            new.put_document(&event(number)).unwrap();
        }
        new.checkpoint().unwrap();
        new.publish(&new_filesystem).unwrap();
        let expected: Vec<i64> = (0..100).chain(200..300).collect();

        // The old writer does not know. It rewrites everything, makes
        // checkpoints, clears out what it thinks is unused, and tries to
        // publish. None of it may reach what the new writer relies on.
        for round in 0..3 {
            for number in 0..=100 {
                let mut changed = event(number);
                changed.fields.insert("round".into(), Value::Integer(round));
                old.put_document(&changed).unwrap();
            }
            let _ = old.checkpoint();
            let _ = old.wait_for_maintenance();
            let _ = old.reclaim_obsolete_files();
            let refused = old.publish(&old_filesystem).err().expect("a replaced writer cannot publish");
            assert!(refused.to_string().contains("taken over"), "{refused}");
        }
        // Once told, it writes nothing more to the object store.
        let before = objects.list("").unwrap();
        old.put_document(&event(101)).unwrap();
        assert!(old.checkpoint().is_err());
        assert_eq!(objects.list("").unwrap(), before);

        // The new writer's database is whole: to itself, to a reader,
        // and after it starts again with its parts only in the store.
        assert_eq!(codes(&new), expected);
        let (reader, _) = read(834, &objects);
        assert_eq!(codes(&reader), expected);
        new.checkpoint().unwrap();
        new.publish(&new_filesystem).unwrap();
        drop(new);
        let (new, new_filesystem) = write(&new_machine, &objects);
        // Whatever parts it holds locally, if any, go.
        new_filesystem.drop_local_parts().unwrap();
        drop(new);
        let (new, _) = write(&new_machine, &objects);
        assert_eq!(codes(&new), expected);

        // And the old machine cannot come back as the writer.
        drop(old);
        let refused =
            ObjectFileSystem::writer(old_machine, objects.clone(), "", "shop.owl", 8 << 20).err().unwrap();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn there_is_nothing_to_take_over_before_a_publication() {
        let objects = Arc::new(MemoryObjectStore::new());
        let first = Arc::new(SimulatedFileSystem::new(835));
        let (store, _) = write(&first, &objects);
        store.put_document(&event(1)).unwrap();
        store.checkpoint().unwrap();

        let second = Arc::new(SimulatedFileSystem::new(836));
        let refused =
            ObjectFileSystem::take_over(second.clone(), objects.clone(), "", "shop.owl", 8 << 20).err().unwrap();
        assert_eq!(refused.kind(), io::ErrorKind::NotFound);
        // And a claim that was never published is not something to read.
        let unread = ObjectFileSystem::reader(second, objects.clone(), "", "shop.owl", 8 << 20).err().unwrap();
        assert_eq!(unread.kind(), io::ErrorKind::NotFound);
    }
}
