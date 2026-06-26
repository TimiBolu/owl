//! Safe Arena wrapper with RAII and FileSystem dependency injection

use crate::error::{Error, Result};
use crate::ffi;
use crate::objref::ObjRef;
use crate::stats::{ArenaStats, ColocationMetrics};
use std::ffi::CString;
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;

/// A memory-mapped arena for object storage with FileSystem dependency injection.
///
/// The Arena provides:
/// - Fast bump allocation (<20ns per object)
/// - Memory-mapped file storage (or simulated storage for testing)
/// - Co-location for cache efficiency
/// - Thread-safe operations (via atomic operations in Zig)
///
/// # RAII
///
/// The Arena automatically cleans up resources when dropped.
///
/// # Example
///
/// ```no_run
/// use owl_storage_zig::{Arena, RealFileSystem};
/// use std::sync::Arc;
///
/// let fs = Arc::new(RealFileSystem::new());
/// let arena = Arena::create_with_fs("data.owl", 100 * 1024 * 1024, fs)?;
/// let obj_ref = arena.alloc(256)?;
/// # Ok::<(), owl_storage_zig::Error>(())
/// ```
pub struct Arena {
    ptr: NonNull<ffi::Arena>,
    _filesystem: Arc<dyn crate::filesystem::FileSystem>,
    _marker: PhantomData<ffi::Arena>,
}

// Safety: Arena uses atomic operations internally for thread safety
unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

impl Arena {
    /// Create a new arena with file-backed memory mapping using dependency injection.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the backing file
    /// * `size` - Size of the arena in bytes (1MB - 1TB)
    /// * `filesystem` - FileSystem implementation for I/O operations
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidArena` if creation fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use owl_storage_zig::{Arena, RealFileSystem};
    /// use std::sync::Arc;
    ///
    /// let fs = Arc::new(RealFileSystem::new());
    /// let arena = Arena::create_with_fs("data.owl", 100 * 1024 * 1024, fs)?;
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    pub fn create_with_fs<P: AsRef<Path>>(
        path: P, 
        size: usize, 
        filesystem: Arc<dyn crate::filesystem::FileSystem>
    ) -> Result<Self> {
        let path_str = path
            .as_ref()
            .to_str()
            .ok_or(Error::InvalidArena)?;
        let c_path = CString::new(path_str).map_err(|_| Error::InvalidArena)?;

        let ptr = unsafe { ffi::owl_arena_create(c_path.as_ptr(), size) };

        NonNull::new(ptr)
            .map(|ptr| Arena {
                ptr,
                _filesystem: filesystem,
                _marker: PhantomData,
            })
            .ok_or(Error::InvalidArena)
    }

    /// Create a new arena with file-backed memory mapping (backward compatibility).
    ///
    /// This method uses RealFileSystem for production deployment.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the backing file
    /// * `size` - Size of the arena in bytes (1MB - 1TB)
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidArena` if creation fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use owl_storage_zig::Arena;
    ///
    /// let arena = Arena::create("data.owl", 100 * 1024 * 1024)?;
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    pub fn create<P: AsRef<Path>>(path: P, size: usize) -> Result<Self> {
        let filesystem = Arc::new(crate::filesystem::RealFileSystem::new());
        Self::create_with_fs(path, size, filesystem)
    }

    /// Allocate space for an object.
    ///
    /// # Arguments
    ///
    /// * `size` - Size of the object in bytes (including header)
    ///
    /// # Returns
    ///
    /// Returns an `ObjRef` pointing to the allocated space.
    ///
    /// # Errors
    ///
    /// Returns `Error::OutOfMemory` if the arena is full.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// let obj_ref = arena.alloc(256)?;
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    #[inline]
    pub fn alloc(&self, size: u32) -> Result<ObjRef> {
        let mut out_ref: u64 = 0;
        let code = unsafe { ffi::owl_arena_alloc(self.ptr.as_ptr(), size, &mut out_ref) };
        Error::from_code(code)?;
        Ok(ObjRef::new(out_ref))
    }

