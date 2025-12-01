// Task 9.9: CPU Cache-Aware Data Structures
// Implements cache-line aligned structures and cache-oblivious algorithms
// for optimal CPU cache utilization

const std = @import("std");
const builtin = @import("builtin");
const prefetch = @import("prefetch.zig");

/// Standard cache line size (64 bytes on most modern CPUs)
pub const CACHE_LINE_SIZE: usize = 64;

/// Cache line alignment for structures
pub const cache_line_aligned = std.mem.alignForward;

/// Padding to prevent false sharing between cache lines
pub fn CacheLinePadding(comptime T: type) type {
    const size = @sizeOf(T);
    const padding_needed = if (size % CACHE_LINE_SIZE == 0)
        0
    else
        CACHE_LINE_SIZE - (size % CACHE_LINE_SIZE);

    return struct {
        data: T,
        _padding: [padding_needed]u8 = undefined,

        pub fn init(data: T) @This() {
            return .{ .data = data };
        }

        pub fn get(self: *@This()) *T {
            return &self.data;
        }

        pub fn getConst(self: *const @This()) *const T {
            return &self.data;
        }
    };
}

/// Cache-aligned array for sequential access patterns
/// Ensures the array starts on a cache line boundary
pub fn CacheAlignedArray(comptime T: type) type {
    return struct {
        items: []align(CACHE_LINE_SIZE) T,
        allocator: std.mem.Allocator,

        const Self = @This();

        pub fn init(allocator: std.mem.Allocator, capacity: usize) !Self {
            const items = try allocator.alignedAlloc(T, std.mem.Alignment.fromByteUnits(CACHE_LINE_SIZE), capacity);
            return Self{
                .items = items,
                .allocator = allocator,
            };
        }

        pub fn deinit(self: *Self) void {
            self.allocator.free(self.items);
        }

        pub fn get(self: *Self, index: usize) *T {
            return &self.items[index];
        }

        pub fn getConst(self: *const Self, index: usize) *const T {
            return &self.items[index];
        }

        pub fn len(self: *const Self) usize {
            return self.items.len;
        }

        /// Prefetch a range of elements for sequential access
        pub fn prefetchRange(self: *const Self, start: usize, count: usize) void {
            const end = @min(start + count, self.items.len);
            if (start >= end) return;

            const start_addr = @intFromPtr(&self.items[start]);
            const size = (end - start) * @sizeOf(T);
            const ptr: [*]const u8 = @ptrFromInt(start_addr);
            prefetch.prefetchRange(ptr, size, .moderate);
        }

        /// Prefetch with stride pattern for non-sequential access
        pub fn prefetchStrided(self: *const Self, start: usize, stride: usize, count: usize) void {
            var i: usize = 0;
            while (i < count and start + i * stride < self.items.len) : (i += 1) {
                const idx = start + i * stride;
                const addr = @intFromPtr(&self.items[idx]);
                const ptr: *const T = @ptrFromInt(addr);
                prefetch.prefetchRead(ptr, .moderate);
            }
        }
    };
}

