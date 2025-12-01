// Benchmark for lock-free arena allocation
// Compares performance of lock-free vs mutex-based allocation

const std = @import("std");
const Arena = @import("../src/arena.zig").Arena;

const ITERATIONS = 100_000;
const SMALL_SIZE = 64;
const MEDIUM_SIZE = 512;
const LARGE_SIZE = 4096;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n=== Lock-Free Arena Allocation Benchmark ===\n\n", .{});

    // Create temporary directory for test files
    var tmp_dir = std.testing.tmpDir(.{});
    defer tmp_dir.cleanup();

    const path = try tmp_dir.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const arena_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_arena.dat" });
    defer allocator.free(arena_path);

    // Initialize arena
    var arena = try Arena.init(allocator, arena_path, 100 * 1024 * 1024, 0); // 100MB
    defer arena.deinit();

    // Benchmark small allocations
    try benchmarkAllocations(&arena, "Small (64B)", SMALL_SIZE, ITERATIONS);

    // Reset arena for next benchmark
    arena.deinit();
    arena = try Arena.init(allocator, arena_path, 100 * 1024 * 1024, 0);

    // Benchmark medium allocations
    try benchmarkAllocations(&arena, "Medium (512B)", MEDIUM_SIZE, ITERATIONS / 10);

    // Reset arena for next benchmark
    arena.deinit();
    arena = try Arena.init(allocator, arena_path, 100 * 1024 * 1024, 0);

    // Benchmark large allocations
    try benchmarkAllocations(&arena, "Large (4KB)", LARGE_SIZE, ITERATIONS / 100);

    // Reset arena for mixed workload
    arena.deinit();
    arena = try Arena.init(allocator, arena_path, 100 * 1024 * 1024, 0);

    // Benchmark mixed workload
    try benchmarkMixedWorkload(&arena);

    // Reset arena for concurrent benchmark
    arena.deinit();
    arena = try Arena.init(allocator, arena_path, 100 * 1024 * 1024, 0);

    // Benchmark concurrent allocations
    try benchmarkConcurrentAllocations(&arena);

    std.debug.print("\n=== Benchmark Complete ===\n", .{});
}

fn benchmarkAllocations(arena: *Arena, name: []const u8, size: u32, iterations: usize) !void {
    std.debug.print("Benchmarking {s} allocations...\n", .{name});

    var timer = try std.time.Timer.start();

    // Allocation benchmark
    const alloc_start = timer.read();
    var i: usize = 0;
    while (i < iterations) : (i += 1) {
        _ = try arena.alloc(size);
    }
    const alloc_end = timer.read();
    const alloc_time = alloc_end - alloc_start;

    const alloc_ns_per_op = alloc_time / iterations;
    const alloc_ops_per_sec = (iterations * std.time.ns_per_s) / alloc_time;

    std.debug.print("  Allocations: {} ops in {} ns\n", .{ iterations, alloc_time });
    std.debug.print("  Average: {} ns/op\n", .{alloc_ns_per_op});
    std.debug.print("  Throughput: {} ops/sec\n", .{alloc_ops_per_sec});

    // Get statistics
    const stats = arena.stats();
    const slab_stats = arena.slabStats();
    const page_stats = arena.pageStats();

    std.debug.print("  Arena stats:\n", .{});
    std.debug.print("    Objects: {}\n", .{stats.object_count});
    std.debug.print("    Used: {} bytes ({d:.2}%)\n", .{ stats.used_size, @as(f64, @floatFromInt(stats.used_size)) / @as(f64, @floatFromInt(stats.total_size)) * 100.0 });

    if (size <= 64) {
        std.debug.print("  Slab stats (64B):\n", .{});
        std.debug.print("    Alloc attempts: {}\n", .{slab_stats.size_64.alloc_attempts});
        std.debug.print("    Free list hits: {}\n", .{slab_stats.size_64.free_list_hits});
        std.debug.print("    Bump allocs: {}\n", .{slab_stats.size_64.bump_allocs});
    } else if (size <= 512) {
        std.debug.print("  Slab stats (512B):\n", .{});
        std.debug.print("    Alloc attempts: {}\n", .{slab_stats.size_512.alloc_attempts});
        std.debug.print("    Free list hits: {}\n", .{slab_stats.size_512.free_list_hits});
        std.debug.print("    Bump allocs: {}\n", .{slab_stats.size_512.bump_allocs});
    } else {
        std.debug.print("  Page stats:\n", .{});
        std.debug.print("    Alloc attempts: {}\n", .{page_stats.alloc_attempts});
        std.debug.print("    Successful: {}\n", .{page_stats.successful_allocs});
        std.debug.print("    Pages allocated: {}\n", .{page_stats.pages_allocated});
    }

    std.debug.print("\n", .{});
}

