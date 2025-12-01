// Performance Profiling System
// Provides CPU profiling, memory profiling, and hot path detection

use crate::error::{OQLError, OQLResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Profile sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSample {
    pub function_name: String,
    pub file: String,
    pub line: u32,
    pub duration_ns: u64,
    pub timestamp_ms: u64,
    pub layer: String,  // "rust" or "zig"
}

/// Hot path information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotPath {
    pub function_name: String,
    pub total_time_ns: u64,
    pub call_count: u64,
    pub avg_time_ns: u64,
    pub max_time_ns: u64,
    pub min_time_ns: u64,
    pub layer: String,
}

/// Memory allocation profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocationProfile {
    pub location: String,
    pub total_bytes: u64,
    pub allocation_count: u64,
    pub avg_size: u64,
    pub max_size: u64,
    pub layer: String,
}

/// Performance profiler
pub struct Profiler {
    samples: Arc<Mutex<Vec<ProfileSample>>>,
    hot_paths: Arc<Mutex<HashMap<String, HotPath>>>,
    allocations: Arc<Mutex<HashMap<String, AllocationProfile>>>,
    enabled: Arc<Mutex<bool>>,
    start_time: Instant,
}

impl Profiler {
    /// Create a new profiler
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            hot_paths: Arc::new(Mutex::new(HashMap::new())),
            allocations: Arc::new(Mutex::new(HashMap::new())),
            enabled: Arc::new(Mutex::new(false)),
            start_time: Instant::now(),
        }
    }
    
    /// Enable profiling
    pub fn enable(&self) {
        if let Ok(mut enabled) = self.enabled.lock() {
            *enabled = true;
        }
    }
    
    /// Disable profiling
    pub fn disable(&self) {
        if let Ok(mut enabled) = self.enabled.lock() {
            *enabled = false;
        }
    }
    
    /// Check if profiling is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled.lock().map(|e| *e).unwrap_or(false)
    }
    
    /// Record a profile sample
    pub fn record_sample(&self, sample: ProfileSample) {
        if !self.is_enabled() {
            return;
        }
        
        // Add to samples
        if let Ok(mut samples) = self.samples.lock() {
            samples.push(sample.clone());
        }
        
        // Update hot paths
        if let Ok(mut hot_paths) = self.hot_paths.lock() {
            let key = format!("{}::{}", sample.layer, sample.function_name);
            
            hot_paths
                .entry(key.clone())
                .and_modify(|hp| {
                    hp.total_time_ns += sample.duration_ns;
                    hp.call_count += 1;
                    hp.avg_time_ns = hp.total_time_ns / hp.call_count;
                    hp.max_time_ns = hp.max_time_ns.max(sample.duration_ns);
                    hp.min_time_ns = hp.min_time_ns.min(sample.duration_ns);
                })
                .or_insert(HotPath {
                    function_name: sample.function_name.clone(),
                    total_time_ns: sample.duration_ns,
                    call_count: 1,
                    avg_time_ns: sample.duration_ns,
                    max_time_ns: sample.duration_ns,
                    min_time_ns: sample.duration_ns,
                    layer: sample.layer.clone(),
                });
        }
    }
    
    /// Record an allocation
    pub fn record_allocation(&self, location: String, size: u64, layer: String) {
        if !self.is_enabled() {
            return;
        }
        
        if let Ok(mut allocations) = self.allocations.lock() {
            let key = format!("{}::{}", layer, location);
            
            allocations
                .entry(key.clone())
                .and_modify(|ap| {
                    ap.total_bytes += size;
                    ap.allocation_count += 1;
                    ap.avg_size = ap.total_bytes / ap.allocation_count;
                    ap.max_size = ap.max_size.max(size);
                })
                .or_insert(AllocationProfile {
                    location: location.clone(),
                    total_bytes: size,
                    allocation_count: 1,
                    avg_size: size,
                    max_size: size,
                    layer: layer.clone(),
                });
        }
    }
    
    /// Get hot paths sorted by total time
    pub fn get_hot_paths(&self, limit: usize) -> Vec<HotPath> {
        if let Ok(hot_paths) = self.hot_paths.lock() {
            let mut paths: Vec<HotPath> = hot_paths.values().cloned().collect();
            paths.sort_by(|a, b| b.total_time_ns.cmp(&a.total_time_ns));
            paths.truncate(limit);
            paths
        } else {
            Vec::new()
        }
    }
    
    /// Get allocation profiles sorted by total bytes
    pub fn get_allocation_profiles(&self, limit: usize) -> Vec<AllocationProfile> {
        if let Ok(allocations) = self.allocations.lock() {
            let mut profiles: Vec<AllocationProfile> = allocations.values().cloned().collect();
            profiles.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));
            profiles.truncate(limit);
            profiles
        } else {
            Vec::new()
        }
    }
    
    /// Get all samples
    pub fn get_samples(&self) -> Vec<ProfileSample> {
        self.samples.lock().map(|s| s.clone()).unwrap_or_default()
    }
    
    /// Clear all profiling data
    pub fn clear(&self) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.clear();
        }
        if let Ok(mut hot_paths) = self.hot_paths.lock() {
            hot_paths.clear();
        }
        if let Ok(mut allocations) = self.allocations.lock() {
            allocations.clear();
        }
    }
    
    /// Generate profiling report
    pub fn generate_report(&self) -> ProfilingReport {
        ProfilingReport {
            duration_ms: self.start_time.elapsed().as_millis() as u64,
            total_samples: self.samples.lock().map(|s| s.len()).unwrap_or(0),
            hot_paths: self.get_hot_paths(20),
            allocation_profiles: self.get_allocation_profiles(20),
        }
    }
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Profiling report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilingReport {
    pub duration_ms: u64,
    pub total_samples: usize,
    pub hot_paths: Vec<HotPath>,
    pub allocation_profiles: Vec<AllocationProfile>,
}

