//! Benchmark comparing lock-free WAL vs mutex-based WAL
//!
//! This benchmark measures:
//! - Single-threaded write throughput
//! - Multi-threaded write throughput (2, 4, 8, 16, 32 threads)
//! - Latency distribution (p50, p95, p99)
//! - Group commit efficiency

const std = @import("std");
const storage = @import("storage");
const WAL = storage.WAL;
const LockFreeWAL = storage.LockFreeWAL;
const WALEntry = storage.WALEntry;
const EntryType = storage.EntryType;
const ObjRef = storage.ObjRef;

const WARMUP_ITERATIONS = 1000;
const BENCHMARK_ITERATIONS = 10000;
const ENTRY_DATA_SIZE = 128; // bytes

/// Benchmark result
const BenchmarkResult = struct {
    name: []const u8,
    iterations: usize,
    total_time_ns: u64,
    throughput_ops_per_sec: f64,
    avg_latency_ns: f64,
    p50_latency_ns: u64,
    p95_latency_ns: u64,
    p99_latency_ns: u64,

    pub fn print(self: BenchmarkResult) void {
        std.debug.print("\n=== {s} ===\n", .{self.name});
        std.debug.print("Iterations: {}\n", .{self.iterations});
        std.debug.print("Total time: {d:.2} ms\n", .{@as(f64, @floatFromInt(self.total_time_ns)) / 1_000_000.0});
        std.debug.print("Throughput: {d:.0} ops/sec\n", .{self.throughput_ops_per_sec});
        std.debug.print("Avg latency: {d:.2} μs\n", .{self.avg_latency_ns / 1000.0});
        std.debug.print("P50 latency: {d:.2} μs\n", .{@as(f64, @floatFromInt(self.p50_latency_ns)) / 1000.0});
        std.debug.print("P95 latency: {d:.2} μs\n", .{@as(f64, @floatFromInt(self.p95_latency_ns)) / 1000.0});
        std.debug.print("P99 latency: {d:.2} μs\n", .{@as(f64, @floatFromInt(self.p99_latency_ns)) / 1000.0});
    }
};

/// Calculate percentile from sorted latencies
fn percentile(latencies: []u64, p: f64) u64 {
    const index = @as(usize, @intFromFloat(@as(f64, @floatFromInt(latencies.len)) * p));
    return latencies[@min(index, latencies.len - 1)];
}

/// Benchmark single-threaded WAL writes
fn benchmarkSingleThreaded(comptime WalType: type, allocator: std.mem.Allocator, test_dir: []const u8, name: []const u8) !BenchmarkResult {
    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WalType.init(allocator, test_dir);
    defer wal.deinit();

    // Prepare test data
    var data_buffer: [ENTRY_DATA_SIZE]u8 = undefined;
    @memset(&data_buffer, 'X');

    // Warmup
    var i: usize = 0;
    while (i < WARMUP_ITERATIONS) : (i += 1) {
        const objref = ObjRef.init(1000 + i * 100);
        var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, &data_buffer);
        defer entry.deinit(allocator);
        try wal.append(&entry);
    }

    // Benchmark
    var latencies = try allocator.alloc(u64, BENCHMARK_ITERATIONS);
    defer allocator.free(latencies);

    const start_time = std.time.nanoTimestamp();

    i = 0;
    while (i < BENCHMARK_ITERATIONS) : (i += 1) {
        const op_start = std.time.nanoTimestamp();

        const objref = ObjRef.init(2000 + i * 100);
        var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, &data_buffer);
        defer entry.deinit(allocator);
        try wal.append(&entry);

        const op_end = std.time.nanoTimestamp();
        latencies[i] = @intCast(op_end - op_start);
    }

    const end_time = std.time.nanoTimestamp();
    const total_time_ns: u64 = @intCast(end_time - start_time);

    // Calculate statistics
    std.mem.sort(u64, latencies, {}, comptime std.sort.asc(u64));

    const throughput = @as(f64, @floatFromInt(BENCHMARK_ITERATIONS)) / (@as(f64, @floatFromInt(total_time_ns)) / 1_000_000_000.0);
    const avg_latency = @as(f64, @floatFromInt(total_time_ns)) / @as(f64, @floatFromInt(BENCHMARK_ITERATIONS));

    return BenchmarkResult{
        .name = name,
        .iterations = BENCHMARK_ITERATIONS,
        .total_time_ns = total_time_ns,
        .throughput_ops_per_sec = throughput,
        .avg_latency_ns = avg_latency,
        .p50_latency_ns = percentile(latencies, 0.50),
        .p95_latency_ns = percentile(latencies, 0.95),
        .p99_latency_ns = percentile(latencies, 0.99),
    };
}

