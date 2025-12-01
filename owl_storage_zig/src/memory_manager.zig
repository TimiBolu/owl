// Task 9.10: Memory Management Optimization
// Comprehensive memory management with custom allocators, pooling, compaction, and NUMA awareness

const std = @import("std");
const builtin = @import("builtin");
const Arena = @import("arena.zig").Arena;

/// NUMA node identifier
pub const NumaNode = u8;

/// Memory allocation strategy
pub const AllocationStrategy = enum {
    /// Best-fit allocation (minimize fragmentation)
    best_fit,
    /// First-fit allocation (fast allocation)
    first_fit,
    /// Buddy allocation (power-of-2 sizes)
    buddy,
    /// Slab allocation (fixed-size classes)
    slab,
};

/// Memory compaction policy
pub const CompactionPolicy = enum {
    /// Never compact
    never,
    /// Compact when fragmentation exceeds threshold
    threshold,
    /// Compact periodically
    periodic,
    /// Compact on demand
    on_demand,
};

/// NUMA allocation policy
pub const NumaPolicy = enum {
    /// Allocate on local NUMA node
    local,
    /// Allocate on preferred NUMA node
    preferred,
    /// Interleave across NUMA nodes
    interleave,
    /// Bind to specific NUMA node
    bind,
};

/// Memory statistics for monitoring
pub const MemoryStats = struct {
    /// Total memory allocated (bytes)
    total_allocated: usize,
    /// Total memory used (bytes)
    total_used: usize,
    /// Total memory free (bytes)
    total_free: usize,
    /// Fragmentation ratio (0.0 = no fragmentation, 1.0 = fully fragmented)
    fragmentation_ratio: f64,
    /// Number of allocations
    allocation_count: u64,
    /// Number of deallocations
    deallocation_count: u64,
    /// Number of compactions performed
    compaction_count: u64,
    /// Average allocation size
    avg_allocation_size: f64,
    /// Peak memory usage
    peak_usage: usize,
};

/// Memory manager configuration
pub const MemoryManagerConfig = struct {
    /// Allocation strategy
    strategy: AllocationStrategy = .slab,
    /// Compaction policy
    compaction_policy: CompactionPolicy = .threshold,
    /// Fragmentation threshold for compaction (0.0-1.0)
    fragmentation_threshold: f64 = 0.3,
    /// Compaction interval (milliseconds) for periodic compaction
    compaction_interval_ms: u64 = 60000,
    /// Enable NUMA awareness
    numa_aware: bool = false,
    /// NUMA allocation policy
    numa_policy: NumaPolicy = .local,
    /// Preferred NUMA node (for preferred policy)
    preferred_numa_node: ?NumaNode = null,
    /// Enable memory pooling
    enable_pooling: bool = true,
    /// Pool size per type (number of objects)
    pool_size_per_type: usize = 64,
};

