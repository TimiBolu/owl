//! Cross-Layer Observability (Zig)
//!
//! Task 8.15: Implement cross-layer observability (Rust + Zig)
//!
//! This module provides observability infrastructure for the Zig storage layer,
//! including metrics collection, tracing, performance monitoring, and logging
//! that integrates with the Rust observability system.

const std = @import("std");
const Allocator = std.mem.Allocator;
const ArrayList = std.ArrayList;
const HashMap = std.HashMap;
const Atomic = std.atomic.Value;

/// Observability manager for Zig layer
pub const ObservabilityManager = struct {
    allocator: Allocator,
    metrics_collector: MetricsCollector,
    trace_manager: TraceManager,
    logger: ZigLogger,
    performance_monitor: PerformanceMonitor,
    enabled: bool,

    const Self = @This();

    pub fn init(allocator: Allocator) !Self {
        return Self{
            .allocator = allocator,
            .metrics_collector = try MetricsCollector.init(allocator),
            .trace_manager = try TraceManager.init(allocator),
            .logger = try ZigLogger.init(allocator),
            .performance_monitor = try PerformanceMonitor.init(allocator),
            .enabled = true,
        };
    }

    pub fn deinit(self: *Self) void {
        self.metrics_collector.deinit();
        self.trace_manager.deinit();
        self.logger.deinit();
        self.performance_monitor.deinit();
    }

    pub fn setEnabled(self: *Self, enabled: bool) void {
        self.enabled = enabled;
    }

    /// Record arena operation metrics
    pub fn recordArenaOperation(
        self: *Self,
        shard_id: u32,
        operation: ArenaOperation,
        bytes: u64,
        latency_ns: u64,
        success: bool,
    ) void {
        if (!self.enabled) return;
        self.metrics_collector.recordArenaOperation(shard_id, operation, bytes, latency_ns, success);
    }

    /// Record B-tree operation metrics
    pub fn recordBTreeOperation(
        self: *Self,
        shard_id: u32,
        operation: BTreeOperation,
        latency_ns: u64,
        success: bool,
    ) void {
        if (!self.enabled) return;
        self.metrics_collector.recordBTreeOperation(shard_id, operation, latency_ns, success);
    }

    /// Record WAL operation metrics
    pub fn recordWALOperation(
        self: *Self,
        shard_id: u32,
        operation: WALOperation,
        bytes: u64,
        latency_ns: u64,
        success: bool,
    ) void {
        if (!self.enabled) return;
        self.metrics_collector.recordWALOperation(shard_id, operation, bytes, latency_ns, success);
    }

    /// Record transaction metrics
    pub fn recordTransactionOperation(
        self: *Self,
        shard_id: u32,
        operation: TransactionOperation,
        duration_ms: f64,
        success: bool,
    ) void {
        if (!self.enabled) return;
        self.metrics_collector.recordTransactionOperation(shard_id, operation, duration_ms, success);
    }

    /// Start a trace span
    pub fn startSpan(
        self: *Self,
        trace_id: []const u8,
        span_id: []const u8,
        operation_name: []const u8,
        shard_id: u32,
    ) !void {
        if (!self.enabled) return;
        try self.trace_manager.startSpan(trace_id, span_id, operation_name, shard_id);
    }

    /// Finish a trace span
    pub fn finishSpan(self: *Self, trace_id: []const u8, span_id: []const u8) !void {
        if (!self.enabled) return;
        try self.trace_manager.finishSpan(trace_id, span_id);
    }

    /// Add tag to a span
    pub fn addSpanTag(
        self: *Self,
        trace_id: []const u8,
        span_id: []const u8,
        key: []const u8,
        value: []const u8,
    ) !void {
        if (!self.enabled) return;
        try self.trace_manager.addSpanTag(trace_id, span_id, key, value);
    }

    /// Log a message
    pub fn log(
        self: *Self,
        level: LogLevel,
        component: []const u8,
        message: []const u8,
        shard_id: u32,
        trace_id: ?[]const u8,
        span_id: ?[]const u8,
    ) void {
        if (!self.enabled) return;
        self.logger.log(level, component, message, shard_id, trace_id, span_id);
    }

    /// Record performance timing
    pub fn recordTiming(self: *Self, operation: []const u8, duration_ns: u64) void {
        if (!self.enabled) return;
        self.performance_monitor.recordTiming(operation, duration_ns);
    }

    /// Get metrics for a shard
    pub fn getMetrics(self: *Self, shard_id: u32) ShardMetrics {
        return self.metrics_collector.getMetrics(shard_id);
    }

    /// Get trace information
    pub fn getTraceInfo(self: *Self, trace_id: []const u8) ?TraceInfo {
        return self.trace_manager.getTraceInfo(trace_id);
    }
};

/// Arena operation types
pub const ArenaOperation = enum {
    Allocate,
    Deallocate,
    AllocateNear,
    Write,
    Read,
    GarbageCollect,
};

/// B-tree operation types
pub const BTreeOperation = enum {
    Insert,
    Delete,
    Lookup,
    Split,
    Merge,
    Rebalance,
};

