use crate::error::{Error, Result};
use crate::ffi;
use std::ffi::CString;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperblockRecord {
    pub sequence: u64,
    pub arena_size: u64,
    pub wal_size: u64,
    pub document_count: u64,
    pub schema_count: u32,
    pub record_crc32: u32,
}

pub fn recover_superblock<P: AsRef<Path>>(path: P, strict: bool) -> Result<Option<SuperblockRecord>> {
    let path_str = path.as_ref().to_str().ok_or(Error::InvalidArena)?;
    let c_path = CString::new(path_str).map_err(|_| Error::InvalidArena)?;

    let mut sequence = 0u64;
    let mut arena_size = 0u64;
    let mut wal_size = 0u64;
    let mut document_count = 0u64;
    let mut schema_count = 0u32;
    let mut record_crc32 = 0u32;

    let code = unsafe {
        ffi::owl_superblock_recover(
            c_path.as_ptr(),
            strict,
            &mut sequence,
            &mut arena_size,
            &mut wal_size,
            &mut document_count,
            &mut schema_count,
            &mut record_crc32,
        )
    };

    if code == 1 {
        return Ok(Some(SuperblockRecord {
            sequence,
            arena_size,
            wal_size,
            document_count,
            schema_count,
            record_crc32,
        }));
    }
    if code == 0 {
        return Ok(None);
    }
    Error::from_code(code)?;
    Ok(None)
}

pub fn persist_superblock<P: AsRef<Path>>(
    path: P,
    sequence: u64,
    parent_crc32: u32,
    arena_size: u64,
    wal_size: u64,
    document_count: u64,
    schema_count: u32,
    initial_full: bool,
) -> Result<u32> {
    let path_str = path.as_ref().to_str().ok_or(Error::InvalidArena)?;
    let c_path = CString::new(path_str).map_err(|_| Error::InvalidArena)?;
    let mut record_crc32 = 0u32;

    let code = unsafe {
        ffi::owl_superblock_persist(
            c_path.as_ptr(),
            sequence,
            parent_crc32,
            arena_size,
            wal_size,
            document_count,
            schema_count,
            initial_full,
            &mut record_crc32,
        )
    };
    Error::from_code(code)?;
    Ok(record_crc32)
}

pub fn append_wal_framed<P: AsRef<Path>>(path: P, payload: &[u8]) -> Result<()> {
    let path_str = path.as_ref().to_str().ok_or(Error::InvalidArena)?;
    let c_path = CString::new(path_str).map_err(|_| Error::InvalidArena)?;
    let code = unsafe { ffi::owl_wal_append_framed(c_path.as_ptr(), payload.as_ptr(), payload.len()) };
    Error::from_code(code)
}
