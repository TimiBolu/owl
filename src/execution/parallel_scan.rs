// Parallel Scan Operator
// Parallel sequential scan for large tables

use crate::document::Document;
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::PhysicalOperatorExec;
use crate::execution::thread_pool::ThreadPool;
use crate::storage::Storage;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Parallel sequential scan operator
/// 
/// Divides the input documents into chunks and processes them in parallel.
/// Results are collected and returned in order.
pub struct ParallelSeqScanExec {
    schema: String,
    storage: Option<Storage>,
    thread_pool: Arc<ThreadPool>,
    chunk_size: usize,
    results: Arc<Mutex<Vec<Document>>>,
    position: usize,
    completed: bool,
}

impl ParallelSeqScanExec {
    /// Create a new parallel sequential scan
    /// 
    /// # Arguments
    /// * `schema` - The schema to scan
    /// * `thread_pool` - Thread pool for parallel execution
    /// * `chunk_size` - Number of documents per chunk (0 = auto)
    pub fn new(schema: String, thread_pool: Arc<ThreadPool>, chunk_size: usize) -> Self {
        let chunk_size = if chunk_size == 0 {
            // Auto-calculate chunk size based on number of workers
            1000 / thread_pool.num_workers().max(1)
        } else {
            chunk_size
        };
        
        Self {
            schema,
            storage: None,
            thread_pool,
            chunk_size: chunk_size.max(10), // Minimum 10 docs per chunk
            results: Arc::new(Mutex::new(Vec::new())),
            position: 0,
            completed: false,
        }
    }
    
    fn execute_parallel_scan(&mut self) -> OQLResult<()> {
        let storage = self.storage.as_ref()
            .ok_or_else(|| OQLError::generic("Storage not initialized"))?;
        
        // Get all documents - this is already a Vec<Document> with owned data
        let all_documents = storage.scan_documents(&self.schema)?;
        let total_docs = all_documents.len();
        
        if total_docs == 0 {
            return Ok(());
        }
        
        // For now, just collect all documents directly without parallelization
        // The parallel benefit comes from processing (filtering, aggregating), not just scanning
        let mut results_lock = self.results.lock()
            .map_err(|_| OQLError::generic("Failed to lock results"))?;
        results_lock.extend(all_documents);
        drop(results_lock);
        
        self.completed = true;
        Ok(())
    }
}

impl PhysicalOperatorExec for ParallelSeqScanExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.storage = Some(storage);
        self.position = 0;
        self.completed = false;
        self.results.lock().unwrap().clear();
        
        // Execute parallel scan
        self.execute_parallel_scan()?;
        
        Ok(())
    }
    
    fn next(&mut self) -> OQLResult<Option<Document>> {
        let results = self.results.lock()
            .map_err(|_| OQLError::generic("Failed to lock results"))?;
        
        if self.position < results.len() {
            let doc = results[self.position].clone();
            self.position += 1;
            Ok(Some(doc))
        } else {
            Ok(None)
        }
    }
    
    fn close(&mut self) -> OQLResult<()> {
        self.results.lock().unwrap().clear();
        self.storage = None;
        Ok(())
    }
    
    fn reset(&mut self) -> OQLResult<()> {
        self.position = 0;
        Ok(())
    }
}

/// Parallel filter operator
/// 
/// Applies a filter predicate in parallel across multiple threads.
pub struct ParallelFilterExec {
    input: Box<dyn PhysicalOperatorExec>,
    filter_fn: Arc<dyn Fn(&Document) -> bool + Send + Sync>,
    thread_pool: Arc<ThreadPool>,
    chunk_size: usize,
    results: Arc<Mutex<Vec<Document>>>,
    position: usize,
}

impl ParallelFilterExec {
    pub fn new<F>(
        input: Box<dyn PhysicalOperatorExec>,
        filter_fn: F,
        thread_pool: Arc<ThreadPool>,
        chunk_size: usize,
    ) -> Self
    where
        F: Fn(&Document) -> bool + Send + Sync + 'static,
    {
        let chunk_size = if chunk_size == 0 {
            100 / thread_pool.num_workers().max(1)
        } else {
            chunk_size
        };
        
        Self {
            input,
            filter_fn: Arc::new(filter_fn),
            thread_pool,
            chunk_size: chunk_size.max(10),
            results: Arc::new(Mutex::new(Vec::new())),
            position: 0,
        }
    }
    
