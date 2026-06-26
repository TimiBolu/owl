// Standalone test for lock-free version store implementation
// This tests the core functionality without depending on the full codebase

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::ptr;
use std::thread;

// Minimal Document implementation for testing
#[derive(Debug, Clone)]
pub struct Document {
    pub id: String,
    pub schema: String,
    pub fields: HashMap<String, String>,
}

impl Document {
    pub fn new(id: String, schema: String, fields: HashMap<String, String>) -> Self {
        Self { id, schema, fields }
    }
}

// Minimal Clock trait for testing
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub struct RealClock;

impl RealClock {
    pub fn new() -> Self {
        Self
    }
}

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

// Version entry in the lock-free version chain
#[derive(Debug)]
struct VersionEntry {
    version: u64,
    document: Document,
    transaction_id: String,
    timestamp: Instant,
    next: AtomicPtr<VersionEntry>,
    prev: AtomicPtr<VersionEntry>,
    committed: AtomicBool,
    marked_for_deletion: AtomicBool,
}

impl VersionEntry {
    fn new(version: u64, document: Document, transaction_id: String, timestamp: Instant) -> Self {
        Self {
            version,
            document,
            transaction_id,
            timestamp,
            next: AtomicPtr::new(ptr::null_mut()),
            prev: AtomicPtr::new(ptr::null_mut()),
            committed: AtomicBool::new(false),
            marked_for_deletion: AtomicBool::new(false),
        }
    }
}

// Lock-free version chain for a document
#[derive(Debug)]
struct VersionChain {
    head: AtomicPtr<VersionEntry>,
    tail: AtomicPtr<VersionEntry>,
    document_key: String,
}

impl VersionChain {
    fn new(document_key: String) -> Self {
        Self {
            head: AtomicPtr::new(ptr::null_mut()),
            tail: AtomicPtr::new(ptr::null_mut()),
            document_key,
        }
    }
    
    fn add_version(&self, version_entry: *mut VersionEntry) -> Result<(), &'static str> {
        loop {
            let current_head = self.head.load(Ordering::Acquire);
            
            unsafe {
                (*version_entry).next.store(ptr::null_mut(), Ordering::Relaxed);
                (*version_entry).prev.store(current_head, Ordering::Relaxed);
                
                if !current_head.is_null() {
                    (*current_head).next.store(version_entry, Ordering::Relaxed);
                }
                
                match self.head.compare_exchange_weak(
                    current_head,
                    version_entry,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        if current_head.is_null() {
                            self.tail.store(version_entry, Ordering::Release);
                        }
                        return Ok(());
                    }
                    Err(_) => {
                        continue;
                    }
                }
            }
        }
    }
    
    fn get_latest_committed(&self) -> Option<*mut VersionEntry> {
        let mut current = self.head.load(Ordering::Acquire);
        
        while !current.is_null() {
            unsafe {
                if (*current).committed.load(Ordering::Acquire) && 
                   !(*current).marked_for_deletion.load(Ordering::Acquire) {
                    return Some(current);
                }
                
                current = (*current).prev.load(Ordering::Acquire);
            }
        }
        
        None
    }
    
    fn get_version_for_transaction(&self, transaction_id: &str) -> Option<*mut VersionEntry> {
        let mut current = self.head.load(Ordering::Acquire);
        
        while !current.is_null() {
            unsafe {
                if (*current).transaction_id == transaction_id && 
                   !(*current).marked_for_deletion.load(Ordering::Acquire) {
                    return Some(current);
                }
                
                current = (*current).prev.load(Ordering::Acquire);
            }
        }
        
        None
    }
    
    fn commit_version(&self, version: u64) -> bool {
        let mut current = self.head.load(Ordering::Acquire);
        
        while !current.is_null() {
            unsafe {
                if (*current).version == version {
                    (*current).committed.store(true, Ordering::Release);
                    return true;
                }
                
                current = (*current).prev.load(Ordering::Acquire);
            }
        }
        
        false
    }
}

// Lock-free version store for MVCC
pub struct LockFreeVersionStore {
    version_chains: HashMap<String, Arc<VersionChain>>,
    clock: Arc<dyn Clock>,
    next_version: AtomicUsize,
}