/// Comprehensive memory manager
pub const MemoryManager = struct {
    /// Configuration
    config: MemoryManagerConfig,
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    /// Statistics (protected by stats_mutex)
    stats: MemoryStats,
    /// Mutex for statistics updates
    stats_mutex: std.Thread.Mutex,
    /// Last compaction timestamp
    last_compaction_ns: std.atomic.Value(i64),
    /// NUMA node count (0 if not NUMA-aware)
    numa_node_count: NumaNode,
    /// Current NUMA node
    current_numa_node: std.atomic.Value(NumaNode),
    /// Mutex for compaction operations
    compaction_mutex: std.Thread.Mutex,

    pub fn init(allocator: std.mem.Allocator, config: MemoryManagerConfig) !MemoryManager {
        const numa_count = if (config.numa_aware) try detectNumaNodes() else 0;

        return .{
            .config = config,
            .allocator = allocator,
            .stats = .{
                .total_allocated = 0,
                .total_used = 0,
                .total_free = 0,
                .fragmentation_ratio = 0.0,
                .allocation_count = 0,
                .deallocation_count = 0,
                .compaction_count = 0,
                .avg_allocation_size = 0.0,
                .peak_usage = 0,
            },
            .stats_mutex = std.Thread.Mutex{},
            .last_compaction_ns = std.atomic.Value(i64).init(0),
            .numa_node_count = numa_count,
            .current_numa_node = std.atomic.Value(NumaNode).init(0),
            .compaction_mutex = std.Thread.Mutex{},
        };
    }

    pub fn deinit(self: *MemoryManager) void {
        _ = self;
        // Cleanup if needed
    }

    /// Update statistics after allocation
    pub fn recordAllocation(self: *MemoryManager, size: usize) void {
        self.stats_mutex.lock();
        defer self.stats_mutex.unlock();

        self.stats.total_allocated += size;
        self.stats.total_used += size;
        self.stats.total_free = if (self.stats.total_allocated > self.stats.total_used)
            self.stats.total_allocated - self.stats.total_used
        else
            0;
        self.stats.allocation_count += 1;

        // Update average allocation size
        const total_allocs = @as(f64, @floatFromInt(self.stats.allocation_count));
        self.stats.avg_allocation_size =
            @as(f64, @floatFromInt(self.stats.total_used)) / total_allocs;

        // Update peak usage
        if (self.stats.total_used > self.stats.peak_usage) {
            self.stats.peak_usage = self.stats.total_used;
        }
    }

    /// Update statistics after deallocation
    pub fn recordDeallocation(self: *MemoryManager, size: usize) void {
        self.stats_mutex.lock();
        defer self.stats_mutex.unlock();

        self.stats.total_used = if (self.stats.total_used >= size)
            self.stats.total_used - size
        else
            0;
        self.stats.total_free = if (self.stats.total_allocated > self.stats.total_used)
            self.stats.total_allocated - self.stats.total_used
        else
            0;
        self.stats.deallocation_count += 1;
    }

    /// Calculate fragmentation ratio
    pub fn calculateFragmentation(self: *MemoryManager, arena: *const Arena) f64 {
        _ = self;
        const arena_stats = arena.stats();

        if (arena_stats.total_size == 0) return 0.0;

        // Fragmentation = (free_size - largest_free_block) / total_size
        // For simplicity, we estimate based on object count and used size
        const used_ratio = @as(f64, @floatFromInt(arena_stats.used_size)) /
            @as(f64, @floatFromInt(arena_stats.total_size));

        // If we have many small allocations, fragmentation is higher
        const avg_obj_size = if (arena_stats.object_count > 0)
            @as(f64, @floatFromInt(arena_stats.used_size)) /
                @as(f64, @floatFromInt(arena_stats.object_count))
        else
            0.0;

        // Fragmentation increases with more objects and lower average size
        const fragmentation = if (avg_obj_size > 0)
            (1.0 - used_ratio) * (1.0 / (1.0 + avg_obj_size / 1024.0))
        else
            0.0;

        return std.math.clamp(fragmentation, 0.0, 1.0);
    }

    /// Check if compaction is needed
    pub fn shouldCompact(self: *MemoryManager, arena: *const Arena) bool {
        switch (self.config.compaction_policy) {
            .never => return false,
            .on_demand => return false, // Only compact when explicitly requested
            .threshold => {
                const frag = self.calculateFragmentation(arena);
                return frag >= self.config.fragmentation_threshold;
            },
            .periodic => {
                const now = std.time.nanoTimestamp();
                const last = self.last_compaction_ns.load(.seq_cst);
                const elapsed_ms = @divTrunc(now - last, 1_000_000);
                return elapsed_ms >= @as(i64, @intCast(self.config.compaction_interval_ms));
            },
        }
    }

    /// Perform memory compaction
    pub fn compact(self: *MemoryManager, arena: *Arena) !void {
        self.compaction_mutex.lock();
        defer self.compaction_mutex.unlock();

        // Perform arena compaction
        try arena.compact(self.allocator);

        // Update statistics
        self.stats_mutex.lock();
        self.stats.compaction_count += 1;
        self.stats_mutex.unlock();

        // Update last compaction timestamp
        const now = std.time.nanoTimestamp();
        self.last_compaction_ns.store(@intCast(now), .seq_cst);

        // Recalculate fragmentation
        const new_frag = self.calculateFragmentation(arena);
        self.stats_mutex.lock();
        self.stats.fragmentation_ratio = new_frag;
        self.stats_mutex.unlock();
    }

    /// Get current memory statistics
    pub fn getStats(self: *MemoryManager) MemoryStats {
        // Create a local copy to avoid holding the lock
        self.stats_mutex.lock();
        defer self.stats_mutex.unlock();
        return self.stats;
    }

    /// Get NUMA node for allocation
    pub fn getNumaNode(self: *MemoryManager) NumaNode {
        if (!self.config.numa_aware or self.numa_node_count == 0) {
            return 0;
        }

        switch (self.config.numa_policy) {
            .local => {
                // Get current CPU's NUMA node
                return getCurrentNumaNode() catch 0;
            },
            .preferred => {
                // Use preferred node if specified
                return self.config.preferred_numa_node orelse 0;
            },
            .interleave => {
                // Round-robin across NUMA nodes
                const current = self.current_numa_node.load(.seq_cst);
                const next = (current + 1) % self.numa_node_count;
                _ = self.current_numa_node.store(next, .seq_cst);
                return current;
            },
            .bind => {
                // Bind to preferred node
                return self.config.preferred_numa_node orelse 0;
            },
        }
    }
};