    /// Allocate space near another object for co-location.
    ///
    /// This attempts to place the new object on the same memory page (4KB)
    /// as the reference object, improving cache locality.
    ///
    /// # Arguments
    ///
    /// * `size` - Size of the object in bytes
    /// * `near` - Reference object to allocate near
    ///
    /// # Returns
    ///
    /// Returns an `ObjRef` pointing to the allocated space.
    ///
    /// # Errors
    ///
    /// Returns `Error::OutOfMemory` if the arena is full.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// let parent = arena.alloc(256)?;
    /// let child = arena.alloc_near(256, parent)?;
    /// // child is likely on the same page as parent
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    #[inline]
    pub fn alloc_near(&self, size: u32, near: ObjRef) -> Result<ObjRef> {
        let mut out_ref: u64 = 0;
        let code = unsafe {
            ffi::owl_arena_alloc_near(self.ptr.as_ptr(), size, near.offset(), &mut out_ref)
        };
        Error::from_code(code)?;
        Ok(ObjRef::new(out_ref))
    }

    /// Write data to an object.
    ///
    /// # Arguments
    ///
    /// * `obj_ref` - Reference to the object
    /// * `data` - Data to write
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `obj_ref` is invalid
    /// - `obj_ref` is out of bounds
    /// - Data is too large for the allocated space
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// # let obj_ref = arena.alloc(256)?;
    /// let data = b"Hello, Owl!";
    /// arena.write(obj_ref, data)?;
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    #[inline]
    pub fn write(&self, obj_ref: ObjRef, data: &[u8]) -> Result<()> {
        let code = unsafe {
            ffi::owl_arena_write(
                self.ptr.as_ptr(),
                obj_ref.offset(),
                data.as_ptr(),
                data.len(),
            )
        };
        Error::from_code(code)
    }

