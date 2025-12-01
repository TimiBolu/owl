// Task 9.9: Cache-Aware Data Structures Demo
// Demonstrates CPU cache-aware optimizations for Owl storage engine

const std = @import("std");
const cache_aware = @import("cache_aware");
const prefetch = @import("prefetch");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n=== Cache-Aware Data Structures Demo ===\n\n", .{});

    // Demo 1: Cache-aligned arrays
    try demoCacheAlignedArray(allocator);

    // Demo 2: Cache-oblivious B-tree
    try demoCacheObliviousBTree(allocator);

    // Demo 3: Cache-aware hash table
    try demoCacheAwareHashTable(allocator);

    // Demo 4: Access pattern analyzer
    try demoAccessPatternAnalyzer(allocator);

    std.debug.print("\n=== Demo Complete ===\n", .{});
}

fn demoCacheAlignedArray(allocator: std.mem.Allocator) !void {
    std.debug.print("1. Cache-Aligned Array Demo\n", .{});
    std.debug.print("   Cache line size: {} bytes\n", .{cache_aware.CACHE_LINE_SIZE});

    var array = try cache_aware.CacheAlignedArray(u64).init(allocator, 1000);
    defer array.deinit();

    // Fill with sequential data
    for (array.items, 0..) |*item, i| {
        item.* = i * 100;
    }

    // Check alignment
    const addr = @intFromPtr(array.items.ptr);
    const is_aligned = addr % cache_aware.CACHE_LINE_SIZE == 0;
    std.debug.print("   Array is cache-line aligned: {}\n", .{is_aligned});

    // Demonstrate prefetching
    std.debug.print("   Prefetching range [0..100]...\n", .{});
    array.prefetchRange(0, 100);

    std.debug.print("   Prefetching strided access (every 10th element)...\n", .{});
    array.prefetchStrided(0, 10, 50);

    // Simulate sequential access
    var sum: u64 = 0;
    for (array.items[0..100]) |item| {
        sum += item;
    }
    std.debug.print("   Sum of first 100 elements: {}\n", .{sum});
    std.debug.print("\n", .{});
}

fn demoCacheObliviousBTree(allocator: std.mem.Allocator) !void {
    std.debug.print("2. Cache-Oblivious B-Tree Demo\n", .{});

    var node = try cache_aware.CacheObliviousBTreeNode.init(allocator, 16);
    defer node.deinit();

    // Insert sorted keys
    node.num_keys = 20;
    var i: usize = 0;
    while (i < 20) : (i += 1) {
        node.setKey(i, (i + 1) * 10);
        node.setValue(i, (i + 1) * 1000);
    }

    std.debug.print("   Inserted {} keys\n", .{node.num_keys});

    // Demonstrate binary search with prefetching
    const search_keys = [_]u64{ 50, 100, 150, 200 };
    std.debug.print("   Searching for keys with prefetching:\n", .{});

    for (search_keys) |key| {
        if (node.binarySearchWithPrefetch(key)) |idx| {
            const value = node.getValue(idx);
            std.debug.print("     Key {} found at index {}, value: {}\n", .{ key, idx, value });
        } else {
            std.debug.print("     Key {} not found\n", .{key});
        }
    }

    // Check data alignment
    const data_addr = @intFromPtr(node.data.ptr);
    const is_aligned = data_addr % cache_aware.CACHE_LINE_SIZE == 0;
    std.debug.print("   Node data is cache-line aligned: {}\n", .{is_aligned});
    std.debug.print("\n", .{});
}

