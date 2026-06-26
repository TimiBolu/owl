use crate::error::{Error, Result};
use crate::ffi;
use crate::ObjRef;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Safe wrapper around Zig B-tree index handle.
pub struct BTreeIndex {
    ptr: NonNull<ffi::BTree>,
    _marker: PhantomData<ffi::BTree>,
}

impl BTreeIndex {
    /// Create a B-tree with a given order and shard id.
    pub fn create(order: u32, shard_id: u32) -> Result<Self> {
        let ptr = unsafe { ffi::owl_btree_create(order, shard_id) };
        let ptr = NonNull::new(ptr).ok_or(Error::StorageError)?;
        Ok(Self {
            ptr,
            _marker: PhantomData,
        })
    }

    /// Insert key -> ObjRef.
    pub fn insert(&self, key: u64, value: ObjRef) -> Result<()> {
        let code = unsafe { ffi::owl_btree_insert(self.ptr.as_ptr(), key, value.offset()) };
        Error::from_code(code)
    }

    /// Search key.
    pub fn search(&self, key: u64) -> Result<Option<ObjRef>> {
        let mut out_offset = 0u64;
        let mut out_found = false;
        let code = unsafe {
            ffi::owl_btree_search(
                self.ptr.as_ptr(),
                key,
                &mut out_offset as *mut u64,
                &mut out_found as *mut bool,
            )
        };
        Error::from_code(code)?;
        if out_found {
            Ok(Some(ObjRef::new(out_offset)))
        } else {
            Ok(None)
        }
    }

    /// Delete key.
    pub fn delete(&self, key: u64) -> Result<bool> {
        let mut out_deleted = false;
        let code =
            unsafe { ffi::owl_btree_delete(self.ptr.as_ptr(), key, &mut out_deleted as *mut bool) };
        Error::from_code(code)?;
        Ok(out_deleted)
    }

    /// Range query [start_key, end_key].
    pub fn range_query(&self, start_key: u64, end_key: u64) -> Result<Vec<ObjRef>> {
        let mut required = 0usize;
        let code = unsafe {
            ffi::owl_btree_range_query(
                self.ptr.as_ptr(),
                start_key,
                end_key,
                std::ptr::null_mut(),
                0,
                &mut required as *mut usize,
            )
        };
        Error::from_code(code)?;
        if required == 0 {
            return Ok(Vec::new());
        }

        let mut offsets = vec![0u64; required];
        let mut returned = required;
        let code = unsafe {
            ffi::owl_btree_range_query(
                self.ptr.as_ptr(),
                start_key,
                end_key,
                offsets.as_mut_ptr(),
                offsets.len(),
                &mut returned as *mut usize,
            )
        };
        Error::from_code(code)?;
        offsets.truncate(returned);
        Ok(offsets.into_iter().map(ObjRef::new).collect())
    }
}

impl Drop for BTreeIndex {
    fn drop(&mut self) {
        unsafe {
            ffi::owl_btree_destroy(self.ptr.as_ptr());
        }
    }
}

unsafe impl Send for BTreeIndex {}
unsafe impl Sync for BTreeIndex {}

