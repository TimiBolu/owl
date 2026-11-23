use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// File system operations for dependency injection
pub trait FileSystem: Send + Sync {
    /// Create or open a file for writing
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>>;

    /// Open a file for reading
    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>>;

    /// Open a file with specific options
    fn open_with_options(
        &self,
        path: &Path,
        options: &FileOptions,
    ) -> io::Result<Box<dyn FileHandle>>;

    /// Remove a file
    fn remove(&self, path: &Path) -> io::Result<()>;

    /// Make sure a directory and its parents exist. A file system with no
    /// directories of its own, such as the simulated one, has nothing to do.
    fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }

    /// Atomically move a file within the same filesystem.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Check if a file exists
    fn exists(&self, path: &Path) -> bool;

    /// Get file size
    fn file_size(&self, path: &Path) -> io::Result<u64>;

    /// Sync all file system operations (fsync equivalent)
    fn sync_all(&self) -> io::Result<()>;

    /// Memory map a file (for high-performance access)
    fn mmap(&self, path: &Path, size: usize) -> io::Result<Box<dyn MemoryMap>>;

    /// The files directly inside a directory. A file system that cannot
    /// say gives none, and whatever depends on the list, such as removing
    /// files nothing refers to any more, then finds nothing to do.
    fn list_dir(&self, _directory: &Path) -> io::Result<Vec<PathBuf>> {
        Ok(Vec::new())
    }
}

/// File handle abstraction
pub trait FileHandle: Send + Sync {
    /// Read data from file
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Read data at an absolute offset without changing the shared cursor.
    ///
    /// Immutable storage segments use positional reads so thousands of
    /// logical segment handles can share one operating-system file handle
    /// without serializing unrelated reads or exhausting descriptor limits.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;

    /// Write data to file
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;

    /// Write data at an absolute offset without changing the shared cursor.
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize>;

    /// Flush data to storage (may not be durable until sync)
    fn flush(&mut self) -> io::Result<()>;

    /// Sync data to durable storage (fsync)
    fn sync(&mut self) -> io::Result<()>;

    /// Seek to position
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64>;

    /// Get current position
    fn position(&self) -> u64;

    /// Get file size
    fn size(&self) -> io::Result<u64>;

    /// Truncate or extend the file to an exact length.
    fn set_len(&mut self, len: u64) -> io::Result<()>;
}

/// Memory map abstraction
pub trait MemoryMap: Send + Sync {
    /// Get a slice of the mapped memory
    fn as_slice(&self) -> &[u8];

    /// Get a mutable slice of the mapped memory
    fn as_mut_slice(&mut self) -> &mut [u8];

    /// Sync mapped memory to storage
    fn sync(&self) -> io::Result<()>;
}

/// File opening options
#[derive(Debug, Clone)]
pub struct FileOptions {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
    pub append: bool,
}

impl Default for FileOptions {
    fn default() -> Self {
        Self {
            read: true,
            write: false,
            create: false,
            truncate: false,
            append: false,
        }
    }
}

/// A write that has not been synced. A crash may keep it, lose it, or keep
/// only the front of it.
enum Pending {
    Write { offset: usize, bytes: Vec<u8> },
    SetLen(usize),
}

/// In-memory file for simulation
struct SimulatedFile {
    data: Vec<u8>,
    position: u64,
    synced_data: Vec<u8>, // Data that survives crashes
    is_synced: bool,
    has_been_synced: bool, // Track if file has ever been synced
    /// The directory knows the file durably: it was made with `create`,
    /// which syncs the directory on a real disk, or renamed into place.
    entry_durable: bool,
    /// What was written since the last sync, in the order it was written.
    pending: Vec<Pending>,
}

