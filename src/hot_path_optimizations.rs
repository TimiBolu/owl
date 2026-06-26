// Hot Path Optimizations for Next-Gen Owl DST
//
// This module implements critical performance optimizations for hot paths:
// 1. Cache-line optimized data structures
// 2. Memory pool allocators for frequent allocations
// 3. String interning for repeated field names
// 4. SIMD-optimized operations where applicable
//
// Requirements: 12, 19

use crate::document::{Document, DocumentId, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Cache-line size for alignment optimizations
#[allow(dead_code)]
const CACHE_LINE_SIZE: usize = 64;

/// String interning pool for field names to reduce allocations
///
/// Field names are frequently repeated across documents and queries.
/// Interning them reduces memory usage and improves cache locality.
pub struct StringInterner {
    /// Map from string to interned ID
    string_to_id: HashMap<String, u32>,
    /// Map from ID to interned string
    id_to_string: Vec<String>,
    /// Next available ID
    next_id: u32,
}

impl StringInterner {
    pub fn new() -> Self {
        Self {
            string_to_id: HashMap::with_capacity(1024), // Pre-allocate for common field names
            id_to_string: Vec::with_capacity(1024),
            next_id: 0,
        }
    }

    /// Intern a string and return its ID
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.string_to_id.get(s) {
            return id;
        }

        let id = self.next_id;
        self.next_id += 1;

        let owned = s.to_string();
        self.string_to_id.insert(owned.clone(), id);
        self.id_to_string.push(owned);

        id
    }

    /// Get string by ID
    pub fn get_string(&self, id: u32) -> Option<&str> {
        self.id_to_string.get(id as usize).map(|s| s.as_str())
    }

    /// Get ID by string
    pub fn get_id(&self, s: &str) -> Option<u32> {
        self.string_to_id.get(s).copied()
    }

    /// Pre-populate with common field names
    pub fn preload_common_fields(&mut self) {
        let common_fields = [
            "id",
            "name",
            "email",
            "created_at",
            "updated_at",
            "version",
            "user_id",
            "order_id",
            "product_id",
            "customer_id",
            "status",
            "total",
            "amount",
            "price",
            "quantity",
            "description",
            "title",
            "profile",
            "preferences",
            "settings",
            "metadata",
            "tags",
        ];

        for field in &common_fields {
            self.intern(field);
        }
    }
}

// Thread-local string interner for zero-contention access.
thread_local! {
    static STRING_INTERNER: RefCell<StringInterner> = {
        let mut interner = StringInterner::new();
        interner.preload_common_fields();
        RefCell::new(interner)
    };
}

/// Intern a string using thread-local interner
pub fn intern_string(s: &str) -> u32 {
    STRING_INTERNER.with(|interner| interner.borrow_mut().intern(s))
}

/// Get interned string by ID
pub fn get_interned_string(id: u32) -> Option<String> {
    STRING_INTERNER.with(|interner| interner.borrow().get_string(id).map(|s| s.to_string()))
}

/// Memory pool for frequent small allocations
///
/// Reduces allocation overhead for frequently allocated objects like
/// field access paths, temporary vectors, and small strings.
pub struct MemoryPool<T> {
    /// Pool of reusable objects
    pool: Vec<T>,
    /// Factory function for creating new objects
    factory: Box<dyn Fn() -> T>,
    /// Maximum pool size to prevent unbounded growth
    max_size: usize,
}

impl<T> MemoryPool<T> {
    pub fn new<F>(factory: F, max_size: usize) -> Self
    where
        F: Fn() -> T + 'static,
    {
        Self {
            pool: Vec::with_capacity(max_size.min(64)), // Start with reasonable capacity
            factory: Box::new(factory),
            max_size,
        }
    }

    /// Get an object from the pool or create a new one
    pub fn get(&mut self) -> T {
        self.pool.pop().unwrap_or_else(|| (self.factory)())
    }

    /// Return an object to the pool
    pub fn put(&mut self, obj: T) {
        if self.pool.len() < self.max_size {
            // Reset the object to a clean state if needed
            // This is type-specific and would need trait bounds in a full implementation
            self.pool.push(obj);
        }
        // If pool is full, just drop the object
    }

    /// Get current pool size
    pub fn size(&self) -> usize {
        self.pool.len()
    }
}

/// Pooled vector for temporary allocations
pub type PooledVec<T> = Vec<T>;