/// WAL operation types
pub const WALOperation = enum {
    Write,
    Sync,
    Checkpoint,
    Replay,
    Truncate,
};

/// Transaction operation types
pub const TransactionOperation = enum {
    Begin,
    Commit,
    Abort,
    Prepare,
    TwoPhaseCommit,
};

/// Log levels
pub const LogLevel = enum(u8) {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
    Fatal = 5,
};

/// Metrics collector for storage operations
pub const MetricsCollector = struct {
    allocator: Allocator,
    shard_metrics: HashMap(u32, ShardMetrics, std.hash_map.AutoContext(u32), std.hash_map.default_max_load_percentage),
    mutex: std.Thread.Mutex,

    const Self = @This();

    pub fn init(allocator: Allocator) !Self {
        return Self{
            .allocator = allocator,
            .shard_metrics = HashMap(u32, ShardMetrics, std.hash_map.AutoContext(u32), std.hash_map.default_max_load_percentage).init(allocator),
            .mutex = std.Thread.Mutex{},
        };
    }

    pub fn deinit(self: *Self) void {
        self.shard_metrics.deinit();
    }

    pub fn recordArenaOperation(
        self: *Self,
        shard_id: u32,
        operation: ArenaOperation,
        bytes: u64,
        latency_ns: u64,
        success: bool,
    ) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const result = self.shard_metrics.getOrPut(shard_id) catch return;
        if (!result.found_existing) {
            result.value_ptr.* = ShardMetrics.init();
        }

        const metrics = result.value_ptr;
        metrics.arena_metrics.recordOperation(operation, bytes, latency_ns, success);
    }

    pub fn recordBTreeOperation(
        self: *Self,
        shard_id: u32,
        operation: BTreeOperation,
        latency_ns: u64,
        success: bool,
    ) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const result = self.shard_metrics.getOrPut(shard_id) catch return;
        if (!result.found_existing) {
            result.value_ptr.* = ShardMetrics.init();
        }

        const metrics = result.value_ptr;
        metrics.btree_metrics.recordOperation(operation, latency_ns, success);
    }

    pub fn recordWALOperation(
        self: *Self,
        shard_id: u32,
        operation: WALOperation,
        bytes: u64,
        latency_ns: u64,
        success: bool,
    ) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const result = self.shard_metrics.getOrPut(shard_id) catch return;
        if (!result.found_existing) {
            result.value_ptr.* = ShardMetrics.init();
        }

        const metrics = result.value_ptr;
        metrics.wal_metrics.recordOperation(operation, bytes, latency_ns, success);
    }

    pub fn recordTransactionOperation(
        self: *Self,
        shard_id: u32,
        operation: TransactionOperation,
        duration_ms: f64,
        success: bool,
    ) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const result = self.shard_metrics.getOrPut(shard_id) catch return;
        if (!result.found_existing) {
            result.value_ptr.* = ShardMetrics.init();
        }

        const metrics = result.value_ptr;
        metrics.transaction_metrics.recordOperation(operation, duration_ms, success);
    }

    pub fn getMetrics(self: *Self, shard_id: u32) ShardMetrics {
        self.mutex.lock();
        defer self.mutex.unlock();

        return self.shard_metrics.get(shard_id) orelse ShardMetrics.init();
    }
};

/// Metrics for a single shard
pub const ShardMetrics = struct {
    arena_metrics: ArenaMetrics,
    btree_metrics: BTreeMetrics,
    wal_metrics: WALMetrics,
    transaction_metrics: TransactionMetrics,
    memory_metrics: MemoryMetrics,
    io_metrics: IOMetrics,

    pub fn init() ShardMetrics {
        return ShardMetrics{
            .arena_metrics = ArenaMetrics.init(),
            .btree_metrics = BTreeMetrics.init(),
            .wal_metrics = WALMetrics.init(),
            .transaction_metrics = TransactionMetrics.init(),
            .memory_metrics = MemoryMetrics.init(),
            .io_metrics = IOMetrics.init(),
        };
    }
};

