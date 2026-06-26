//! Benchmark for WalIntentRingWAL against existing WAL implementations.
//!
//! This benchmark intentionally separates:
//! 1. end-to-end durable single-thread append
//! 2. multi-producer buffered ingress submission + final flush
//!
//! The prototype ring WAL does not yet run a background writer thread, so
//! separating ingress from final durable flush is the fairest comparison.

const std = @import("std");
const storage = @import("storage");

const WAL = storage.WAL;
const LockFreeWAL = storage.LockFreeWAL;
const WalIntentRingWAL = storage.WalIntentRingWAL;
const WalIntentRingConfig = storage.WalIntentRingConfig;
const WALEntry = storage.WALEntry;
const ObjRef = storage.ObjRef;
const GroupCommitConfig = storage.GroupCommitConfig;

const ENTRY_DATA_SIZE = 128;
const WARMUP_ITERATIONS = 1000;
const SINGLE_THREAD_ITERATIONS = 10_000;
const MULTI_THREAD_TOTAL_ITERATIONS = 20_000;

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

const IngressBenchmarkResult = struct {
    name: []const u8,
    iterations: usize,
    submit_time_ns: u64,
    flush_time_ns: u64,
    total_time_ns: u64,
    submit_throughput_ops_per_sec: f64,
    total_throughput_ops_per_sec: f64,
    avg_submit_latency_ns: f64,
    p50_submit_latency_ns: u64,
    p95_submit_latency_ns: u64,
    p99_submit_latency_ns: u64,

    pub fn print(self: IngressBenchmarkResult) void {
        std.debug.print("\n=== {s} ===\n", .{self.name});
        std.debug.print("Iterations: {}\n", .{self.iterations});
        std.debug.print("Submit phase: {d:.2} ms\n", .{@as(f64, @floatFromInt(self.submit_time_ns)) / 1_000_000.0});
        std.debug.print("Flush phase: {d:.2} ms\n", .{@as(f64, @floatFromInt(self.flush_time_ns)) / 1_000_000.0});
        std.debug.print("Total time: {d:.2} ms\n", .{@as(f64, @floatFromInt(self.total_time_ns)) / 1_000_000.0});
        std.debug.print("Submit throughput: {d:.0} ops/sec\n", .{self.submit_throughput_ops_per_sec});
        std.debug.print("End-to-end throughput: {d:.0} ops/sec\n", .{self.total_throughput_ops_per_sec});
        std.debug.print("Avg submit latency: {d:.2} μs\n", .{self.avg_submit_latency_ns / 1000.0});
        std.debug.print("P50 submit latency: {d:.2} μs\n", .{@as(f64, @floatFromInt(self.p50_submit_latency_ns)) / 1000.0});
        std.debug.print("P95 submit latency: {d:.2} μs\n", .{@as(f64, @floatFromInt(self.p95_submit_latency_ns)) / 1000.0});
        std.debug.print("P99 submit latency: {d:.2} μs\n", .{@as(f64, @floatFromInt(self.p99_submit_latency_ns)) / 1000.0});
    }
};

fn percentile(latencies: []u64, p: f64) u64 {
    const index = @as(usize, @intFromFloat(@as(f64, @floatFromInt(latencies.len)) * p));
    return latencies[@min(index, latencies.len - 1)];
}