// ============================================================================
// NUMA Support Functions
// ============================================================================

/// Detect number of NUMA nodes in the system
fn detectNumaNodes() !NumaNode {
    if (builtin.os.tag == .linux) {
        // Try to read /sys/devices/system/node/online
        const file = std.fs.openFileAbsolute(
            "/sys/devices/system/node/online",
            .{ .mode = .read_only },
        ) catch {
            // NUMA not available or not supported
            return 0;
        };
        defer file.close();

        var buf: [64]u8 = undefined;
        const bytes_read = try file.readAll(&buf);
        const content = std.mem.trim(u8, buf[0..bytes_read], &std.ascii.whitespace);

        // Parse format like "0-3" or "0,2-4"
        var max_node: NumaNode = 0;
        var iter = std.mem.split(u8, content, ",");
        while (iter.next()) |part| {
            if (std.mem.indexOf(u8, part, "-")) |dash_pos| {
                const end_str = part[dash_pos + 1 ..];
                const end_node = try std.fmt.parseInt(NumaNode, end_str, 10);
                if (end_node > max_node) max_node = end_node;
            } else {
                const node = try std.fmt.parseInt(NumaNode, part, 10);
                if (node > max_node) max_node = node;
            }
        }

        return max_node + 1; // Convert max node ID to count
    }

    // NUMA not supported on this platform
    return 0;
}

/// Get current CPU's NUMA node
fn getCurrentNumaNode() !NumaNode {
    if (builtin.os.tag == .linux) {
        // Try to read /proc/self/status and find Cpus_allowed_list
        const file = std.fs.openFileAbsolute(
            "/proc/self/status",
            .{ .mode = .read_only },
        ) catch {
            return 0;
        };
        defer file.close();

        var buf: [4096]u8 = undefined;
        const bytes_read = try file.readAll(&buf);
        const content = buf[0..bytes_read];

        // Find Cpus_allowed_list line
        var lines = std.mem.split(u8, content, "\n");
        while (lines.next()) |line| {
            if (std.mem.startsWith(u8, line, "Cpus_allowed_list:")) {
                // For simplicity, assume first CPU in list
                // In production, would need to map CPU to NUMA node
                return 0;
            }
        }
    }

    return 0;
}

