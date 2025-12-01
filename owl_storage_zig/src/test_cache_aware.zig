// Task 9.9: Tests for Cache-Aware Data Structures
// Comprehensive tests for cache-line alignment, cache-oblivious algorithms,
// and memory access pattern optimization

const std = @import("std");
const cache_aware = @import("cache_aware.zig");
const prefetch = @import("prefetch.zig");

test "Cache-aware: all tests" {
    std.testing.refAllDecls(@This());
}

test "Cache line size constant" {
    try std.testing.expectEqual(@as(usize, 64), cache_aware.CACHE_LINE_SIZE);
}

test "CacheLinePadding: size and alignment" {
    const SmallStruct = struct {
        value: u32,
    };

    const Padded = cache_aware.CacheLinePadding(SmallStruct);

    // Should be padded to cache line size
    try std.testing.expect(@sizeOf(Padded) >= cache_aware.CACHE_LINE_SIZE);
    try std.testing.expect(@sizeOf(Padded) % cache_aware.CACHE_LINE_SIZE == 0);

    // Test initialization and access
    var padded = Padded.init(.{ .value = 42 });
    try std.testing.expectEqual(@as(u32, 42), padded.get().value);

    padded.get().value = 99;
    try std.testing.expectEqual(@as(u32, 99), padded.getConst().value);
}

test "CacheLinePadding: prevents false sharing" {
    const Counter = struct {
        count: std.atomic.Value(u64),

        pub fn init() @This() {
            return .{ .count = std.atomic.Value(u64).init(0) };
        }
    };

    const PaddedCounter = cache_aware.CacheLinePadding(Counter);

    // Create two padded counters
    var counter1 = PaddedCounter.init(Counter.init());
    var counter2 = PaddedCounter.init(Counter.init());

    // They should be on different cache lines
    _ = &counter1;
    _ = &counter2;

    // If they're in an array, they should be at least one cache line apart
    try std.testing.expect(@sizeOf(PaddedCounter) >= cache_aware.CACHE_LINE_SIZE);
}

test "CacheAlignedArray: creation and basic access" {
    const allocator = std.testing.allocator;

    var array = try cache_aware.CacheAlignedArray(u64).init(allocator, 100);
    defer array.deinit();

    try std.testing.expectEqual(@as(usize, 100), array.len());

    // Test write and read
    array.items[0] = 1;
    array.items[50] = 50;
    array.items[99] = 99;

    try std.testing.expectEqual(@as(u64, 1), array.getConst(0).*);
    try std.testing.expectEqual(@as(u64, 50), array.getConst(50).*);
    try std.testing.expectEqual(@as(u64, 99), array.getConst(99).*);
}

test "CacheAlignedArray: cache line alignment" {
    const allocator = std.testing.allocator;

    var array = try cache_aware.CacheAlignedArray(u32).init(allocator, 50);
    defer array.deinit();

    // Check that array starts on cache line boundary
    const addr = @intFromPtr(array.items.ptr);
    try std.testing.expect(addr % cache_aware.CACHE_LINE_SIZE == 0);
}

test "CacheAlignedArray: prefetch range" {
    const allocator = std.testing.allocator;

    var array = try cache_aware.CacheAlignedArray(u64).init(allocator, 1000);
    defer array.deinit();

    // Fill with data
    for (array.items, 0..) |*item, i| {
        item.* = i;
    }

    // Prefetch should not crash
    array.prefetchRange(0, 100);
    array.prefetchRange(500, 100);
    array.prefetchRange(900, 100); // Near end

    // Verify data is still correct
    try std.testing.expectEqual(@as(u64, 0), array.items[0]);
    try std.testing.expectEqual(@as(u64, 500), array.items[500]);
    try std.testing.expectEqual(@as(u64, 999), array.items[999]);
}

