//! Batch Operations Benchmark
//!
//! This benchmark measures the performance of batch operations including:
//! - Batch WAL operations with group commit
//! - Batch object allocation
//! - Batch index updates
//!
//! Run with: zig build bench-batch

const std = @import("std");
const Arena = @import("../src/arena.zig").Arena;
const WAL = @import("../src/wal.zig").WAL;
const WALEntry = @import("../src/wal.zig").WALEntry;
const BatchWALEntry = @import("../src/wal.zig").BatchWALEntry;
const ObjRef = @import("../src/objref.zig").ObjRef;
const BTree = @import("../src/btree.zig").BTree;
const BTreeConfig = @import("../src/btree.zig").BTreeConfig;
const KeyValuePair = @import("../src/btree.zig").KeyValuePair;
const DistributedKey = @import("../src/btree.zig").DistributedKey;
const GlobalObjRef = @import("../src/global_objref.zig").GlobalObjRef;

/// Benchmark configuration
const BenchConfig = struct {
    batch_sizes: []const usize = &[_]usize{ 10, 100, 1000, 10000 },
    iterations: usize = 100,
    warmup_iterations: usize = 10,
};

/// Benchmark result
const BenchResult = struct {
    name: []const u8,
    batch_size: usize,
    total_time_ns: u64,
    avg_time_ns: u64,
    throughput_ops_per_sec: f64,

    pub fn print(self: BenchResult) void {
        std.debug.print("{s} (batch_size={d}): avg={d}ns, throughput={d:.2} ops/sec\n", .{
            self.name,
            self.batch_size,
            self.avg_time_ns,
            self.throughput_ops_per_sec,
        });
    }
};

/// Benchmark batch WAL operations
fn benchmarkBatchWAL(allocator: std.mem.Allocator, batch_size: usize, iterations: usize) !BenchResult {
    const test_dir = "bench_batch_wal";
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    var timer = try std.time.Timer.start();
    var total_time: u64 = 0;

    var iter: usize = 0;
    while (iter < iterations) : (iter += 1) {
        // Create batch entries
        var entries = std.ArrayList(WALEntry).init(allocator);
        defer {
            for (entries.items) |*entry| {
                entry.deinit(allocator);
            }
            entries.deinit();
        }

        var i: usize = 0;
        while (i < batch_size) : (i += 1) {
            const data = "batch entry data";
            const objref = ObjRef.init(1000 + i * 100);
            const entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
            try entries.append(entry);
        }

        const batch_lsn = wal.nextLSN();
        var batch = try BatchWALEntry.init(allocator, batch_lsn, try entries.toOwnedSlice());
        defer batch.deinit();

        // Measure batch append time
        timer.reset();
        try wal.appendBatchAtomic(&batch);
        const elapsed = timer.read();
        total_time += elapsed;
    }

    const avg_time = total_time / iterations;
    const throughput = (@as(f64, @floatFromInt(batch_size)) * @as(f64, @floatFromInt(iterations))) / (@as(f64, @floatFromInt(total_time)) / 1_000_000_000.0);

    return BenchResult{
        .name = "Batch WAL Append",
        .batch_size = batch_size,
        .total_time_ns = total_time,
        .avg_time_ns = avg_time,
        .throughput_ops_per_sec = throughput,
    };
}

/// Benchmark individual WAL operations for comparison
fn benchmarkIndividualWAL(allocator: std.mem.Allocator, batch_size: usize, iterations: usize) !BenchResult {
    const test_dir = "bench_individual_wal";
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    var timer = try std.time.Timer.start();
    var total_time: u64 = 0;

    var iter: usize = 0;
    while (iter < iterations) : (iter += 1) {
        var i: usize = 0;
        while (i < batch_size) : (i += 1) {
            const data = "individual entry data";
            const objref = ObjRef.init(1000 + i * 100);
            var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
            defer entry.deinit(allocator);

            // Measure individual append time
            timer.reset();
            try wal.append(&entry);
            const elapsed = timer.read();
            total_time += elapsed;
        }
    }

    const avg_time = total_time / (iterations * batch_size);
    const throughput = (@as(f64, @floatFromInt(batch_size)) * @as(f64, @floatFromInt(iterations))) / (@as(f64, @floatFromInt(total_time)) / 1_000_000_000.0);

    return BenchResult{
        .name = "Individual WAL Append",
        .batch_size = batch_size,
        .total_time_ns = total_time,
        .avg_time_ns = avg_time,
        .throughput_ops_per_sec = throughput,
    };
}