impl SimulatedFile {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            position: 0,
            synced_data: Vec::new(),
            is_synced: true,
            has_been_synced: false, // New files haven't been synced yet
            entry_durable: false,
            pending: Vec::new(),
        }
    }

    fn crash_recover(&mut self) {
        // Lose all unsynced writes
        self.data = self.synced_data.clone();
        self.position = 0; // Reset position to beginning
        self.is_synced = true;
        self.pending.clear();
    }

    /// What a real disk may hold after power is lost: the synced bytes,
    /// and over them some of the unsynced writes, one of them possibly cut
    /// short at a sector boundary.
    fn crash_torn(&mut self, rng: &mut rand::rngs::StdRng, out_of_order: bool) {
        use rand::Rng;

        const SECTOR: usize = 512;
        let pending = std::mem::take(&mut self.pending);
        let mut image = self.synced_data.clone();
        if !pending.is_empty() {
            // Either the writes reached the disk in order and stopped
            // somewhere, or each made it or did not on its own.
            let in_order = !out_of_order || rng.gen_bool(0.5);
            let stop = rng.gen_range(0..=pending.len());
            let kept: Vec<bool> = (0..pending.len())
                .map(|at| if in_order { at < stop } else { rng.gen_bool(0.5) })
                .collect();
            let last = kept.iter().rposition(|kept| *kept);
            let tear = rng.gen_bool(0.5);
            for (at, write) in pending.iter().enumerate() {
                if !kept[at] {
                    continue;
                }
                match write {
                    Pending::SetLen(len) => image.resize(*len, 0),
                    Pending::Write { offset, bytes } => {
                        let mut len = bytes.len();
                        if tear && Some(at) == last && len > 0 {
                            let sectors = len / SECTOR;
                            len = if sectors == 0 {
                                rng.gen_range(0..len)
                            } else {
                                rng.gen_range(0..=sectors) * SECTOR
                            }
                            .min(len);
                        }
                        if len == 0 {
                            continue;
                        }
                        if offset + len > image.len() {
                            image.resize(offset + len, 0);
                        }
                        image[*offset..offset + len].copy_from_slice(&bytes[..len]);
                    }
                }
            }
        }
        self.synced_data = image.clone();
        self.data = image;
        self.position = 0;
        self.is_synced = true;
    }

    fn synced(&mut self) {
        self.synced_data = self.data.clone();
        self.is_synced = true;
        self.has_been_synced = true;
        self.pending.clear();
    }
}

/// Faults the simulated disk has been asked to produce.
#[derive(Default)]
struct Faults {
    /// Crash in the middle of this operation, counted from the start.
    crash_at: Option<u64>,
    /// Whether that crash may keep part of what was not synced.
    crash_torn: bool,
    /// Chance that a write, a sync or a change of length fails.
    io_failure_rate: f64,
    /// Whether a torn crash keeps only writes up to some point, in the
    /// order they were made, rather than any of them.
    in_order_only: bool,
}

struct Shared {
    /// In-memory files
    files: Mutex<HashMap<PathBuf, Arc<Mutex<SimulatedFile>>>>,
    /// Whether the system has crashed
    crashed: Mutex<bool>,
    /// Counts crashes. A handle opened before a crash belongs to a process
    /// that died in it, and must not touch the files afterwards.
    epoch: std::sync::atomic::AtomicU64,
    /// Operations that change the disk, counted since the start.
    operations: std::sync::atomic::AtomicU64,
    /// Failure injection probability (0.0 to 1.0)
    failure_rate: Mutex<f64>,
    faults: Mutex<Faults>,
    /// Random number generator for failures
    rng: Mutex<rand::rngs::StdRng>,
}

/// What an operation that changes the disk is to do.
enum Turn {
    Proceed,
    /// The machine dies during this operation.
    Crash { torn: bool },
}

impl Shared {
    fn is_crashed(&self) -> bool {
        *self.crashed.lock().unwrap()
    }

    fn epoch(&self) -> u64 {
        self.epoch.load(std::sync::atomic::Ordering::Acquire)
    }

    fn coin(&self) -> bool {
        use rand::Rng;
        self.rng.lock().unwrap().gen_bool(0.5)
    }

    fn crashed_error() -> io::Error {
        io::Error::new(io::ErrorKind::Other, "File system crashed")
    }

    /// Count an operation that changes the disk and say what becomes of it.
    fn turn(&self, may_fail: bool) -> io::Result<Turn> {
        use rand::Rng;

        if self.is_crashed() {
            return Err(Self::crashed_error());
        }
        let count = self
            .operations
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        let faults = self.faults.lock().unwrap();
        if faults.crash_at == Some(count) {
            return Ok(Turn::Crash {
                torn: faults.crash_torn,
            });
        }
        if may_fail
            && faults.io_failure_rate > 0.0
            && self.rng.lock().unwrap().gen::<f64>() < faults.io_failure_rate
        {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }
        Ok(Turn::Proceed)
    }

