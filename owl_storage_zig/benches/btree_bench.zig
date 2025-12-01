// B-tree benchmark suite
// Measures performance of B-tree operations

const std = @import("std");
const storage = @import("storage");
const BTree = storage.BTree;
const BTreeConfig = storage.BTreeConfig;
const Key = storage.Key;
const ObjRef = storage.ObjRef;

const ITERATIONS = 10_000;

fn benchmarkInsert(allocator: std.mem.Allocator, order: usize) !u64 {
    const config = BTreeConfig{ .order = order };
    const tree = try BTree.init(allocator, config, 0); // shard_id = 0
    defer tree.deinit();

    const start = std.time.nanoTimestamp();

    for (0..ITERATIONS) |i| {
        try tree.insert(i, ObjRef.init(i * 10));
    }

    const end = std.time.nanoTimestamp();
    return @intCast(end - start);
}

fn benchmarkSearch(allocator: std.mem.Allocator, order: usize) !u64 {
    const config = BTreeConfig{ .order = order };
    const tree = try BTree.init(allocator, config, 0); // shard_id = 0
    defer tree.deinit();

    // Pre-populate tree
    for (0..ITERATIONS) |i| {
        try tree.insert(i, ObjRef.init(i * 10));
    }

    const start = std.time.nanoTimestamp();

    for (0..ITERATIONS) |i| {
        _ = tree.search(i);
    }

    const end = std.time.nanoTimestamp();
    return @intCast(end - start);
}

fn benchmarkRangeQuery(allocator: std.mem.Allocator, order: usize) !u64 {
    const config = BTreeConfig{ .order = order };
    const tree = try BTree.init(allocator, config, 0); // shard_id = 0
    defer tree.deinit();

    // Pre-populate tree
    for (0..ITERATIONS) |i| {
        try tree.insert(i, ObjRef.init(i * 10));
    }

    const start = std.time.nanoTimestamp();

    // Perform 100 range queries
    for (0..100) |i| {
        const start_key = i * 100;
        const end_key = start_key + 100;
        const results = try tree.rangeQuery(start_key, end_key, allocator);
        allocator.free(results);
    }

    const end = std.time.nanoTimestamp();
    return @intCast(end - start);
}

fn benchmarkDelete(allocator: std.mem.Allocator, order: usize) !u64 {
    const config = BTreeConfig{ .order = order };
    const tree = try BTree.init(allocator, config, 0); // shard_id = 0
    defer tree.deinit();

    // Pre-populate tree
    for (0..ITERATIONS) |i| {
        try tree.insert(i, ObjRef.init(i * 10));
    }

    const start = std.time.nanoTimestamp();

    // Delete half the keys
    for (0..ITERATIONS / 2) |i| {
        _ = try tree.delete(i * 2);
    }

    const end = std.time.nanoTimestamp();
    return @intCast(end - start);
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n=== B-Tree Benchmarks ===\n", .{});
    std.debug.print("Iterations: {d}\n\n", .{ITERATIONS});

    // Test different B-tree orders
    const orders = [_]usize{ 4, 8, 16, 32, 64, 128 };

    for (orders) |order| {
        std.debug.print("Order: {d}\n", .{order});
        std.debug.print("----------------------------------------\n", .{});

        // Insert benchmark
        const insert_time = try benchmarkInsert(allocator, order);
        const insert_per_op = insert_time / ITERATIONS;
        std.debug.print("Insert:      {d} ns total, {d} ns/op\n", .{ insert_time, insert_per_op });

        // Search benchmark
        const search_time = try benchmarkSearch(allocator, order);
        const search_per_op = search_time / ITERATIONS;
        std.debug.print("Search:      {d} ns total, {d} ns/op\n", .{ search_time, search_per_op });

        // Range query benchmark
        const range_time = try benchmarkRangeQuery(allocator, order);
        const range_per_op = range_time / 100;
        std.debug.print("Range Query: {d} ns total, {d} ns/op\n", .{ range_time, range_per_op });

        // Delete benchmark
        const delete_time = try benchmarkDelete(allocator, order);
        const delete_per_op = delete_time / (ITERATIONS / 2);
        std.debug.print("Delete:      {d} ns total, {d} ns/op\n", .{ delete_time, delete_per_op });

        std.debug.print("\n", .{});
    }

    std.debug.print("=== Benchmark Complete ===\n", .{});
}