/// Benchmark batch arena allocation
fn benchmarkBatchAllocation(allocator: std.mem.Allocator, batch_size: usize, iterations: usize) !BenchResult {
    const test_path = "bench_batch_arena.dat";
    std.fs.cwd().deleteFile(test_path) catch {};
    defer std.fs.cwd().deleteFile(test_path) catch {};

    const arena_size = 64 * 1024 * 1024; // 64MB
    var arena = try Arena.init(allocator, test_path, arena_size, 0);
    defer arena.deinit();

    var timer = try std.time.Timer.start();
    var total_time: u64 = 0;

    var iter: usize = 0;
    while (iter < iterations) : (iter += 1) {
        // Create batch of sizes
        var sizes = try allocator.alloc(u32, batch_size);
        defer allocator.free(sizes);

        for (sizes) |*size| {
            size.* = 128; // Fixed size for simplicity
        }

        // Measure batch allocation time
        timer.reset();
        var batch_result = try arena.allocBatch(sizes, allocator);
        const elapsed = timer.read();
        total_time += elapsed;

        batch_result.deinit();
    }

    const avg_time = total_time / iterations;
    const throughput = (@as(f64, @floatFromInt(batch_size)) * @as(f64, @floatFromInt(iterations))) / (@as(f64, @floatFromInt(total_time)) / 1_000_000_000.0);

    return BenchResult{
        .name = "Batch Arena Allocation",
        .batch_size = batch_size,
        .total_time_ns = total_time,
        .avg_time_ns = avg_time,
        .throughput_ops_per_sec = throughput,
    };
}

/// Benchmark individual arena allocation for comparison
fn benchmarkIndividualAllocation(allocator: std.mem.Allocator, batch_size: usize, iterations: usize) !BenchResult {
    const test_path = "bench_individual_arena.dat";
    std.fs.cwd().deleteFile(test_path) catch {};
    defer std.fs.cwd().deleteFile(test_path) catch {};

    const arena_size = 64 * 1024 * 1024; // 64MB
    var arena = try Arena.init(allocator, test_path, arena_size, 0);
    defer arena.deinit();

    var timer = try std.time.Timer.start();
    var total_time: u64 = 0;

    var iter: usize = 0;
    while (iter < iterations) : (iter += 1) {
        var i: usize = 0;
        while (i < batch_size) : (i += 1) {
            // Measure individual allocation time
            timer.reset();
            _ = try arena.alloc(128);
            const elapsed = timer.read();
            total_time += elapsed;
        }
    }

    const avg_time = total_time / (iterations * batch_size);
    const throughput = (@as(f64, @floatFromInt(batch_size)) * @as(f64, @floatFromInt(iterations))) / (@as(f64, @floatFromInt(total_time)) / 1_000_000_000.0);

    return BenchResult{
        .name = "Individual Arena Allocation",
        .batch_size = batch_size,
        .total_time_ns = total_time,
        .avg_time_ns = avg_time,
        .throughput_ops_per_sec = throughput,
    };
}

