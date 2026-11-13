use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorMetric {
    Cosine,
    L2,
    Dot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorHit {
    pub index: u32,
    pub score: f32,
}

/// Exact Rust top-k primitive used by the query fallback path.
///
/// Vector indexing is outside the initial storage-engine scope. Keeping this
/// small exact implementation prevents that deferred feature from retaining a
/// native storage dependency.
pub fn vector_topk_f32(
    rows: &[f32],
    row_count: usize,
    dimension: usize,
    query: &[f32],
    metric: VectorMetric,
    k: usize,
) -> Result<Vec<VectorHit>, String> {
    if dimension == 0 || query.len() != dimension {
        return Err("vector dimension mismatch".to_string());
    }
    if rows.len() != row_count.saturating_mul(dimension) {
        return Err("row buffer length does not match its shape".to_string());
    }

    let query_norm = query.iter().map(|v| v * v).sum::<f32>().sqrt();
    let mut hits = Vec::with_capacity(row_count);
    for (index, row) in rows.chunks_exact(dimension).enumerate() {
        let dot = row.iter().zip(query).map(|(a, b)| a * b).sum::<f32>();
        let score = match metric {
            VectorMetric::Dot => dot,
            VectorMetric::L2 => -row
                .iter()
                .zip(query)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f32>(),
            VectorMetric::Cosine => {
                let row_norm = row.iter().map(|v| v * v).sum::<f32>().sqrt();
                if row_norm == 0.0 || query_norm == 0.0 {
                    0.0
                } else {
                    dot / (row_norm * query_norm)
                }
            }
        };
        hits.push(VectorHit {
            index: index as u32,
            score,
        });
    }
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    hits.truncate(k.min(hits.len()));
    Ok(hits)
}