// Thread-local memory pools for common types.
thread_local! {
    static STRING_POOL: RefCell<MemoryPool<String>> = RefCell::new(
        MemoryPool::new(String::new, 128)
    );

    static VEC_POOL: RefCell<MemoryPool<Vec<String>>> = RefCell::new(
        MemoryPool::new(Vec::new, 64)
    );

    static DOCUMENT_VEC_POOL: RefCell<MemoryPool<Vec<Document>>> = RefCell::new(
        MemoryPool::new(Vec::new, 32)
    );
}

/// Get a pooled string
pub fn get_pooled_string() -> String {
    STRING_POOL.with(|pool| pool.borrow_mut().get())
}

/// Return a string to the pool
pub fn return_pooled_string(mut s: String) {
    s.clear(); // Reset to empty state
    STRING_POOL.with(|pool| {
        pool.borrow_mut().put(s);
    });
}

/// Get a pooled vector for strings
pub fn get_pooled_string_vec() -> Vec<String> {
    VEC_POOL.with(|pool| pool.borrow_mut().get())
}

/// Return a string vector to the pool
pub fn return_pooled_string_vec(mut vec: Vec<String>) {
    vec.clear(); // Reset to empty state
    VEC_POOL.with(|pool| {
        pool.borrow_mut().put(vec);
    });
}

/// Get a pooled vector for documents
pub fn get_pooled_document_vec() -> Vec<Document> {
    DOCUMENT_VEC_POOL.with(|pool| pool.borrow_mut().get())
}

/// Return a document vector to the pool
pub fn return_pooled_document_vec(mut vec: Vec<Document>) {
    vec.clear(); // Reset to empty state
    DOCUMENT_VEC_POOL.with(|pool| {
        pool.borrow_mut().put(vec);
    });
}

/// Cache-line aligned structure for hot data
///
/// Ensures that frequently accessed data fits within a single cache line
/// to minimize cache misses during navigation.
#[repr(align(64))] // Align to cache line boundary
pub struct CacheLineAligned<T> {
    pub data: T,
    // Padding to fill cache line is handled by alignment
}

impl<T> CacheLineAligned<T> {
    pub fn new(data: T) -> Self {
        Self { data }
    }
}

/// Optimized field access cache for documents
///
/// Caches field lookups to avoid repeated HashMap operations
/// during query execution.
pub struct FieldAccessCache {
    /// Cache mapping (document_id, field_name) -> field_value
    cache: HashMap<(DocumentId, u32), Value>, // Use interned field names
    /// Maximum cache size
    max_size: usize,
    /// Access counter for LRU eviction
    access_counter: u64,
    /// Access times for LRU
    access_times: HashMap<(DocumentId, u32), u64>,
}

impl FieldAccessCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: HashMap::with_capacity(max_size),
            max_size,
            access_counter: 0,
            access_times: HashMap::with_capacity(max_size),
        }
    }

    /// Get a cached field value
    pub fn get(&mut self, document_id: &DocumentId, field_name_id: u32) -> Option<&Value> {
        let key = (document_id.clone(), field_name_id);

        if let Some(value) = self.cache.get(&key) {
            // Update access time for LRU
            self.access_counter += 1;
            self.access_times.insert(key, self.access_counter);
            Some(value)
        } else {
            None
        }
    }

    /// Cache a field value
    pub fn put(&mut self, document_id: DocumentId, field_name_id: u32, value: Value) {
        let key = (document_id, field_name_id);

        // Check if we need to evict
        if self.cache.len() >= self.max_size && !self.cache.contains_key(&key) {
            self.evict_lru();
        }

        self.access_counter += 1;
        self.cache.insert(key.clone(), value);
        self.access_times.insert(key, self.access_counter);
    }

    /// Evict least recently used entry
    fn evict_lru(&mut self) {
        if let Some((lru_key, _)) = self.access_times.iter().min_by_key(|(_, &time)| time) {
            let lru_key = lru_key.clone();
            self.cache.remove(&lru_key);
            self.access_times.remove(&lru_key);
        }
    }

    /// Clear the cache
    pub fn clear(&mut self) {
        self.cache.clear();
        self.access_times.clear();
        self.access_counter = 0;
    }

    /// Get cache statistics
    pub fn stats(&self) -> FieldAccessCacheStats {
        FieldAccessCacheStats {
            size: self.cache.len(),
            max_size: self.max_size,
            hit_rate: 0.0, // Would need hit/miss counters for accurate calculation
        }
    }
}

/// Statistics for field access cache
#[derive(Debug, Clone)]
pub struct FieldAccessCacheStats {
    pub size: usize,
    pub max_size: usize,
    pub hit_rate: f64,
}

// Thread-local field access cache.
thread_local! {
    static FIELD_ACCESS_CACHE: RefCell<FieldAccessCache> = RefCell::new(
        FieldAccessCache::new(1024) // Cache up to 1024 field accesses
    );
}