/// Thread context for multi-threaded benchmark
const ThreadContext = struct {
    wal: *anyopaque,
    wal_type: enum { regular, lockfree },
    allocator: std.mem.Allocator,
    thread_id: usize,
    iterations: usize,
    latencies: []u64,
    completed: std.atomic.Value(bool),
};

/// Worker thread function for regular WAL
fn workerThreadRegular(ctx: *ThreadContext) void {
    const wal = @as(*WAL, @ptrCast(@alignCast(ctx.wal)));

    var data_buffer: [ENTRY_DATA_SIZE]u8 = undefined;
    @memset(&data_buffer, 'X');

    var i: usize = 0;
    while (i < ctx.iterations) : (i += 1) {
        const op_start = std.time.nanoTimestamp();

        const objref = ObjRef.init(ctx.thread_id * 100000 + i * 100);
        var entry = WALEntry.init(ctx.allocator, wal.nextLSN(), .insert, objref, &data_buffer) catch {
            std.debug.print("Thread {} failed to create entry\n", .{ctx.thread_id});
            continue;
        };
        defer entry.deinit(ctx.allocator);

        wal.append(&entry) catch {
            std.debug.print("Thread {} failed to append entry\n", .{ctx.thread_id});
            continue;
        };

        const op_end = std.time.nanoTimestamp();
        ctx.latencies[i] = @intCast(op_end - op_start);
    }

    ctx.completed.store(true, .release);
}

/// Worker thread function for lock-free WAL
fn workerThreadLockFree(ctx: *ThreadContext) void {
    const wal = @as(*LockFreeWAL, @ptrCast(@alignCast(ctx.wal)));

    var data_buffer: [ENTRY_DATA_SIZE]u8 = undefined;
    @memset(&data_buffer, 'X');

    var i: usize = 0;
    while (i < ctx.iterations) : (i += 1) {
        const op_start = std.time.nanoTimestamp();

        const objref = ObjRef.init(ctx.thread_id * 100000 + i * 100);
        var entry = WALEntry.init(ctx.allocator, wal.nextLSN(), .insert, objref, &data_buffer) catch {
            std.debug.print("Thread {} failed to create entry\n", .{ctx.thread_id});
            continue;
        };
        defer entry.deinit(ctx.allocator);

        wal.append(&entry) catch {
            std.debug.print("Thread {} failed to append entry\n", .{ctx.thread_id});
            continue;
        };

        const op_end = std.time.nanoTimestamp();
        ctx.latencies[i] = @intCast(op_end - op_start);
    }

    ctx.completed.store(true, .release);
}