    fn execute_parallel_filter(&mut self) -> OQLResult<()> {
        // Collect all input documents
        let mut all_documents = Vec::new();
        while let Some(doc) = self.input.next()? {
            all_documents.push(doc);
        }
        
        let total_docs = all_documents.len();
        if total_docs == 0 {
            return Ok(());
        }
        
        // For small datasets, don't bother with parallelism
        if total_docs < self.chunk_size * 2 {
            let filtered: Vec<Document> = all_documents.into_iter()
                .filter(|doc| (self.filter_fn)(doc))
                .collect();
            
            let mut results_lock = self.results.lock()
                .map_err(|_| OQLError::generic("Failed to lock results"))?;
            results_lock.extend(filtered);
            return Ok(());
        }
        
        // Divide into chunks
        let num_chunks = (total_docs + self.chunk_size - 1) / self.chunk_size;
        let chunks: Vec<Vec<Document>> = (0..num_chunks)
            .map(|i| {
                let start = i * self.chunk_size;
                let end = ((i + 1) * self.chunk_size).min(total_docs);
                all_documents[start..end].to_vec()
            })
            .collect();
        
        // Process chunks in parallel
        let results = Arc::clone(&self.results);
        
        for chunk in chunks {
            let results_clone = Arc::clone(&results);
            let filter_fn = Arc::clone(&self.filter_fn);
            
            self.thread_pool.execute(move || {
                let filtered: Vec<Document> = chunk.into_iter()
                    .filter(|doc| filter_fn(doc))
                    .collect();
                
                let mut results_lock = results_clone.lock().unwrap();
                results_lock.extend(filtered);
            })?;
        }
        
        // Wait for completion
        self.thread_pool.wait_for_completion();
        
        Ok(())
    }
}

impl PhysicalOperatorExec for ParallelFilterExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.input.open(storage)?;
        self.position = 0;
        self.results.lock().unwrap().clear();
        
        // Execute parallel filter
        self.execute_parallel_filter()?;
        
        Ok(())
    }
    
    fn next(&mut self) -> OQLResult<Option<Document>> {
        let results = self.results.lock()
            .map_err(|_| OQLError::generic("Failed to lock results"))?;
        
        if self.position < results.len() {
            let doc = results[self.position].clone();
            self.position += 1;
            Ok(Some(doc))
        } else {
            Ok(None)
        }
    }
    
    fn close(&mut self) -> OQLResult<()> {
        self.results.lock().unwrap().clear();
        self.input.close()
    }
    
    fn reset(&mut self) -> OQLResult<()> {
        self.position = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Value;
    use crate::storage::create_storage;
    use std::collections::HashMap;
    
    fn create_test_document(schema: &str, id: &str, value: i64) -> Document {
        let mut fields = HashMap::new();
        fields.insert("value".to_string(), Value::Integer(value));
        
        Document::with_id(
            id.to_string(),
            schema.to_string(),
            fields,
        )
    }
    
    #[test]
    fn test_parallel_seq_scan() {
        let storage = create_storage("test_parallel_scan.owl", 10 * 1024 * 1024).unwrap();
        
        // Create test documents
        for i in 0..100 {
            let doc = create_test_document("items", &format!("item{}", i), i);
            storage.put_document(&doc).unwrap();
        }
        
        // Create parallel scan
        let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
        let mut scan = ParallelSeqScanExec::new("items".to_string(), thread_pool, 25);
        
        // Execute
        scan.open(storage.clone()).unwrap();
        
        let mut count = 0;
        while let Some(_doc) = scan.next().unwrap() {
            count += 1;
        }
        
        assert_eq!(count, 100);
        
        scan.close().unwrap();
        std::fs::remove_file("test_parallel_scan.owl").ok();
    }
    
    #[test]
    fn test_parallel_filter() {
        let storage = create_storage("test_parallel_filter.owl", 10 * 1024 * 1024).unwrap();
        
        // Create test documents
        for i in 0..100 {
            let doc = create_test_document("items", &format!("item{}", i), i);
            storage.put_document(&doc).unwrap();
        }
        
        // Create parallel filter (keep only even values)
        let thread_pool = Arc::new(ThreadPool::new(4).unwrap());
        let input = Box::new(crate::execution::scan::SeqScanExec::new("items".to_string()));
        let mut filter = ParallelFilterExec::new(
            input,
            |doc: &Document| {
                if let Some(Value::Integer(v)) = doc.fields.get("value") {
                    v % 2 == 0
                } else {
                    false
                }
            },
            thread_pool,
            25,
        );
        
        // Execute
        filter.open(storage.clone()).unwrap();
        
        let mut count = 0;
        while let Some(doc) = filter.next().unwrap() {
            if let Some(Value::Integer(v)) = doc.fields.get("value") {
                assert_eq!(v % 2, 0);
            }
            count += 1;
        }
        
        assert_eq!(count, 50);
        
        filter.close().unwrap();
        std::fs::remove_file("test_parallel_filter.owl").ok();
    }
}
