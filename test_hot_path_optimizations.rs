// Simple test for hot path optimizations
//
// This tests the core optimization components without dependencies

use std::collections::HashMap;
use std::time::Instant;

/// String interning pool for field names to reduce allocations
pub struct StringInterner {
    string_to_id: HashMap<String, u32>,
    id_to_string: Vec<String>,
    next_id: u32,
}

impl StringInterner {
    pub fn new() -> Self {
        Self {
            string_to_id: HashMap::with_capacity(1024),
            id_to_string: Vec::with_capacity(1024),
            next_id: 0,
        }
    }
    
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
    
    pub fn get_string(&self, id: u32) -> Option<&str> {
        self.id_to_string.get(id as usize).map(|s| s.as_str())
    }
}

/// Memory pool for frequent small allocations
pub struct MemoryPool<T> {
    pool: Vec<T>,
    factory: Box<dyn Fn() -> T>,
    max_size: usize,
}

impl<T> MemoryPool<T> {
    pub fn new<F>(factory: F, max_size: usize) -> Self 
    where 
        F: Fn() -> T + 'static,
    {
        Self {
            pool: Vec::with_capacity(max_size.min(64)),
            factory: Box::new(factory),
            max_size,
        }
    }
    
    pub fn get(&mut self) -> T {
        self.pool.pop().unwrap_or_else(|| (self.factory)())
    }
    
    pub fn put(&mut self, obj: T) {
        if self.pool.len() < self.max_size {
            self.pool.push(obj);
        }
    }
    
    pub fn size(&self) -> usize {
        self.pool.len()
    }
}

/// Performance profiler for hot path analysis
pub struct HotPathProfiler {
    profiles: HashMap<String, ProfileData>,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct ProfileData {
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
            enabled: true,
        }
    }
    
    pub fn record_call(&mut self, function_name: &str, duration_ns: u64) {
        if !self.enabled {
            return;
        }
        
        let profile = self.profiles.entry(function_name.to_string()).or_insert_with(ProfileData::new);
        profile.record_call(duration_ns);
    }
    
    pub fn get_results(&self) -> Vec<(String, ProfileData)> {
        let mut results: Vec<_> = self.profiles.iter()
            .map(|(name, data)| (name.clone(), data.clone()))
            .collect();
        
        results.sort_by(|a, b| b.1.total_time_ns.cmp(&a.1.total_time_ns));
        results
    }
    
    pub fn print_report(&self) {
        println!("\n=== Hot Path Profiling Report ===");
        println!("{:<30} {:>10} {:>12} {:>12} {:>12} {:>12}", 
                 "Function", "Calls", "Total (μs)", "Avg (ns)", "Min (ns)", "Max (ns)");
        println!("{:-<90}", "");
        
        for (name, data) in self.get_results() {
            println!("{:<30} {:>10} {:>12} {:>12} {:>12} {:>12}",
                     name,
                     data.call_count,
                     data.total_time_ns / 1000,
                     data.avg_time_ns(),
                     data.min_time_ns,
                     data.max_time_ns);
        }
        println!();
    }
}

fn main() {
    println!("Testing Hot Path Optimizations...");
    
    // Test string interning
    println!("\n1. Testing String Interning:");
    let mut interner = StringInterner::new();
    
    let start = Instant::now();
    let id1 = interner.intern("name");
    let id2 = interner.intern("email");
    let id3 = interner.intern("name"); // Should reuse existing ID
    let duration = start.elapsed();
    
    println!("   Interned 3 strings in {:?}", duration);
    println!("   id1={}, id2={}, id3={}", id1, id2, id3);
    println!("   id1 == id3: {}", id1 == id3);
    println!("   Retrieved: '{}', '{}'", 
             interner.get_string(id1).unwrap(), 
             interner.get_string(id2).unwrap());
    
    // Test memory pool
    println!("\n2. Testing Memory Pool:");
    let mut pool = MemoryPool::new(|| Vec::<i32>::new(), 5);
    
    let start = Instant::now();
    let mut vec1 = pool.get();
    vec1.push(1);
    vec1.push(2);
    
    let mut vec2 = pool.get();
    vec2.push(3);
    
    vec1.clear();
    pool.put(vec1);
    vec2.clear();
    pool.put(vec2);
    
    let vec3 = pool.get(); // Should reuse from pool
    let duration = start.elapsed();
    
    println!("   Pool operations completed in {:?}", duration);
    println!("   Pool size: {}", pool.size());
    println!("   Reused vector length: {}", vec3.len());
    
    // Test profiler
    println!("\n3. Testing Hot Path Profiler:");
    let mut profiler = HotPathProfiler::new();
    
    // Simulate some function calls
    for i in 0..1000 {
        let start = Instant::now();
        
        // Simulate work
        let mut sum = 0;
        for j in 0..100 {
            sum += i * j;
        }
        std::hint::black_box(sum);
        
        let duration = start.elapsed().as_nanos() as u64;
        profiler.record_call("test_function", duration);
        
        if i % 2 == 0 {
            let start = Instant::now();
            let mut vec = Vec::new();
            for k in 0..50 {
                vec.push(k);
            }
            std::hint::black_box(vec);
            let duration = start.elapsed().as_nanos() as u64;
            profiler.record_call("vector_creation", duration);
        }
    }
    
    profiler.print_report();
    
    // Performance comparison
    println!("\n4. Performance Comparison:");
    
    // Standard string operations
    let start = Instant::now();
    let mut strings = Vec::new();
    for _ in 0..10000 {
        strings.push("name".to_string());
        strings.push("email".to_string());
        strings.push("status".to_string());
    }
    let standard_duration = start.elapsed();
    println!("   Standard string allocation: {:?}", standard_duration);
    
    // Interned strings
    let start = Instant::now();
    let mut interner = StringInterner::new();
    let mut ids = Vec::new();
    for _ in 0..10000 {
        ids.push(interner.intern("name"));
        ids.push(interner.intern("email"));
        ids.push(interner.intern("status"));
    }
    let interned_duration = start.elapsed();
    println!("   Interned string operations: {:?}", interned_duration);
    
    if standard_duration > interned_duration {
        let speedup = standard_duration.as_nanos() as f64 / interned_duration.as_nanos() as f64;
        println!("   Speedup: {:.2}x faster with interning", speedup);
    } else {
        println!("   Standard allocation was faster (overhead from small test size)");
    }
    
    println!("\nHot Path Optimizations Test Complete!");
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
    fn test_hot_path_profiler() {
        let mut profiler = HotPathProfiler::new();
        
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
}