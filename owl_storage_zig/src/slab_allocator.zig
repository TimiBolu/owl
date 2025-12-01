// Lock-free slab allocator for small objects
// Uses atomic operations for thread-safe allocation

const std = @import("std");
const LockFreeStack = @import("lockfree_stack.zig").LockFreeStack;

/// Slab size classes (powers of 2 for efficient allocation)
pub const SlabSizeClass = enum(u8) {
    size_64 = 0, // 64 bytes
    size_128 = 1, // 128 bytes
    size_256 = 2, // 256 bytes
    size_512 = 3, // 512 bytes
    size_1024 = 4, // 1024 bytes

    pub fn size(self: SlabSizeClass) u32 {
        return switch (self) {
            .size_64 => 64,
            .size_128 => 128,
            .size_256 => 256,
            .size_512 => 512,
            .size_1024 => 1024,
        };
    }

    pub fn fromSize(requested_size: u32) ?SlabSizeClass {
        if (requested_size <= 64) return .size_64;
        if (requested_size <= 128) return .size_128;
        if (requested_size <= 256) return .size_256;
        if (requested_size <= 512) return .size_512;
        if (requested_size <= 1024) return .size_1024;
        return null; // Too large for slab allocation
    }
};

/// Slab allocator with lock-free free lists
pub const SlabAllocator = struct {
    /// Free lists for each size class
    free_lists: [5]LockFreeStack(u32),
    /// Base pointer for offset calculations
    base_ptr: [*]u8,
    /// Arena size
    arena_size: usize,
    /// Bump allocator for new slabs
    bump: std.atomic.Value(u32),
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    /// Statistics
    stats: SlabStats,

    pub fn init(allocator: std.mem.Allocator, base_ptr: [*]u8, arena_size: usize, initial_offset: u32) SlabAllocator {
        return .{
            .free_lists = .{
                LockFreeStack(u32).init(allocator),
                LockFreeStack(u32).init(allocator),
                LockFreeStack(u32).init(allocator),
                LockFreeStack(u32).init(allocator),
                LockFreeStack(u32).init(allocator),
            },
            .base_ptr = base_ptr,
            .arena_size = arena_size,
            .bump = std.atomic.Value(u32).init(initial_offset),
            .allocator = allocator,
            .stats = SlabStats.init(),
        };
    }

    pub fn deinit(self: *SlabAllocator) void {
        for (&self.free_lists) |*list| {
            list.deinit();
        }
    }

    /// Allocate a slab of the given size (thread-safe)
    pub fn alloc(self: *SlabAllocator, size: u32) !u32 {
        const size_class = SlabSizeClass.fromSize(size) orelse {
            return error.SizeTooBig; // Use bump allocator for large objects
        };

        const class_idx = @intFromEnum(size_class);
        _ = self.stats.alloc_attempts[class_idx].fetchAdd(1, .monotonic);

        // Try to get from free list first
        if (self.free_lists[class_idx].pop()) |offset| {
            _ = self.stats.free_list_hits[class_idx].fetchAdd(1, .monotonic);
            return offset;
        }

        // Free list empty, allocate new slab from bump allocator
        _ = self.stats.bump_allocs[class_idx].fetchAdd(1, .monotonic);
        const slab_size = size_class.size();
        const offset = self.bump.fetchAdd(slab_size, .seq_cst);

        if (offset + slab_size > self.arena_size) {
            return error.OutOfMemory;
        }

        return offset;
    }

    /// Free a slab (thread-safe)
    pub fn free(self: *SlabAllocator, offset: u32, size: u32) !void {
        const size_class = SlabSizeClass.fromSize(size) orelse {
            return error.InvalidSize;
        };

        const class_idx = @intFromEnum(size_class);
        try self.free_lists[class_idx].push(offset);
        _ = self.stats.frees[class_idx].fetchAdd(1, .monotonic);
    }

    /// Get statistics for a size class
    pub fn getStats(self: *const SlabAllocator, size_class: SlabSizeClass) SlabClassStats {
        const idx = @intFromEnum(size_class);
        return .{
            .size_class = size_class,
            .alloc_attempts = self.stats.alloc_attempts[idx].load(.monotonic),
            .free_list_hits = self.stats.free_list_hits[idx].load(.monotonic),
            .bump_allocs = self.stats.bump_allocs[idx].load(.monotonic),
            .frees = self.stats.frees[idx].load(.monotonic),
            .free_list_size = self.free_lists[idx].len(),
        };
    }

    /// Get hit rate for a size class (percentage of allocations from free list)
    pub fn hitRate(self: *const SlabAllocator, size_class: SlabSizeClass) f64 {
        const stats = self.getStats(size_class);
        if (stats.alloc_attempts == 0) return 0.0;
        return (@as(f64, @floatFromInt(stats.free_list_hits)) / @as(f64, @floatFromInt(stats.alloc_attempts))) * 100.0;
    }
};

