//! Safe Rust bindings for Owl Storage Engine (Zig implementation)
//!
//! This crate provides safe, idiomatic Rust wrappers around the Zig-based
//! Owl Storage Engine. The storage engine provides:
//!
//! - O(1) pointer-based navigation
//! - Memory-mapped file storage
//! - Co-location for cache efficiency
//! - Zero-copy object access
//!
//! # Example
//!
//! ```no_run
//! use owl_storage_zig::{Arena, ObjRef};
//!
//! // Create a 100MB arena
//! let arena = Arena::create("data.owl", 100 * 1024 * 1024)?;
//!
//! // Allocate an object
//! let obj_ref = arena.alloc(256)?;
//!
//! // Write data
//! let data = b"Hello, Owl!";
//! arena.write(obj_ref, data)?;
//!
//! // Read data back
//! let mut buffer = vec![0u8; 256];
//! let bytes_read = arena.read(obj_ref, &mut buffer)?;
//!
//! # Ok::<(), owl_storage_zig::Error>(())
//! ```

mod arena;
mod error;
mod ffi;
mod filesystem;
mod object;
mod objref;
mod stats;

pub use arena::Arena;
pub use error::{Error, Result};
pub use filesystem::{FileSystem, FileHandle, RealFileSystem};
pub use object::Object;
pub use objref::ObjRef;
pub use stats::{ArenaStats, ColocationMetrics};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_arena_lifecycle() {
        let path = "/tmp/test_arena_lifecycle.owl";
        let _ = fs::remove_file(path);

        let arena = Arena::create(path, 1024 * 1024).expect("Failed to create arena");
        drop(arena);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_object_allocation() {
        let path = "/tmp/test_object_allocation.owl";
        let _ = fs::remove_file(path);

        let arena = Arena::create(path, 1024 * 1024).expect("Failed to create arena");

        let obj1 = arena.alloc(100).expect("Failed to allocate");
        let obj2 = arena.alloc(200).expect("Failed to allocate");

        assert!(!obj1.is_null());
        assert!(!obj2.is_null());
        assert_ne!(obj1.offset(), obj2.offset());

        drop(arena);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_write_and_read() {
        let path = "/tmp/test_write_and_read.owl";
        let _ = fs::remove_file(path);

        let arena = Arena::create(path, 1024 * 1024).expect("Failed to create arena");

        // Create an object
        let id = [1u8; 16];
        let fixed = b"Hello, Owl Storage!".to_vec();
        let var_data = b"Variable data here".to_vec();
        let refs = vec![ObjRef::new(1024), ObjRef::new(2048)];

        let obj = Object::new(1, id, fixed.clone(), var_data.clone(), refs.clone());

        // Serialize the object
        let data = obj.to_bytes();

        // Allocate space and write
        let obj_ref = arena.alloc(data.len() as u32).expect("Failed to allocate");
        arena.write(obj_ref, &data).expect("Failed to write");

        // Read back
        let mut buffer = vec![0u8; data.len()];
        let bytes_read = arena.read(obj_ref, &mut buffer).expect("Failed to read");

        assert_eq!(bytes_read, data.len());

        // Deserialize and verify
        let obj2 = Object::from_bytes(&buffer).expect("Failed to deserialize");
        assert_eq!(obj2.type_tag, 1);
        assert_eq!(obj2.id, id);
        assert_eq!(obj2.fixed, fixed);
        assert_eq!(obj2.var_data, var_data);
        assert_eq!(obj2.refs.len(), 2);
        assert_eq!(obj2.refs[0].offset(), 1024);
        assert_eq!(obj2.refs[1].offset(), 2048);

        drop(arena);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_co_location() {
        let path = "/tmp/test_co_location.owl";
        let _ = fs::remove_file(path);

        let arena = Arena::create(path, 1024 * 1024).expect("Failed to create arena");

        let parent = arena.alloc(100).expect("Failed to allocate parent");
        let child = arena
            .alloc_near(100, parent)
            .expect("Failed to allocate child");

        // Check they're on the same page (4KB pages)
        assert_eq!(parent.page_id(), child.page_id());

        drop(arena);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_stats() {
        let path = "/tmp/test_stats.owl";
        let _ = fs::remove_file(path);

        let arena = Arena::create(path, 1024 * 1024).expect("Failed to create arena");

        let stats = arena.stats().expect("Failed to get stats");
        assert_eq!(stats.total_size, 1024 * 1024);
        assert_eq!(stats.used_size, 0);
        assert_eq!(stats.object_count, 0);

        let _obj = arena.alloc(100).expect("Failed to allocate");

        let stats = arena.stats().expect("Failed to get stats");
        assert!(stats.used_size > 0);
        assert_eq!(stats.object_count, 1);

        drop(arena);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_error_handling() {
        let path = "/tmp/test_error_handling.owl";
        let _ = fs::remove_file(path);

        let arena = Arena::create(path, 1000).expect("Failed to create arena");

        // Try to allocate more than arena size
        let result = arena.alloc(2000);
        assert!(result.is_err());
        assert!(matches!(result, Err(Error::OutOfMemory)));

        drop(arena);
        let _ = fs::remove_file(path);
    }
}