/// Profiled function execution
pub struct ProfiledFunction<'a> {
    profiler: &'a Profiler,
    function_name: String,
    file: String,
    line: u32,
    layer: String,
    start: Instant,
}

impl<'a> ProfiledFunction<'a> {
    /// Create a new profiled function
    pub fn new(
        profiler: &'a Profiler,
        function_name: String,
        file: String,
        line: u32,
        layer: String,
    ) -> Self {
        Self {
            profiler,
            function_name,
            file,
            line,
            layer,
            start: Instant::now(),
        }
    }
    
    /// Finish profiling
    pub fn finish(self) {
        let duration = self.start.elapsed();
        
        let sample = ProfileSample {
            function_name: self.function_name,
            file: self.file,
            line: self.line,
            duration_ns: duration.as_nanos() as u64,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            layer: self.layer,
        };
        
        self.profiler.record_sample(sample);
    }
}

impl<'a> Drop for ProfiledFunction<'a> {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        
        let sample = ProfileSample {
            function_name: self.function_name.clone(),
            file: self.file.clone(),
            line: self.line,
            duration_ns: duration.as_nanos() as u64,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            layer: self.layer.clone(),
        };
        
        self.profiler.record_sample(sample);
    }
}

/// Macro for profiling a function
#[macro_export]
macro_rules! profile_function {
    ($profiler:expr, $name:expr) => {
        $crate::profiling::ProfiledFunction::new(
            $profiler,
            $name.to_string(),
            file!().to_string(),
            line!(),
            "rust".to_string(),
        )
    };
}

/// Performance regression detector
pub struct RegressionDetector {
    baseline: Arc<Mutex<HashMap<String, u64>>>,  // function -> avg_time_ns
    threshold_percent: f64,  // e.g., 20.0 for 20% regression
}

impl RegressionDetector {
    /// Create a new regression detector
    pub fn new(threshold_percent: f64) -> Self {
        Self {
            baseline: Arc::new(Mutex::new(HashMap::new())),
            threshold_percent,
        }
    }
    
    /// Set baseline from profiling report
    pub fn set_baseline(&self, report: &ProfilingReport) {
        if let Ok(mut baseline) = self.baseline.lock() {
            baseline.clear();
            for hot_path in &report.hot_paths {
                let key = format!("{}::{}", hot_path.layer, hot_path.function_name);
                baseline.insert(key, hot_path.avg_time_ns);
            }
        }
    }
    
    /// Check for regressions
    pub fn check_regressions(&self, report: &ProfilingReport) -> Vec<PerformanceRegression> {
        let mut regressions = Vec::new();
        
        if let Ok(baseline) = self.baseline.lock() {
            for hot_path in &report.hot_paths {
                let key = format!("{}::{}", hot_path.layer, hot_path.function_name);
                
                if let Some(&baseline_time) = baseline.get(&key) {
                    let current_time = hot_path.avg_time_ns;
                    let increase_percent = 
                        ((current_time as f64 - baseline_time as f64) / baseline_time as f64) * 100.0;
                    
                    if increase_percent > self.threshold_percent {
                        regressions.push(PerformanceRegression {
                            function_name: hot_path.function_name.clone(),
                            layer: hot_path.layer.clone(),
                            baseline_ns: baseline_time,
                            current_ns: current_time,
                            increase_percent,
                        });
                    }
                }
            }
        }
        
        regressions
    }
}