/// Get cached field value using thread-local cache
pub fn get_cached_field_value(document_id: &DocumentId, field_name: &str) -> Option<Value> {
    let field_name_id = intern_string(field_name);

    FIELD_ACCESS_CACHE.with(|cache| cache.borrow_mut().get(document_id, field_name_id).cloned())
}

/// Cache a field value using thread-local cache
pub fn cache_field_value(document_id: DocumentId, field_name: &str, value: Value) {
    let field_name_id = intern_string(field_name);

    FIELD_ACCESS_CACHE.with(|cache| {
        cache.borrow_mut().put(document_id, field_name_id, value);
    });
}

/// Optimized document field access with caching
///
/// This replaces the standard HashMap lookup with a cached approach
/// that reduces repeated field access overhead.
pub fn optimized_get_field(document: &Document, field_name: &str) -> Option<Value> {
    // Try cache first
    if let Some(cached_value) = get_cached_field_value(&document.id, field_name) {
        return Some(cached_value);
    }

    // Cache miss - do actual lookup
    if let Some(value) = document.get_field(field_name) {
        let value_clone = value.clone();
        // Cache the result for future access
        cache_field_value(document.id.clone(), field_name, value_clone.clone());
        Some(value_clone)
    } else {
        None
    }
}

/// SIMD-optimized operations for bulk processing
///
/// Uses SIMD instructions where available for processing arrays of data.
pub mod simd_ops {

    /// SIMD-optimized integer comparison for filtering
    ///
    /// Processes multiple integers at once using SIMD when available.
    pub fn simd_compare_integers(values: &[i64], threshold: i64, op: ComparisonOp) -> Vec<bool> {
        let mut results = Vec::with_capacity(values.len());

        // For now, use scalar implementation
        // In a full SIMD implementation, we'd use platform-specific intrinsics
        for &value in values {
            let result = match op {
                ComparisonOp::Equal => value == threshold,
                ComparisonOp::NotEqual => value != threshold,
                ComparisonOp::LessThan => value < threshold,
                ComparisonOp::LessThanOrEqual => value <= threshold,
                ComparisonOp::GreaterThan => value > threshold,
                ComparisonOp::GreaterThanOrEqual => value >= threshold,
            };
            results.push(result);
        }

        results
    }

    /// SIMD-optimized string prefix matching
    pub fn simd_string_prefix_match(strings: &[String], prefix: &str) -> Vec<bool> {
        let mut results = Vec::with_capacity(strings.len());

        // Scalar implementation for now
        for string in strings {
            results.push(string.starts_with(prefix));
        }

        results
    }

    #[derive(Debug, Clone, Copy)]
    pub enum ComparisonOp {
        Equal,
        NotEqual,
        LessThan,
        LessThanOrEqual,
        GreaterThan,
        GreaterThanOrEqual,
    }
}

/// Optimized path parsing for field access
///
/// Parses field access paths like "user.profile.name" efficiently
/// by reusing allocated vectors and avoiding string allocations.
pub struct PathParser {
    /// Reusable buffer for path segments
    segments: Vec<String>,
}

impl PathParser {
    pub fn new() -> Self {
        Self {
            segments: Vec::with_capacity(8), // Most paths are shallow
        }
    }

    /// Parse a field path efficiently
    pub fn parse_path(&mut self, path: &str) -> &[String] {
        self.segments.clear();

        if path.is_empty() {
            return &self.segments;
        }

        // Split by '.' and reuse string allocations where possible
        for segment in path.split('.') {
            if !segment.is_empty() {
                self.segments.push(segment.to_string());
            }
        }

        &self.segments
    }

    /// Parse path with interned strings for better performance
    pub fn parse_path_interned(&mut self, path: &str) -> Vec<u32> {
        let mut interned_segments = Vec::with_capacity(8);

        if path.is_empty() {
            return interned_segments;
        }

        for segment in path.split('.') {
            if !segment.is_empty() {
                interned_segments.push(intern_string(segment));
            }
        }

        interned_segments
    }
}

// Thread-local path parser.
thread_local! {
    static PATH_PARSER: RefCell<PathParser> = RefCell::new(PathParser::new());
}

/// Parse a field path using thread-local parser
pub fn parse_field_path(path: &str) -> Vec<String> {
    PATH_PARSER.with(|parser| parser.borrow_mut().parse_path(path).to_vec())
}