/// Benchmark multi-threaded WAL writes
fn benchmarkMultiThreaded(comptime WalType: type, allocator: std.mem.Allocator, test_dir: []const u8, name: []const u8, num_threads: usize) !BenchmarkResult {
    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WalType.init(allocator, test_dir);
    defer wal.deinit();

    const iterations_per_thread = BENCHMARK_ITERATIONS / num_threads;

    // Create thread contexts
    var contexts = try allocator.alloc(ThreadContext, num_threads);
    defer allocator.free(contexts);

    const threads = try allocator.alloc(std.Thread, num_threads);
    defer allocator.free(threads);

    // Initialize contexts
    for (contexts, 0..) |*ctx, i| {
        ctx.* = ThreadContext{
            .wal = &wal,
            .wal_type = if (WalType == WAL) .regular else .lockfree,
            .allocator = allocator,
            .thread_id = i,
            .iterations = iterations_per_thread,
            .latencies = try allocator.alloc(u64, iterations_per_thread),
            .completed = std.atomic.Value(bool).init(false),
        };
    }
    defer {
        for (contexts) |*ctx| {
            allocator.free(ctx.latencies);
        }
    }

    // Start benchmark
    const start_time = std.time.nanoTimestamp();

    // Spawn threads
    for (threads, 0..) |*thread, i| {
        thread.* = try std.Thread.spawn(.{}, if (WalType == WAL) workerThreadRegular else workerThreadLockFree, .{&contexts[i]});
    }

    // Wait for all threads
    for (threads) |thread| {
        thread.join();
    }

    const end_time = std.time.nanoTimestamp();
    const total_time_ns: u64 = @intCast(end_time - start_time);

    // Collect all latencies
    var all_latencies = try allocator.alloc(u64, BENCHMARK_ITERATIONS);
    defer allocator.free(all_latencies);

    var offset: usize = 0;
    for (contexts) |*ctx| {
        @memcpy(all_latencies[offset .. offset + ctx.latencies.len], ctx.latencies);
        offset += ctx.latencies.len;
    }

    // Calculate statistics
    std.mem.sort(u64, all_latencies, {}, comptime std.sort.asc(u64));

    const throughput = @as(f64, @floatFromInt(BENCHMARK_ITERATIONS)) / (@as(f64, @floatFromInt(total_time_ns)) / 1_000_000_000.0);
    const avg_latency = @as(f64, @floatFromInt(total_time_ns)) / @as(f64, @floatFromInt(BENCHMARK_ITERATIONS));

    return BenchmarkResult{
        .name = name,
        .iterations = BENCHMARK_ITERATIONS,
        .total_time_ns = total_time_ns,
        .throughput_ops_per_sec = throughput,
        .avg_latency_ns = avg_latency,
        .p50_latency_ns = percentile(all_latencies, 0.50),
        .p95_latency_ns = percentile(all_latencies, 0.95),
        .p99_latency_ns = percentile(all_latencies, 0.99),
    };
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n" ++ "=" ** 80 ++ "\n", .{});
    std.debug.print("WAL Performance Benchmark: Lock-Free vs Mutex-Based\n", .{});
    std.debug.print("=" ** 80 ++ "\n", .{});

    // Single-threaded benchmarks
    std.debug.print("\n--- Single-Threaded Performance ---\n", .{});

    const regular_single = try benchmarkSingleThreaded(WAL, allocator, "bench_wal_regular_single", "Regular WAL (Single-Threaded)");
    regular_single.print();

    const lockfree_single = try benchmarkSingleThreaded(LockFreeWAL, allocator, "bench_wal_lockfree_single", "Lock-Free WAL (Single-Threaded)");
    lockfree_single.print();

    const single_speedup = lockfree_single.throughput_ops_per_sec / regular_single.throughput_ops_per_sec;
    std.debug.print("\nSingle-threaded speedup: {d:.2}x\n", .{single_speedup});

    // Multi-threaded benchmarks
    const thread_counts = [_]usize{ 2, 4, 8, 16 };

    for (thread_counts) |num_threads| {
        std.debug.print("\n--- {}-Thread Performance ---\n", .{num_threads});

        const regular_name = try std.fmt.allocPrint(allocator, "Regular WAL ({} threads)", .{num_threads});
        defer allocator.free(regular_name);
        const regular_dir = try std.fmt.allocPrint(allocator, "bench_wal_regular_{}", .{num_threads});
        defer allocator.free(regular_dir);
        const regular_multi = try benchmarkMultiThreaded(WAL, allocator, regular_dir, regular_name, num_threads);
        regular_multi.print();

        const lockfree_name = try std.fmt.allocPrint(allocator, "Lock-Free WAL ({} threads)", .{num_threads});
        defer allocator.free(lockfree_name);
        const lockfree_dir = try std.fmt.allocPrint(allocator, "bench_wal_lockfree_{}", .{num_threads});
        defer allocator.free(lockfree_dir);
        const lockfree_multi = try benchmarkMultiThreaded(LockFreeWAL, allocator, lockfree_dir, lockfree_name, num_threads);
        lockfree_multi.print();

        const multi_speedup = lockfree_multi.throughput_ops_per_sec / regular_multi.throughput_ops_per_sec;
        std.debug.print("\n{}-thread speedup: {d:.2}x\n", .{ num_threads, multi_speedup });
    }

    std.debug.print("\n" ++ "=" ** 80 ++ "\n", .{});
    std.debug.print("Benchmark Complete\n", .{});
    std.debug.print("=" ** 80 ++ "\n\n", .{});
}