    fn crash(&self, torn: bool) {
        *self.crashed.lock().unwrap() = true;
        self.epoch
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.faults.lock().unwrap().crash_at = None;

        let mut files = self.files.lock().unwrap();
        // In the order of their names, so that one seed gives one outcome.
        let mut paths: Vec<PathBuf> = files.keys().cloned().collect();
        paths.sort();
        let out_of_order = !self.faults.lock().unwrap().in_order_only;
        let mut rng = self.rng.lock().unwrap();
        for path in paths {
            let keep = {
                use rand::Rng;
                let mut file = files[&path].lock().unwrap();
                if torn {
                    file.crash_torn(&mut rng, out_of_order);
                    // A file the directory was never made to remember may
                    // or may not be there.
                    file.has_been_synced || file.entry_durable || rng.gen_bool(0.5)
                } else {
                    file.crash_recover();
                    // If file has never been synced, it should be removed (never existed on disk)
                    file.has_been_synced
                }
            };
            if !keep {
                files.remove(&path);
            }
        }
    }
}

/// Simulated file system for crash testing
pub struct SimulatedFileSystem {
    shared: Arc<Shared>,
}

impl SimulatedFileSystem {
    /// Create a new simulated file system
    pub fn new(seed: u64) -> Self {
        use rand::SeedableRng;

        Self {
            shared: Arc::new(Shared {
                files: Mutex::new(HashMap::new()),
                crashed: Mutex::new(false),
                epoch: std::sync::atomic::AtomicU64::new(0),
                operations: std::sync::atomic::AtomicU64::new(0),
                failure_rate: Mutex::new(0.0),
                faults: Mutex::new(Faults::default()),
                rng: Mutex::new(rand::rngs::StdRng::seed_from_u64(seed)),
            }),
        }
    }

    /// Simulate a crash - lose all unsynced writes
    pub fn crash(&self) {
        self.shared.crash(false);
    }

    /// Simulate losing power the way a real disk does: of what was not
    /// synced, some writes are kept and some lost, and one may be cut
    /// short. Which ones follows from the seed.
    pub fn crash_torn(&self) {
        self.shared.crash(true);
    }

    /// Crash in the middle of an operation that changes the disk: the
    /// `after`-th one from now, counting writes, syncs, changes of length,
    /// creations, renames and removals. That operation and every one after
    /// it fail until `recover`. With `torn`, the crash is a `crash_torn`.
    pub fn crash_after(&self, after: u64, torn: bool) {
        let now = self.operations();
        let mut faults = self.shared.faults.lock().unwrap();
        faults.crash_at = Some(now + after.max(1));
        faults.crash_torn = torn;
    }

    /// Say how a torn crash treats what was not synced. By default any
    /// of those writes may be kept or lost on its own, so a later one can
    /// survive an earlier one, as on a disk that reorders them. With
    /// `in_order_only`, what survives is the writes up to some point, the
    /// last of them possibly cut short: a file is only ever a prefix of
    /// what was written to it.
    pub fn set_in_order_only(&self, in_order_only: bool) {
        self.shared.faults.lock().unwrap().in_order_only = in_order_only;
    }

    /// Call off a crash asked for with `crash_after` that has not happened.
    pub fn disarm(&self) {
        self.shared.faults.lock().unwrap().crash_at = None;
    }

