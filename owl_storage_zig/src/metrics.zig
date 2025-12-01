// Metrics Collection for Zig Storage Layer
// Provides performance counters and statistics for storage operations

const std = @import("std");
const Atomic = std.atomic.Value;

/// Storage operation metrics
pub const StorageMetrics = struct {
    // Allocation metrics
    allocations_total: Atomic(u64),
    allocations_failed: Atomic(u64),
    bytes_allocated: Atomic(u64),
    bytes_freed: Atomic(u64),

    // Navigation metrics
    navigations_local: Atomic(u64),
    navigations_remote: Atomic(u64),
    navigation_time_ns_total: Atomic(u64),

    // I/O metrics
    reads_total: Atomic(u64),
    writes_total: Atomic(u64),
    syncs_total: Atomic(u64),
    bytes_read: Atomic(u64),
    bytes_written: Atomic(u64),
    io_time_ns_total: Atomic(u64),

    // WAL metrics
    wal_appends: Atomic(u64),
    wal_syncs: Atomic(u64),
    wal_bytes_written: Atomic(u64),
    wal_replay_count: Atomic(u64),

    // B-tree metrics
    btree_inserts: Atomic(u64),
    btree_deletes: Atomic(u64),
    btree_lookups: Atomic(u64),
    btree_splits: Atomic(u64),
    btree_merges: Atomic(u64),

    // Compression metrics
    compressions_total: Atomic(u64),
    decompressions_total: Atomic(u64),
    compression_bytes_saved: Atomic(u64),
    compression_time_ns_total: Atomic(u64),

    // Memory metrics
    arena_size_bytes: Atomic(u64),
    arena_used_bytes: Atomic(u64),
    arena_fragmentation_percent: Atomic(u64),

    // Cache metrics
    cache_hits: Atomic(u64),
    cache_misses: Atomic(u64),
    cache_evictions: Atomic(u64),

    // Error metrics
    errors_total: Atomic(u64),
    corruption_detected: Atomic(u64),
    recovery_attempts: Atomic(u64),

    const Self = @This();

    /// Initialize metrics with zero values
    pub fn init() Self {
        return Self{
            .allocations_total = Atomic(u64).init(0),
            .allocations_failed = Atomic(u64).init(0),
            .bytes_allocated = Atomic(u64).init(0),
            .bytes_freed = Atomic(u64).init(0),

            .navigations_local = Atomic(u64).init(0),
            .navigations_remote = Atomic(u64).init(0),
            .navigation_time_ns_total = Atomic(u64).init(0),

            .reads_total = Atomic(u64).init(0),
            .writes_total = Atomic(u64).init(0),
            .syncs_total = Atomic(u64).init(0),
            .bytes_read = Atomic(u64).init(0),
            .bytes_written = Atomic(u64).init(0),
            .io_time_ns_total = Atomic(u64).init(0),

            .wal_appends = Atomic(u64).init(0),
            .wal_syncs = Atomic(u64).init(0),
            .wal_bytes_written = Atomic(u64).init(0),
            .wal_replay_count = Atomic(u64).init(0),

            .btree_inserts = Atomic(u64).init(0),
            .btree_deletes = Atomic(u64).init(0),
            .btree_lookups = Atomic(u64).init(0),
            .btree_splits = Atomic(u64).init(0),
            .btree_merges = Atomic(u64).init(0),

            .compressions_total = Atomic(u64).init(0),
            .decompressions_total = Atomic(u64).init(0),
            .compression_bytes_saved = Atomic(u64).init(0),
            .compression_time_ns_total = Atomic(u64).init(0),

            .arena_size_bytes = Atomic(u64).init(0),
            .arena_used_bytes = Atomic(u64).init(0),
            .arena_fragmentation_percent = Atomic(u64).init(0),

            .cache_hits = Atomic(u64).init(0),
            .cache_misses = Atomic(u64).init(0),
            .cache_evictions = Atomic(u64).init(0),

            .errors_total = Atomic(u64).init(0),
            .corruption_detected = Atomic(u64).init(0),
            .recovery_attempts = Atomic(u64).init(0),
        };
    }

    /// Record an allocation
    pub fn recordAllocation(self: *Self, bytes: u64, success: bool) void {
        if (success) {
            _ = self.allocations_total.fetchAdd(1, .monotonic);
            _ = self.bytes_allocated.fetchAdd(bytes, .monotonic);
        } else {
            _ = self.allocations_failed.fetchAdd(1, .monotonic);
        }
    }

    /// Record a deallocation
    pub fn recordDeallocation(self: *Self, bytes: u64) void {
        _ = self.bytes_freed.fetchAdd(bytes, .monotonic);
    }

    /// Record a navigation operation
    pub fn recordNavigation(self: *Self, is_local: bool, duration_ns: u64) void {
        if (is_local) {
            _ = self.navigations_local.fetchAdd(1, .monotonic);
        } else {
            _ = self.navigations_remote.fetchAdd(1, .monotonic);
        }
        _ = self.navigation_time_ns_total.fetchAdd(duration_ns, .monotonic);
    }

    /// Record an I/O read operation
    pub fn recordRead(self: *Self, bytes: u64, duration_ns: u64) void {
        _ = self.reads_total.fetchAdd(1, .monotonic);
        _ = self.bytes_read.fetchAdd(bytes, .monotonic);
        _ = self.io_time_ns_total.fetchAdd(duration_ns, .monotonic);
    }

    /// Record an I/O write operation
    pub fn recordWrite(self: *Self, bytes: u64, duration_ns: u64) void {
        _ = self.writes_total.fetchAdd(1, .monotonic);
        _ = self.bytes_written.fetchAdd(bytes, .monotonic);
        _ = self.io_time_ns_total.fetchAdd(duration_ns, .monotonic);
    }

    /// Record a sync operation
    pub fn recordSync(self: *Self, duration_ns: u64) void {
        _ = self.syncs_total.fetchAdd(1, .monotonic);
        _ = self.io_time_ns_total.fetchAdd(duration_ns, .monotonic);
    }

    /// Record a WAL append
    pub fn recordWALAppend(self: *Self, bytes: u64) void {
        _ = self.wal_appends.fetchAdd(1, .monotonic);
        _ = self.wal_bytes_written.fetchAdd(bytes, .monotonic);
    }

    /// Record a WAL sync
    pub fn recordWALSync(self: *Self) void {
        _ = self.wal_syncs.fetchAdd(1, .monotonic);
    }

    /// Record a WAL replay
    pub fn recordWALReplay(self: *Self) void {
        _ = self.wal_replay_count.fetchAdd(1, .monotonic);
    }

    /// Record a B-tree insert
    pub fn recordBTreeInsert(self: *Self) void {
        _ = self.btree_inserts.fetchAdd(1, .monotonic);
    }

    /// Record a B-tree delete
    pub fn recordBTreeDelete(self: *Self) void {
        _ = self.btree_deletes.fetchAdd(1, .monotonic);
    }

    /// Record a B-tree lookup
    pub fn recordBTreeLookup(self: *Self) void {
        _ = self.btree_lookups.fetchAdd(1, .monotonic);
    }

    /// Record a B-tree split
    pub fn recordBTreeSplit(self: *Self) void {
        _ = self.btree_splits.fetchAdd(1, .monotonic);
    }

    /// Record a B-tree merge
    pub fn recordBTreeMerge(self: *Self) void {
        _ = self.btree_merges.fetchAdd(1, .monotonic);
    }

    /// Record a compression operation
    pub fn recordCompression(self: *Self, bytes_saved: u64, duration_ns: u64) void {
        _ = self.compressions_total.fetchAdd(1, .monotonic);
        _ = self.compression_bytes_saved.fetchAdd(bytes_saved, .monotonic);
        _ = self.compression_time_ns_total.fetchAdd(duration_ns, .monotonic);
    }

    /// Record a decompression operation
    pub fn recordDecompression(self: *Self, duration_ns: u64) void {
        _ = self.decompressions_total.fetchAdd(1, .monotonic);
        _ = self.compression_time_ns_total.fetchAdd(duration_ns, .monotonic);
    }

    /// Update arena memory statistics
    pub fn updateArenaStats(self: *Self, size_bytes: u64, used_bytes: u64, fragmentation_percent: u64) void {
        self.arena_size_bytes.store(size_bytes, .monotonic);
        self.arena_used_bytes.store(used_bytes, .monotonic);
        self.arena_fragmentation_percent.store(fragmentation_percent, .monotonic);
    }

    /// Record a cache hit
    pub fn recordCacheHit(self: *Self) void {
        _ = self.cache_hits.fetchAdd(1, .monotonic);
    }

    /// Record a cache miss
    pub fn recordCacheMiss(self: *Self) void {
        _ = self.cache_misses.fetchAdd(1, .monotonic);
    }

    /// Record a cache eviction
    pub fn recordCacheEviction(self: *Self) void {
        _ = self.cache_evictions.fetchAdd(1, .monotonic);
    }

    /// Record an error
    pub fn recordError(self: *Self) void {
        _ = self.errors_total.fetchAdd(1, .monotonic);
    }

    /// Record corruption detection
    pub fn recordCorruption(self: *Self) void {
        _ = self.corruption_detected.fetchAdd(1, .monotonic);
    }

    /// Record a recovery attempt
    pub fn recordRecovery(self: *Self) void {
        _ = self.recovery_attempts.fetchAdd(1, .monotonic);
    }

    /// Get a snapshot of current metrics
    pub fn snapshot(self: *const Self) MetricsSnapshot {
        return MetricsSnapshot{
            .allocations_total = self.allocations_total.load(.monotonic),
            .allocations_failed = self.allocations_failed.load(.monotonic),
            .bytes_allocated = self.bytes_allocated.load(.monotonic),
            .bytes_freed = self.bytes_freed.load(.monotonic),

            .navigations_local = self.navigations_local.load(.monotonic),
            .navigations_remote = self.navigations_remote.load(.monotonic),
            .navigation_time_ns_total = self.navigation_time_ns_total.load(.monotonic),

            .reads_total = self.reads_total.load(.monotonic),
            .writes_total = self.writes_total.load(.monotonic),
            .syncs_total = self.syncs_total.load(.monotonic),
            .bytes_read = self.bytes_read.load(.monotonic),
            .bytes_written = self.bytes_written.load(.monotonic),
            .io_time_ns_total = self.io_time_ns_total.load(.monotonic),

            .wal_appends = self.wal_appends.load(.monotonic),
            .wal_syncs = self.wal_syncs.load(.monotonic),
            .wal_bytes_written = self.wal_bytes_written.load(.monotonic),
            .wal_replay_count = self.wal_replay_count.load(.monotonic),

            .btree_inserts = self.btree_inserts.load(.monotonic),
            .btree_deletes = self.btree_deletes.load(.monotonic),
            .btree_lookups = self.btree_lookups.load(.monotonic),
            .btree_splits = self.btree_splits.load(.monotonic),
            .btree_merges = self.btree_merges.load(.monotonic),

            .compressions_total = self.compressions_total.load(.monotonic),
            .decompressions_total = self.decompressions_total.load(.monotonic),
            .compression_bytes_saved = self.compression_bytes_saved.load(.monotonic),
            .compression_time_ns_total = self.compression_time_ns_total.load(.monotonic),

            .arena_size_bytes = self.arena_size_bytes.load(.monotonic),
            .arena_used_bytes = self.arena_used_bytes.load(.monotonic),
            .arena_fragmentation_percent = self.arena_fragmentation_percent.load(.monotonic),

            .cache_hits = self.cache_hits.load(.monotonic),
            .cache_misses = self.cache_misses.load(.monotonic),
            .cache_evictions = self.cache_evictions.load(.monotonic),

            .errors_total = self.errors_total.load(.monotonic),
            .corruption_detected = self.corruption_detected.load(.monotonic),
            .recovery_attempts = self.recovery_attempts.load(.monotonic),
        };
    }
};

