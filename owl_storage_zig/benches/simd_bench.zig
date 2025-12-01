// Benchmark for SIMD operations
// Demonstrates performance improvements over scalar operations

const std = @import("std");
const simd_ops = @import("simd_ops");

const ITERATIONS = 100000;
const SMALL_SIZE = 64;
const MEDIUM_SIZE = 1024;
const LARGE_SIZE = 16384;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n=== SIMD Operations Benchmark ===\n\n", .{});

    // Benchmark memory copy
    try benchmarkMemcpy(allocator);

    // Benchmark memory comparison
    try benchmarkMemcmp(allocator);

    // Benchmark CRC32
    try benchmarkCRC32(allocator);

    // Benchmark string operations
    try benchmarkStringOps(allocator);

    // Benchmark byte counting
    try benchmarkCountByte(allocator);
}

fn benchmarkMemcpy(allocator: std.mem.Allocator) !void {
    std.debug.print("Memory Copy Benchmark:\n", .{});

    // Small buffers
    {
        const src = try allocator.alloc(u8, SMALL_SIZE);
        defer allocator.free(src);
        const dest = try allocator.alloc(u8, SMALL_SIZE);
        defer allocator.free(dest);

        for (src, 0..) |*byte, i| {
            byte.* = @intCast(i % 256);
        }

        var timer = try std.time.Timer.start();
        for (0..ITERATIONS) |_| {
            simd_ops.simdMemcpy(dest, src);
        }
        const simd_time = timer.read();

        timer.reset();
        for (0..ITERATIONS) |_| {
            @memcpy(dest, src);
        }
        const scalar_time = timer.read();

        const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
        std.debug.print("  Small ({} bytes): SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n", .{
            SMALL_SIZE,
            simd_time / ITERATIONS,
            scalar_time / ITERATIONS,
            speedup,
        });
    }

    // Medium buffers
    {
        const src = try allocator.alloc(u8, MEDIUM_SIZE);
        defer allocator.free(src);
        const dest = try allocator.alloc(u8, MEDIUM_SIZE);
        defer allocator.free(dest);

        for (src, 0..) |*byte, i| {
            byte.* = @intCast(i % 256);
        }

        var timer = try std.time.Timer.start();
        for (0..ITERATIONS) |_| {
            simd_ops.simdMemcpy(dest, src);
        }
        const simd_time = timer.read();

        timer.reset();
        for (0..ITERATIONS) |_| {
            @memcpy(dest, src);
        }
        const scalar_time = timer.read();

        const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
        std.debug.print("  Medium ({} bytes): SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n", .{
            MEDIUM_SIZE,
            simd_time / ITERATIONS,
            scalar_time / ITERATIONS,
            speedup,
        });
    }

    // Large buffers
    {
        const src = try allocator.alloc(u8, LARGE_SIZE);
        defer allocator.free(src);
        const dest = try allocator.alloc(u8, LARGE_SIZE);
        defer allocator.free(dest);

        for (src, 0..) |*byte, i| {
            byte.* = @intCast(i % 256);
        }

        var timer = try std.time.Timer.start();
        for (0..ITERATIONS) |_| {
            simd_ops.simdMemcpy(dest, src);
        }
        const simd_time = timer.read();

        timer.reset();
        for (0..ITERATIONS) |_| {
            @memcpy(dest, src);
        }
        const scalar_time = timer.read();

        const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
        std.debug.print("  Large ({} bytes): SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n\n", .{
            LARGE_SIZE,
            simd_time / ITERATIONS,
            scalar_time / ITERATIONS,
            speedup,
        });
    }
}