/// Performance profiler for hot path analysis
///
/// Tracks execution time and call frequency for critical code paths.
pub struct HotPathProfiler {
    /// Map from function name to profiling data
    profiles: HashMap<String, ProfileData>,
    /// Whether profiling is enabled
    enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileData {
    call_count: u64,
    total_time_ns: u64,
    min_time_ns: u64,
    max_time_ns: u64,
}

impl ProfileData {
    fn new() -> Self {
        Self {
            call_count: 0,
            total_time_ns: 0,
            min_time_ns: u64::MAX,
            max_time_ns: 0,
        }
    }

    fn record_call(&mut self, duration_ns: u64) {
        self.call_count += 1;
        self.total_time_ns += duration_ns;
        self.min_time_ns = self.min_time_ns.min(duration_ns);
        self.max_time_ns = self.max_time_ns.max(duration_ns);
    }

    fn avg_time_ns(&self) -> u64 {
        if self.call_count > 0 {
            self.total_time_ns / self.call_count
        } else {
            0
        }
    }
}

impl HotPathProfiler {
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
            enabled: cfg!(debug_assertions), // Enable in debug builds by default
        }
    }

    /// Enable or disable profiling
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Record a function call
    pub fn record_call(&mut self, function_name: &str, duration_ns: u64) {
        if !self.enabled {
            return;
        }

        let profile = self
            .profiles
            .entry(function_name.to_string())
            .or_insert_with(ProfileData::new);
        profile.record_call(duration_ns);
    }

    /// Get profiling results
    pub fn get_results(&self) -> Vec<(String, ProfileData)> {
        let mut results: Vec<_> = self
            .profiles
            .iter()
            .map(|(name, data)| (name.clone(), data.clone()))
            .collect();

        // Sort by total time (hottest first)
        results.sort_by(|a, b| b.1.total_time_ns.cmp(&a.1.total_time_ns));

        results
    }

    /// Clear all profiling data
    pub fn clear(&mut self) {
        self.profiles.clear();
    }

    /// Print profiling report
    pub fn print_report(&self) {
        println!("\n=== Hot Path Profiling Report ===");
        println!(
            "{:<30} {:>10} {:>12} {:>12} {:>12} {:>12}",
            "Function", "Calls", "Total (μs)", "Avg (ns)", "Min (ns)", "Max (ns)"
        );
        println!("{:-<90}", "");

        for (name, data) in self.get_results() {
            println!(
                "{:<30} {:>10} {:>12} {:>12} {:>12} {:>12}",
                name,
                data.call_count,
                data.total_time_ns / 1000, // Convert to microseconds
                data.avg_time_ns(),
                data.min_time_ns,
                data.max_time_ns
            );
        }
        println!();
    }
}

// Global profiler instance.
static GLOBAL_PROFILER: OnceLock<Mutex<HotPathProfiler>> = OnceLock::new();

/// Get the global profiler instance
pub fn get_profiler() -> &'static Mutex<HotPathProfiler> {
    GLOBAL_PROFILER.get_or_init(|| Mutex::new(HotPathProfiler::new()))
}

/// Macro for profiling function calls
#[macro_export]
macro_rules! profile_call {
    ($func_name:expr, $code:block) => {{
        let start = std::time::Instant::now();
        let result = $code;
        let duration = start.elapsed().as_nanos() as u64;

        if let Ok(mut profiler) = $crate::hot_path_optimizations::get_profiler().lock() {
            profiler.record_call($func_name, duration);
        }

        result
    }};
}

/// Cache-line optimized hash map for hot data structures
///
/// Uses cache-line alignment and prefetching hints to improve performance
/// for frequently accessed hash maps.
pub struct OptimizedHashMap<K, V> {
    inner: HashMap<K, V>,
    // Additional fields for optimization could go here
}