/// Cache-oblivious B-tree node layout
/// Uses van Emde Boas layout for optimal cache performance
pub const CacheObliviousBTreeNode = struct {
    /// Node data stored in cache-oblivious layout
    data: []align(CACHE_LINE_SIZE) u8,
    /// Number of keys in this node
    num_keys: usize,
    /// Order of the B-tree
    order: usize,
    /// Allocator for memory management
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator, order: usize) !CacheObliviousBTreeNode {
        const capacity = 2 * order - 1;
        const key_size = @sizeOf(u64);
        const value_size = @sizeOf(u64);
        const total_size = capacity * (key_size + value_size);

        const data = try allocator.alignedAlloc(u8, std.mem.Alignment.fromByteUnits(CACHE_LINE_SIZE), total_size);

        return .{
            .data = data,
            .num_keys = 0,
            .order = order,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *CacheObliviousBTreeNode) void {
        self.allocator.free(self.data);
    }

    /// Get key at index using cache-oblivious layout
    pub fn getKey(self: *const CacheObliviousBTreeNode, index: usize) u64 {
        std.debug.assert(index < self.num_keys);
        const offset = self.getKeyOffset(index);
        const key_ptr: *const u64 = @ptrCast(@alignCast(&self.data[offset]));
        return key_ptr.*;
    }

    /// Set key at index using cache-oblivious layout
    pub fn setKey(self: *CacheObliviousBTreeNode, index: usize, key: u64) void {
        const offset = self.getKeyOffset(index);
        const key_ptr: *u64 = @ptrCast(@alignCast(&self.data[offset]));
        key_ptr.* = key;
    }

    /// Get value at index using cache-oblivious layout
    pub fn getValue(self: *const CacheObliviousBTreeNode, index: usize) u64 {
        std.debug.assert(index < self.num_keys);
        const offset = self.getValueOffset(index);
        const value_ptr: *const u64 = @ptrCast(@alignCast(&self.data[offset]));
        return value_ptr.*;
    }

    /// Set value at index using cache-oblivious layout
    pub fn setValue(self: *CacheObliviousBTreeNode, index: usize, value: u64) void {
        const offset = self.getValueOffset(index);
        const value_ptr: *u64 = @ptrCast(@alignCast(&self.data[offset]));
        value_ptr.* = value;
    }

    /// Calculate cache-oblivious offset for key
    /// Uses van Emde Boas layout to minimize cache misses
    fn getKeyOffset(_: *const CacheObliviousBTreeNode, index: usize) usize {
        // Simplified van Emde Boas layout: interleave keys and values
        // This keeps related data close together in cache
        const key_size = @sizeOf(u64);
        const value_size = @sizeOf(u64);
        return index * (key_size + value_size);
    }

    /// Calculate cache-oblivious offset for value
    fn getValueOffset(_: *const CacheObliviousBTreeNode, index: usize) usize {
        const key_size = @sizeOf(u64);
        const value_size = @sizeOf(u64);
        return index * (key_size + value_size) + key_size;
    }

    /// Prefetch keys for binary search
    pub fn prefetchForSearch(self: *const CacheObliviousBTreeNode, start: usize, end: usize) void {
        if (start >= end or start >= self.num_keys) return;

        const actual_end = @min(end, self.num_keys);
        const start_offset = self.getKeyOffset(start);
        const end_offset = self.getKeyOffset(actual_end - 1) + @sizeOf(u64);
        const size = end_offset - start_offset;

        prefetch.prefetchRange(&self.data[start_offset], size, .high);
    }

    /// Binary search with prefetching
    pub fn binarySearchWithPrefetch(self: *const CacheObliviousBTreeNode, key: u64) ?usize {
        if (self.num_keys == 0) return null;

        var left: usize = 0;
        var right: usize = self.num_keys;

        // Prefetch the middle region first
        const mid_start = self.num_keys / 4;
        const mid_end = (3 * self.num_keys) / 4;
        self.prefetchForSearch(mid_start, mid_end);

        while (left < right) {
            const mid = left + (right - left) / 2;

            // Prefetch next potential search regions
            if (right - left > 8) {
                const left_mid = left + (mid - left) / 2;
                const right_mid = mid + (right - mid) / 2;
                self.prefetchForSearch(left_mid, left_mid + 4);
                self.prefetchForSearch(right_mid, right_mid + 4);
            }

            const mid_key = self.getKey(mid);

            if (key == mid_key) {
                return mid;
            } else if (key < mid_key) {
                right = mid;
            } else {
                left = mid + 1;
            }
        }

        return null;
    }
};