test "CacheAlignedArray: prefetch strided" {
    const allocator = std.testing.allocator;

    var array = try cache_aware.CacheAlignedArray(u64).init(allocator, 1000);
    defer array.deinit();

    // Fill with data
    for (array.items, 0..) |*item, i| {
        item.* = i * 2;
    }

    // Prefetch every 10th element
    array.prefetchStrided(0, 10, 50);

    // Verify data
    try std.testing.expectEqual(@as(u64, 0), array.items[0]);
    try std.testing.expectEqual(@as(u64, 20), array.items[10]);
    try std.testing.expectEqual(@as(u64, 40), array.items[20]);
}

test "CacheObliviousBTreeNode: initialization" {
    const allocator = std.testing.allocator;

    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 4);
    defer node.deinit();

    try std.testing.expectEqual(@as(usize, 0), node.num_keys);
    try std.testing.expectEqual(@as(usize, 4), node.order);

    // Check cache line alignment
    const addr = @intFromPtr(node.data.ptr);
    try std.testing.expect(addr % cache_aware.CACHE_LINE_SIZE == 0);
}

test "CacheObliviousBTreeNode: key-value operations" {
    const allocator = std.testing.allocator;

    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 8);
    defer node.deinit();

    node.num_keys = 5;

    // Set keys and values
    node.setKey(0, 100);
    node.setKey(1, 200);
    node.setKey(2, 300);
    node.setKey(3, 400);
    node.setKey(4, 500);

    node.setValue(0, 1000);
    node.setValue(1, 2000);
    node.setValue(2, 3000);
    node.setValue(3, 4000);
    node.setValue(4, 5000);

    // Verify keys
    try std.testing.expectEqual(@as(u64, 100), node.getKey(0));
    try std.testing.expectEqual(@as(u64, 200), node.getKey(1));
    try std.testing.expectEqual(@as(u64, 300), node.getKey(2));
    try std.testing.expectEqual(@as(u64, 400), node.getKey(3));
    try std.testing.expectEqual(@as(u64, 500), node.getKey(4));

    // Verify values
    try std.testing.expectEqual(@as(u64, 1000), node.getValue(0));
    try std.testing.expectEqual(@as(u64, 2000), node.getValue(1));
    try std.testing.expectEqual(@as(u64, 3000), node.getValue(2));
    try std.testing.expectEqual(@as(u64, 4000), node.getValue(3));
    try std.testing.expectEqual(@as(u64, 5000), node.getValue(4));
}

test "CacheObliviousBTreeNode: binary search found" {
    const allocator = std.testing.allocator;

    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 16);
    defer node.deinit();

    node.num_keys = 10;
    var i: usize = 0;
    while (i < 10) : (i += 1) {
        node.setKey(i, (i + 1) * 10);
        node.setValue(i, (i + 1) * 100);
    }

    // Search for existing keys
    try std.testing.expectEqual(@as(?usize, 0), node.binarySearchWithPrefetch(10));
    try std.testing.expectEqual(@as(?usize, 4), node.binarySearchWithPrefetch(50));
    try std.testing.expectEqual(@as(?usize, 9), node.binarySearchWithPrefetch(100));
}

test "CacheObliviousBTreeNode: binary search not found" {
    const allocator = std.testing.allocator;

    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 16);
    defer node.deinit();

    node.num_keys = 5;
    node.setKey(0, 10);
    node.setKey(1, 30);
    node.setKey(2, 50);
    node.setKey(3, 70);
    node.setKey(4, 90);

    // Search for non-existent keys
    try std.testing.expectEqual(@as(?usize, null), node.binarySearchWithPrefetch(5));
    try std.testing.expectEqual(@as(?usize, null), node.binarySearchWithPrefetch(20));
    try std.testing.expectEqual(@as(?usize, null), node.binarySearchWithPrefetch(100));
}

test "CacheObliviousBTreeNode: prefetch for search" {
    const allocator = std.testing.allocator;

    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 32);
    defer node.deinit();

    node.num_keys = 20;
    var i: usize = 0;
    while (i < 20) : (i += 1) {
        node.setKey(i, i * 5);
    }

    // Prefetch should not crash
    node.prefetchForSearch(0, 10);
    node.prefetchForSearch(10, 20);
    node.prefetchForSearch(15, 20); // Partial range
}

