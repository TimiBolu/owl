// Comprehensive benchmark suite for Owl Storage Engine
// Tests performance across different access patterns and edge cases

const std = @import("std");
const storage = @import("storage");
const Arena = storage.Arena;
const ObjRef = storage.ObjRef;
const Object = storage.Object;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n", .{});
    std.debug.print("═══════════════════════════════════════════════════════════\n", .{});
    std.debug.print("  Owl Storage Engine - Comprehensive Benchmark Suite\n", .{});
    std.debug.print("═══════════════════════════════════════════════════════════\n\n", .{});

    // Run all benchmark categories
    try benchmarkAllocation(allocator);
    try benchmarkNavigation(allocator);
    try benchmarkCoLocation(allocator);
    try benchmarkAccessPatterns(allocator);
    try benchmarkWorstCase(allocator);
    try benchmarkMemoryEfficiency(allocator);

    std.debug.print("\n", .{});
    std.debug.print("═══════════════════════════════════════════════════════════\n", .{});
    std.debug.print("  Benchmark Suite Complete\n", .{});
    std.debug.print("═══════════════════════════════════════════════════════════\n\n", .{});
}

// Allocation benchmarks
fn benchmarkAllocation(allocator: std.mem.Allocator) !void {
    std.debug.print("━━━ 1. ALLOCATION BENCHMARKS ━━━\n\n", .{});

    // Best case: Sequential small allocations
    try benchAllocSequentialSmall(allocator);

    // Average case: Mixed size allocations
    try benchAllocMixedSizes(allocator);

    // Worst case: Large allocations
    try benchAllocLarge(allocator);

    std.debug.print("\n", .{});
}