/// Snapshot of metrics at a point in time
pub const MetricsSnapshot = struct {
    allocations_total: u64,
    allocations_failed: u64,
    bytes_allocated: u64,
    bytes_freed: u64,

    navigations_local: u64,
    navigations_remote: u64,
    navigation_time_ns_total: u64,

    reads_total: u64,
    writes_total: u64,
    syncs_total: u64,
    bytes_read: u64,
    bytes_written: u64,
    io_time_ns_total: u64,

    wal_appends: u64,
    wal_syncs: u64,
    wal_bytes_written: u64,
    wal_replay_count: u64,

    btree_inserts: u64,
    btree_deletes: u64,
    btree_lookups: u64,
    btree_splits: u64,
    btree_merges: u64,

    compressions_total: u64,
    decompressions_total: u64,
    compression_bytes_saved: u64,
    compression_time_ns_total: u64,

    arena_size_bytes: u64,
    arena_used_bytes: u64,
    arena_fragmentation_percent: u64,

    cache_hits: u64,
    cache_misses: u64,
    cache_evictions: u64,

    errors_total: u64,
    corruption_detected: u64,
    recovery_attempts: u64,

    /// Calculate derived metrics
    pub fn calculateDerived(self: *const MetricsSnapshot) DerivedMetrics {
        const total_navigations = self.navigations_local + self.navigations_remote;
        const avg_navigation_ns = if (total_navigations > 0)
            self.navigation_time_ns_total / total_navigations
        else
            0;

        const total_io_ops = self.reads_total + self.writes_total + self.syncs_total;
        const avg_io_ns = if (total_io_ops > 0)
            self.io_time_ns_total / total_io_ops
        else
            0;

        const total_cache_accesses = self.cache_hits + self.cache_misses;
        const cache_hit_rate = if (total_cache_accesses > 0)
            @as(f64, @floatFromInt(self.cache_hits)) / @as(f64, @floatFromInt(total_cache_accesses))
        else
            0.0;

        const compression_ratio = if (self.bytes_allocated > 0)
            @as(f64, @floatFromInt(self.compression_bytes_saved)) / @as(f64, @floatFromInt(self.bytes_allocated))
        else
            0.0;

        const arena_utilization = if (self.arena_size_bytes > 0)
            @as(f64, @floatFromInt(self.arena_used_bytes)) / @as(f64, @floatFromInt(self.arena_size_bytes))
        else
            0.0;

        return DerivedMetrics{
            .avg_navigation_ns = avg_navigation_ns,
            .avg_io_ns = avg_io_ns,
            .cache_hit_rate = cache_hit_rate,
            .compression_ratio = compression_ratio,
            .arena_utilization = arena_utilization,
        };
    }
};