    /// How many operations have changed the disk so far.
    pub fn operations(&self) -> u64 {
        self.shared
            .operations
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Every file there is, with its length, in the order of the names.
    pub fn files(&self) -> Vec<(PathBuf, usize)> {
        let files = self.shared.files.lock().unwrap();
        let mut listed: Vec<(PathBuf, usize)> = files
            .iter()
            .map(|(path, file)| (path.clone(), file.lock().unwrap().data.len()))
            .collect();
        listed.sort();
        listed
    }

    /// Whether the system is down after a crash.
    pub fn is_down(&self) -> bool {
        self.shared.is_crashed()
    }

    /// Recover from crash
    pub fn recover(&self) {
        *self.shared.crashed.lock().unwrap() = false;
    }

    /// Set failure injection rate (0.0 = no failures, 1.0 = always fail)
    pub fn set_failure_rate(&self, rate: f64) {
        *self.shared.failure_rate.lock().unwrap() = rate.clamp(0.0, 1.0);
    }

    /// Set the chance that a write, a sync or a change of length on an
    /// open file fails. A failed write changes nothing; a failed sync
    /// leaves what was written unsynced.
    pub fn set_io_failure_rate(&self, rate: f64) {
        self.shared.faults.lock().unwrap().io_failure_rate = rate.clamp(0.0, 1.0);
    }

    /// Check if operation should fail
    fn should_fail(&self) -> bool {
        use rand::Rng;

        let rate = *self.shared.failure_rate.lock().unwrap();
        if rate <= 0.0 {
            return false;
        }

        let mut rng = self.shared.rng.lock().unwrap();
        rng.gen::<f64>() < rate
    }

    /// Check if system is crashed
    fn is_crashed(&self) -> bool {
        self.shared.is_crashed()
    }

    fn handle(&self, file: Arc<Mutex<SimulatedFile>>, path: &Path) -> Box<dyn FileHandle> {
        Box::new(SimulatedFileHandle {
            file,
            path: path.to_path_buf(),
            shared: self.shared.clone(),
            epoch: self.shared.epoch(),
        })
    }

    /// Start an operation that changes the directory. `Ok(false)` means
    /// the machine died before it took effect.
    fn directory_turn(&self) -> io::Result<bool> {
        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }
        match self.shared.turn(false)? {
            Turn::Proceed => Ok(true),
            Turn::Crash { torn } => {
                // The crash falls before the operation or after it.
                if self.shared.coin() {
                    self.shared.crash(torn);
                    Err(Shared::crashed_error())
                } else {
                    self.shared.faults.lock().unwrap().crash_at = Some(u64::MAX);
                    self.shared.faults.lock().unwrap().crash_torn = torn;
                    Ok(false)
                }
            }
        }
    }

    /// Finish an operation that `directory_turn` said the machine dies
    /// right after.
    fn crash_after_effect(&self) -> io::Error {
        let torn = self.shared.faults.lock().unwrap().crash_torn;
        self.shared.crash(torn);
        Shared::crashed_error()
    }
}

impl FileSystem for SimulatedFileSystem {
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }
        let lives = self.directory_turn()?;

        let file = Arc::new(Mutex::new(SimulatedFile::new()));
        file.lock().unwrap().entry_durable = true;
        self.shared
            .files
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), file.clone());
        if !lives {
            return Err(self.crash_after_effect());
        }

        Ok(self.handle(file, path))
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }

        let files = self.shared.files.lock().unwrap();
        if let Some(file) = files.get(path) {
            // Reset position to 0 for reading
            file.lock().unwrap().position = 0;
            Ok(self.handle(file.clone(), path))
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "File not found"))
        }
    }

    fn open_with_options(
        &self,
        path: &Path,
        options: &FileOptions,
    ) -> io::Result<Box<dyn FileHandle>> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }

        let mut files = self.shared.files.lock().unwrap();

        if options.create {
            let file = Arc::new(Mutex::new(SimulatedFile::new()));
            files.insert(path.to_path_buf(), file.clone());

            Ok(self.handle(file, path))
        } else if let Some(file) = files.get(path) {
            Ok(self.handle(file.clone(), path))
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "File not found"))
        }
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }
        let lives = self.directory_turn()?;

        self.shared.files.lock().unwrap().remove(path);
        if !lives {
            return Err(self.crash_after_effect());
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }
        let lives = self.directory_turn()?;
        {
            let mut files = self.shared.files.lock().unwrap();
            let file = files
                .remove(from)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))?;
            file.lock().unwrap().entry_durable = true;
            files.insert(to.to_path_buf(), file);
        }
        if !lives {
            return Err(self.crash_after_effect());
        }
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        if self.is_crashed() {
            return false;
        }

        let files = self.shared.files.lock().unwrap();
        files.contains_key(path)
    }

    fn list_dir(&self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }
        let files = self.shared.files.lock().unwrap();
        // In path order, so that a run is the same run every time.
        let mut listed: Vec<PathBuf> = files
            .keys()
            .filter(|path| path.parent() == Some(directory))
            .cloned()
            .collect();
        listed.sort();
        Ok(listed)
    }

    fn file_size(&self, path: &Path) -> io::Result<u64> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        let files = self.shared.files.lock().unwrap();
        if let Some(file) = files.get(path) {
            let file = file.lock().unwrap();
            Ok(file.data.len() as u64)
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "File not found"))
        }
    }

    fn sync_all(&self) -> io::Result<()> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }

        let files = self.shared.files.lock().unwrap();
        for file in files.values() {
            let mut file = file.lock().unwrap();
            file.synced_data = file.data.clone();
            file.is_synced = true;
            file.pending.clear();
        }

        Ok(())
    }

    fn mmap(&self, path: &Path, size: usize) -> io::Result<Box<dyn MemoryMap>> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }

        let files = self.shared.files.lock().unwrap();
        if let Some(file) = files.get(path) {
            Ok(Box::new(SimulatedMemoryMap {
                file: file.clone(),
                size,
            }))
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "File not found"))
        }
    }
}