test "CacheAwareHashTable: initialization" {
    const allocator = std.testing.allocator;

    var table = try cache_aware.CacheAwareHashTable.init(allocator, 32);
    defer table.deinit();

    try std.testing.expectEqual(@as(usize, 0), table.size);
    try std.testing.expectEqual(@as(usize, 32), table.buckets.len);
}

test "CacheAwareHashTable: insert and get" {
    const allocator = std.testing.allocator;

    var table = try cache_aware.CacheAwareHashTable.init(allocator, 64);
    defer table.deinit();

    // Insert multiple entries
    try table.insert(1, 100);
    try table.insert(2, 200);
    try table.insert(3, 300);
    try table.insert(10, 1000);
    try table.insert(20, 2000);

    // Verify retrieval
    try std.testing.expectEqual(@as(?u64, 100), table.get(1));
    try std.testing.expectEqual(@as(?u64, 200), table.get(2));
    try std.testing.expectEqual(@as(?u64, 300), table.get(3));
    try std.testing.expectEqual(@as(?u64, 1000), table.get(10));
    try std.testing.expectEqual(@as(?u64, 2000), table.get(20));

    // Non-existent key
    try std.testing.expectEqual(@as(?u64, null), table.get(999));
}

test "CacheAwareHashTable: collision resolution" {
    const allocator = std.testing.allocator;

    var table = try cache_aware.CacheAwareHashTable.init(allocator, 8);
    defer table.deinit();

    // Insert keys that will definitely collide (same hash)
    try table.insert(0, 100);
    try table.insert(8, 800); // hash(8) == hash(0) for size 8
    try table.insert(16, 1600); // hash(16) == hash(0) for size 8

    // All should be retrievable
    try std.testing.expectEqual(@as(?u64, 100), table.get(0));
    try std.testing.expectEqual(@as(?u64, 800), table.get(8));
    try std.testing.expectEqual(@as(?u64, 1600), table.get(16));
}

test "CacheAwareHashTable: update value" {
    const allocator = std.testing.allocator;

    var table = try cache_aware.CacheAwareHashTable.init(allocator, 32);
    defer table.deinit();

    try table.insert(5, 500);
    try std.testing.expectEqual(@as(?u64, 500), table.get(5));

    // Update the value
    try table.insert(5, 5000);
    try std.testing.expectEqual(@as(?u64, 5000), table.get(5));

    // Size should not increase
    try std.testing.expectEqual(@as(usize, 1), table.size);
}

test "CacheAwareHashTable: bucket alignment" {
    const allocator = std.testing.allocator;

    var table = try cache_aware.CacheAwareHashTable.init(allocator, 16);
    defer table.deinit();

    // Check that buckets are cache-line aligned
    const addr = @intFromPtr(table.buckets.ptr);
    try std.testing.expect(addr % cache_aware.CACHE_LINE_SIZE == 0);

    // Check that each bucket is cache-line sized
    try std.testing.expect(@sizeOf(@TypeOf(table.buckets[0])) == cache_aware.CACHE_LINE_SIZE);
}

test "AccessPatternAnalyzer: initialization" {
    const allocator = std.testing.allocator;

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    try std.testing.expectEqual(@as(usize, 0), analyzer.position);
    try std.testing.expectEqual(@as(i64, 0), analyzer.detected_stride);
    try std.testing.expectEqual(@as(u8, 0), analyzer.confidence);
}

test "AccessPatternAnalyzer: sequential access pattern" {
    const allocator = std.testing.allocator;

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Record sequential accesses with stride 100
    var i: u64 = 0;
    while (i < 16) : (i += 1) {
        analyzer.recordAccess(i * 100);
    }

    // Should detect stride of 100
    try std.testing.expectEqual(@as(i64, 100), analyzer.detected_stride);
    try std.testing.expect(analyzer.confidence > 50);
}