/// Derived metrics calculated from raw counters
pub const DerivedMetrics = struct {
    avg_navigation_ns: u64,
    avg_io_ns: u64,
    cache_hit_rate: f64,
    compression_ratio: f64,
    arena_utilization: f64,
};

/// Global metrics instance
var global_metrics: StorageMetrics = undefined;
var metrics_initialized: bool = false;

/// Initialize global metrics
pub fn initGlobalMetrics() void {
    if (!metrics_initialized) {
        global_metrics = StorageMetrics.init();
        metrics_initialized = true;
    }
}

/// Get global metrics instance
pub fn getGlobalMetrics() *StorageMetrics {
    if (!metrics_initialized) {
        initGlobalMetrics();
    }
    return &global_metrics;
}

// C API for FFI with Rust
export fn owl_metrics_init() void {
    initGlobalMetrics();
}

export fn owl_metrics_record_allocation(bytes: u64, success: bool) void {
    getGlobalMetrics().recordAllocation(bytes, success);
}

export fn owl_metrics_record_deallocation(bytes: u64) void {
    getGlobalMetrics().recordDeallocation(bytes);
}

export fn owl_metrics_record_navigation(is_local: bool, duration_ns: u64) void {
    getGlobalMetrics().recordNavigation(is_local, duration_ns);
}