/// Arena operation metrics
pub const ArenaMetrics = struct {
    total_allocations: Atomic(u64),
    successful_allocations: Atomic(u64),
    failed_allocations: Atomic(u64),
    total_bytes_allocated: Atomic(u64),
    total_bytes_freed: Atomic(u64),
    current_usage_bytes: Atomic(u64),
    peak_usage_bytes: Atomic(u64),
    allocation_latency_sum_ns: Atomic(u64),
    allocation_count: Atomic(u64),
    colocation_attempts: Atomic(u64),
    colocation_successes: Atomic(u64),

    pub fn init() ArenaMetrics {
        return ArenaMetrics{
            .total_allocations = Atomic(u64).init(0),
            .successful_allocations = Atomic(u64).init(0),
            .failed_allocations = Atomic(u64).init(0),
            .total_bytes_allocated = Atomic(u64).init(0),
            .total_bytes_freed = Atomic(u64).init(0),
            .current_usage_bytes = Atomic(u64).init(0),
            .peak_usage_bytes = Atomic(u64).init(0),
            .allocation_latency_sum_ns = Atomic(u64).init(0),
            .allocation_count = Atomic(u64).init(0),
            .colocation_attempts = Atomic(u64).init(0),
            .colocation_successes = Atomic(u64).init(0),
        };
    }

    pub fn recordOperation(
        self: *ArenaMetrics,
        operation: ArenaOperation,
        bytes: u64,
        latency_ns: u64,
        success: bool,
    ) void {
        switch (operation) {
            .Allocate, .AllocateNear => {
                _ = self.total_allocations.fetchAdd(1, .seq_cst);
                if (success) {
                    _ = self.successful_allocations.fetchAdd(1, .seq_cst);
                    _ = self.total_bytes_allocated.fetchAdd(bytes, .seq_cst);
                    _ = self.current_usage_bytes.fetchAdd(bytes, .seq_cst);

                    // Update peak usage
                    const current = self.current_usage_bytes.load(.seq_cst);
                    const peak = self.peak_usage_bytes.load(.seq_cst);
                    if (current > peak) {
                        _ = self.peak_usage_bytes.cmpxchgWeak(peak, current, .seq_cst, .seq_cst);
                    }

                    if (operation == .AllocateNear) {
                        _ = self.colocation_attempts.fetchAdd(1, .seq_cst);
                        _ = self.colocation_successes.fetchAdd(1, .seq_cst);
                    }
                } else {
                    _ = self.failed_allocations.fetchAdd(1, .seq_cst);
                    if (operation == .AllocateNear) {
                        _ = self.colocation_attempts.fetchAdd(1, .seq_cst);
                    }
                }

                _ = self.allocation_latency_sum_ns.fetchAdd(latency_ns, .seq_cst);
                _ = self.allocation_count.fetchAdd(1, .seq_cst);
            },
            .Deallocate => {
                if (success) {
                    _ = self.total_bytes_freed.fetchAdd(bytes, .seq_cst);
                    _ = self.current_usage_bytes.fetchSub(bytes, .seq_cst);
                }
            },
            else => {},
        }
    }

    pub fn getFragmentationRatio(self: *const ArenaMetrics) f64 {
        const allocated = self.total_bytes_allocated.load(.seq_cst);
        const freed = self.total_bytes_freed.load(.seq_cst);
        const current = self.current_usage_bytes.load(.seq_cst);

        if (allocated == 0) return 0.0;

        const expected_current = allocated - freed;
        if (expected_current == 0) return 0.0;

        return @as(f64, @floatFromInt(current)) / @as(f64, @floatFromInt(expected_current));
    }

    pub fn getAverageAllocationSize(self: *const ArenaMetrics) f64 {
        const total_bytes = self.total_bytes_allocated.load(.seq_cst);
        const count = self.successful_allocations.load(.seq_cst);

        if (count == 0) return 0.0;
        return @as(f64, @floatFromInt(total_bytes)) / @as(f64, @floatFromInt(count));
    }

    pub fn getAverageAllocationLatency(self: *const ArenaMetrics) u64 {
        const total_latency = self.allocation_latency_sum_ns.load(.seq_cst);
        const count = self.allocation_count.load(.seq_cst);

        if (count == 0) return 0;
        return total_latency / count;
    }

    pub fn getColocationSuccessRate(self: *const ArenaMetrics) f64 {
        const attempts = self.colocation_attempts.load(.seq_cst);
        const successes = self.colocation_successes.load(.seq_cst);

        if (attempts == 0) return 0.0;
        return @as(f64, @floatFromInt(successes)) / @as(f64, @floatFromInt(attempts));
    }
};