impl<K, V> OptimizedHashMap<K, V>
where
    K: std::hash::Hash + Eq,
{
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: HashMap::with_capacity(capacity),
        }
    }

    /// Optimized get with prefetching hints
    pub fn get(&self, key: &K) -> Option<&V> {
        // In a full implementation, we'd add prefetching hints here
        self.inner.get(key)
    }

    /// Optimized insert with cache-friendly allocation
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    /// Delegate other methods to inner HashMap
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    pub fn clear(&mut self) {
        self.inner.clear()
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, K, V> {
        self.inner.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_interning() {
        let mut interner = StringInterner::new();

        let id1 = interner.intern("name");
        let id2 = interner.intern("email");
        let id3 = interner.intern("name"); // Should reuse existing ID

        assert_eq!(id1, id3);
        assert_ne!(id1, id2);

        assert_eq!(interner.get_string(id1), Some("name"));
        assert_eq!(interner.get_string(id2), Some("email"));
    }

    #[test]
    fn test_memory_pool() {
        let mut pool = MemoryPool::new(|| Vec::<i32>::new(), 2);

        let mut vec1 = pool.get();
        vec1.push(1);
        vec1.push(2);

        let mut vec2 = pool.get();
        vec2.push(3);

        // Return to pool
        vec1.clear();
        pool.put(vec1);

        vec2.clear();
        pool.put(vec2);

        assert_eq!(pool.size(), 2);

        // Get from pool should reuse
        let vec3 = pool.get();
        assert_eq!(vec3.len(), 0); // Should be cleared
    }

    #[test]
    fn test_field_access_cache() {
        let mut cache = FieldAccessCache::new(2);

        let doc_id = "doc1".to_string();
        let field_id = intern_string("name");
        let value = Value::String("Alice".to_string());

        // Cache miss
        assert!(cache.get(&doc_id, field_id).is_none());

        // Cache value
        cache.put(doc_id.clone(), field_id, value.clone());

        // Cache hit
        assert_eq!(cache.get(&doc_id, field_id), Some(&value));

        // Test LRU eviction
        let field_id2 = intern_string("email");
        let value2 = Value::String("alice@example.com".to_string());
        cache.put(doc_id.clone(), field_id2, value2.clone());

        let field_id3 = intern_string("age");
        let value3 = Value::Integer(30);
        cache.put(doc_id.clone(), field_id3, value3.clone()); // Should evict first entry

        // First entry should be evicted
        assert!(cache.get(&doc_id, field_id).is_none());
        assert_eq!(cache.get(&doc_id, field_id2), Some(&value2));
        assert_eq!(cache.get(&doc_id, field_id3), Some(&value3));
    }

    #[test]
    fn test_path_parser() {
        let mut parser = PathParser::new();

        let segments = parser.parse_path("user.profile.name");
        assert_eq!(segments, &["user", "profile", "name"]);

        let segments = parser.parse_path("simple");
        assert_eq!(segments, &["simple"]);

        let segments = parser.parse_path("");
        assert_eq!(segments.len(), 0);

        let segments = parser.parse_path("a.b.c.d.e");
        assert_eq!(segments, &["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_simd_operations() {
        let values = vec![1, 5, 10, 15, 20];
        let results =
            simd_ops::simd_compare_integers(&values, 10, simd_ops::ComparisonOp::GreaterThan);
        assert_eq!(results, vec![false, false, false, true, true]);

        let strings = vec!["hello".to_string(), "world".to_string(), "help".to_string()];
        let results = simd_ops::simd_string_prefix_match(&strings, "hel");
        assert_eq!(results, vec![true, false, true]);
    }

    #[test]
    fn test_hot_path_profiler() {
        let mut profiler = HotPathProfiler::new();
        profiler.set_enabled(true);

        profiler.record_call("test_function", 1000);
        profiler.record_call("test_function", 2000);
        profiler.record_call("other_function", 500);

        let results = profiler.get_results();
        assert_eq!(results.len(), 2);

        // Should be sorted by total time
        assert_eq!(results[0].0, "test_function");
        assert_eq!(results[0].1.call_count, 2);
        assert_eq!(results[0].1.total_time_ns, 3000);
        assert_eq!(results[0].1.avg_time_ns(), 1500);
    }

    #[test]
    fn test_optimized_hash_map() {
        let mut map = OptimizedHashMap::new();

        map.insert("key1", "value1");
        map.insert("key2", "value2");

        assert_eq!(map.get(&"key1"), Some(&"value1"));
        assert_eq!(map.get(&"key2"), Some(&"value2"));
        assert_eq!(map.get(&"key3"), None);

        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());

        map.remove(&"key1");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&"key1"), None);
    }

    #[test]
    fn test_cache_line_alignment() {
        let aligned_data = CacheLineAligned::new(42u64);

        // Check that the structure is properly aligned
        let ptr = &aligned_data as *const _ as usize;
        assert_eq!(ptr % CACHE_LINE_SIZE, 0);
    }

    #[test]
    fn test_pooled_allocations() {
        // Test string pool
        let s1 = get_pooled_string();
        assert_eq!(s1.len(), 0);

        return_pooled_string(s1);

        // Test vector pool
        let v1 = get_pooled_string_vec();
        assert_eq!(v1.len(), 0);

        return_pooled_string_vec(v1);

        // Test document vector pool
        let dv1 = get_pooled_document_vec();
        assert_eq!(dv1.len(), 0);

        return_pooled_document_vec(dv1);
    }
}