export fn owl_metrics_record_read(bytes: u64, duration_ns: u64) void {
    getGlobalMetrics().recordRead(bytes, duration_ns);
}

export fn owl_metrics_record_write(bytes: u64, duration_ns: u64) void {
    getGlobalMetrics().recordWrite(bytes, duration_ns);
}

export fn owl_metrics_record_sync(duration_ns: u64) void {
    getGlobalMetrics().recordSync(duration_ns);
}

export fn owl_metrics_record_wal_append(bytes: u64) void {
    getGlobalMetrics().recordWALAppend(bytes);
}

export fn owl_metrics_record_wal_sync() void {
    getGlobalMetrics().recordWALSync();
}

export fn owl_metrics_record_btree_insert() void {
    getGlobalMetrics().recordBTreeInsert();
}

export fn owl_metrics_record_btree_delete() void {
    getGlobalMetrics().recordBTreeDelete();
}

export fn owl_metrics_record_btree_lookup() void {
    getGlobalMetrics().recordBTreeLookup();
}

export fn owl_metrics_record_compression(bytes_saved: u64, duration_ns: u64) void {
    getGlobalMetrics().recordCompression(bytes_saved, duration_ns);
}

export fn owl_metrics_record_decompression(duration_ns: u64) void {
    getGlobalMetrics().recordDecompression(duration_ns);
}

