use crate::error::{Error, Result};
use crate::ffi;
use std::ffi::CString;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HnswHit {
    pub id: u64,
    pub score: f32,
}

pub struct HnswIndex {
    raw: *mut ffi::HnswIndex,
}

impl HnswIndex {
    pub fn create(dimension: u32, m: u32, ef_construction: u32) -> Result<Self> {
        if dimension == 0 {
            return Err(Error::InvalidSize);
        }
        let raw = unsafe { ffi::owl_hnsw_index_create(dimension, m, ef_construction) };
        if raw.is_null() {
            return Err(Error::StorageError);
        }
        Ok(Self { raw })
    }

    pub fn load(path: &str) -> Result<Self> {
        let c_path = CString::new(path).map_err(|_| Error::InvalidReference)?;
        let raw = unsafe { ffi::owl_hnsw_index_load(c_path.as_ptr()) };
        if raw.is_null() {
            return Err(Error::StorageError);
        }
        Ok(Self { raw })
    }

    pub fn add_vector(&mut self, id: u64, vector: &[f32]) -> Result<()> {
        let code = unsafe {
            ffi::owl_hnsw_index_add_vector(self.raw, id, vector.as_ptr(), vector.len())
        };
        Error::from_code(code)
    }

    pub fn search(&self, query: &[f32], k: usize, ef: usize) -> Result<Vec<HnswHit>> {
        if k == 0 {
            return Ok(Vec::new());
        }

        let mut out_ids = vec![0u64; k];
        let mut out_scores = vec![0f32; k];
        let mut out_count: usize = 0;
        let code = unsafe {
            ffi::owl_hnsw_index_search(
                self.raw,
                query.as_ptr(),
                query.len(),
                k,
                ef,
                out_ids.as_mut_ptr(),
                out_scores.as_mut_ptr(),
                &mut out_count,
            )
        };
        Error::from_code(code)?;

        let mut hits = Vec::with_capacity(out_count);
        for i in 0..out_count {
            hits.push(HnswHit {
                id: out_ids[i],
                score: out_scores[i],
            });
        }
        Ok(hits)
    }

    pub fn save(&self, path: &str) -> Result<()> {
        let c_path = CString::new(path).map_err(|_| Error::InvalidReference)?;
        let code = unsafe { ffi::owl_hnsw_index_save(self.raw, c_path.as_ptr()) };
        Error::from_code(code)
    }
}

impl Drop for HnswIndex {
    fn drop(&mut self) {
        unsafe { ffi::owl_hnsw_index_destroy(self.raw) };
    }
}