/// B-tree operation metrics
pub const BTreeMetrics = struct {
    total_insertions: Atomic(u64),
    total_deletions: Atomic(u64),
    total_lookups: Atomic(u64),
    successful_lookups: Atomic(u64),
    node_splits: Atomic(u64),
    node_merges: Atomic(u64),
    tree_height: Atomic(u32),
    total_nodes: Atomic(u64),
    lookup_latency_sum_ns: Atomic(u64),
    insertion_latency_sum_ns: Atomic(u64),
    cache_hits: Atomic(u64),
    cache_misses: Atomic(u64),

    pub fn init() BTreeMetrics {
        return BTreeMetrics{
            .total_insertions = Atomic(u64).init(0),
            .total_deletions = Atomic(u64).init(0),
            .total_lookups = Atomic(u64).init(0),
            .successful_lookups = Atomic(u64).init(0),
            .node_splits = Atomic(u64).init(0),
            .node_merges = Atomic(u64).init(0),
            .tree_height = Atomic(u32).init(0),
            .total_nodes = Atomic(u64).init(0),
            .lookup_latency_sum_ns = Atomic(u64).init(0),
            .insertion_latency_sum_ns = Atomic(u64).init(0),
            .cache_hits = Atomic(u64).init(0),
            .cache_misses = Atomic(u64).init(0),
        };
    }

    pub fn recordOperation(
        self: *BTreeMetrics,
        operation: BTreeOperation,
        latency_ns: u64,
        success: bool,
    ) void {
        switch (operation) {
            .Insert => {
                _ = self.total_insertions.fetchAdd(1, .seq_cst);
                _ = self.insertion_latency_sum_ns.fetchAdd(latency_ns, .seq_cst);
            },
            .Delete => {
                _ = self.total_deletions.fetchAdd(1, .seq_cst);
            },
            .Lookup => {
                _ = self.total_lookups.fetchAdd(1, .seq_cst);
                _ = self.lookup_latency_sum_ns.fetchAdd(latency_ns, .seq_cst);
                if (success) {
                    _ = self.successful_lookups.fetchAdd(1, .seq_cst);
                }
            },
            .Split => {
                _ = self.node_splits.fetchAdd(1, .seq_cst);
            },
            .Merge => {
                _ = self.node_merges.fetchAdd(1, .seq_cst);
            },
            else => {},
        }
    }

    pub fn getAverageLookupLatency(self: *const BTreeMetrics) u64 {
        const total_latency = self.lookup_latency_sum_ns.load(.seq_cst);
        const count = self.total_lookups.load(.seq_cst);

        if (count == 0) return 0;
        return total_latency / count;
    }

    pub fn getAverageInsertionLatency(self: *const BTreeMetrics) u64 {
        const total_latency = self.insertion_latency_sum_ns.load(.seq_cst);
        const count = self.total_insertions.load(.seq_cst);

        if (count == 0) return 0;
        return total_latency / count;
    }

    pub fn getCacheHitRate(self: *const BTreeMetrics) f64 {
        const hits = self.cache_hits.load(.seq_cst);
        const misses = self.cache_misses.load(.seq_cst);
        const total = hits + misses;

        if (total == 0) return 0.0;
        return @as(f64, @floatFromInt(hits)) / @as(f64, @floatFromInt(total));
    }
};

/// WAL operation metrics
pub const WALMetrics = struct {
    total_writes: Atomic(u64),
    total_bytes_written: Atomic(u64),
    total_syncs: Atomic(u64),
    write_latency_sum_ns: Atomic(u64),
    sync_latency_sum_ns: Atomic(u64),
    current_wal_size_bytes: Atomic(u64),
    checkpoint_count: Atomic(u64),
    last_checkpoint_timestamp_ns: Atomic(u64),

    pub fn init() WALMetrics {
        return WALMetrics{
            .total_writes = Atomic(u64).init(0),
            .total_bytes_written = Atomic(u64).init(0),
            .total_syncs = Atomic(u64).init(0),
            .write_latency_sum_ns = Atomic(u64).init(0),
            .sync_latency_sum_ns = Atomic(u64).init(0),
            .current_wal_size_bytes = Atomic(u64).init(0),
            .checkpoint_count = Atomic(u64).init(0),
            .last_checkpoint_timestamp_ns = Atomic(u64).init(0),
        };
    }

    pub fn recordOperation(
        self: *WALMetrics,
        operation: WALOperation,
        bytes: u64,
        latency_ns: u64,
        success: bool,
    ) void {
        switch (operation) {
            .Write => {
                _ = self.total_writes.fetchAdd(1, .seq_cst);
                _ = self.write_latency_sum_ns.fetchAdd(latency_ns, .seq_cst);
                if (success) {
                    _ = self.total_bytes_written.fetchAdd(bytes, .seq_cst);
                    _ = self.current_wal_size_bytes.fetchAdd(bytes, .seq_cst);
                }
            },
            .Sync => {
                _ = self.total_syncs.fetchAdd(1, .seq_cst);
                _ = self.sync_latency_sum_ns.fetchAdd(latency_ns, .seq_cst);
            },
            .Checkpoint => {
                if (success) {
                    _ = self.checkpoint_count.fetchAdd(1, .seq_cst);
                    self.last_checkpoint_timestamp_ns.store(@intCast(std.time.nanoTimestamp()), .seq_cst);
                }
            },
            else => {},
        }
    }

    pub fn getAverageWriteLatency(self: *const WALMetrics) u64 {
        const total_latency = self.write_latency_sum_ns.load(.seq_cst);
        const count = self.total_writes.load(.seq_cst);

        if (count == 0) return 0;
        return total_latency / count;
    }

    pub fn getAverageSyncLatency(self: *const WALMetrics) u64 {
        const total_latency = self.sync_latency_sum_ns.load(.seq_cst);
        const count = self.total_syncs.load(.seq_cst);

        if (count == 0) return 0;
        return total_latency / count;
    }

    pub fn getWriteThroughput(self: *const WALMetrics, duration_ns: u64) f64 {
        const bytes = self.total_bytes_written.load(.seq_cst);
        if (duration_ns == 0) return 0.0;

        const duration_s = @as(f64, @floatFromInt(duration_ns)) / 1_000_000_000.0;
        return @as(f64, @floatFromInt(bytes)) / duration_s;
    }
};