/// Simulated file handle
#[allow(dead_code)]
struct SimulatedFileHandle {
    file: Arc<Mutex<SimulatedFile>>,
    path: PathBuf,
    shared: Arc<Shared>,
    /// The crash count when this handle was opened.
    epoch: u64,
}

impl SimulatedFileHandle {
    /// A handle from before a crash belongs to a process that is gone.
    fn alive(&self) -> io::Result<()> {
        if self.epoch != self.shared.epoch() || self.shared.is_crashed() {
            return Err(Shared::crashed_error());
        }
        Ok(())
    }

    fn put(file: &mut SimulatedFile, start: usize, buf: &[u8]) {
        if start + buf.len() > file.data.len() {
            file.data.resize(start + buf.len(), 0);
        }
        file.data[start..start + buf.len()].copy_from_slice(buf);
        file.is_synced = false;
        file.pending.push(Pending::Write {
            offset: start,
            bytes: buf.to_vec(),
        });
    }
}

impl FileHandle for SimulatedFileHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.alive()?;
        let mut file = self.file.lock().unwrap();
        let start = (file.position as usize).min(file.data.len());
        let end = std::cmp::min(start + buf.len(), file.data.len());
        let bytes_to_read = end - start;

        if bytes_to_read > 0 {
            buf[..bytes_to_read].copy_from_slice(&file.data[start..end]);
            file.position += bytes_to_read as u64;
        }

        Ok(bytes_to_read)
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.alive()?;
        let file = self.file.lock().unwrap();
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset exceeds usize"))?;
        let end = std::cmp::min(start.saturating_add(buf.len()), file.data.len());
        let bytes_to_read = end.saturating_sub(start);
        if bytes_to_read > 0 {
            buf[..bytes_to_read].copy_from_slice(&file.data[start..end]);
        }
        Ok(bytes_to_read)
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.alive()?;
        let turn = self.shared.turn(true)?;
        {
            let mut file = self.file.lock().unwrap();
            let start = file.position as usize;
            Self::put(&mut file, start, buf);
            file.position += buf.len() as u64;
        }
        if let Turn::Crash { torn } = turn {
            // The write was on its way when the machine died.
            self.shared.crash(torn);
            return Err(Shared::crashed_error());
        }
        Ok(buf.len())
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.alive()?;
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset exceeds usize"))?;
        start
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "write range overflows"))?;
        let turn = self.shared.turn(true)?;
        {
            let mut file = self.file.lock().unwrap();
            Self::put(&mut file, start, buf);
        }
        if let Turn::Crash { torn } = turn {
            self.shared.crash(torn);
            return Err(Shared::crashed_error());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Flush is a no-op in simulation (data is already in memory)
        self.alive()
    }

    fn sync(&mut self) -> io::Result<()> {
        self.alive()?;
        match self.shared.turn(true)? {
            Turn::Proceed => {
                self.file.lock().unwrap().synced();
                Ok(())
            }
            Turn::Crash { torn } => {
                // The machine died during the sync: either it had finished
                // and nobody was told, or it had not.
                if self.shared.coin() {
                    self.file.lock().unwrap().synced();
                }
                self.shared.crash(torn);
                Err(Shared::crashed_error())
            }
        }
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let mut file = self.file.lock().unwrap();

        let new_pos = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => {
                let file_size = file.data.len() as i64;
                (file_size + offset) as u64
            }
            SeekFrom::Current(offset) => {
                let current = file.position as i64;
                (current + offset) as u64
            }
        };

        file.position = new_pos;
        Ok(new_pos)
    }

    fn position(&self) -> u64 {
        self.file.lock().unwrap().position
    }

    fn size(&self) -> io::Result<u64> {
        self.alive()?;
        Ok(self.file.lock().unwrap().data.len() as u64)
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.alive()?;
        let turn = self.shared.turn(true)?;
        {
            let mut file = self.file.lock().unwrap();
            file.data.resize(len as usize, 0);
            file.position = file.position.min(len);
            file.is_synced = false;
            file.pending.push(Pending::SetLen(len as usize));
        }
        if let Turn::Crash { torn } = turn {
            self.shared.crash(torn);
            return Err(Shared::crashed_error());
        }
        Ok(())
    }
}