fn benchmarkSingleThreadedDurable(comptime WalType: type, allocator: std.mem.Allocator, test_dir: []const u8, name: []const u8) !BenchmarkResult {
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var data_buffer: [ENTRY_DATA_SIZE]u8 = undefined;
    @memset(&data_buffer, 'X');

    var latencies = try allocator.alloc(u64, SINGLE_THREAD_ITERATIONS);
    defer allocator.free(latencies);

    var start_time: i128 = 0;
    var end_time: i128 = 0;

    if (WalType == WalIntentRingWAL) {
        var wal = try WalIntentRingWAL.initWithConfig(
            allocator,
            test_dir,
            .{},
            .{ .producer_count = 1, .ring_capacity = 2048, .completion_capacity = 2048 },
        );
        defer wal.deinit();

        for (0..WARMUP_ITERATIONS) |i| {
            var entry = try WALEntry.init(allocator, 0, .insert, ObjRef.init(1000 + i * 100), &data_buffer);
            defer entry.deinit(allocator);
            try wal.append(&entry);
        }

        start_time = std.time.nanoTimestamp();
        for (0..SINGLE_THREAD_ITERATIONS) |i| {
            const op_start = std.time.nanoTimestamp();
            var entry = try WALEntry.init(allocator, 0, .insert, ObjRef.init(5000 + i * 100), &data_buffer);
            defer entry.deinit(allocator);
            try wal.append(&entry);
            const op_end = std.time.nanoTimestamp();
            latencies[i] = @intCast(op_end - op_start);
        }
        end_time = std.time.nanoTimestamp();
    } else {
        var wal = try WalType.init(allocator, test_dir);
        defer wal.deinit();

        for (0..WARMUP_ITERATIONS) |i| {
            const objref = ObjRef.init(1000 + i * 100);
            var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, &data_buffer);
            defer entry.deinit(allocator);
            try wal.append(&entry);
        }

        start_time = std.time.nanoTimestamp();
        for (0..SINGLE_THREAD_ITERATIONS) |i| {
            const op_start = std.time.nanoTimestamp();
            const objref = ObjRef.init(5000 + i * 100);
            var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, &data_buffer);
            defer entry.deinit(allocator);
            try wal.append(&entry);
            const op_end = std.time.nanoTimestamp();
            latencies[i] = @intCast(op_end - op_start);
        }
        end_time = std.time.nanoTimestamp();
    }

    const total_time_ns: u64 = @intCast(end_time - start_time);
    std.mem.sort(u64, latencies, {}, comptime std.sort.asc(u64));
    const throughput = @as(f64, @floatFromInt(SINGLE_THREAD_ITERATIONS)) / (@as(f64, @floatFromInt(total_time_ns)) / 1_000_000_000.0);
    const avg_latency = @as(f64, @floatFromInt(total_time_ns)) / @as(f64, @floatFromInt(SINGLE_THREAD_ITERATIONS));

    return .{
        .name = name,
        .iterations = SINGLE_THREAD_ITERATIONS,
        .total_time_ns = total_time_ns,
        .throughput_ops_per_sec = throughput,
        .avg_latency_ns = avg_latency,
        .p50_latency_ns = percentile(latencies, 0.50),
        .p95_latency_ns = percentile(latencies, 0.95),
        .p99_latency_ns = percentile(latencies, 0.99),
    };
}

const LockFreeWorkerContext = struct {
    wal: *LockFreeWAL,
    allocator: std.mem.Allocator,
    thread_id: usize,
    iterations: usize,
    latencies: []u64,
};

fn workerThreadLockFreeBuffered(ctx: *LockFreeWorkerContext) void {
    var data_buffer: [ENTRY_DATA_SIZE]u8 = undefined;
    @memset(&data_buffer, 'X');

    for (0..ctx.iterations) |i| {
        const op_start = std.time.nanoTimestamp();
        var entry = WALEntry.init(ctx.allocator, ctx.wal.nextLSN(), .insert, ObjRef.init(ctx.thread_id * 100000 + i * 100), &data_buffer) catch continue;
        ctx.wal.appendBuffered(entry) catch {
            entry.deinit(ctx.allocator);
            continue;
        };
        const op_end = std.time.nanoTimestamp();
        ctx.latencies[i] = @intCast(op_end - op_start);
    }
}

const RingWorkerContext = struct {
    producer: storage.WalIntentProducerHandle,
    allocator: std.mem.Allocator,
    thread_id: usize,
    iterations: usize,
    latencies: []u64,
};

fn workerThreadRingBuffered(ctx: *RingWorkerContext) void {
    var data_buffer: [ENTRY_DATA_SIZE]u8 = undefined;
    @memset(&data_buffer, 'X');

    for (0..ctx.iterations) |i| {
        const op_start = std.time.nanoTimestamp();
        var entry = WALEntry.init(ctx.allocator, 0, .insert, ObjRef.init(ctx.thread_id * 100000 + i * 100), &data_buffer) catch continue;
        _ = ctx.producer.appendBuffered(entry) catch {
            entry.deinit(ctx.allocator);
            continue;
        };
        const op_end = std.time.nanoTimestamp();
        ctx.latencies[i] = @intCast(op_end - op_start);
    }
}