impl LockFreeVersionStore {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            version_chains: HashMap::new(),
            clock,
            next_version: AtomicUsize::new(1),
        }
    }
    
    fn get_or_create_chain(&mut self, document_key: &str) -> Arc<VersionChain> {
        if let Some(chain) = self.version_chains.get(document_key) {
            return chain.clone();
        }
        
        let new_chain = Arc::new(VersionChain::new(document_key.to_string()));
        self.version_chains.insert(document_key.to_string(), new_chain.clone());
        new_chain
    }
    
    pub fn add_version(
        &mut self,
        document_key: &str,
        document: Document,
        transaction_id: &str,
    ) -> Result<u64, &'static str> {
        let chain = self.get_or_create_chain(document_key);
        let version = self.next_version.fetch_add(1, Ordering::Relaxed) as u64;
        let timestamp = self.clock.now();
        
        let version_entry = Box::into_raw(Box::new(VersionEntry::new(
            version,
            document,
            transaction_id.to_string(),
            timestamp,
        )));
        
        chain.add_version(version_entry)?;
        
        Ok(version)
    }
    
    pub fn get_latest_version(&self, document_key: &str) -> Option<Document> {
        if let Some(chain) = self.version_chains.get(document_key) {
            if let Some(version_entry) = chain.get_latest_committed() {
                unsafe {
                    return Some((*version_entry).document.clone());
                }
            }
        }
        
        None
    }
    
    pub fn get_version_for_transaction(&self, document_key: &str, transaction_id: &str) -> Option<Document> {
        if let Some(chain) = self.version_chains.get(document_key) {
            if let Some(version_entry) = chain.get_version_for_transaction(transaction_id) {
                unsafe {
                    return Some((*version_entry).document.clone());
                }
            }
            
            if let Some(version_entry) = chain.get_latest_committed() {
                unsafe {
                    return Some((*version_entry).document.clone());
                }
            }
        }
        
        None
    }
    
    pub fn commit_version(&self, document_key: &str, version: u64) -> bool {
        if let Some(chain) = self.version_chains.get(document_key) {
            return chain.commit_version(version);
        }
        
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_store_basic_operations() {
        let clock = Arc::new(RealClock::new());
        let mut version_store = LockFreeVersionStore::new(clock);
        
        // Create test document
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), "Alice".to_string());
        let document = Document::new("user1".to_string(), "users".to_string(), fields);
        
        let document_key = "users#user1";
        let transaction_id = "txn1";
        
        // Add version
        let version = version_store.add_version(document_key, document.clone(), transaction_id).unwrap();
        assert_eq!(version, 1);
        
        // Get version for transaction (should return uncommitted version)
        let retrieved = version_store.get_version_for_transaction(document_key, transaction_id).unwrap();
        assert_eq!(retrieved.id, document.id);
        
        // Get latest version (should return None since not committed)
        let latest = version_store.get_latest_version(document_key);
        assert!(latest.is_none());
        
        // Commit version
        assert!(version_store.commit_version(document_key, version));
        
        // Now get latest version should work
        let latest = version_store.get_latest_version(document_key).unwrap();
        assert_eq!(latest.id, document.id);
    }
    
    #[test]
    fn test_mvcc_behavior() {
        let clock = Arc::new(RealClock::new());
        let mut version_store = LockFreeVersionStore::new(clock);
        
        let document_key = "users#user1";
        
        // Create multiple versions
        let mut fields1 = HashMap::new();
        fields1.insert("name".to_string(), "Alice".to_string());
        let doc1 = Document::new("user1".to_string(), "users".to_string(), fields1);
        
        let mut fields2 = HashMap::new();
        fields2.insert("name".to_string(), "Alice Updated".to_string());
        let doc2 = Document::new("user1".to_string(), "users".to_string(), fields2);
        
        // Add first version
        let version1 = version_store.add_version(document_key, doc1, "txn1").unwrap();
        version_store.commit_version(document_key, version1);
        
        // Add second version
        let version2 = version_store.add_version(document_key, doc2, "txn2").unwrap();
        
        // Transaction 2 should see its own uncommitted version
        let txn2_view = version_store.get_version_for_transaction(document_key, "txn2").unwrap();
        assert_eq!(txn2_view.fields.get("name").unwrap(), "Alice Updated");
        
        // Other transactions should see the committed version
        let latest = version_store.get_latest_version(document_key).unwrap();
        assert_eq!(latest.fields.get("name").unwrap(), "Alice");
        
        // Commit second version
        version_store.commit_version(document_key, version2);
        
        // Now latest should be the updated version
        let latest = version_store.get_latest_version(document_key).unwrap();
        assert_eq!(latest.fields.get("name").unwrap(), "Alice Updated");
    }
    
    #[test]
    fn test_concurrent_operations() {
        let clock = Arc::new(RealClock::new());
        let version_store = Arc::new(std::sync::Mutex::new(LockFreeVersionStore::new(clock)));
        
        let mut handles = Vec::new();
        
        // Spawn multiple threads adding versions concurrently
        for i in 0..5 {
            let store_clone = version_store.clone();
            let handle = thread::spawn(move || {
                for j in 0..5 {
                    let document_key = format!("doc_{}", i);
                    let transaction_id = format!("txn_{}_{}", i, j);
                    
                    let mut fields = HashMap::new();
                    fields.insert("value".to_string(), j.to_string());
                    let document = Document::new(
                        format!("doc_{}", i),
                        "test".to_string(),
                        fields,
                    );
                    
                    let mut store = store_clone.lock().unwrap();
                    if let Ok(version) = store.add_version(&document_key, document, &transaction_id) {
                        store.commit_version(&document_key, version);
                    }
                }
            });
            handles.push(handle);
        }
        
        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }
        
        // Verify that all documents have versions
        let store = version_store.lock().unwrap();
        for i in 0..5 {
            let document_key = format!("doc_{}", i);
            let latest = store.get_latest_version(&document_key);
            assert!(latest.is_some());
        }
    }
}

fn main() {
    println!("Running lock-free version store tests...");
    
    // Run basic test
    let clock = Arc::new(RealClock::new());
    let mut version_store = LockFreeVersionStore::new(clock);
    
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), "Test User".to_string());
    let document = Document::new("user1".to_string(), "users".to_string(), fields);
    
    let version = version_store.add_version("users#user1", document.clone(), "txn1").unwrap();
    println!("Added version: {}", version);
    
    let retrieved = version_store.get_version_for_transaction("users#user1", "txn1").unwrap();
    println!("Retrieved document: {:?}", retrieved);
    
    version_store.commit_version("users#user1", version);
    let latest = version_store.get_latest_version("users#user1").unwrap();
    println!("Latest committed document: {:?}", latest);
    
    println!("Lock-free version store implementation working correctly!");
}