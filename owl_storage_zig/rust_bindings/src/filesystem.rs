//! FileSystem trait for dependency injection in Zig storage layer
//!
//! This module provides the FileSystem trait that allows the Zig storage
//! layer to work with both real file systems (production) and simulated
//! file systems (testing).

use std::io;
use std::path::Path;

/// File system operations for dependency injection
pub trait FileSystem: Send + Sync {
    /// Create or open a file for writing
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>>;
    
    /// Open an existing file for reading
    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>>;
    
    /// Remove a file
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    
    /// Check if a file exists
    fn exists(&self, path: &Path) -> bool;
    
    /// Get file size
    fn file_size(&self, path: &Path) -> io::Result<u64>;
    
    /// Memory map a file (returns base pointer and size)
    fn mmap(&self, path: &Path, size: usize) -> io::Result<(*mut u8, usize)>;
    
    /// Unmap memory
    fn munmap(&self, ptr: *mut u8, size: usize) -> io::Result<()>;
    
    /// Sync file to disk (fsync)
    fn sync(&self, path: &Path) -> io::Result<()>;
}

/// File handle for reading/writing operations
pub trait FileHandle: Send + Sync {
    /// Write data to file
    fn write(&mut self, data: &[u8]) -> io::Result<usize>;
    
    /// Read data from file
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
    
    /// Seek to position
    fn seek(&mut self, pos: u64) -> io::Result<u64>;
    
    /// Sync file to disk
    fn sync(&mut self) -> io::Result<()>;
    
    /// Get file size
    fn size(&self) -> io::Result<u64>;
}

/// Real file system implementation for production
pub struct RealFileSystem;

impl RealFileSystem {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RealFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for RealFileSystem {
    fn create(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(RealFileHandle { file }))
    }
    
    fn open(&self, path: &Path) -> io::Result<Box<dyn FileHandle>> {
        let file = std::fs::File::open(path)?;
        Ok(Box::new(RealFileHandle { file }))
    }
    
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
    
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    
    fn file_size(&self, path: &Path) -> io::Result<u64> {
        let metadata = std::fs::metadata(path)?;
        Ok(metadata.len())
    }
    
    fn mmap(&self, _path: &Path, _size: usize) -> io::Result<(*mut u8, usize)> {
        // For now, return an error - mmap will be implemented later
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory mapping not yet implemented in RealFileSystem"
        ))
    }
    
    fn munmap(&self, _ptr: *mut u8, _size: usize) -> io::Result<()> {
        // For now, return an error - munmap will be implemented later
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory unmapping not yet implemented in RealFileSystem"
        ))
    }
    
    fn sync(&self, path: &Path) -> io::Result<()> {
        // Open file and sync it
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)?;
        file.sync_all()
    }
}

/// Real file handle implementation
struct RealFileHandle {
    file: std::fs::File,
}

impl FileHandle for RealFileHandle {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        use std::io::Write;
        self.file.write(data)
    }
    
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        use std::io::Read;
        self.file.read(buffer)
    }
    
    fn seek(&mut self, pos: u64) -> io::Result<u64> {
        use std::io::{Seek, SeekFrom};
        self.file.seek(SeekFrom::Start(pos))
    }
    
    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
    
    fn size(&self) -> io::Result<u64> {
        let metadata = self.file.metadata()?;
        Ok(metadata.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    
    #[test]
    fn test_real_filesystem_basic() {
        let fs = RealFileSystem::new();
        let path = Path::new("/tmp/test_real_fs.txt");
        
        // Clean up any existing file
        let _ = fs.remove_file(path);
        
        // Create and write
        {
            let mut handle = fs.create(path).unwrap();
            handle.write(b"Hello, FileSystem!").unwrap();
            handle.sync().unwrap();
        }
        
        // Check exists
        assert!(fs.exists(path));
        
        // Read back
        {
            let mut handle = fs.open(path).unwrap();
            let mut buffer = vec![0u8; 100];
            let bytes_read = handle.read(&mut buffer).unwrap();
            assert_eq!(&buffer[..bytes_read], b"Hello, FileSystem!");
        }
        
        // Clean up
        fs.remove_file(path).unwrap();
        assert!(!fs.exists(path));
    }
}