/// Benchmark batch index updates
fn benchmarkBatchIndexUpdate(allocator: std.mem.Allocator, batch_size: usize, iterations: usize) !BenchResult {
    const config = BTreeConfig{ .order = 64 };
    const tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    var timer = try std.time.Timer.start();
    var total_time: u64 = 0;

    var iter: usize = 0;
    while (iter < iterations) : (iter += 1) {
        // Create batch of entries
        var entries = std.ArrayList(KeyValuePair).init(allocator);
        defer entries.deinit();

        var i: usize = 0;
        while (i < batch_size) : (i += 1) {
            const key = DistributedKey.init((iter * batch_size + i) * 10, 0);
            const value = GlobalObjRef.init(0, (iter * batch_size + i) * 100, 0);
            try entries.append(KeyValuePair{ .key = key, .value = value });
        }

        // Measure batch insert time
        timer.reset();
        try tree.insertBatch(entries.items);
        const elapsed = timer.read();
        total_time += elapsed;
    }

    const avg_time = total_time / iterations;
    const throughput = (@as(f64, @floatFromInt(batch_size)) * @as(f64, @floatFromInt(iterations))) / (@as(f64, @floatFromInt(total_time)) / 1_000_000_000.0);

    return BenchResult{
        .name = "Batch Index Update",
        .batch_size = batch_size,
        .total_time_ns = total_time,
        .avg_time_ns = avg_time,
        .throughput_ops_per_sec = throughput,
    };
}

/// Benchmark individual index updates for comparison
fn benchmarkIndividualIndexUpdate(allocator: std.mem.Allocator, batch_size: usize, iterations: usize) !BenchResult {
    const config = BTreeConfig{ .order = 64 };
    const tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    var timer = try std.time.Timer.start();
    var total_time: u64 = 0;

    var iter: usize = 0;
    while (iter < iterations) : (iter += 1) {
        var i: usize = 0;
        while (i < batch_size) : (i += 1) {
            const key = DistributedKey.init((iter * batch_size + i) * 10, 0);
            const value = GlobalObjRef.init(0, (iter * batch_size + i) * 100, 0);

            // Measure individual insert time
            timer.reset();
            try tree.insertDistributed(key, value);
            const elapsed = timer.read();
            total_time += elapsed;
        }
    }

    const avg_time = total_time / (iterations * batch_size);
    const throughput = (@as(f64, @floatFromInt(batch_size)) * @as(f64, @floatFromInt(iterations))) / (@as(f64, @floatFromInt(total_time)) / 1_000_000_000.0);

    return BenchResult{
        .name = "Individual Index Update",
        .batch_size = batch_size,
        .total_time_ns = total_time,
        .avg_time_ns = avg_time,
        .throughput_ops_per_sec = throughput,
    };
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const config = BenchConfig{};

    std.debug.print("\n=== Batch Operations Benchmark ===\n\n", .{});

    // Benchmark each batch size
    for (config.batch_sizes) |batch_size| {
        std.debug.print("Batch size: {d}\n", .{batch_size});
        std.debug.print("-------------------\n", .{});

        // WAL benchmarks
        const batch_wal_result = try benchmarkBatchWAL(allocator, batch_size, config.iterations);
        batch_wal_result.print();

        const individual_wal_result = try benchmarkIndividualWAL(allocator, batch_size, config.iterations);
        individual_wal_result.print();

        const wal_speedup = individual_wal_result.throughput_ops_per_sec / batch_wal_result.throughput_ops_per_sec;
        std.debug.print("WAL Speedup: {d:.2}x\n\n", .{wal_speedup});

        // Arena allocation benchmarks
        const batch_alloc_result = try benchmarkBatchAllocation(allocator, batch_size, config.iterations);
        batch_alloc_result.print();

        const individual_alloc_result = try benchmarkIndividualAllocation(allocator, batch_size, config.iterations);
        individual_alloc_result.print();

        const alloc_speedup = individual_alloc_result.throughput_ops_per_sec / batch_alloc_result.throughput_ops_per_sec;
        std.debug.print("Allocation Speedup: {d:.2}x\n\n", .{alloc_speedup});

        // Index update benchmarks
        const batch_index_result = try benchmarkBatchIndexUpdate(allocator, batch_size, config.iterations);
        batch_index_result.print();

        const individual_index_result = try benchmarkIndividualIndexUpdate(allocator, batch_size, config.iterations);
        individual_index_result.print();

        const index_speedup = individual_index_result.throughput_ops_per_sec / batch_index_result.throughput_ops_per_sec;
        std.debug.print("Index Speedup: {d:.2}x\n\n", .{index_speedup});

        std.debug.print("\n", .{});
    }

    std.debug.print("=== Benchmark Complete ===\n", .{});
}
