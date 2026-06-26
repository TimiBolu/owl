use crate::error::{Error, Result};
use crate::ffi;
use std::sync::OnceLock;

type MojoTopkFn = unsafe extern "C" fn(
    vectors_ptr: *const f32,
    vector_count: usize,
    dimension: usize,
    query_ptr: *const f32,
    metric: u32,
    k: usize,
    out_indices: *mut u32,
    out_scores: *mut f32,
    out_count: *mut usize,
) -> i32;

static MOJO_TOPK: OnceLock<Option<MojoTopkFn>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VectorMetric {
    Cosine = 0,
    L2 = 1,
    Dot = 2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorHit {
    pub index: u32,
    pub score: f32,
}

/// Exact top-k vector search over a dense f32 matrix.
///
/// `vectors` is row-major with shape `(vector_count, dimension)`.
pub fn vector_topk_f32(
    vectors: &[f32],
    vector_count: usize,
    dimension: usize,
    query: &[f32],
    metric: VectorMetric,
    k: usize,
) -> Result<Vec<VectorHit>> {
    if dimension == 0 || vector_count == 0 || k == 0 {
        return Ok(Vec::new());
    }
    if query.len() != dimension {
        return Err(Error::InvalidSize);
    }
    if vectors.len() != vector_count.saturating_mul(dimension) {
        return Err(Error::InvalidSize);
    }

    let out_cap = k.min(vector_count);
    let mut out_indices = vec![0u32; out_cap];
    let mut out_scores = vec![0.0f32; out_cap];
    let mut out_count: usize = 0;

    let code = if let Some(topk_fn) = load_mojo_kernel() {
        unsafe {
            (topk_fn)(
                vectors.as_ptr(),
                vector_count,
                dimension,
                query.as_ptr(),
                metric as u32,
                out_cap,
                out_indices.as_mut_ptr(),
                out_scores.as_mut_ptr(),
                &mut out_count,
            )
        }
    } else {
        unsafe {
            ffi::owl_vector_topk_f32(
                vectors.as_ptr(),
                vector_count,
                dimension,
                query.as_ptr(),
                metric as u32,
                out_cap,
                out_indices.as_mut_ptr(),
                out_scores.as_mut_ptr(),
                &mut out_count,
            )
        }
    };
    Error::from_code(code)?;

    let mut hits = Vec::with_capacity(out_count);
    for i in 0..out_count {
        hits.push(VectorHit {
            index: out_indices[i],
            score: out_scores[i],
        });
    }
    Ok(hits)
}

fn load_mojo_kernel() -> Option<MojoTopkFn> {
    *MOJO_TOPK
        .get_or_init(|| {
            let lib_path = std::env::var("OWL_MOJO_KERNELS_LIB").ok()?;
            let lib = unsafe { libloading::Library::new(&lib_path).ok()? };
            let leaked: &'static libloading::Library = Box::leak(Box::new(lib));
            let topk = unsafe {
                let sym: libloading::Symbol<MojoTopkFn> =
                    leaked.get(b"owl_mojo_vector_topk_f32").ok()?;
                *sym
            };
            Some(topk)
        })
}