/// Statistics for slab allocator
const SlabStats = struct {
    /// Number of allocation attempts per size class
    alloc_attempts: [5]std.atomic.Value(usize),
    /// Number of successful free list hits per size class
    free_list_hits: [5]std.atomic.Value(usize),
    /// Number of bump allocations per size class
    bump_allocs: [5]std.atomic.Value(usize),
    /// Number of frees per size class
    frees: [5]std.atomic.Value(usize),

    fn init() SlabStats {
        return .{
            .alloc_attempts = .{
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
            },
            .free_list_hits = .{
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
            },
            .bump_allocs = .{
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
            },
            .frees = .{
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
                std.atomic.Value(usize).init(0),
            },
        };
    }
};

/// Statistics for a single size class
pub const SlabClassStats = struct {
    size_class: SlabSizeClass,
    alloc_attempts: usize,
    free_list_hits: usize,
    bump_allocs: usize,
    frees: usize,
    free_list_size: usize,
};

// Tests
test "SlabAllocator: basic allocation" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024; // 1MB
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var slab = SlabAllocator.init(allocator, arena.ptr, arena_size, 64);
    defer slab.deinit();

    const offset1 = try slab.alloc(64);
    const offset2 = try slab.alloc(64);

    try std.testing.expect(offset1 != offset2);
    try std.testing.expectEqual(@as(u32, 64), offset1);
    try std.testing.expectEqual(@as(u32, 128), offset2);
}

test "SlabAllocator: free and reuse" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var slab = SlabAllocator.init(allocator, arena.ptr, arena_size, 64);
    defer slab.deinit();

    const offset1 = try slab.alloc(64);
    try slab.free(offset1, 64);

    const offset2 = try slab.alloc(64);
    try std.testing.expectEqual(offset1, offset2); // Should reuse freed slab
}

test "SlabAllocator: size classes" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var slab = SlabAllocator.init(allocator, arena.ptr, arena_size, 64);
    defer slab.deinit();

    const offset_64 = try slab.alloc(64);
    const offset_128 = try slab.alloc(128);
    const offset_256 = try slab.alloc(256);

    try std.testing.expectEqual(@as(u32, 64), offset_64);
    try std.testing.expectEqual(@as(u32, 128), offset_128);
    try std.testing.expectEqual(@as(u32, 256), offset_256); // 64 + 128 + 256 (rounded to size class)
}

test "SlabAllocator: statistics" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var slab = SlabAllocator.init(allocator, arena.ptr, arena_size, 64);
    defer slab.deinit();

    _ = try slab.alloc(64);
    _ = try slab.alloc(64);

    const stats = slab.getStats(.size_64);
    try std.testing.expectEqual(@as(usize, 2), stats.alloc_attempts);
    try std.testing.expectEqual(@as(usize, 2), stats.bump_allocs);
}

test "SlabAllocator: hit rate" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var slab = SlabAllocator.init(allocator, arena.ptr, arena_size, 64);
    defer slab.deinit();

    const offset1 = try slab.alloc(64);
    try slab.free(offset1, 64);
    _ = try slab.alloc(64); // Should hit free list

    const hit_rate = slab.hitRate(.size_64);
    try std.testing.expectEqual(@as(f64, 50.0), hit_rate); // 1 hit out of 2 attempts
}