export fn owl_metrics_update_arena_stats(size_bytes: u64, used_bytes: u64, fragmentation_percent: u64) void {
    getGlobalMetrics().updateArenaStats(size_bytes, used_bytes, fragmentation_percent);
}

export fn owl_metrics_record_cache_hit() void {
    getGlobalMetrics().recordCacheHit();
}

export fn owl_metrics_record_cache_miss() void {
    getGlobalMetrics().recordCacheMiss();
}

export fn owl_metrics_record_error() void {
    getGlobalMetrics().recordError();
}

/// Get metrics snapshot (for FFI)
export fn owl_metrics_get_snapshot(out: *MetricsSnapshot) void {
    out.* = getGlobalMetrics().snapshot();
}

test "metrics initialization" {
    initGlobalMetrics();
    const metrics = getGlobalMetrics();
    const snapshot = metrics.snapshot();

    try std.testing.expectEqual(@as(u64, 0), snapshot.allocations_total);
    try std.testing.expectEqual(@as(u64, 0), snapshot.navigations_local);
}

test "record allocation" {
    var metrics = StorageMetrics.init();

    metrics.recordAllocation(1024, true);
    metrics.recordAllocation(2048, true);
    metrics.recordAllocation(512, false);

    const snapshot = metrics.snapshot();
    try std.testing.expectEqual(@as(u64, 2), snapshot.allocations_total);
    try std.testing.expectEqual(@as(u64, 1), snapshot.allocations_failed);
    try std.testing.expectEqual(@as(u64, 3072), snapshot.bytes_allocated);
}

test "record navigation" {
    var metrics = StorageMetrics.init();

    metrics.recordNavigation(true, 45);
    metrics.recordNavigation(true, 50);
    metrics.recordNavigation(false, 900);

    const snapshot = metrics.snapshot();
    try std.testing.expectEqual(@as(u64, 2), snapshot.navigations_local);
    try std.testing.expectEqual(@as(u64, 1), snapshot.navigations_remote);
    try std.testing.expectEqual(@as(u64, 995), snapshot.navigation_time_ns_total);
}

test "derived metrics" {
    var metrics = StorageMetrics.init();

    metrics.recordNavigation(true, 100);
    metrics.recordNavigation(true, 200);
    metrics.recordCacheHit();
    metrics.recordCacheHit();
    metrics.recordCacheMiss();

    const snapshot = metrics.snapshot();
    const derived = snapshot.calculateDerived();

    try std.testing.expectEqual(@as(u64, 150), derived.avg_navigation_ns);
    try std.testing.expectApproxEqAbs(@as(f64, 0.666), derived.cache_hit_rate, 0.01);
}
