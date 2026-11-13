// Physical Operator Trait
// Defines the iterator-based execution interface for all physical operators

use crate::document::Document;
use crate::error::OQLResult;
use crate::storage::{ProjectedRowBatch, ProjectedRows, Storage};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const DEFAULT_CHUNK_ROWS: usize = 1024;

/// An owned row or column-major batch passed between physical operators.
///
/// Chunks amortize dynamic dispatch and let operators reserve output once per
/// group. Projected storage scans remain column-major until an operator needs
/// row semantics or the public collector materializes `Document` values.
#[derive(Debug)]
pub struct DataChunk {
    rows: ChunkRows,
}

#[derive(Debug)]
enum ChunkRows {
    Documents(Vec<Document>),
    Projected(ProjectedRowBatch),
}

impl Default for DataChunk {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl DataChunk {
    pub fn new(documents: Vec<Document>) -> Self {
        Self {
            rows: ChunkRows::Documents(documents),
        }
    }

    pub fn projected(batch: ProjectedRowBatch) -> Self {
        Self {
            rows: ChunkRows::Projected(batch),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            rows: ChunkRows::Documents(Vec::with_capacity(capacity)),
        }
    }

    pub fn push(&mut self, document: Document) {
        match &mut self.rows {
            ChunkRows::Documents(documents) => documents.push(document),
            ChunkRows::Projected(_) => {
                unreachable!("cannot push a document into a projected batch")
            }
        }
    }

    pub fn len(&self) -> usize {
        match &self.rows {
            ChunkRows::Documents(documents) => documents.len(),
            ChunkRows::Projected(batch) => batch.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_projected(&self) -> bool {
        matches!(self.rows, ChunkRows::Projected(_))
    }

    pub fn documents(&self) -> Option<&[Document]> {
        match &self.rows {
            ChunkRows::Documents(documents) => Some(documents),
            ChunkRows::Projected(_) => None,
        }
    }

    pub fn projected_rows(&self) -> Option<ProjectedRows<'_>> {
        match &self.rows {
            ChunkRows::Documents(_) => None,
            ChunkRows::Projected(batch) => Some(batch.rows()),
        }
    }

    pub fn truncate(&mut self, len: usize) {
        match &mut self.rows {
            ChunkRows::Documents(documents) => documents.truncate(len),
            ChunkRows::Projected(batch) => batch.truncate(len),
        }
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        match &mut self.rows {
            ChunkRows::Documents(documents) => Self::new(documents.split_off(at)),
            ChunkRows::Projected(batch) => Self::projected(batch.split_off(at)),
        }
    }

    pub fn into_documents(self) -> Vec<Document> {
        match self.rows {
            ChunkRows::Documents(documents) => documents,
            ChunkRows::Projected(batch) => batch.into_documents(),
        }
    }
}

/// Physical operator execution trait
///
/// All physical operators implement this trait to provide iterator-based execution.
/// This allows for efficient pipelined execution with minimal memory overhead.
pub trait PhysicalOperatorExec: Send + Sync {
    /// Open the operator for execution
    ///
    /// This initializes any state needed for execution (e.g., opening files,
    /// building hash tables, etc.)
    fn open(&mut self, storage: Storage) -> OQLResult<()>;

    /// Get the next document from this operator
    ///
    /// Returns:
    /// - Ok(Some(document)) if there is a next document
    /// - Ok(None) if there are no more documents
    /// - Err(error) if an error occurred
    fn next(&mut self) -> OQLResult<Option<Document>>;

    /// Produce up to `target_rows` documents at once.
    ///
    /// The default preserves compatibility for operators that have not yet
    /// been vectorized. Hot pipeline operators override this method.
    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        let target_rows = target_rows.max(1);
        let mut chunk = DataChunk::with_capacity(target_rows);
        while chunk.len() < target_rows {
            let Some(document) = self.next()? else {
                break;
            };
            chunk.push(document);
        }
        Ok((!chunk.is_empty()).then_some(chunk))
    }

    /// Close the operator and release resources
    ///
    /// This is called when execution is complete or aborted.
    fn close(&mut self) -> OQLResult<()>;

    /// Reset the operator to its initial state
    ///
    /// This allows the operator to be re-executed without recreating it.
    fn reset(&mut self) -> OQLResult<()>;

    /// Operator-specific counters copied once after execution. Implementations
    /// should keep this allocation-free on the hot path.
    fn runtime_metrics(&self) -> BTreeMap<String, u64> {
        BTreeMap::new()
    }
}

/// Helper trait for collecting all results from an operator
pub trait PhysicalOperatorCollect {
    /// Collect all documents from this operator into a Vec
    fn collect_all(&mut self) -> OQLResult<Vec<Document>>;
    fn collect_all_timed(&mut self) -> OQLResult<(Vec<Document>, Duration)>;
    fn collect_chunks(&mut self) -> OQLResult<Vec<DataChunk>>;
}

impl<T: PhysicalOperatorExec + ?Sized> PhysicalOperatorCollect for T {
    fn collect_all(&mut self) -> OQLResult<Vec<Document>> {
        let mut results = Vec::new();
        while let Some(chunk) = self.next_batch(DEFAULT_CHUNK_ROWS)? {
            results.extend(chunk.into_documents());
        }
        Ok(results)
    }

    fn collect_all_timed(&mut self) -> OQLResult<(Vec<Document>, Duration)> {
        let mut results = Vec::new();
        let mut materialization = Duration::ZERO;
        while let Some(chunk) = self.next_batch(DEFAULT_CHUNK_ROWS)? {
            let started = Instant::now();
            results.extend(chunk.into_documents());
            materialization = materialization.saturating_add(started.elapsed());
        }
        Ok((results, materialization))
    }

    fn collect_chunks(&mut self) -> OQLResult<Vec<DataChunk>> {
        let mut chunks = Vec::new();
        while let Some(chunk) = self.next_batch(DEFAULT_CHUNK_ROWS)? {
            chunks.push(chunk);
        }
        Ok(chunks)
    }
}
