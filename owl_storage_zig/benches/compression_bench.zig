// Task 10.4: Compression Performance Benchmarks
// Benchmarks for LZ4 and ZSTD compression with various data sizes

const std = @import("std");
const storage = @import("storage");
const CompressionEngine = storage.CompressionEngine;
const CompressionType = storage.AdaptiveCompressionType;

const ITERATIONS = 1000;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n=== Compression Performance Benchmarks ===\n\n", .{});

    var engine = CompressionEngine.init(allocator);

    // Test data of various sizes
    const small_data = [_]u8{0xAA} ** 512; // 512 bytes
    const medium_data = [_]u8{0xBB} ** 2048; // 2KB
    const large_data = [_]u8{0xCC} ** 8192; // 8KB
    const xlarge_data = [_]u8{0xDD} ** 32768; // 32KB

    // Benchmark LZ4 compression
    std.debug.print("--- LZ4 Compression ---\n\n", .{});

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&small_data, .lz4);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("LZ4 compress 512B", elapsed, ITERATIONS);
    }

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&medium_data, .lz4);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("LZ4 compress 2KB", elapsed, ITERATIONS);
    }

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&large_data, .lz4);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("LZ4 compress 8KB", elapsed, ITERATIONS);
    }

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&xlarge_data, .lz4);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("LZ4 compress 32KB", elapsed, ITERATIONS);
    }

    // Benchmark ZSTD compression
    std.debug.print("--- ZSTD Compression ---\n\n", .{});

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&small_data, .zstd);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("ZSTD compress 512B", elapsed, ITERATIONS);
    }

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&medium_data, .zstd);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("ZSTD compress 2KB", elapsed, ITERATIONS);
    }

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&large_data, .zstd);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("ZSTD compress 8KB", elapsed, ITERATIONS);
    }

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const compressed = try engine.compressWithType(&xlarge_data, .zstd);
            allocator.free(compressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("ZSTD compress 32KB", elapsed, ITERATIONS);
    }

    // Benchmark decompression
    std.debug.print("--- Decompression ---\n\n", .{});

    const lz4_compressed = try engine.compressWithType(&medium_data, .lz4);
    defer allocator.free(lz4_compressed);

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const decompressed = try engine.decompress(lz4_compressed, .lz4);
            allocator.free(decompressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("LZ4 decompress 2KB", elapsed, ITERATIONS);
    }

    const zstd_compressed = try engine.compressWithType(&medium_data, .zstd);
    defer allocator.free(zstd_compressed);

    {
        const start = std.time.nanoTimestamp();
        var i: usize = 0;
        while (i < ITERATIONS) : (i += 1) {
            const decompressed = try engine.decompress(zstd_compressed, .zstd);
            allocator.free(decompressed);
        }
        const elapsed: i64 = @intCast(std.time.nanoTimestamp() - start);
        printBenchmark("ZSTD decompress 2KB", elapsed, ITERATIONS);
    }

    // Benchmark compression ratios
    std.debug.print("--- Compression Ratios ---\n\n", .{});

    const test_sizes = [_]usize{ 512, 2048, 8192, 32768 };
    const test_names = [_][]const u8{ "512B", "2KB", "8KB", "32KB" };

    for (test_sizes, test_names) |size, name| {
        const data = try allocator.alloc(u8, size);
        defer allocator.free(data);
        @memset(data, 0xAA);

        const lz4_comp = try engine.compressWithType(data, .lz4);
        defer allocator.free(lz4_comp);

        const zstd_comp = try engine.compressWithType(data, .zstd);
        defer allocator.free(zstd_comp);

        const lz4_ratio = @as(f64, @floatFromInt(lz4_comp.len)) / @as(f64, @floatFromInt(size));
        const zstd_ratio = @as(f64, @floatFromInt(zstd_comp.len)) / @as(f64, @floatFromInt(size));

        std.debug.print("{s}:\n", .{name});
        std.debug.print("  LZ4:  {d} -> {d} bytes ({d:.2}% of original)\n", .{ size, lz4_comp.len, lz4_ratio * 100.0 });
        std.debug.print("  ZSTD: {d} -> {d} bytes ({d:.2}% of original)\n\n", .{ size, zstd_comp.len, zstd_ratio * 100.0 });
    }

    // Print final statistics
    std.debug.print("--- Final Statistics ---\n\n", .{});

    const comp_stats = engine.getStats();
    std.debug.print("Compression operations: {d}\n", .{comp_stats.compression_count.load(.seq_cst)});
    std.debug.print("Decompression operations: {d}\n", .{comp_stats.decompression_count.load(.seq_cst)});
    std.debug.print("Compression ratio: {d:.2}\n", .{comp_stats.compressionRatio()});
    std.debug.print("Space savings: {d:.2}%\n\n", .{comp_stats.spaceSavings()});
}

fn printBenchmark(comptime name: []const u8, elapsed_ns: i64, iterations: usize) void {
    const avg_ns = @divTrunc(elapsed_ns, @as(i64, @intCast(iterations)));
    const ops_per_sec = @divTrunc(1_000_000_000, avg_ns);

    std.debug.print("{s}:\n", .{name});
    std.debug.print("  Total time: {d}ms\n", .{@divTrunc(elapsed_ns, 1_000_000)});
    std.debug.print("  Avg time: {d}ns\n", .{avg_ns});
    std.debug.print("  Ops/sec: {d}\n\n", .{ops_per_sec});
}