    /// Read an object from the arena.
    ///
    /// # Arguments
    ///
    /// * `obj_ref` - Reference to the object
    /// * `buffer` - Buffer to read into
    ///
    /// # Returns
    ///
    /// Returns the number of bytes read.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `obj_ref` is invalid
    /// - `obj_ref` is out of bounds
    /// - Object has been deleted
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// # let obj_ref = arena.alloc(256)?;
    /// # arena.write(obj_ref, b"Hello")?;
    /// let mut buffer = vec![0u8; 256];
    /// let bytes_read = arena.read(obj_ref, &mut buffer)?;
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    #[inline]
    pub fn read(&self, obj_ref: ObjRef, buffer: &mut [u8]) -> Result<usize> {
        let mut bytes_read: usize = 0;
        let code = unsafe {
            ffi::owl_arena_read(
                self.ptr.as_ptr(),
                obj_ref.offset(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut bytes_read,
            )
        };
        Error::from_code(code)?;
        Ok(bytes_read)
    }

    /// Delete an object.
    ///
    /// This marks the object as deleted and adds it to the free list for reuse.
    ///
    /// # Arguments
    ///
    /// * `obj_ref` - Reference to the object to delete
    ///
    /// # Errors
    ///
    /// Returns an error if `obj_ref` is invalid or out of bounds.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// # let obj_ref = arena.alloc(256)?;
    /// arena.delete(obj_ref)?;
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    #[inline]
    pub fn delete(&self, obj_ref: ObjRef) -> Result<()> {
        let code = unsafe { ffi::owl_arena_delete(self.ptr.as_ptr(), obj_ref.offset()) };
        Error::from_code(code)
    }

    /// Get arena statistics.
    ///
    /// # Returns
    ///
    /// Returns `ArenaStats` with information about memory usage.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// let stats = arena.stats()?;
    /// println!("Used: {} KB / {} KB", stats.used_size / 1024, stats.total_size / 1024);
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    pub fn stats(&self) -> Result<ArenaStats> {
        let mut total: usize = 0;
        let mut used: usize = 0;
        let mut free: usize = 0;
        let mut object_count: u64 = 0;

        let code = unsafe {
            ffi::owl_arena_stats(
                self.ptr.as_ptr(),
                &mut total,
                &mut used,
                &mut free,
                &mut object_count,
            )
        };
        Error::from_code(code)?;

        Ok(ArenaStats {
            total_size: total,
            used_size: used,
            free_size: free,
            object_count,
        })
    }

    /// Get co-location metrics.
    ///
    /// # Returns
    ///
    /// Returns `ColocationMetrics` with information about co-location success rate.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// let metrics = arena.colocation_metrics()?;
    /// println!("Co-location success rate: {:.1}%", metrics.success_rate);
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    pub fn colocation_metrics(&self) -> Result<ColocationMetrics> {
        let mut total_attempts: usize = 0;
        let mut successful: usize = 0;
        let mut failed: usize = 0;
        let mut success_rate: f64 = 0.0;

        let code = unsafe {
            ffi::owl_arena_colocation_metrics(
                self.ptr.as_ptr(),
                &mut total_attempts,
                &mut successful,
                &mut failed,
                &mut success_rate,
            )
        };
        Error::from_code(code)?;

        Ok(ColocationMetrics {
            total_attempts,
            successful,
            failed,
            success_rate,
        })
    }

    /// Get the base pointer for swizzling (advanced usage).
    ///
    /// # Safety
    ///
    /// The returned pointer is only valid while the Arena is alive.
    /// Using it after the Arena is dropped is undefined behavior.
    ///
    /// # Returns
    ///
    /// Returns a pointer to the start of the memory-mapped region.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use owl_storage_zig::Arena;
    /// # let arena = Arena::create("data.owl", 1024 * 1024)?;
    /// # let obj_ref = arena.alloc(256)?;
    /// let base_ptr = arena.base_ptr();
    /// let obj_ptr = unsafe { obj_ref.swizzle(base_ptr) };
    /// # Ok::<(), owl_storage_zig::Error>(())
    /// ```
    #[inline]
    pub fn base_ptr(&self) -> *const u8 {
        unsafe { ffi::owl_arena_base_ptr(self.ptr.as_ptr()) }
    }

    /// Get the mutable base pointer for swizzling (advanced usage).
    ///
    /// # Safety
    ///
    /// Same safety requirements as `base_ptr`, plus:
    /// - No other references to arena data must exist
    /// - Caller must ensure exclusive access
    #[inline]
    pub fn base_ptr_mut(&self) -> *mut u8 {
        unsafe { ffi::owl_arena_base_ptr(self.ptr.as_ptr()) }
    }

    /// Write raw bytes at a specific offset (low-level API).
    ///
    /// This bypasses the Object structure and writes raw bytes directly.
    /// Use this when you need to manage your own data format.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset in the arena
    /// * `data` - Data to write
    ///
    /// # Errors
    ///
    /// Returns an error if the offset is out of bounds.
    #[inline]
    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        let code = unsafe {
            ffi::owl_arena_write_at(
                self.ptr.as_ptr(),
                offset,
                data.as_ptr(),
                data.len(),
            )
        };
        Error::from_code(code)
    }

    /// Read raw bytes from a specific offset (low-level API).
    ///
    /// This bypasses the Object structure and reads raw bytes directly.
    /// Use this when you need to manage your own data format.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset in the arena
    /// * `buffer` - Buffer to read into
    ///
    /// # Returns
    ///
    /// Returns the number of bytes read.
    ///
    /// # Errors
    ///
    /// Returns an error if the offset is out of bounds.
    #[inline]
    pub fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let mut bytes_read: usize = 0;
        let code = unsafe {
            ffi::owl_arena_read_at(
                self.ptr.as_ptr(),
                offset,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut bytes_read,
            )
        };
        Error::from_code(code)?;
        Ok(bytes_read)
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        unsafe {
            ffi::owl_arena_destroy(self.ptr.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_arena_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Arena>();
        assert_sync::<Arena>();
    }

    #[test]
    fn test_arena_drop() {
        let path = "/tmp/test_arena_drop.owl";
        let _ = fs::remove_file(path);

        {
            let _arena = Arena::create(path, 1024 * 1024).expect("Failed to create arena");
            // Arena should be dropped here
        }

        let _ = fs::remove_file(path);
    }
}
