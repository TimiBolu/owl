use crate::storage_engine::ObjRef;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

#[derive(Default)]
struct ArenaState {
    bytes: Vec<u8>,
    allocations: HashMap<ObjRef, usize>,
}

/// Rust compatibility arena for the deferred distributed prototype.
///
/// The primary single-node engine does not use this allocator. Distributed WAL
/// replay currently rebuilds it in memory until that code is moved onto the
/// stable object directory and record segments.
pub(crate) struct Arena {
    state: Mutex<ArenaState>,
    capacity: usize,
}

impl Arena {
    pub(crate) fn create_with_fs<P: AsRef<Path>>(
        _path: P,
        capacity: usize,
        _filesystem: std::sync::Arc<dyn crate::simulation::filesystem::FileSystem>,
    ) -> Result<Self, String> {
        Ok(Self {
            state: Mutex::new(ArenaState {
                bytes: vec![0; 64],
                allocations: HashMap::new(),
            }),
            capacity,
        })
    }

    pub(crate) fn alloc(&self, size: u32) -> Result<ObjRef, String> {
        let mut state = self.state.lock().map_err(|_| "arena lock poisoned")?;
        let offset = state.bytes.len();
        let end = offset
            .checked_add(size as usize)
            .ok_or("arena size overflow")?;
        if end > self.capacity {
            return Err("arena capacity exceeded".to_string());
        }
        state.bytes.resize(end, 0);
        let object_ref = ObjRef::new(offset as u64);
        state.allocations.insert(object_ref, size as usize);
        Ok(object_ref)
    }

    pub(crate) fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "arena lock poisoned")?;
        let start = offset as usize;
        let end = start
            .checked_add(bytes.len())
            .ok_or("arena offset overflow")?;
        if end > state.bytes.len() {
            return Err("arena write exceeds allocation".to_string());
        }
        state.bytes[start..end].copy_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<usize, String> {
        let state = self.state.lock().map_err(|_| "arena lock poisoned")?;
        let start = offset as usize;
        if start >= state.bytes.len() {
            return Err("arena reference is out of bounds".to_string());
        }
        let count = output.len().min(state.bytes.len() - start);
        output[..count].copy_from_slice(&state.bytes[start..start + count]);
        Ok(count)
    }

    pub(crate) fn delete(&self, object_ref: ObjRef) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "arena lock poisoned".to_string())?
            .allocations
            .remove(&object_ref);
        Ok(())
    }
}