fn benchmarkMixedWorkload(arena: *Arena) !void {
    std.debug.print("Benchmarking mixed workload...\n", .{});

    var timer = try std.time.Timer.start();
    const start = timer.read();

    // Mix of small, medium, and large allocations
    var i: usize = 0;
    while (i < 10000) : (i += 1) {
        _ = try arena.alloc(64); // Small
        _ = try arena.alloc(512); // Medium
        _ = try arena.alloc(4096); // Large
    }

    const end = timer.read();
    const elapsed = end - start;

    const total_ops = 30000;
    const ns_per_op = elapsed / total_ops;
    const ops_per_sec = (total_ops * std.time.ns_per_s) / elapsed;

    std.debug.print("  Total: {} ops in {} ns\n", .{ total_ops, elapsed });
    std.debug.print("  Average: {} ns/op\n", .{ns_per_op});
    std.debug.print("  Throughput: {} ops/sec\n", .{ops_per_sec});

    const stats = arena.stats();
    std.debug.print("  Objects allocated: {}\n", .{stats.object_count});
    std.debug.print("  Memory used: {} bytes\n", .{stats.used_size});

    std.debug.print("\n", .{});
}

fn benchmarkConcurrentAllocations(arena: *Arena) !void {
    std.debug.print("Benchmarking concurrent allocations (simulated)...\n", .{});

    var timer = try std.time.Timer.start();
    const start = timer.read();

    // Simulate concurrent allocations by interleaving different sizes
    // In a real concurrent scenario, these would be on different threads
    var i: usize = 0;
    while (i < 10000) : (i += 1) {
        _ = try arena.atomicAlloc(64);
        _ = try arena.atomicAlloc(128);
        _ = try arena.atomicAlloc(256);
        _ = try arena.atomicAlloc(512);
    }

    const end = timer.read();
    const elapsed = end - start;

    const total_ops = 40000;
    const ns_per_op = elapsed / total_ops;
    const ops_per_sec = (total_ops * std.time.ns_per_s) / elapsed;

    std.debug.print("  Total: {} ops in {} ns\n", .{ total_ops, elapsed });
    std.debug.print("  Average: {} ns/op\n", .{ns_per_op});
    std.debug.print("  Throughput: {} ops/sec\n", .{ops_per_sec});

    const slab_stats = arena.slabStats();
    std.debug.print("  Slab allocator efficiency:\n", .{});
    std.debug.print("    64B hit rate: {d:.2}%\n", .{hitRate(slab_stats.size_64)});
    std.debug.print("    128B hit rate: {d:.2}%\n", .{hitRate(slab_stats.size_128)});
    std.debug.print("    256B hit rate: {d:.2}%\n", .{hitRate(slab_stats.size_256)});
    std.debug.print("    512B hit rate: {d:.2}%\n", .{hitRate(slab_stats.size_512)});

    std.debug.print("\n", .{});
}

fn hitRate(stats: @import("../src/slab_allocator.zig").SlabClassStats) f64 {
    if (stats.alloc_attempts == 0) return 0.0;
    return (@as(f64, @floatFromInt(stats.free_list_hits)) / @as(f64, @floatFromInt(stats.alloc_attempts))) * 100.0;
}