/// Simulated memory map
#[allow(dead_code)]
struct SimulatedMemoryMap {
    file: Arc<Mutex<SimulatedFile>>,
    size: usize,
}

impl MemoryMap for SimulatedMemoryMap {
    fn as_slice(&self) -> &[u8] {
        // This is unsafe in real implementation, but for simulation we'll use a different approach
        // In practice, we'd need to return a reference with proper lifetime management
        unimplemented!("Memory mapping requires unsafe code - use read/write operations instead")
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unimplemented!("Memory mapping requires unsafe code - use read/write operations instead")
    }

    fn sync(&self) -> io::Result<()> {
        let mut file = self.file.lock().unwrap();
        file.synced_data = file.data.clone();
        file.is_synced = true;
        Ok(())
    }
}

/// Real file system for production deployment
#[allow(dead_code)]
pub struct RealFileSystem {
    /// Optional name for debugging
    name: Option<String>,
}

impl RealFileSystem {
    pub fn new() -> Self {
        Self { name: None }
    }

    pub fn with_name(name: String) -> Self {
        Self { name: Some(name) }
    }
}

impl Default for RealFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for RealFileSystem {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        let file = File::create(path)?;
        sync_parent_directory(path)?;
        Ok(Box::new(RealFileHandle { file, position: 0 }))
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        let file = File::open(path)?;
        Ok(Box::new(RealFileHandle { file, position: 0 }))
    }

    fn open_with_options(
        &self,
        path: &Path,
        options: &FileOptions,
    ) -> io::Result<Box<dyn FileHandle>> {
        let mut open_options = OpenOptions::new();
        open_options
            .read(options.read)
            .write(options.write)
            .create(options.create)
            .truncate(options.truncate)
            .append(options.append);

        let file = open_options.open(path)?;
        let position = if options.append {
            file.metadata()?.len()
        } else {
            0
        };
        Ok(Box::new(RealFileHandle { file, position }))
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)?;
        sync_parent_directory(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)?;
        sync_parent_directory(from)?;
        if from.parent() != to.parent() {
            sync_parent_directory(to)?;
        }
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn list_dir(&self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        // A path with no directory in it is in the current one.
        let directory = match directory.as_os_str().is_empty() {
            true => Path::new("."),
            false => directory,
        };
        let mut listed = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                listed.push(entry.path());
            }
        }
        listed.sort();
        Ok(listed)
    }

    fn file_size(&self, path: &Path) -> io::Result<u64> {
        let metadata = std::fs::metadata(path)?;
        Ok(metadata.len())
    }

    fn sync_all(&self) -> io::Result<()> {
        // In real implementation, this would sync all open files
        // For now, this is a no-op
        Ok(())
    }

    fn mmap(&self, _path: &Path, _size: usize) -> io::Result<Box<dyn MemoryMap>> {
        // Real memory mapping would use mmap system call
        // For now, return an error
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory mapping not implemented in RealFileSystem",
        ))
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    File::open(parent_directory(path))?.sync_all()
}