fn demoCacheAwareHashTable(allocator: std.mem.Allocator) !void {
    std.debug.print("3. Cache-Aware Hash Table Demo\n", .{});

    var table = try cache_aware.CacheAwareHashTable.init(allocator, 128);
    defer table.deinit();

    // Insert key-value pairs
    std.debug.print("   Inserting 50 key-value pairs...\n", .{});
    var i: u64 = 1;
    while (i <= 50) : (i += 1) {
        try table.insert(i, i * 100);
    }

    std.debug.print("   Table size: {}\n", .{table.size});

    // Demonstrate lookups with prefetching
    std.debug.print("   Looking up keys with probe sequence prefetching:\n", .{});
    const lookup_keys = [_]u64{ 5, 15, 25, 35, 45 };

    for (lookup_keys) |key| {
        if (table.get(key)) |value| {
            std.debug.print("     Key {}: {}\n", .{ key, value });
        }
    }

    // Check bucket alignment
    const bucket_addr = @intFromPtr(table.buckets.ptr);
    const is_aligned = bucket_addr % cache_aware.CACHE_LINE_SIZE == 0;
    std.debug.print("   Buckets are cache-line aligned: {}\n", .{is_aligned});
    std.debug.print("   Each bucket size: {} bytes\n", .{@sizeOf(@TypeOf(table.buckets[0]))});
    std.debug.print("\n", .{});
}

fn demoAccessPatternAnalyzer(allocator: std.mem.Allocator) !void {
    std.debug.print("4. Access Pattern Analyzer Demo\n", .{});

    var analyzer = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer.deinit();

    // Simulate sequential access pattern
    std.debug.print("   Recording sequential access pattern (stride 64)...\n", .{});
    var i: u64 = 0;
    while (i < 16) : (i += 1) {
        analyzer.recordAccess(i * 64);
    }

    std.debug.print("   Detected stride: {}\n", .{analyzer.detected_stride});
    std.debug.print("   Confidence: {}%\n", .{analyzer.confidence});
    std.debug.print("   Prefetch distance: {}\n", .{analyzer.getPrefetchDistance()});

    // Get prefetch hint
    if (analyzer.getPrefetchHint(1024)) |hint| {
        std.debug.print("   Prefetch hint for address 1024: {}\n", .{hint});
    }

    // Simulate random access pattern
    std.debug.print("\n   Recording random access pattern...\n", .{});
    var analyzer2 = try cache_aware.AccessPatternAnalyzer.init(allocator);
    defer analyzer2.deinit();

    const random_addrs = [_]u64{ 100, 500, 200, 800, 150, 900, 300, 1000, 400, 1100, 600, 1200, 700, 1300, 1400, 1500 };
    for (random_addrs) |addr| {
        analyzer2.recordAccess(addr);
    }

    std.debug.print("   Detected stride: {}\n", .{analyzer2.detected_stride});
    std.debug.print("   Confidence: {}%\n", .{analyzer2.confidence});
    std.debug.print("   Prefetch distance: {}\n", .{analyzer2.getPrefetchDistance()});

    if (analyzer2.getPrefetchHint(2000)) |hint| {
        std.debug.print("   Prefetch hint for address 2000: {}\n", .{hint});
    } else {
        std.debug.print("   No prefetch hint (confidence too low)\n", .{});
    }

    std.debug.print("\n", .{});
}

fn demonstratePrefetching(allocator: std.mem.Allocator) !void {
    std.debug.print("5. Prefetching Strategies Demo\n", .{});

    // Create a large array to demonstrate prefetching
    var array = try cache_aware.CacheAlignedArray(u64).init(allocator, 10000);
    defer array.deinit();

    // Fill with data
    for (array.items, 0..) |*item, i| {
        item.* = i;
    }

    std.debug.print("   Array size: {} elements\n", .{array.len()});

    // Strategy 1: Sequential prefetching
    std.debug.print("   Strategy 1: Sequential prefetching\n", .{});
    var sum1: u64 = 0;
    var idx: usize = 0;
    while (idx < 1000) : (idx += 1) {
        // Prefetch ahead
        if (idx + 8 < array.len()) {
            array.prefetchRange(idx + 8, 8);
        }
        sum1 += array.items[idx];
    }
    std.debug.print("     Sum: {}\n", .{sum1});

    // Strategy 2: Strided prefetching
    std.debug.print("   Strategy 2: Strided prefetching (every 10th element)\n", .{});
    var sum2: u64 = 0;
    idx = 0;
    while (idx < 1000) : (idx += 10) {
        // Prefetch next stride
        if (idx + 40 < array.len()) {
            array.prefetchStrided(idx + 40, 10, 4);
        }
        sum2 += array.items[idx];
    }
    std.debug.print("     Sum: {}\n", .{sum2});

    std.debug.print("\n", .{});
}