/// Transaction operation metrics
pub const TransactionMetrics = struct {
    active_transactions: Atomic(u64),
    committed_transactions: Atomic(u64),
    aborted_transactions: Atomic(u64),
    transaction_duration_sum_ms: Atomic(u64),
    lock_conflicts: Atomic(u64),
    deadlocks_detected: Atomic(u64),
    two_phase_commits: Atomic(u64),
    distributed_transactions: Atomic(u64),

    pub fn init() TransactionMetrics {
        return TransactionMetrics{
            .active_transactions = Atomic(u64).init(0),
            .committed_transactions = Atomic(u64).init(0),
            .aborted_transactions = Atomic(u64).init(0),
            .transaction_duration_sum_ms = Atomic(u64).init(0),
            .lock_conflicts = Atomic(u64).init(0),
            .deadlocks_detected = Atomic(u64).init(0),
            .two_phase_commits = Atomic(u64).init(0),
            .distributed_transactions = Atomic(u64).init(0),
        };
    }

    pub fn recordOperation(
        self: *TransactionMetrics,
        operation: TransactionOperation,
        duration_ms: f64,
        success: bool,
    ) void {
        switch (operation) {
            .Begin => {
                _ = self.active_transactions.fetchAdd(1, .seq_cst);
            },
            .Commit => {
                if (success) {
                    _ = self.committed_transactions.fetchAdd(1, .seq_cst);
                    _ = self.active_transactions.fetchSub(1, .seq_cst);
                    _ = self.transaction_duration_sum_ms.fetchAdd(@as(u64, @intFromFloat(duration_ms)), .seq_cst);
                }
            },
            .Abort => {
                _ = self.aborted_transactions.fetchAdd(1, .seq_cst);
                _ = self.active_transactions.fetchSub(1, .seq_cst);
            },
            .TwoPhaseCommit => {
                _ = self.two_phase_commits.fetchAdd(1, .seq_cst);
            },
            .Prepare => {
                // Handle prepare operation
            },
        }
    }

    pub fn getAverageTransactionDuration(self: *const TransactionMetrics) f64 {
        const total_duration = self.transaction_duration_sum_ms.load(.seq_cst);
        const count = self.committed_transactions.load(.seq_cst);

        if (count == 0) return 0.0;
        return @as(f64, @floatFromInt(total_duration)) / @as(f64, @floatFromInt(count));
    }
};

/// Memory usage metrics
pub const MemoryMetrics = struct {
    total_allocated_bytes: Atomic(u64),
    peak_allocated_bytes: Atomic(u64),
    current_rss_bytes: Atomic(u64),
    gc_cycles: Atomic(u64),
    gc_time_ms: Atomic(u64),
    objects_collected: Atomic(u64),
    memory_pressure_events: Atomic(u64),

    pub fn init() MemoryMetrics {
        return MemoryMetrics{
            .total_allocated_bytes = Atomic(u64).init(0),
            .peak_allocated_bytes = Atomic(u64).init(0),
            .current_rss_bytes = Atomic(u64).init(0),
            .gc_cycles = Atomic(u64).init(0),
            .gc_time_ms = Atomic(u64).init(0),
            .objects_collected = Atomic(u64).init(0),
            .memory_pressure_events = Atomic(u64).init(0),
        };
    }
};

/// I/O operation metrics
pub const IOMetrics = struct {
    total_reads: Atomic(u64),
    total_writes: Atomic(u64),
    total_bytes_read: Atomic(u64),
    total_bytes_written: Atomic(u64),
    read_latency_sum_ns: Atomic(u64),
    write_latency_sum_ns: Atomic(u64),
    io_errors: Atomic(u64),
    cache_hits: Atomic(u64),
    cache_misses: Atomic(u64),

    pub fn init() IOMetrics {
        return IOMetrics{
            .total_reads = Atomic(u64).init(0),
            .total_writes = Atomic(u64).init(0),
            .total_bytes_read = Atomic(u64).init(0),
            .total_bytes_written = Atomic(u64).init(0),
            .read_latency_sum_ns = Atomic(u64).init(0),
            .write_latency_sum_ns = Atomic(u64).init(0),
            .io_errors = Atomic(u64).init(0),
            .cache_hits = Atomic(u64).init(0),
            .cache_misses = Atomic(u64).init(0),
        };
    }

    pub fn getAverageReadLatency(self: *const IOMetrics) u64 {
        const total_latency = self.read_latency_sum_ns.load(.seq_cst);
        const count = self.total_reads.load(.seq_cst);

        if (count == 0) return 0;
        return total_latency / count;
    }

    pub fn getAverageWriteLatency(self: *const IOMetrics) u64 {
        const total_latency = self.write_latency_sum_ns.load(.seq_cst);
        const count = self.total_writes.load(.seq_cst);

        if (count == 0) return 0;
        return total_latency / count;
    }
};