fn benchmarkMemcmp(allocator: std.mem.Allocator) !void {
    std.debug.print("Memory Comparison Benchmark:\n", .{});

    // Medium buffers
    {
        const a = try allocator.alloc(u8, MEDIUM_SIZE);
        defer allocator.free(a);
        const b = try allocator.alloc(u8, MEDIUM_SIZE);
        defer allocator.free(b);

        for (a, 0..) |*byte, i| {
            byte.* = @intCast(i % 256);
        }
        @memcpy(b, a);

        var timer = try std.time.Timer.start();
        for (0..ITERATIONS) |_| {
            _ = simd_ops.simdMemcmp(a, b);
        }
        const simd_time = timer.read();

        timer.reset();
        for (0..ITERATIONS) |_| {
            _ = std.mem.eql(u8, a, b);
        }
        const scalar_time = timer.read();

        const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
        std.debug.print("  Medium ({} bytes): SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n", .{
            MEDIUM_SIZE,
            simd_time / ITERATIONS,
            scalar_time / ITERATIONS,
            speedup,
        });
    }

    // Large buffers
    {
        const a = try allocator.alloc(u8, LARGE_SIZE);
        defer allocator.free(a);
        const b = try allocator.alloc(u8, LARGE_SIZE);
        defer allocator.free(b);

        for (a, 0..) |*byte, i| {
            byte.* = @intCast(i % 256);
        }
        @memcpy(b, a);

        var timer = try std.time.Timer.start();
        for (0..ITERATIONS) |_| {
            _ = simd_ops.simdMemcmp(a, b);
        }
        const simd_time = timer.read();

        timer.reset();
        for (0..ITERATIONS) |_| {
            _ = std.mem.eql(u8, a, b);
        }
        const scalar_time = timer.read();

        const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
        std.debug.print("  Large ({} bytes): SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n\n", .{
            LARGE_SIZE,
            simd_time / ITERATIONS,
            scalar_time / ITERATIONS,
            speedup,
        });
    }
}

fn benchmarkCRC32(allocator: std.mem.Allocator) !void {
    std.debug.print("CRC32 Checksum Benchmark:\n", .{});

    const data = try allocator.alloc(u8, LARGE_SIZE);
    defer allocator.free(data);

    for (data, 0..) |*byte, i| {
        byte.* = @intCast(i % 256);
    }

    var timer = try std.time.Timer.start();
    for (0..ITERATIONS) |_| {
        _ = simd_ops.simdCRC32(data);
    }
    const simd_time = timer.read();

    timer.reset();
    for (0..ITERATIONS) |_| {
        _ = std.hash.Crc32.hash(data);
    }
    const scalar_time = timer.read();

    const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
    std.debug.print("  {} bytes: SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n\n", .{
        LARGE_SIZE,
        simd_time / ITERATIONS,
        scalar_time / ITERATIONS,
        speedup,
    });
}

fn benchmarkStringOps(_: std.mem.Allocator) !void {
    std.debug.print("String Operations Benchmark:\n", .{});

    // String search
    {
        const haystack = "The quick brown fox jumps over the lazy dog. " ** 100;
        const needle = "lazy";

        var timer = try std.time.Timer.start();
        for (0..ITERATIONS) |_| {
            _ = simd_ops.simdStrstr(haystack, needle);
        }
        const simd_time = timer.read();

        timer.reset();
        for (0..ITERATIONS) |_| {
            _ = std.mem.indexOf(u8, haystack, needle);
        }
        const scalar_time = timer.read();

        const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
        std.debug.print("  String search: SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n\n", .{
            simd_time / ITERATIONS,
            scalar_time / ITERATIONS,
            speedup,
        });
    }
}

fn benchmarkCountByte(allocator: std.mem.Allocator) !void {
    std.debug.print("Byte Counting Benchmark:\n", .{});

    const data = try allocator.alloc(u8, LARGE_SIZE);
    defer allocator.free(data);

    for (data, 0..) |*byte, i| {
        byte.* = @intCast(i % 256);
    }

    var timer = try std.time.Timer.start();
    for (0..ITERATIONS) |_| {
        _ = simd_ops.simdCountByte(data, 42);
    }
    const simd_time = timer.read();

    timer.reset();
    for (0..ITERATIONS) |_| {
        var count: usize = 0;
        for (data) |byte| {
            if (byte == 42) count += 1;
        }
    }
    const scalar_time = timer.read();

    const speedup = @as(f64, @floatFromInt(scalar_time)) / @as(f64, @floatFromInt(simd_time));
    std.debug.print("  {} bytes: SIMD={} ns, Scalar={} ns, Speedup={d:.2}x\n\n", .{
        LARGE_SIZE,
        simd_time / ITERATIONS,
        scalar_time / ITERATIONS,
        speedup,
    });
}