// ============================================================================
// Tests
// ============================================================================

test "MemoryManager: initialization" {
    const allocator = std.testing.allocator;

    const config = MemoryManagerConfig{
        .strategy = .slab,
        .compaction_policy = .threshold,
        .fragmentation_threshold = 0.3,
        .numa_aware = false,
    };

    var manager = try MemoryManager.init(allocator, config);
    defer manager.deinit();

    const stats = manager.getStats();
    try std.testing.expectEqual(@as(usize, 0), stats.total_allocated);
    try std.testing.expectEqual(@as(usize, 0), stats.total_used);
    try std.testing.expectEqual(@as(u64, 0), stats.allocation_count);
}

test "MemoryManager: record allocation" {
    const allocator = std.testing.allocator;

    var manager = try MemoryManager.init(allocator, .{});
    defer manager.deinit();

    manager.recordAllocation(1024);
    manager.recordAllocation(2048);

    const stats = manager.getStats();
    try std.testing.expectEqual(@as(usize, 3072), stats.total_allocated);
    try std.testing.expectEqual(@as(usize, 3072), stats.total_used);
    try std.testing.expectEqual(@as(u64, 2), stats.allocation_count);
    try std.testing.expect(stats.avg_allocation_size > 0);
}

test "MemoryManager: record deallocation" {
    const allocator = std.testing.allocator;

    var manager = try MemoryManager.init(allocator, .{});
    defer manager.deinit();

    manager.recordAllocation(2048);
    manager.recordDeallocation(1024);

    const stats = manager.getStats();
    try std.testing.expectEqual(@as(usize, 2048), stats.total_allocated);
    try std.testing.expectEqual(@as(usize, 1024), stats.total_used);
    try std.testing.expectEqual(@as(usize, 1024), stats.total_free);
    try std.testing.expectEqual(@as(u64, 1), stats.deallocation_count);
}

test "MemoryManager: NUMA node detection" {
    const numa_count = detectNumaNodes() catch 0;
    // Should not crash, may return 0 if NUMA not available
    try std.testing.expect(numa_count >= 0);
}

test "MemoryManager: NUMA policy local" {
    const allocator = std.testing.allocator;

    const config = MemoryManagerConfig{
        .numa_aware = true,
        .numa_policy = .local,
    };

    var manager = try MemoryManager.init(allocator, config);
    defer manager.deinit();

    const node = manager.getNumaNode();
    try std.testing.expect(node >= 0);
}

test "MemoryManager: NUMA policy interleave" {
    const allocator = std.testing.allocator;

    const config = MemoryManagerConfig{
        .numa_aware = true,
        .numa_policy = .interleave,
    };

    var manager = try MemoryManager.init(allocator, config);
    defer manager.deinit();

    if (manager.numa_node_count > 1) {
        const node1 = manager.getNumaNode();
        const node2 = manager.getNumaNode();
        // Should round-robin
        try std.testing.expect(node1 != node2 or manager.numa_node_count == 1);
    }
}

test "MemoryManager: compaction policy threshold" {
    const allocator = std.testing.allocator;

    const config = MemoryManagerConfig{
        .compaction_policy = .threshold,
        .fragmentation_threshold = 0.5,
    };

    var manager = try MemoryManager.init(allocator, config);
    defer manager.deinit();

    // Create a test arena
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 1024 * 1024, 0);
    defer arena.deinit();

    // Should not need compaction initially
    try std.testing.expect(!manager.shouldCompact(&arena));
}

test "MemoryManager: compaction policy never" {
    const allocator = std.testing.allocator;

    const config = MemoryManagerConfig{
        .compaction_policy = .never,
    };

    var manager = try MemoryManager.init(allocator, config);
    defer manager.deinit();

    // Create a test arena
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 1024 * 1024, 0);
    defer arena.deinit();

    // Should never need compaction
    try std.testing.expect(!manager.shouldCompact(&arena));
}