/// Directory containing `path`. A bare relative name such as `db.owl` has the
/// empty path as its parent, which cannot be opened; it lives in the current
/// directory.
fn parent_directory(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Real file handle wrapping std::fs::File
struct RealFileHandle {
    file: File,
    position: u64,
}

impl FileHandle for RealFileHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.file.read(buf)?;
        self.position = self.position.saturating_add(read as u64);
        Ok(read)
    }

    #[cfg(unix)]
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
    }

    #[cfg(windows)]
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_read(&self.file, buf, offset)
    }

    #[cfg(not(any(unix, windows)))]
    fn read_at(&self, _buf: &mut [u8], _offset: u64) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "positional file reads are unsupported on this platform",
        ))
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.file.write(buf)?;
        self.position = self.position.saturating_add(written as u64);
        Ok(written)
    }

    #[cfg(unix)]
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        std::os::unix::fs::FileExt::write_at(&self.file, buf, offset)
    }

    #[cfg(windows)]
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_write(&self.file, buf, offset)
    }

    #[cfg(not(any(unix, windows)))]
    fn write_at(&self, _buf: &[u8], _offset: u64) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "positional file writes are unsupported on this platform",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let position = self.file.seek(pos)?;
        self.position = position;
        Ok(position)
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn size(&self) -> io::Result<u64> {
        let metadata = self.file.metadata()?;
        Ok(metadata.len())
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_relative_path_syncs_the_current_directory() {
        assert_eq!(parent_directory(Path::new("db.owl")), Path::new("."));
        assert_eq!(parent_directory(Path::new("./db.owl")), Path::new("."));
        assert_eq!(parent_directory(Path::new("data/db.owl")), Path::new("data"));
        assert_eq!(parent_directory(Path::new("/db.owl")), Path::new("/"));
        // The root has no parent at all; syncing "." is harmless there.
        assert_eq!(parent_directory(Path::new("/")), Path::new("."));
        #[cfg(unix)]
        sync_parent_directory(Path::new("bare-name-that-need-not-exist.owl")).unwrap();
    }

    #[test]
    fn test_simulated_filesystem_basic() {
        let fs = SimulatedFileSystem::new(42);
        let path = Path::new("test.txt");

        // Create and write to file
        let mut file = fs.create(path).unwrap();
        file.write(b"Hello, World!").unwrap();

        // File should exist
        assert!(fs.exists(path));
        assert_eq!(fs.file_size(path).unwrap(), 13);

        // Read from file
        let mut file = fs.open(path).unwrap();
        let mut buf = vec![0u8; 13];
        let bytes_read = file.read(&mut buf).unwrap();
        assert_eq!(bytes_read, 13);
        assert_eq!(&buf, b"Hello, World!");
    }

    #[test]
    fn positional_io_does_not_move_the_shared_cursor() {
        let fs = SimulatedFileSystem::new(43);
        let path = Path::new("positional.txt");
        let mut file = fs.create(path).unwrap();
        file.write(b"abcdefgh").unwrap();
        file.seek(SeekFrom::Start(2)).unwrap();

        let mut selected = [0u8; 3];
        assert_eq!(file.read_at(&mut selected, 4).unwrap(), 3);
        assert_eq!(&selected, b"efg");
        assert_eq!(file.position(), 2);

        assert_eq!(file.write_at(b"XYZ", 4).unwrap(), 3);
        assert_eq!(file.position(), 2);
        let mut complete = [0u8; 8];
        assert_eq!(file.read_at(&mut complete, 0).unwrap(), 8);
        assert_eq!(&complete, b"abcdXYZh");
    }

    #[test]
    fn test_crash_recovery() {
        let fs = SimulatedFileSystem::new(42);
        let path = Path::new("test.txt");

        // Write data without sync
        let mut file = fs.create(path).unwrap();
        file.write(b"Unsynced data").unwrap();

        // Crash the system. Nothing answers until it has recovered.
        fs.crash();
        assert!(!fs.exists(path));
        fs.recover();

        // The file was never synced, so the crash took it.
        assert!(!fs.exists(path));

        // Try again with sync
        let mut file = fs.create(path).unwrap();
        file.write(b"Synced data").unwrap();
        file.sync().unwrap();

        // Crash again
        fs.crash();
        fs.recover();

        // Synced data should survive
        assert!(fs.exists(path));

        let mut file = fs.open(path).unwrap();
        let mut buf = vec![0u8; 11];
        file.read(&mut buf).unwrap();
        assert_eq!(&buf, b"Synced data");
    }

    #[test]
    fn test_failure_injection() {
        let fs = SimulatedFileSystem::new(42);
        fs.set_failure_rate(1.0); // Always fail

        let path = Path::new("test.txt");

        // Operations should fail
        assert!(fs.create(path).is_err());

        // Disable failures
        fs.set_failure_rate(0.0);

        // Operations should succeed
        assert!(fs.create(path).is_ok());
    }
}