test "AccessPatternAnalyzer: strided access pattern" {
    const allocator = std.testing.allocator;

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Record accesses with stride 64 (cache line size)
    var i: u64 = 1000;
    while (i < 1000 + 16 * 64) : (i += 64) {
        analyzer.recordAccess(i);
    }

    // Should detect stride of 64
    try std.testing.expectEqual(@as(i64, 64), analyzer.detected_stride);
    try std.testing.expect(analyzer.confidence > 60);
}

test "AccessPatternAnalyzer: random access pattern" {
    const allocator = std.testing.allocator;

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Record random accesses
    const random_addrs = [_]u64{
        1000,  5000,  2000,  8000,  1500,
        9000,  3000,  10000, 4000,  11000,
        6000,  12000, 7000,  13000, 14000,
        15000,
    };

    for (random_addrs) |addr| {
        analyzer.recordAccess(addr);
    }

    // Should have lower confidence for random pattern than sequential
    // Note: may still detect some pattern due to limited history
    try std.testing.expect(analyzer.confidence < 80);
}

test "AccessPatternAnalyzer: prefetch hint" {
    const allocator = std.testing.allocator;

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Establish a pattern
    var i: u64 = 0;
    while (i < 16) : (i += 1) {
        analyzer.recordAccess(i * 256);
    }

    // Should provide prefetch hint
    const hint = analyzer.getPrefetchHint(5000);
    try std.testing.expect(hint != null);
    try std.testing.expectEqual(@as(u64, 5000 + 256), hint.?);
}

test "AccessPatternAnalyzer: no hint for low confidence" {
    const allocator = std.testing.allocator;

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Record just a few random accesses
    analyzer.recordAccess(100);
    analyzer.recordAccess(500);
    analyzer.recordAccess(200);

    // Should not provide hint with low confidence
    const hint = analyzer.getPrefetchHint(1000);
    try std.testing.expect(hint == null);
}

test "AccessPatternAnalyzer: prefetch distance scaling" {
    const allocator = std.testing.allocator;

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Low confidence
    analyzer.confidence = 30;
    try std.testing.expectEqual(@as(usize, 1), analyzer.getPrefetchDistance());

    // Medium confidence
    analyzer.confidence = 65;
    try std.testing.expectEqual(@as(usize, 2), analyzer.getPrefetchDistance());

    // High confidence
    analyzer.confidence = 85;
    try std.testing.expectEqual(@as(usize, 4), analyzer.getPrefetchDistance());
}

test "Integration: cache-aware array with pattern analyzer" {
    const allocator = std.testing.allocator;

    var array = try cache_aware.CacheAlignedArray(u64).init(allocator, 1000);
    defer array.deinit();

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Fill array
    for (array.items, 0..) |*item, i| {
        item.* = i;
    }

    // Simulate sequential access pattern
    var i: usize = 0;
    while (i < 100) : (i += 1) {
        const addr = @intFromPtr(&array.items[i]);
        analyzer.recordAccess(addr);

        // Use prefetch hint if available
        if (analyzer.getPrefetchHint(addr)) |hint_addr| {
            const hint_idx = (hint_addr - @intFromPtr(array.items.ptr)) / @sizeOf(u64);
            if (hint_idx < array.len()) {
                array.prefetchRange(hint_idx, analyzer.getPrefetchDistance());
            }
        }
    }

    // Should have detected pattern
    try std.testing.expect(analyzer.confidence > 50);
}

test "Integration: cache-oblivious B-tree with prefetching" {
    const allocator = std.testing.allocator;

    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 32);
    defer node.deinit();

    // Fill with sorted keys
    node.num_keys = 50;
    var i: usize = 0;
    while (i < 50) : (i += 1) {
        node.setKey(i, i * 10);
        node.setValue(i, i * 100);
    }

    // Perform multiple searches with prefetching
    const search_keys = [_]u64{ 50, 150, 250, 350, 450 };
    for (search_keys) |key| {
        const result = node.binarySearchWithPrefetch(key);
        try std.testing.expect(result != null);
    }
}
