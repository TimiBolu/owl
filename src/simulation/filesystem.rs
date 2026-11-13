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

/// In-memory file for simulation
struct SimulatedFile {
    data: Vec<u8>,
    position: u64,
    synced_data: Vec<u8>, // Data that survives crashes
    is_synced: bool,
    has_been_synced: bool, // Track if file has ever been synced
}

impl SimulatedFile {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            position: 0,
            synced_data: Vec::new(),
            is_synced: true,
            has_been_synced: false, // New files haven't been synced yet
        }
    }

    fn crash_recover(&mut self) {
        // Lose all unsynced writes
        self.data = self.synced_data.clone();
        self.position = 0; // Reset position to beginning
        self.is_synced = true;
    }
}

/// Simulated file system for crash testing
pub struct SimulatedFileSystem {
    /// In-memory files
    files: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<SimulatedFile>>>>>,
    /// Whether the system has crashed
    crashed: Arc<Mutex<bool>>,
    /// Failure injection probability (0.0 to 1.0)
    failure_rate: Arc<Mutex<f64>>,
    /// Random number generator for failures
    rng: Arc<Mutex<rand::rngs::StdRng>>,
}

impl SimulatedFileSystem {
    /// Create a new simulated file system
    pub fn new(seed: u64) -> Self {
        use rand::SeedableRng;

        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            crashed: Arc::new(Mutex::new(false)),
            failure_rate: Arc::new(Mutex::new(0.0)),
            rng: Arc::new(Mutex::new(rand::rngs::StdRng::seed_from_u64(seed))),
        }
    }

    /// Simulate a crash - lose all unsynced writes
    pub fn crash(&self) {
        *self.crashed.lock().unwrap() = true;

        let mut files = self.files.lock().unwrap();
        let mut files_to_remove = Vec::new();

        for (path, file) in files.iter() {
            let mut file_guard = file.lock().unwrap();
            file_guard.crash_recover();

            // If file has never been synced, it should be removed (never existed on disk)
            if !file_guard.has_been_synced {
                files_to_remove.push(path.clone());
            }
        }

        // Remove files that were never synced
        for path in files_to_remove {
            files.remove(&path);
        }
    }

    /// Recover from crash
    pub fn recover(&self) {
        *self.crashed.lock().unwrap() = false;
    }

    /// Set failure injection rate (0.0 = no failures, 1.0 = always fail)
    pub fn set_failure_rate(&self, rate: f64) {
        *self.failure_rate.lock().unwrap() = rate.clamp(0.0, 1.0);
    }

    /// Check if operation should fail
    fn should_fail(&self) -> bool {
        use rand::Rng;

        let rate = *self.failure_rate.lock().unwrap();
        if rate <= 0.0 {
            return false;
        }

        let mut rng = self.rng.lock().unwrap();
        rng.gen::<f64>() < rate
    }

    /// Check if system is crashed
    fn is_crashed(&self) -> bool {
        *self.crashed.lock().unwrap()
    }
}

impl FileSystem for SimulatedFileSystem {
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }

        let mut files = self.files.lock().unwrap();
        let file = Arc::new(Mutex::new(SimulatedFile::new()));
        files.insert(path.to_path_buf(), file.clone());

        Ok(Box::new(SimulatedFileHandle {
            file,
            path: path.to_path_buf(),
        }))
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }

        let files = self.files.lock().unwrap();
        if let Some(file) = files.get(path) {
            // Reset position to 0 for reading
            file.lock().unwrap().position = 0;
            Ok(Box::new(SimulatedFileHandle {
                file: file.clone(),
                path: path.to_path_buf(),
            }))
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

        let mut files = self.files.lock().unwrap();

        if options.create {
            let file = Arc::new(Mutex::new(SimulatedFile::new()));
            files.insert(path.to_path_buf(), file.clone());

            Ok(Box::new(SimulatedFileHandle {
                file,
                path: path.to_path_buf(),
            }))
        } else if let Some(file) = files.get(path) {
            Ok(Box::new(SimulatedFileHandle {
                file: file.clone(),
                path: path.to_path_buf(),
            }))
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "File not found"))
        }
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }

        let mut files = self.files.lock().unwrap();
        files.remove(path);
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }
        if self.should_fail() {
            return Err(io::Error::new(io::ErrorKind::Other, "Injected failure"));
        }
        let mut files = self.files.lock().unwrap();
        let file = files
            .remove(from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))?;
        files.insert(to.to_path_buf(), file);
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        if self.is_crashed() {
            return false;
        }

        let files = self.files.lock().unwrap();
        files.contains_key(path)
    }

    fn file_size(&self, path: &Path) -> io::Result<u64> {
        if self.is_crashed() {
            return Err(io::Error::new(io::ErrorKind::Other, "File system crashed"));
        }

        let files = self.files.lock().unwrap();
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

        let files = self.files.lock().unwrap();
        for file in files.values() {
            let mut file = file.lock().unwrap();
            file.synced_data = file.data.clone();
            file.is_synced = true;
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

        let files = self.files.lock().unwrap();
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
}

impl FileHandle for SimulatedFileHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut file = self.file.lock().unwrap();
        let start = file.position as usize;
        let end = std::cmp::min(start + buf.len(), file.data.len());
        let bytes_to_read = end - start;

        if bytes_to_read > 0 {
            buf[..bytes_to_read].copy_from_slice(&file.data[start..end]);
            file.position += bytes_to_read as u64;
        }

        Ok(bytes_to_read)
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
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
        let mut file = self.file.lock().unwrap();
        let start = file.position as usize;

        // Extend file if necessary
        if start + buf.len() > file.data.len() {
            file.data.resize(start + buf.len(), 0);
        }

        // Write data
        file.data[start..start + buf.len()].copy_from_slice(buf);
        file.position += buf.len() as u64;
        file.is_synced = false;

        Ok(buf.len())
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        let mut file = self.file.lock().unwrap();
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset exceeds usize"))?;
        let end = start
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "write range overflows"))?;
        if end > file.data.len() {
            file.data.resize(end, 0);
        }
        file.data[start..end].copy_from_slice(buf);
        file.is_synced = false;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Flush is a no-op in simulation (data is already in memory)
        Ok(())
    }

    fn sync(&mut self) -> io::Result<()> {
        let mut file = self.file.lock().unwrap();
        file.synced_data = file.data.clone();
        file.is_synced = true;
        file.has_been_synced = true; // Mark that this file has been synced
        Ok(())
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
        Ok(self.file.lock().unwrap().data.len() as u64)
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        let mut file = self.file.lock().unwrap();
        file.data.resize(len as usize, 0);
        file.position = file.position.min(len);
        file.is_synced = false;
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
    File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
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

        // Crash the system
        fs.crash();

        // Data should be lost
        assert!(!fs.exists(path));

        // Recover and try again with sync
        fs.recover();
        let mut file = fs.create(path).unwrap();
        file.write(b"Synced data").unwrap();
        file.sync().unwrap();

        // Crash again
        fs.crash();

        // Synced data should survive
        assert!(fs.exists(path));

        fs.recover();
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