/// Trace manager for distributed tracing
pub const TraceManager = struct {
    allocator: Allocator,
    traces: HashMap([]const u8, TraceInfo, std.hash_map.StringContext, std.hash_map.default_max_load_percentage),
    mutex: std.Thread.Mutex,

    const Self = @This();

    pub fn init(allocator: Allocator) !Self {
        return Self{
            .allocator = allocator,
            .traces = HashMap([]const u8, TraceInfo, std.hash_map.StringContext, std.hash_map.default_max_load_percentage).init(allocator),
            .mutex = std.Thread.Mutex{},
        };
    }

    pub fn deinit(self: *Self) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        var iterator = self.traces.iterator();
        while (iterator.next()) |entry| {
            entry.value_ptr.deinit(self.allocator);
        }
        self.traces.deinit();
    }

    pub fn startSpan(
        self: *Self,
        trace_id: []const u8,
        span_id: []const u8,
        operation_name: []const u8,
        shard_id: u32,
    ) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const result = try self.traces.getOrPut(try self.allocator.dupe(u8, trace_id));
        if (!result.found_existing) {
            result.value_ptr.* = TraceInfo.init(self.allocator);
        }

        const trace_info = result.value_ptr;
        try trace_info.startSpan(span_id, operation_name, shard_id);
    }

    pub fn finishSpan(self: *Self, trace_id: []const u8, span_id: []const u8) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.traces.getPtr(trace_id)) |trace_info| {
            try trace_info.finishSpan(span_id);
        }
    }

    pub fn addSpanTag(
        self: *Self,
        trace_id: []const u8,
        span_id: []const u8,
        key: []const u8,
        value: []const u8,
    ) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.traces.getPtr(trace_id)) |trace_info| {
            try trace_info.addSpanTag(span_id, key, value);
        }
    }

    pub fn getTraceInfo(self: *Self, trace_id: []const u8) ?TraceInfo {
        self.mutex.lock();
        defer self.mutex.unlock();

        return self.traces.get(trace_id);
    }
};

/// Trace information
pub const TraceInfo = struct {
    allocator: Allocator,
    spans: ArrayList(SpanInfo),
    start_time_ns: u64,

    const Self = @This();

    pub fn init(allocator: Allocator) Self {
        return Self{
            .allocator = allocator,
            .spans = ArrayList(SpanInfo){},
            .start_time_ns = @intCast(std.time.nanoTimestamp()),
        };
    }

    pub fn deinit(self: *Self, allocator: Allocator) void {
        for (self.spans.items) |*span| {
            span.deinit(allocator);
        }
        self.spans.deinit(allocator);
    }

    pub fn startSpan(
        self: *Self,
        span_id: []const u8,
        operation_name: []const u8,
        shard_id: u32,
    ) !void {
        const span = SpanInfo{
            .span_id = try self.allocator.dupe(u8, span_id),
            .operation_name = try self.allocator.dupe(u8, operation_name),
            .start_time_ns = @intCast(std.time.nanoTimestamp()),
            .end_time_ns = null,
            .shard_id = shard_id,
            .tags = HashMap([]const u8, []const u8, std.hash_map.StringContext, std.hash_map.default_max_load_percentage).init(self.allocator),
        };

        try self.spans.append(self.allocator, span);
    }

    pub fn finishSpan(self: *Self, span_id: []const u8) !void {
        for (self.spans.items) |*span| {
            if (std.mem.eql(u8, span.span_id, span_id)) {
                span.end_time_ns = @intCast(std.time.nanoTimestamp());
                return;
            }
        }
    }

    pub fn addSpanTag(
        self: *Self,
        span_id: []const u8,
        key: []const u8,
        value: []const u8,
    ) !void {
        for (self.spans.items) |*span| {
            if (std.mem.eql(u8, span.span_id, span_id)) {
                try span.tags.put(
                    try self.allocator.dupe(u8, key),
                    try self.allocator.dupe(u8, value),
                );
                return;
            }
        }
    }

    pub fn getSpanCount(self: *const Self) u32 {
        return @intCast(self.spans.items.len);
    }

    pub fn getSpan(self: *const Self, index: u32) ?*const SpanInfo {
        if (index >= self.spans.items.len) return null;
        return &self.spans.items[index];
    }
};

/// Span information
pub const SpanInfo = struct {
    span_id: []const u8,
    operation_name: []const u8,
    start_time_ns: u64,
    end_time_ns: ?u64,
    shard_id: u32,
    tags: HashMap([]const u8, []const u8, std.hash_map.StringContext, std.hash_map.default_max_load_percentage),

    pub fn deinit(self: *SpanInfo, allocator: Allocator) void {
        allocator.free(self.span_id);
        allocator.free(self.operation_name);

        var iterator = self.tags.iterator();
        while (iterator.next()) |entry| {
            allocator.free(entry.key_ptr.*);
            allocator.free(entry.value_ptr.*);
        }
        self.tags.deinit();
    }

    pub fn getDurationNs(self: *const SpanInfo) ?u64 {
        if (self.end_time_ns) |end_time| {
            return end_time - self.start_time_ns;
        }
        return null;
    }
};