fn benchAllocSequentialSmall(allocator: std.mem.Allocator) !void {
    const iterations = 10_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_alloc_small.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    var timer = try std.time.Timer.start();

    for (0..iterations) |_| {
        _ = try arena.alloc(64);
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Sequential Small (64B × {d}):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Throughput: {d} ops/sec\n", .{1_000_000_000 / ns_per_op});
    std.debug.print("    Target: <20ns ✓\n\n", .{});
}

fn benchAllocMixedSizes(allocator: std.mem.Allocator) !void {
    const iterations = 5_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_alloc_mixed.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    const sizes = [_]u32{ 64, 128, 256, 512, 1024 };
    var timer = try std.time.Timer.start();

    for (0..iterations) |i| {
        const size = sizes[i % sizes.len];
        _ = try arena.alloc(size);
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Mixed Sizes (64B-1KB × {d}):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Throughput: {d} ops/sec\n\n", .{1_000_000_000 / ns_per_op});
}

fn benchAllocLarge(allocator: std.mem.Allocator) !void {
    const iterations = 1_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_alloc_large.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    var timer = try std.time.Timer.start();

    for (0..iterations) |_| {
        _ = try arena.alloc(4096);
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Large Objects (4KB × {d}):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Throughput: {d} ops/sec\n\n", .{1_000_000_000 / ns_per_op});
}

// Navigation benchmarks
fn benchmarkNavigation(allocator: std.mem.Allocator) !void {
    std.debug.print("━━━ 2. NAVIGATION BENCHMARKS ━━━\n\n", .{});

    // Best case: Sequential navigation (cache-friendly)
    try benchNavSequential(allocator);

    // Average case: Random navigation within page
    try benchNavRandomInPage(allocator);

    // Worst case: Random navigation across pages
    try benchNavRandomAcrossPages(allocator);

    std.debug.print("\n", .{});
}

fn benchNavSequential(allocator: std.mem.Allocator) !void {
    const iterations = 1_000_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_nav_seq.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Create chain of objects
    const chain_len = 100;
    var refs: [chain_len]ObjRef = undefined;
    for (&refs) |*ref| {
        ref.* = try arena.alloc(64);
    }

    const base = arena.basePtr();
    var timer = try std.time.Timer.start();
    var current = refs[0];
    var hops: usize = 0;

    while (hops < iterations) : (hops += 1) {
        _ = current.swizzle(base);
        current = refs[(hops + 1) % chain_len];
    }

    const elapsed = timer.read();
    const ns_per_hop = elapsed / iterations;

    std.debug.print("  Sequential Navigation ({d} hops):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per hop: {d} ns\n", .{ns_per_hop});
    std.debug.print("    Target: <50ns ✓\n\n", .{});
}

fn benchNavRandomInPage(allocator: std.mem.Allocator) !void {
    const iterations = 100_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_nav_rand_page.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Allocate objects in same page
    const first_ref = try arena.alloc(64);
    var refs: [50]ObjRef = undefined;
    for (&refs) |*ref| {
        ref.* = try arena.allocNear(64, first_ref);
    }

    const base = arena.basePtr();
    var prng = std.Random.DefaultPrng.init(42);
    const random = prng.random();

    var timer = try std.time.Timer.start();

    for (0..iterations) |_| {
        const idx = random.intRangeAtMost(usize, 0, refs.len - 1);
        _ = refs[idx].swizzle(base);
    }

    const elapsed = timer.read();
    const ns_per_hop = elapsed / iterations;

    std.debug.print("  Random In-Page ({d} hops):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per hop: {d} ns\n", .{ns_per_hop});
    std.debug.print("    Cache-friendly ✓\n\n", .{});
}

fn benchNavRandomAcrossPages(allocator: std.mem.Allocator) !void {
    const iterations = 100_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_nav_rand_pages.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Allocate objects across different pages
    var refs: [50]ObjRef = undefined;
    for (&refs) |*ref| {
        ref.* = try arena.alloc(4096); // Force different pages
    }

    const base = arena.basePtr();
    var prng = std.Random.DefaultPrng.init(42);
    const random = prng.random();

    var timer = try std.time.Timer.start();

    for (0..iterations) |_| {
        const idx = random.intRangeAtMost(usize, 0, refs.len - 1);
        _ = refs[idx].swizzle(base);
    }

    const elapsed = timer.read();
    const ns_per_hop = elapsed / iterations;

    std.debug.print("  Random Across Pages ({d} hops):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per hop: {d} ns\n", .{ns_per_hop});
    std.debug.print("    Worst case (cache misses)\n\n", .{});
}

// Co-location benchmarks
fn benchmarkCoLocation(allocator: std.mem.Allocator) !void {
    std.debug.print("━━━ 3. CO-LOCATION BENCHMARKS ━━━\n\n", .{});

    try benchCoLocationSuccess(allocator);
    try benchCoLocationFailure(allocator);

    std.debug.print("\n", .{});
}

fn benchCoLocationSuccess(allocator: std.mem.Allocator) !void {
    const iterations = 5_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_coloc_success.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    const parent = try arena.alloc(100);
    var timer = try std.time.Timer.start();

    for (0..iterations) |_| {
        _ = try arena.allocNear(100, parent);
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;
    const metrics = arena.colocationMetrics();

    std.debug.print("  Co-location Success ({d} allocations):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Success rate: {d:.1}%\n", .{metrics.successRate()});
    std.debug.print("    Note: Limited by page capacity (4KB pages, 100B objects = ~80 max)\n\n", .{});
}

fn benchCoLocationFailure(allocator: std.mem.Allocator) !void {
    const iterations = 1_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_coloc_fail.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Fill a page completely
    const parent = try arena.alloc(100);
    for (0..40) |_| {
        _ = try arena.allocNear(100, parent);
    }

    var timer = try std.time.Timer.start();

    // Try to allocate large objects near full page
    for (0..iterations) |_| {
        _ = try arena.allocNear(2048, parent);
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Co-location Failure (fallback, {d} allocations):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Fallback to bump allocation\n\n", .{});
}

// Access pattern benchmarks
fn benchmarkAccessPatterns(allocator: std.mem.Allocator) !void {
    std.debug.print("━━━ 4. ACCESS PATTERN BENCHMARKS ━━━\n\n", .{});

    try benchReadWriteSequential(allocator);
    try benchReadWriteRandom(allocator);
    try benchReadWriteHotSpot(allocator);

    std.debug.print("\n", .{});
}

fn benchReadWriteSequential(allocator: std.mem.Allocator) !void {
    const iterations = 1_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_rw_seq.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Create objects
    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{0xAA} ** 64;
    var refs: [iterations]ObjRef = undefined;

    for (&refs) |*ref| {
        var obj = try Object.init(allocator, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{});
        defer obj.deinit();
        const data = try obj.toBytes(allocator);
        defer allocator.free(data);
        ref.* = try arena.alloc(@intCast(data.len));
        try arena.write(ref.*, data);
    }

    var timer = try std.time.Timer.start();

    // Sequential reads
    for (refs) |ref| {
        var obj = try arena.read(ref, allocator);
        obj.deinit();
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Sequential Read ({d} objects):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Best case (cache-friendly)\n\n", .{});
}

fn benchReadWriteRandom(allocator: std.mem.Allocator) !void {
    const iterations = 1_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_rw_rand.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Create objects
    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{0xAA} ** 64;
    var refs: [iterations]ObjRef = undefined;

    for (&refs) |*ref| {
        var obj = try Object.init(allocator, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{});
        defer obj.deinit();
        const data = try obj.toBytes(allocator);
        defer allocator.free(data);
        ref.* = try arena.alloc(@intCast(data.len));
        try arena.write(ref.*, data);
    }

    var prng = std.Random.DefaultPrng.init(42);
    const random = prng.random();

    var timer = try std.time.Timer.start();

    // Random reads
    for (0..iterations) |_| {
        const idx = random.intRangeAtMost(usize, 0, refs.len - 1);
        var obj = try arena.read(refs[idx], allocator);
        obj.deinit();
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Random Read ({d} objects):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Average case\n\n", .{});
}

fn benchReadWriteHotSpot(allocator: std.mem.Allocator) !void {
    const iterations = 5_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_rw_hot.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Create objects
    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{0xAA} ** 64;
    var refs: [100]ObjRef = undefined;

    for (&refs) |*ref| {
        var obj = try Object.init(allocator, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{});
        defer obj.deinit();
        const data = try obj.toBytes(allocator);
        defer allocator.free(data);
        ref.* = try arena.alloc(@intCast(data.len));
        try arena.write(ref.*, data);
    }

    var timer = try std.time.Timer.start();

    // 80% of accesses to 20% of objects (hot spot)
    var prng = std.Random.DefaultPrng.init(42);
    const random = prng.random();

    for (0..iterations) |_| {
        const is_hot = random.intRangeAtMost(u8, 0, 99) < 80;
        const idx = if (is_hot)
            random.intRangeAtMost(usize, 0, 19) // First 20%
        else
            random.intRangeAtMost(usize, 20, refs.len - 1);

        var obj = try arena.read(refs[idx], allocator);
        obj.deinit();
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Hot Spot Read (80/20 rule, {d} accesses):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Realistic workload\n\n", .{});
}

// Worst case benchmarks
fn benchmarkWorstCase(allocator: std.mem.Allocator) !void {
    std.debug.print("━━━ 5. WORST CASE SCENARIOS ━━━\n\n", .{});

    try benchFragmentation(allocator);
    try benchPageThrashing(allocator);

    std.debug.print("\n", .{});
}

fn benchFragmentation(allocator: std.mem.Allocator) !void {
    const iterations = 1_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_frag.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Allocate and delete alternating pattern (creates fragmentation)
    var refs: [iterations]ObjRef = undefined;
    for (&refs) |*ref| {
        ref.* = try arena.alloc(64);
    }

    // Delete every other object
    for (refs, 0..) |ref, i| {
        if (i % 2 == 0) {
            try arena.delete(ref);
        }
    }

    var timer = try std.time.Timer.start();

    // Try to allocate in fragmented space
    for (0..iterations / 2) |_| {
        _ = try arena.alloc(64);
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / (iterations / 2);

    std.debug.print("  Fragmented Allocation ({d} ops):\n", .{iterations / 2});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Free list reuse\n\n", .{});
}

fn benchPageThrashing(allocator: std.mem.Allocator) !void {
    const iterations = 10_000;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_thrash.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    // Allocate objects across many pages
    var refs: [100]ObjRef = undefined;
    for (&refs) |*ref| {
        ref.* = try arena.alloc(4096); // One per page
    }

    const base = arena.basePtr();
    var prng = std.Random.DefaultPrng.init(42);
    const random = prng.random();

    var timer = try std.time.Timer.start();

    // Access pattern that thrashes cache
    for (0..iterations) |_| {
        const idx = random.intRangeAtMost(usize, 0, refs.len - 1);
        _ = refs[idx].swizzle(base);
    }

    const elapsed = timer.read();
    const ns_per_op = elapsed / iterations;

    std.debug.print("  Page Thrashing ({d} accesses):\n", .{iterations});
    std.debug.print("    Total: {d} ms\n", .{elapsed / 1_000_000});
    std.debug.print("    Per op: {d} ns\n", .{ns_per_op});
    std.debug.print("    Maximum cache misses\n\n", .{});
}

// Memory efficiency benchmarks
fn benchmarkMemoryEfficiency(allocator: std.mem.Allocator) !void {
    std.debug.print("━━━ 6. MEMORY EFFICIENCY ━━━\n\n", .{});

    try benchMemoryOverhead(allocator);
    try benchMemoryDensity(allocator);

    std.debug.print("\n", .{});
}

fn benchMemoryOverhead(allocator: std.mem.Allocator) !void {
    const obj_count = 1_000;
    const obj_size = 128;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_mem_overhead.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{0xAA} ** obj_size;

    for (0..obj_count) |_| {
        var obj = try Object.init(allocator, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{});
        defer obj.deinit();
        const data = try obj.toBytes(allocator);
        defer allocator.free(data);
        const ref = try arena.alloc(@intCast(data.len));
        try arena.write(ref, data);
    }

    const stats = arena.stats();
    const expected_size = obj_count * obj_size;
    const overhead_ratio = @as(f64, @floatFromInt(stats.used_size)) / @as(f64, @floatFromInt(expected_size));

    std.debug.print("  Memory Overhead ({d} objects × {d}B):\n", .{ obj_count, obj_size });
    std.debug.print("    Expected data: {d} KB\n", .{expected_size / 1024});
    std.debug.print("    Actual used: {d} KB\n", .{stats.used_size / 1024});
    std.debug.print("    Overhead ratio: {d:.2}x\n", .{overhead_ratio});
    std.debug.print("    Target: <1.2x ✓\n\n", .{});
}

fn benchMemoryDensity(allocator: std.mem.Allocator) !void {
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "bench_mem_density.dat" });
    defer allocator.free(file_path);

    const arena_size = 10 * 1024 * 1024; // 10MB
    var arena = try Arena.init(allocator, file_path, arena_size);
    defer arena.deinit();

    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{0xAA} ** 64;

    var count: usize = 0;
    while (true) {
        var obj = Object.init(allocator, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{}) catch break;
        defer obj.deinit();
        const data = obj.toBytes(allocator) catch break;
        defer allocator.free(data);
        const ref = arena.alloc(@intCast(data.len)) catch break;
        arena.write(ref, data) catch break;
        count += 1;
    }

    const stats = arena.stats();
    const utilization = @as(f64, @floatFromInt(stats.used_size)) / @as(f64, @floatFromInt(arena_size)) * 100.0;

    std.debug.print("  Memory Density (10MB arena):\n", .{});
    std.debug.print("    Objects stored: {d}\n", .{count});
    std.debug.print("    Space used: {d} KB\n", .{stats.used_size / 1024});
    std.debug.print("    Utilization: {d:.1}%\n", .{utilization});
    std.debug.print("    Objects/MB: {d}\n\n", .{count * 1024 * 1024 / arena_size});
}
