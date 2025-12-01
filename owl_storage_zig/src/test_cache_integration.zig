// Task 9.9: Integration test demonstrating cache-aware optimizations
// in a realistic storage engine scenario

const std = @import("std");
const cache_aware = @import("cache_aware.zig");
const prefetch = @import("prefetch.zig");

test "Cache integration: simulated object navigation" {
    const allocator = std.testing.allocator;

    // Simulate an arena with objects
    var objects = try cache_aware.CacheAlignedArray(u64).init(allocator, 1000);
    defer objects.deinit();

    // Fill with "object addresses"
    for (objects.items, 0..) |*item, i| {
        item.* = i * 256; // Simulate 256-byte objects
    }

    // Create access pattern analyzer
    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Simulate sequential navigation with adaptive prefetching
    var sum: u64 = 0;
    var i: usize = 0;
    while (i < 100) : (i += 1) {
        const addr = @intFromPtr(&objects.items[i]);
        analyzer.recordAccess(addr);

        // Adaptive prefetching based on detected pattern
        if (analyzer.getPrefetchHint(addr)) |hint_addr| {
            const distance = analyzer.getPrefetchDistance();
            const hint_idx = (hint_addr - @intFromPtr(objects.items.ptr)) / @sizeOf(u64);
            if (hint_idx < objects.len()) {
                objects.prefetchRange(hint_idx, distance);
            }
        }

        sum += objects.items[i];
    }

    // Should have detected sequential pattern
    try std.testing.expect(analyzer.confidence > 60);
    try std.testing.expectEqual(@as(i64, @sizeOf(u64)), analyzer.detected_stride);
}

test "Cache integration: B-tree index with prefetching" {
    const allocator = std.testing.allocator;

    // Create cache-oblivious B-tree node
    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 32);
    defer node.deinit();

    // Populate with sorted keys
    node.num_keys = 50;
    var i: usize = 0;
    while (i < 50) : (i += 1) {
        node.setKey(i, i * 10);
        node.setValue(i, i * 100);
    }

    // Perform multiple searches with prefetching
    const search_keys = [_]u64{ 50, 100, 150, 200, 250, 300, 350, 400, 450 };
    var found_count: usize = 0;

    for (search_keys) |key| {
        if (node.binarySearchWithPrefetch(key)) |_| {
            found_count += 1;
        }
    }

    try std.testing.expectEqual(@as(usize, 9), found_count);
}

test "Cache integration: hash table with collision prefetching" {
    const allocator = std.testing.allocator;

    var table = try cache_aware.CacheAwareHashTable.init(allocator, 128);
    defer table.deinit();

    // Insert many keys to create collisions
    var i: u64 = 0;
    while (i < 50) : (i += 1) {
        try table.insert(i, i * 100);
    }

    // Lookup with probe sequence prefetching
    var sum: u64 = 0;
    i = 0;
    while (i < 50) : (i += 1) {
        if (table.get(i)) |value| {
            sum += value;
        }
    }

    // All values should be found
    const expected_sum = (0 + 49) * 50 / 2 * 100; // Sum of arithmetic sequence
    try std.testing.expectEqual(expected_sum, sum);
}

test "Cache integration: false sharing prevention" {
    _ = std.testing.allocator;

    const Counter = struct {
        value: std.atomic.Value(u64),

        pub fn init() @This() {
            return .{ .value = std.atomic.Value(u64).init(0) };
        }

        pub fn increment(self: *@This()) void {
            _ = self.value.fetchAdd(1, .seq_cst);
        }
    };

    const PaddedCounter = cache_aware.CacheLinePadding(Counter);

    // Create array of padded counters
    var counters: [4]PaddedCounter = undefined;
    for (&counters) |*counter| {
        counter.* = PaddedCounter.init(Counter.init());
    }

    // Simulate concurrent access (single-threaded for test)
    var i: usize = 0;
    while (i < 1000) : (i += 1) {
        counters[i % 4].get().increment();
    }

    // Verify counts
    var total: u64 = 0;
    for (&counters) |*counter| {
        total += counter.get().value.load(.seq_cst);
    }

    try std.testing.expectEqual(@as(u64, 1000), total);

    // Verify padding prevents false sharing
    try std.testing.expect(@sizeOf(PaddedCounter) >= cache_aware.CACHE_LINE_SIZE);
}

test "Cache integration: adaptive prefetching for different patterns" {
    const allocator = std.testing.allocator;

    // Test 1: Sequential pattern
    {
        var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
        defer analyzer.deinit();

        var i: u64 = 0;
        while (i < 16) : (i += 1) {
            analyzer.recordAccess(i * 64);
        }

        try std.testing.expect(analyzer.confidence > 60);
        try std.testing.expectEqual(@as(usize, 4), analyzer.getPrefetchDistance());
    }

    // Test 2: Strided pattern
    {
        var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
        defer analyzer.deinit();

        var i: u64 = 0;
        while (i < 16) : (i += 1) {
            analyzer.recordAccess(i * 256);
        }

        try std.testing.expect(analyzer.confidence > 60);
        try std.testing.expectEqual(@as(i64, 256), analyzer.detected_stride);
    }

    // Test 3: Random pattern
    {
        var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
        defer analyzer.deinit();

        const random_addrs = [_]u64{ 100, 500, 200, 800, 150, 900, 300, 1000 };
        for (random_addrs) |addr| {
            analyzer.recordAccess(addr);
        }

        // Lower confidence for random pattern
        try std.testing.expect(analyzer.confidence < 80);
    }
}

test "Cache integration: memory layout optimization" {
    // Verify cache line size constant
    try std.testing.expectEqual(@as(usize, 64), cache_aware.CACHE_LINE_SIZE);

    // Verify alignment helper
    const aligned_addr = std.mem.alignForward(usize, 100, cache_aware.CACHE_LINE_SIZE);
    try std.testing.expectEqual(@as(usize, 128), aligned_addr);

    // Verify cache line padding size calculation
    const SmallStruct = struct {
        a: u32,
        b: u32,
    };

    const PaddedStruct = cache_aware.CacheLinePadding(SmallStruct);
    try std.testing.expect(@sizeOf(PaddedStruct) % cache_aware.CACHE_LINE_SIZE == 0);
}