/// Performance regression information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceRegression {
    pub function_name: String,
    pub layer: String,
    pub baseline_ns: u64,
    pub current_ns: u64,
    pub increase_percent: f64,
}

// FFI for Zig profiling
#[repr(C)]
pub struct ZigProfileSample {
    function_name: *const u8,
    function_name_len: usize,
    file: *const u8,
    file_len: usize,
    line: u32,
    duration_ns: u64,
}

/// Record profile sample from Zig
#[no_mangle]
pub extern "C" fn owl_profile_record_sample(
    profiler: *mut Profiler,
    sample: *const ZigProfileSample,
) {
    if profiler.is_null() || sample.is_null() {
        return;
    }
    
    unsafe {
        let profiler_ref = &*profiler;
        let sample_ref = &*sample;
        
        let function_name = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(sample_ref.function_name, sample_ref.function_name_len)
        );
        
        let file = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(sample_ref.file, sample_ref.file_len)
        );
        
        let sample = ProfileSample {
            function_name: function_name.to_string(),
            file: file.to_string(),
            line: sample_ref.line,
            duration_ns: sample_ref.duration_ns,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            layer: "zig".to_string(),
        };
        
        profiler_ref.record_sample(sample);
    }
}

/// Record allocation from Zig
#[no_mangle]
pub extern "C" fn owl_profile_record_allocation(
    profiler: *mut Profiler,
    location: *const u8,
    location_len: usize,
    size: u64,
) {
    if profiler.is_null() || location.is_null() {
        return;
    }
    
    unsafe {
        let profiler_ref = &*profiler;
        
        let location_str = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts(location, location_len)
        );
        
        profiler_ref.record_allocation(location_str.to_string(), size, "zig".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_profiler() {
        let profiler = Profiler::new();
        profiler.enable();
        
        assert!(profiler.is_enabled());
        
        let sample = ProfileSample {
            function_name: "test_function".to_string(),
            file: "test.rs".to_string(),
            line: 42,
            duration_ns: 1000,
            timestamp_ms: 0,
            layer: "rust".to_string(),
        };
        
        profiler.record_sample(sample);
        
        let hot_paths = profiler.get_hot_paths(10);
        assert_eq!(hot_paths.len(), 1);
        assert_eq!(hot_paths[0].function_name, "test_function");
    }
    
    #[test]
    fn test_allocation_profiling() {
        let profiler = Profiler::new();
        profiler.enable();
        
        profiler.record_allocation("arena::allocate".to_string(), 1024, "zig".to_string());
        profiler.record_allocation("arena::allocate".to_string(), 2048, "zig".to_string());
        
        let profiles = profiler.get_allocation_profiles(10);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].total_bytes, 3072);
        assert_eq!(profiles[0].allocation_count, 2);
    }
    
    #[test]
    fn test_regression_detector() {
        let detector = RegressionDetector::new(20.0);
        
        let baseline_report = ProfilingReport {
            duration_ms: 1000,
            total_samples: 100,
            hot_paths: vec![HotPath {
                function_name: "test_func".to_string(),
                total_time_ns: 100000,
                call_count: 100,
                avg_time_ns: 1000,
                max_time_ns: 2000,
                min_time_ns: 500,
                layer: "rust".to_string(),
            }],
            allocation_profiles: vec![],
        };
        
        detector.set_baseline(&baseline_report);
        
        let current_report = ProfilingReport {
            duration_ms: 1000,
            total_samples: 100,
            hot_paths: vec![HotPath {
                function_name: "test_func".to_string(),
                total_time_ns: 150000,
                call_count: 100,
                avg_time_ns: 1500,  // 50% increase
                max_time_ns: 3000,
                min_time_ns: 750,
                layer: "rust".to_string(),
            }],
            allocation_profiles: vec![],
        };
        
        let regressions = detector.check_regressions(&current_report);
        assert_eq!(regressions.len(), 1);
        assert!(regressions[0].increase_percent > 20.0);
    }
}