fn benchmarkMultiThreadedLockFreeBuffered(allocator: std.mem.Allocator, test_dir: []const u8, name: []const u8, num_threads: usize) !IngressBenchmarkResult {
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try LockFreeWAL.initWithConfig(
        allocator,
        test_dir,
        .{ .max_entries = 1024, .max_wait_ms = 1, .max_buffer_size = 4 * 1024 * 1024 },
    );
    defer wal.deinit();

    const iterations_per_thread = MULTI_THREAD_TOTAL_ITERATIONS / num_threads;
    var contexts = try allocator.alloc(LockFreeWorkerContext, num_threads);
    defer allocator.free(contexts);
    const threads = try allocator.alloc(std.Thread, num_threads);
    defer allocator.free(threads);

    for (contexts, 0..) |*ctx, i| {
        ctx.* = .{
            .wal = &wal,
            .allocator = allocator,
            .thread_id = i,
            .iterations = iterations_per_thread,
            .latencies = try allocator.alloc(u64, iterations_per_thread),
        };
    }
    defer for (contexts) |ctx| allocator.free(ctx.latencies);

    const submit_start = std.time.nanoTimestamp();
    for (threads, 0..) |*thread, i| {
        thread.* = try std.Thread.spawn(.{}, workerThreadLockFreeBuffered, .{&contexts[i]});
    }
    for (threads) |thread| thread.join();
    const submit_end = std.time.nanoTimestamp();

    const flush_start = std.time.nanoTimestamp();
    try wal.flush();
    const flush_end = std.time.nanoTimestamp();

    return buildIngressResult(LockFreeWorkerContext, name, allocator, contexts, @intCast(submit_end - submit_start), @intCast(flush_end - flush_start));
}

fn benchmarkMultiThreadedRingBuffered(allocator: std.mem.Allocator, test_dir: []const u8, name: []const u8, num_threads: usize) !IngressBenchmarkResult {
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WalIntentRingWAL.initWithConfig(
        allocator,
        test_dir,
        .{ .max_entries = 1024, .max_wait_ms = 1, .max_buffer_size = 4 * 1024 * 1024 },
        .{
            .producer_count = @intCast(num_threads),
            .ring_capacity = 4096,
            .completion_capacity = 4096,
            .max_batch_entries = 1024,
            .max_batch_bytes = 8 * 1024 * 1024,
            .per_ring_batch_cap = 64,
            .spawn_writer_thread = true,
            .writer_idle_sleep_ns = 10_000,
        },
    );
    defer wal.deinit();
    try wal.startWriterThread();

    const iterations_per_thread = MULTI_THREAD_TOTAL_ITERATIONS / num_threads;
    var contexts = try allocator.alloc(RingWorkerContext, num_threads);
    defer allocator.free(contexts);
    const threads = try allocator.alloc(std.Thread, num_threads);
    defer allocator.free(threads);

    for (contexts, 0..) |*ctx, i| {
        ctx.* = .{
            .producer = try wal.registerProducer(@intCast(i)),
            .allocator = allocator,
            .thread_id = i,
            .iterations = iterations_per_thread,
            .latencies = try allocator.alloc(u64, iterations_per_thread),
        };
    }
    defer for (contexts) |ctx| allocator.free(ctx.latencies);

    const submit_start = std.time.nanoTimestamp();
    for (threads, 0..) |*thread, i| {
        thread.* = try std.Thread.spawn(.{}, workerThreadRingBuffered, .{&contexts[i]});
    }
    for (threads) |thread| thread.join();
    const submit_end = std.time.nanoTimestamp();

    const flush_start = std.time.nanoTimestamp();
    try wal.waitForIdle(30_000);
    const flush_end = std.time.nanoTimestamp();

    return buildIngressResult(RingWorkerContext, name, allocator, contexts, @intCast(submit_end - submit_start), @intCast(flush_end - flush_start));
}

