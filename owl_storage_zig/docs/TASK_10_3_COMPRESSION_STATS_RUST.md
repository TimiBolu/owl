# Task 10.3: Compression Statistics and Tuning (Rust)

## Summary

This task involves adding compression statistics tracking and adaptive tuning in the Rust coordination layer. The Zig storage layer already provides the compression engine and statistics, so the Rust layer needs to:

1. **Expose compression statistics through FFI**
2. **Track compression ratios per object type**
3. **Monitor compression/decompression performance**
4. **Implement adaptive threshold tuning**
5. **Add compression metrics to observability system**

## Implementation Guide

### 1. FFI Bindings for Compression Stats

Add to `src/storage_engine.rs` or create `src/compression_stats.rs`:

```rust
#[repr(C)]
pub struct CompressionStats {
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub compression_count: u64,
    pub decompression_count: u64,
    pub lz4_count: u64,
    pub zstd_count: u64,
}

extern "C" {
    fn zig_get_compression_stats(arena: *const c_void) -> CompressionStats;
    fn zig_get_cache_stats(arena: *const c_void) -> CacheStats;
    fn zig_set_compression_enabled(arena: *mut c_void, enabled: bool);
}
```

### 2. Per-Type Compression Tracking

```rust
pub struct CompressionTracker {
    /// Compression ratios by schema type
    type_ratios: HashMap<u16, CompressionRatio>,
    /// Performance metrics by compression type
    performance: HashMap<CompressionType, PerformanceMetrics>,
}

pub struct CompressionRatio {
    pub compressed_size: AtomicU64,
    pub uncompressed_size: AtomicU64,
    pub sample_count: AtomicU64,
}

impl CompressionRatio {
    pub fn ratio(&self) -> f64 {
        let uncompressed = self.uncompressed_size.load(Ordering::SeqCst);
        if uncompressed == 0 {
            return 1.0;
        }
        let compressed = self.compressed_size.load(Ordering::SeqCst);
        compressed as f64 / uncompressed as f64
    }
    
    pub fn space_savings(&self) -> f64 {
        (1.0 - self.ratio()) * 100.0
    }
}
```

### 3. Performance Monitoring

```rust
pub struct PerformanceMetrics {
    pub compression_time_ns: AtomicU64,
    pub decompression_time_ns: AtomicU64,
    pub operation_count: AtomicU64,
}

impl PerformanceMetrics {
    pub fn avg_compression_time(&self) -> f64 {
        let total = self.compression_time_ns.load(Ordering::SeqCst);
        let count = self.operation_count.load(Ordering::SeqCst);
        if count == 0 {
            return 0.0;
        }
        total as f64 / count as f64
    }
}
```

### 4. Adaptive Threshold Tuning

```rust
pub struct AdaptiveCompressionTuner {
    /// Current compression thresholds
    lz4_threshold: AtomicU32,
    zstd_threshold: AtomicU32,
    
    /// Tuning parameters
    target_compression_ratio: f64,
    max_compression_time_ns: u64,
}

impl AdaptiveCompressionTuner {
    pub fn tune(&self, stats: &CompressionStats, performance: &PerformanceMetrics) {
        // Adjust thresholds based on observed performance
        let avg_time = performance.avg_compression_time();
        
        if avg_time > self.max_compression_time_ns as f64 {
            // Compression is too slow - increase threshold
            let current = self.lz4_threshold.load(Ordering::SeqCst);
            self.lz4_threshold.store(current + 512, Ordering::SeqCst);
        }
        
        // Similar logic for compression ratio targets
        let ratio = stats.compressed_bytes as f64 / stats.uncompressed_bytes as f64;
        if ratio > self.target_compression_ratio {
            // Not compressing enough - decrease threshold
            let current = self.lz4_threshold.load(Ordering::SeqCst);
            if current > 512 {
                self.lz4_threshold.store(current - 512, Ordering::SeqCst);
            }
        }
    }
}
```

### 5. Observability Integration

Add to Prometheus metrics:

```rust
pub struct CompressionMetrics {
    // Compression ratios
    compression_ratio: Gauge,
    space_savings_percent: Gauge,
    
    // Performance
    compression_time_seconds: Histogram,
    decompression_time_seconds: Histogram,
    
    // Cache metrics
    cache_hit_rate: Gauge,
    cache_size_bytes: Gauge,
    cache_evictions_total: Counter,
    
    // Per-type metrics
    type_compression_ratio: GaugeVec, // Labels: type_tag
    type_space_savings: GaugeVec,     // Labels: type_tag
}

impl CompressionMetrics {
    pub fn update(&self, stats: &CompressionStats, cache_stats: &CacheStats) {
        // Update compression ratio
        let ratio = stats.compressed_bytes as f64 / stats.uncompressed_bytes as f64;
        self.compression_ratio.set(ratio);
        self.space_savings_percent.set((1.0 - ratio) * 100.0);
        
        // Update cache metrics
        let hit_rate = cache_stats.hits as f64 / (cache_stats.hits + cache_stats.misses) as f64;
        self.cache_hit_rate.set(hit_rate * 100.0);
        self.cache_size_bytes.set(cache_stats.current_size as f64);
        self.cache_evictions_total.inc_by(cache_stats.evictions);
    }
}
```

### 6. Integration with Storage Engine

```rust
impl StorageEngine {
    pub fn compression_stats(&self) -> CompressionStats {
        unsafe { zig_get_compression_stats(self.arena_ptr) }
    }
    
    pub fn cache_stats(&self) -> CacheStats {
        unsafe { zig_get_cache_stats(self.arena_ptr) }
    }
    
    pub fn set_compression_enabled(&mut self, enabled: bool) {
        unsafe { zig_set_compression_enabled(self.arena_ptr, enabled) }
    }
    
    pub fn tune_compression(&mut self) {
        let stats = self.compression_stats();
        let performance = self.compression_performance();
        self.tuner.tune(&stats, &performance);
    }
}
```

## Testing

Add tests in `tests/compression_stats_test.rs`:

```rust
#[test]
fn test_compression_ratio_tracking() {
    let mut engine = StorageEngine::new();
    
    // Insert objects of different sizes
    for i in 0..100 {
        let data = vec![0u8; 1024 * (i + 1)];
        engine.write_compressed(ObjRef::new(i), &data).unwrap();
    }
    
    let stats = engine.compression_stats();
    assert!(stats.compression_count > 0);
    assert!(stats.compressed_bytes < stats.uncompressed_bytes);
}

#[test]
fn test_adaptive_tuning() {
    let mut engine = StorageEngine::new();
    let tuner = AdaptiveCompressionTuner::new();
    
    // Simulate slow compression
    let stats = CompressionStats {
        compressed_bytes: 1000,
        uncompressed_bytes: 2000,
        compression_count: 10,
        decompression_count: 5,
        lz4_count: 8,
        zstd_count: 2,
    };
    
    let performance = PerformanceMetrics {
        compression_time_ns: AtomicU64::new(10_000_000), // 10ms avg
        decompression_time_ns: AtomicU64::new(1_000_000),
        operation_count: AtomicU64::new(10),
    };
    
    tuner.tune(&stats, &performance);
    
    // Threshold should increase due to slow compression
    assert!(tuner.lz4_threshold.load(Ordering::SeqCst) > 4096);
}
```

## Metrics Dashboard

Create Grafana dashboard with:

1. **Compression Ratio Over Time** - Line graph showing compression effectiveness
2. **Space Savings** - Gauge showing percentage of space saved
3. **Cache Hit Rate** - Gauge showing decompression cache effectiveness
4. **Compression Performance** - Histogram of compression/decompression times
5. **Per-Type Compression** - Table showing compression ratios by object type
6. **Threshold Tuning** - Line graph showing adaptive threshold changes

## Status

- [x] Zig compression engine implemented (Task 10)
- [x] Zig decompression cache implemented (Task 10.1)
- [x] Zig compression-aware navigation implemented (Task 10.2)
- [ ] Rust FFI bindings for compression stats
- [ ] Rust per-type compression tracking
- [ ] Rust performance monitoring
- [ ] Rust adaptive threshold tuning
- [ ] Rust observability integration
- [ ] Rust tests for compression statistics

## Notes

The Zig layer provides all the core compression functionality. The Rust layer's role is to:
- Expose statistics through FFI
- Track higher-level metrics (per-type, performance)
- Implement adaptive tuning based on observed behavior
- Integrate with the observability system (Prometheus/Grafana)

This separation allows the hot path (compression/decompression) to stay in Zig for performance, while the coordination and monitoring logic lives in Rust.