/// Zig logger
pub const ZigLogger = struct {
    allocator: Allocator,
    log_entries: ArrayList(LogEntry),
    max_entries: usize,
    mutex: std.Thread.Mutex,

    const Self = @This();

    pub fn init(allocator: Allocator) !Self {
        return Self{
            .allocator = allocator,
            .log_entries = ArrayList(LogEntry){},
            .max_entries = 10000,
            .mutex = std.Thread.Mutex{},
        };
    }

    pub fn deinit(self: *Self) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        for (self.log_entries.items) |*entry| {
            entry.deinit(self.allocator);
        }
        self.log_entries.deinit(self.allocator);
    }

    pub fn log(
        self: *Self,
        level: LogLevel,
        component: []const u8,
        message: []const u8,
        shard_id: u32,
        trace_id: ?[]const u8,
        span_id: ?[]const u8,
    ) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const entry = LogEntry{
            .timestamp_ns = @intCast(std.time.nanoTimestamp()),
            .level = level,
            .component = self.allocator.dupe(u8, component) catch return,
            .message = self.allocator.dupe(u8, message) catch return,
            .shard_id = shard_id,
            .trace_id = if (trace_id) |tid| self.allocator.dupe(u8, tid) catch null else null,
            .span_id = if (span_id) |sid| self.allocator.dupe(u8, sid) catch null else null,
        };

        self.log_entries.append(self.allocator, entry) catch return;

        // Maintain size limit
        while (self.log_entries.items.len > self.max_entries) {
            var old_entry = self.log_entries.orderedRemove(0);
            old_entry.deinit(self.allocator);
        }
    }

    pub fn getRecentEntries(self: *Self, count: usize) []const LogEntry {
        self.mutex.lock();
        defer self.mutex.unlock();

        const start_idx = if (self.log_entries.items.len > count)
            self.log_entries.items.len - count
        else
            0;

        return self.log_entries.items[start_idx..];
    }
};

/// Log entry
pub const LogEntry = struct {
    timestamp_ns: u64,
    level: LogLevel,
    component: []const u8,
    message: []const u8,
    shard_id: u32,
    trace_id: ?[]const u8,
    span_id: ?[]const u8,

    pub fn deinit(self: *LogEntry, allocator: Allocator) void {
        allocator.free(self.component);
        allocator.free(self.message);
        if (self.trace_id) |tid| allocator.free(tid);
        if (self.span_id) |sid| allocator.free(sid);
    }
};

/// Performance monitor
pub const PerformanceMonitor = struct {
    allocator: Allocator,
    operation_timings: HashMap([]const u8, ArrayList(u64), std.hash_map.StringContext, std.hash_map.default_max_load_percentage),
    mutex: std.Thread.Mutex,

    const Self = @This();

    pub fn init(allocator: Allocator) !Self {
        return Self{
            .allocator = allocator,
            .operation_timings = HashMap([]const u8, ArrayList(u64), std.hash_map.StringContext, std.hash_map.default_max_load_percentage).init(allocator),
            .mutex = std.Thread.Mutex{},
        };
    }

    pub fn deinit(self: *Self) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        var iterator = self.operation_timings.iterator();
        while (iterator.next()) |entry| {
            self.allocator.free(entry.key_ptr.*);
            entry.value_ptr.deinit(self.allocator);
        }
        self.operation_timings.deinit();
    }

    pub fn recordTiming(self: *Self, operation: []const u8, duration_ns: u64) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const result = self.operation_timings.getOrPut(self.allocator.dupe(u8, operation) catch return) catch return;
        if (!result.found_existing) {
            result.value_ptr.* = ArrayList(u64){};
        }

        const timings = result.value_ptr;
        timings.append(self.allocator, duration_ns) catch return;

        // Keep only last 1000 measurements
        while (timings.items.len > 1000) {
            _ = timings.orderedRemove(0);
        }
    }
};

// Global observability manager instance
var global_observability_manager: ?ObservabilityManager = null;
var global_mutex: std.Thread.Mutex = std.Thread.Mutex{};

/// Initialize global observability manager
pub fn initGlobalObservability(allocator: Allocator) !void {
    global_mutex.lock();
    defer global_mutex.unlock();

    if (global_observability_manager == null) {
        global_observability_manager = try ObservabilityManager.init(allocator);
    }
}

/// Deinitialize global observability manager
pub fn deinitGlobalObservability() void {
    global_mutex.lock();
    defer global_mutex.unlock();

    if (global_observability_manager) |*manager| {
        manager.deinit();
        global_observability_manager = null;
    }
}

/// Get global observability manager
pub fn getGlobalObservability() ?*ObservabilityManager {
    global_mutex.lock();
    defer global_mutex.unlock();

    return if (global_observability_manager) |*manager| manager else null;
}

// Convenience functions for global observability
pub fn recordArenaOperation(
    shard_id: u32,
    operation: ArenaOperation,
    bytes: u64,
    latency_ns: u64,
    success: bool,
) void {
    if (getGlobalObservability()) |manager| {
        manager.recordArenaOperation(shard_id, operation, bytes, latency_ns, success);
    }
}