fn buildIngressResult(comptime T: type, name: []const u8, allocator: std.mem.Allocator, contexts: []T, submit_time_ns: u64, flush_time_ns: u64) !IngressBenchmarkResult {
    var all_latencies = try allocator.alloc(u64, contexts.len * contexts[0].latencies.len);
    defer allocator.free(all_latencies);

    var offset: usize = 0;
    for (contexts) |ctx| {
        @memcpy(all_latencies[offset .. offset + ctx.latencies.len], ctx.latencies);
        offset += ctx.latencies.len;
    }

    std.mem.sort(u64, all_latencies, {}, comptime std.sort.asc(u64));
    const total_iterations = all_latencies.len;
    const total_time_ns = submit_time_ns + flush_time_ns;

    return .{
        .name = name,
        .iterations = total_iterations,
        .submit_time_ns = submit_time_ns,
        .flush_time_ns = flush_time_ns,
        .total_time_ns = total_time_ns,
        .submit_throughput_ops_per_sec = @as(f64, @floatFromInt(total_iterations)) / (@as(f64, @floatFromInt(submit_time_ns)) / 1_000_000_000.0),
        .total_throughput_ops_per_sec = @as(f64, @floatFromInt(total_iterations)) / (@as(f64, @floatFromInt(total_time_ns)) / 1_000_000_000.0),
        .avg_submit_latency_ns = @as(f64, @floatFromInt(submit_time_ns)) / @as(f64, @floatFromInt(total_iterations)),
        .p50_submit_latency_ns = percentile(all_latencies, 0.50),
        .p95_submit_latency_ns = percentile(all_latencies, 0.95),
        .p99_submit_latency_ns = percentile(all_latencies, 0.99),
    };
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n" ++ "=" ** 80 ++ "\n", .{});
    std.debug.print("WAL Performance Benchmark: Regular vs Lock-Free vs WalIntentRing\n", .{});
    std.debug.print("=" ** 80 ++ "\n", .{});

    std.debug.print("\n--- Single-Threaded Durable Append ---\n", .{});
    const regular_single = try benchmarkSingleThreadedDurable(WAL, allocator, "bench_wal_regular_single", "Regular WAL (Durable Single-Threaded)");
    regular_single.print();
    const lockfree_single = try benchmarkSingleThreadedDurable(LockFreeWAL, allocator, "bench_wal_lockfree_single", "Lock-Free WAL (Durable Single-Threaded)");
    lockfree_single.print();
    const ring_single = try benchmarkSingleThreadedDurable(WalIntentRingWAL, allocator, "bench_wal_ring_single", "WalIntentRingWAL (Durable Single-Threaded)");
    ring_single.print();

    std.debug.print("\n--- Multi-Producer Buffered Ingress + Final Flush ---\n", .{});
    const thread_counts = [_]usize{ 2, 4, 8, 16 };
    for (thread_counts) |num_threads| {
        std.debug.print("\n--- {} Producers ---\n", .{num_threads});

        const lockfree_name = try std.fmt.allocPrint(allocator, "Lock-Free WAL Buffered Ingress ({} producers)", .{num_threads});
        defer allocator.free(lockfree_name);
        const lockfree_dir = try std.fmt.allocPrint(allocator, "bench_wal_lockfree_ingress_{}", .{num_threads});
        defer allocator.free(lockfree_dir);
        const lockfree_result = try benchmarkMultiThreadedLockFreeBuffered(allocator, lockfree_dir, lockfree_name, num_threads);
        lockfree_result.print();

        const ring_name = try std.fmt.allocPrint(allocator, "WalIntentRingWAL Buffered Ingress ({} producers)", .{num_threads});
        defer allocator.free(ring_name);
        const ring_dir = try std.fmt.allocPrint(allocator, "bench_wal_ring_ingress_{}", .{num_threads});
        defer allocator.free(ring_dir);
        const ring_result = try benchmarkMultiThreadedRingBuffered(allocator, ring_dir, ring_name, num_threads);
        ring_result.print();
    }

    std.debug.print("\n" ++ "=" ** 80 ++ "\n", .{});
    std.debug.print("Benchmark Complete\n", .{});
    std.debug.print("=" ** 80 ++ "\n\n", .{});
}