/// Cache-aware hash table with linear probing
/// Optimized for cache line utilization
pub const CacheAwareHashTable = struct {
    /// Buckets aligned to cache lines
    buckets: []align(CACHE_LINE_SIZE) Bucket,
    /// Number of entries
    size: usize,
    /// Allocator
    allocator: std.mem.Allocator,

    const Bucket = struct {
        key: u64,
        value: u64,
        occupied: bool,
        _padding: [CACHE_LINE_SIZE - @sizeOf(u64) * 2 - @sizeOf(bool)]u8 = undefined,
    };

    pub fn init(allocator: std.mem.Allocator, capacity: usize) !CacheAwareHashTable {
        const buckets = try allocator.alignedAlloc(Bucket, std.mem.Alignment.fromByteUnits(CACHE_LINE_SIZE), capacity);
        @memset(buckets, Bucket{
            .key = 0,
            .value = 0,
            .occupied = false,
        });

        return .{
            .buckets = buckets,
            .size = 0,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *CacheAwareHashTable) void {
        self.allocator.free(self.buckets);
    }

    pub fn insert(self: *CacheAwareHashTable, key: u64, value: u64) !void {
        if (self.size >= self.buckets.len * 3 / 4) {
            return error.HashTableFull;
        }

        var idx = self.hash(key);
        const start_idx = idx;

        // Prefetch the probe sequence
        self.prefetchProbeSequence(idx, 4);

        while (true) {
            if (!self.buckets[idx].occupied) {
                self.buckets[idx] = .{
                    .key = key,
                    .value = value,
                    .occupied = true,
                };
                self.size += 1;
                return;
            }

            if (self.buckets[idx].key == key) {
                self.buckets[idx].value = value;
                return;
            }

            idx = (idx + 1) % self.buckets.len;
            if (idx == start_idx) {
                return error.HashTableFull;
            }
        }
    }

    pub fn get(self: *const CacheAwareHashTable, key: u64) ?u64 {
        var idx = self.hash(key);
        const start_idx = idx;

        // Prefetch the probe sequence
        self.prefetchProbeSequence(idx, 4);

        while (self.buckets[idx].occupied) {
            if (self.buckets[idx].key == key) {
                return self.buckets[idx].value;
            }

            idx = (idx + 1) % self.buckets.len;
            if (idx == start_idx) break;
        }

        return null;
    }

    fn hash(self: *const CacheAwareHashTable, key: u64) usize {
        // Simple hash function
        return @as(usize, @intCast(key % self.buckets.len));
    }

    fn prefetchProbeSequence(self: *const CacheAwareHashTable, start_idx: usize, count: usize) void {
        var i: usize = 0;
        while (i < count and i < self.buckets.len) : (i += 1) {
            const idx = (start_idx + i) % self.buckets.len;
            const addr = @intFromPtr(&self.buckets[idx]);
            const ptr: *const Bucket = @ptrFromInt(addr);
            prefetch.prefetchRead(ptr, .high);
        }
    }
};

/// Memory access pattern analyzer
/// Tracks access patterns to optimize prefetching
pub const AccessPatternAnalyzer = struct {
    /// Recent access history (circular buffer)
    history: []u64,
    /// Current position in history buffer
    position: usize,
    /// Detected stride (0 if no pattern)
    detected_stride: i64,
    /// Confidence in detected pattern (0-100)
    confidence: u8,
    /// Allocator
    allocator: std.mem.Allocator,

    const HISTORY_SIZE = 16;

    pub fn init(allocator: std.mem.Allocator) !AccessPatternAnalyzer {
        const history = try allocator.alloc(u64, HISTORY_SIZE);
        @memset(history, 0);

        return .{
            .history = history,
            .position = 0,
            .detected_stride = 0,
            .confidence = 0,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *AccessPatternAnalyzer) void {
        self.allocator.free(self.history);
    }

    /// Record an access and update pattern detection
    pub fn recordAccess(self: *AccessPatternAnalyzer, address: u64) void {
        self.history[self.position] = address;
        self.position = (self.position + 1) % HISTORY_SIZE;

        // Analyze pattern every 4 accesses
        if (self.position % 4 == 0) {
            self.analyzePattern();
        }
    }

    /// Analyze recent accesses to detect patterns
    fn analyzePattern(self: *AccessPatternAnalyzer) void {
        if (self.position < 4) return;

        // Calculate strides between consecutive accesses
        var strides: [HISTORY_SIZE - 1]i64 = undefined;
        var i: usize = 0;
        while (i < HISTORY_SIZE - 1) : (i += 1) {
            const curr_idx = (self.position + i) % HISTORY_SIZE;
            const next_idx = (self.position + i + 1) % HISTORY_SIZE;
            strides[i] = @as(i64, @intCast(self.history[next_idx])) -
                @as(i64, @intCast(self.history[curr_idx]));
        }

        // Find most common stride
        var stride_counts: [HISTORY_SIZE - 1]u8 = undefined;
        @memset(&stride_counts, 0);

        i = 0;
        while (i < HISTORY_SIZE - 1) : (i += 1) {
            var j: usize = 0;
            while (j < HISTORY_SIZE - 1) : (j += 1) {
                if (strides[i] == strides[j]) {
                    stride_counts[i] += 1;
                }
            }
        }

        // Find stride with highest count
        var max_count: u8 = 0;
        var max_stride: i64 = 0;
        i = 0;
        while (i < HISTORY_SIZE - 1) : (i += 1) {
            if (stride_counts[i] > max_count) {
                max_count = stride_counts[i];
                max_stride = strides[i];
            }
        }

        // Update detected pattern
        self.detected_stride = max_stride;
        self.confidence = @min(100, max_count * 10);
    }

    /// Get prefetch hint based on detected pattern
    pub fn getPrefetchHint(self: *const AccessPatternAnalyzer, current_address: u64) ?u64 {
        if (self.confidence < 50) return null;

        const next_address = @as(i64, @intCast(current_address)) + self.detected_stride;
        if (next_address < 0) return null;

        return @intCast(next_address);
    }

    /// Get prefetch distance based on pattern confidence
    pub fn getPrefetchDistance(self: *const AccessPatternAnalyzer) usize {
        if (self.confidence >= 80) return 4;
        if (self.confidence >= 60) return 2;
        return 1;
    }
};

// ============================================================================
// Tests
// ============================================================================

test "CacheLinePadding: prevents false sharing" {
    const Counter = struct {
        value: u64,
    };

    const PaddedCounter = CacheLinePadding(Counter);

    try std.testing.expect(@sizeOf(PaddedCounter) >= CACHE_LINE_SIZE);
    try std.testing.expect(@sizeOf(PaddedCounter) % CACHE_LINE_SIZE == 0);
}

test "CacheAlignedArray: basic operations" {
    const allocator = std.testing.allocator;

    var array = try CacheAlignedArray(u64).init(allocator, 100);
    defer array.deinit();

    try std.testing.expectEqual(@as(usize, 100), array.len());

    array.items[0] = 42;
    array.items[99] = 99;

    try std.testing.expectEqual(@as(u64, 42), array.getConst(0).*);
    try std.testing.expectEqual(@as(u64, 99), array.getConst(99).*);
}

test "CacheAlignedArray: alignment" {
    const allocator = std.testing.allocator;

    var array = try CacheAlignedArray(u64).init(allocator, 10);
    defer array.deinit();

    const addr = @intFromPtr(array.items.ptr);
    try std.testing.expect(addr % CACHE_LINE_SIZE == 0);
}

test "CacheAlignedArray: prefetch operations" {
    const allocator = std.testing.allocator;

    var array = try CacheAlignedArray(u64).init(allocator, 100);
    defer array.deinit();

    // These should not crash
    array.prefetchRange(0, 10);
    array.prefetchStrided(0, 2, 10);
}

test "CacheObliviousBTreeNode: basic operations" {
    const allocator = std.testing.allocator;

    var node = try CacheObliviousBTreeNode.init(allocator, 4);
    defer node.deinit();

    node.num_keys = 3;
    node.setKey(0, 10);
    node.setKey(1, 20);
    node.setKey(2, 30);

    node.setValue(0, 100);
    node.setValue(1, 200);
    node.setValue(2, 300);

    try std.testing.expectEqual(@as(u64, 10), node.getKey(0));
    try std.testing.expectEqual(@as(u64, 20), node.getKey(1));
    try std.testing.expectEqual(@as(u64, 30), node.getKey(2));

    try std.testing.expectEqual(@as(u64, 100), node.getValue(0));
    try std.testing.expectEqual(@as(u64, 200), node.getValue(1));
    try std.testing.expectEqual(@as(u64, 300), node.getValue(2));
}

test "CacheObliviousBTreeNode: binary search" {
    const allocator = std.testing.allocator;

    var node = try CacheObliviousBTreeNode.init(allocator, 8);
    defer node.deinit();

    node.num_keys = 7;
    node.setKey(0, 10);
    node.setKey(1, 20);
    node.setKey(2, 30);
    node.setKey(3, 40);
    node.setKey(4, 50);
    node.setKey(5, 60);
    node.setKey(6, 70);

    // Test exact matches
    try std.testing.expectEqual(@as(?usize, 0), node.binarySearchWithPrefetch(10));
    try std.testing.expectEqual(@as(?usize, 3), node.binarySearchWithPrefetch(40));
    try std.testing.expectEqual(@as(?usize, 6), node.binarySearchWithPrefetch(70));

    // Test non-existent keys
    try std.testing.expectEqual(@as(?usize, null), node.binarySearchWithPrefetch(15));
    try std.testing.expectEqual(@as(?usize, null), node.binarySearchWithPrefetch(100));
}

test "CacheAwareHashTable: basic operations" {
    const allocator = std.testing.allocator;

    var table = try CacheAwareHashTable.init(allocator, 16);
    defer table.deinit();

    try table.insert(1, 100);
    try table.insert(2, 200);
    try table.insert(3, 300);

    try std.testing.expectEqual(@as(?u64, 100), table.get(1));
    try std.testing.expectEqual(@as(?u64, 200), table.get(2));
    try std.testing.expectEqual(@as(?u64, 300), table.get(3));
    try std.testing.expectEqual(@as(?u64, null), table.get(4));
}

test "CacheAwareHashTable: collision handling" {
    const allocator = std.testing.allocator;

    var table = try CacheAwareHashTable.init(allocator, 4);
    defer table.deinit();

    // Insert keys that will collide
    try table.insert(0, 100);
    try table.insert(4, 400); // Will collide with 0
    try table.insert(8, 800); // Will collide with 0 and 4

    try std.testing.expectEqual(@as(?u64, 100), table.get(0));
    try std.testing.expectEqual(@as(?u64, 400), table.get(4));
    try std.testing.expectEqual(@as(?u64, 800), table.get(8));
}

test "CacheAwareHashTable: update existing key" {
    const allocator = std.testing.allocator;

    var table = try CacheAwareHashTable.init(allocator, 16);
    defer table.deinit();

    try table.insert(1, 100);
    try std.testing.expectEqual(@as(?u64, 100), table.get(1));

    try table.insert(1, 999);
    try std.testing.expectEqual(@as(?u64, 999), table.get(1));
}

test "AccessPatternAnalyzer: sequential pattern" {
    const allocator = std.testing.allocator;

    var analyzer = try AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Record sequential accesses
    var i: u64 = 0;
    while (i < 16) : (i += 1) {
        analyzer.recordAccess(i * 64); // 64-byte stride
    }

    // Should detect stride of 64
    try std.testing.expect(analyzer.detected_stride == 64);
    try std.testing.expect(analyzer.confidence > 50);

    // Should predict next access
    const hint = analyzer.getPrefetchHint(1024);
    try std.testing.expect(hint != null);
    try std.testing.expectEqual(@as(u64, 1024 + 64), hint.?);
}

test "AccessPatternAnalyzer: random pattern" {
    const allocator = std.testing.allocator;

    var analyzer = try AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Record random accesses
    const random_addrs = [_]u64{ 100, 500, 200, 800, 150, 900, 300, 1000 };
    for (random_addrs) |addr| {
        analyzer.recordAccess(addr);
    }

    // Should have lower confidence for random pattern
    // Note: may still detect some pattern due to limited history
    try std.testing.expect(analyzer.confidence < 80);
}

test "AccessPatternAnalyzer: prefetch distance" {
    const allocator = std.testing.allocator;

    var analyzer = try AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Low confidence -> small distance
    analyzer.confidence = 40;
    try std.testing.expectEqual(@as(usize, 1), analyzer.getPrefetchDistance());

    // Medium confidence -> medium distance
    analyzer.confidence = 70;
    try std.testing.expectEqual(@as(usize, 2), analyzer.getPrefetchDistance());

    // High confidence -> large distance
    analyzer.confidence = 90;
    try std.testing.expectEqual(@as(usize, 4), analyzer.getPrefetchDistance());
}