pub fn recordBTreeOperation(
    shard_id: u32,
    operation: BTreeOperation,
    latency_ns: u64,
    success: bool,
) void {
    if (getGlobalObservability()) |manager| {
        manager.recordBTreeOperation(shard_id, operation, latency_ns, success);
    }
}

pub fn recordWALOperation(
    shard_id: u32,
    operation: WALOperation,
    bytes: u64,
    latency_ns: u64,
    success: bool,
) void {
    if (getGlobalObservability()) |manager| {
        manager.recordWALOperation(shard_id, operation, bytes, latency_ns, success);
    }
}

pub fn recordTransactionOperation(
    shard_id: u32,
    operation: TransactionOperation,
    duration_ms: f64,
    success: bool,
) void {
    if (getGlobalObservability()) |manager| {
        manager.recordTransactionOperation(shard_id, operation, duration_ms, success);
    }
}

pub fn logMessage(
    level: LogLevel,
    component: []const u8,
    message: []const u8,
    shard_id: u32,
    trace_id: ?[]const u8,
    span_id: ?[]const u8,
) void {
    if (getGlobalObservability()) |manager| {
        manager.log(level, component, message, shard_id, trace_id, span_id);
    }
}

pub fn recordTiming(operation: []const u8, duration_ns: u64) void {
    if (getGlobalObservability()) |manager| {
        manager.recordTiming(operation, duration_ns);
    }
}

// Tests
test "observability manager initialization" {
    const allocator = std.testing.allocator;

    var manager = try ObservabilityManager.init(allocator);
    defer manager.deinit();

    // Test that manager initializes properly
    try std.testing.expect(manager.enabled);
}

test "arena metrics recording" {
    const allocator = std.testing.allocator;

    var manager = try ObservabilityManager.init(allocator);
    defer manager.deinit();

    // Record some arena operations
    manager.recordArenaOperation(0, .Allocate, 1024, 1000, true);
    manager.recordArenaOperation(0, .Allocate, 2048, 1500, true);
    manager.recordArenaOperation(0, .Allocate, 512, 800, false);

    const metrics = manager.getMetrics(0);

    try std.testing.expectEqual(@as(u64, 3), metrics.arena_metrics.total_allocations.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 2), metrics.arena_metrics.successful_allocations.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 1), metrics.arena_metrics.failed_allocations.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 3072), metrics.arena_metrics.total_bytes_allocated.load(.seq_cst));
}

test "btree metrics recording" {
    const allocator = std.testing.allocator;

    var manager = try ObservabilityManager.init(allocator);
    defer manager.deinit();

    // Record some B-tree operations
    manager.recordBTreeOperation(0, .Insert, 2000, true);
    manager.recordBTreeOperation(0, .Lookup, 500, true);
    manager.recordBTreeOperation(0, .Lookup, 600, false);

    const metrics = manager.getMetrics(0);

    try std.testing.expectEqual(@as(u64, 1), metrics.btree_metrics.total_insertions.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 2), metrics.btree_metrics.total_lookups.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 1), metrics.btree_metrics.successful_lookups.load(.seq_cst));
}

test "trace manager" {
    const allocator = std.testing.allocator;

    var trace_manager = try TraceManager.init(allocator);
    defer trace_manager.deinit();

    // Start a span
    try trace_manager.startSpan("trace_123", "span_456", "test_operation", 0);

    // Add a tag
    try trace_manager.addSpanTag("trace_123", "span_456", "user_id", "user_789");

    // Finish the span
    try trace_manager.finishSpan("trace_123", "span_456");

    // Check trace info
    const trace_info = trace_manager.getTraceInfo("trace_123");
    try std.testing.expect(trace_info != null);

    if (trace_info) |info| {
        try std.testing.expectEqual(@as(u32, 1), info.getSpanCount());

        const span = info.getSpan(0);
        try std.testing.expect(span != null);

        if (span) |s| {
            try std.testing.expect(std.mem.eql(u8, s.span_id, "span_456"));
            try std.testing.expect(std.mem.eql(u8, s.operation_name, "test_operation"));
            try std.testing.expect(s.end_time_ns != null);
        }
    }
}

test "zig logger" {
    const allocator = std.testing.allocator;

    var logger = try ZigLogger.init(allocator);
    defer logger.deinit();

    // Log some messages
    logger.log(.Info, "test_component", "Test message 1", 0, "trace_123", "span_456");
    logger.log(.Warn, "test_component", "Test message 2", 0, null, null);

    const entries = logger.getRecentEntries(10);
    try std.testing.expectEqual(@as(usize, 2), entries.len);

    try std.testing.expectEqual(LogLevel.Info, entries[0].level);
    try std.testing.expect(std.mem.eql(u8, entries[0].message, "Test message 1"));

    try std.testing.expectEqual(LogLevel.Warn, entries[1].level);
    try std.testing.expect(std.mem.eql(u8, entries[1].message, "Test message 2"));
}
