// Physical Operator Trait
// Defines the iterator-based execution interface for all physical operators

use crate::document::Document;
use crate::error::OQLResult;
use crate::storage::Storage;

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

    /// Close the operator and release resources
    ///
    /// This is called when execution is complete or aborted.
    fn close(&mut self) -> OQLResult<()>;

    /// Reset the operator to its initial state
    ///
    /// This allows the operator to be re-executed without recreating it.
    fn reset(&mut self) -> OQLResult<()>;
}

/// Helper trait for collecting all results from an operator
pub trait PhysicalOperatorCollect {
    /// Collect all documents from this operator into a Vec
    fn collect_all(&mut self) -> OQLResult<Vec<Document>>;
}

impl<T: PhysicalOperatorExec + ?Sized> PhysicalOperatorCollect for T {
    fn collect_all(&mut self) -> OQLResult<Vec<Document>> {
        let mut results = Vec::new();
        while let Some(doc) = self.next()? {
            results.push(doc);
        }
        Ok(results)
    }
}
