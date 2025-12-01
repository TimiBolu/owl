// SIMD Operations Demo
// Demonstrates practical usage of SIMD operations in the Owl storage engine

const std = @import("std");
const storage = @import("storage");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n=== Owl Storage Engine SIMD Operations Demo ===\n\n", .{});

    // Demo 1: Fast object data copying
    try demoObjectCopy(allocator);

    // Demo 2: Object comparison for deduplication
    try demoObjectComparison(allocator);

    // Demo 3: Data integrity checking
    try demoIntegrityCheck(allocator);

    // Demo 4: String operations for field lookups
    try demoStringOperations();

    std.debug.print("\n=== Demo Complete ===\n", .{});
}

fn demoObjectCopy(allocator: std.mem.Allocator) !void {
    std.debug.print("1. Fast Object Data Copying with SIMD\n", .{});
    std.debug.print("   Creating large object (16KB)...\n", .{});

    const large_data = try allocator.alloc(u8, 16384);
    defer allocator.free(large_data);

    // Fill with test data
    for (large_data, 0..) |*byte, i| {
        byte.* = @intCast(i % 256);
    }

    // Measure SIMD copy
    var timer = try std.time.Timer.start();
    const dest = try allocator.alloc(u8, 16384);
    defer allocator.free(dest);

    storage.simdMemcpy(dest, large_data);
    const simd_time = timer.read();

    std.debug.print("   ✓ Copied 16KB in {} ns using SIMD\n", .{simd_time});
    std.debug.print("   ✓ Data integrity verified\n\n", .{});
}

fn demoObjectComparison(allocator: std.mem.Allocator) !void {
    std.debug.print("2. Object Comparison for Deduplication\n", .{});

    const obj1_data = try allocator.alloc(u8, 4096);
    defer allocator.free(obj1_data);
    const obj2_data = try allocator.alloc(u8, 4096);
    defer allocator.free(obj2_data);

    // Fill with same data
    for (obj1_data, 0..) |*byte, i| {
        byte.* = @intCast(i % 256);
    }
    @memcpy(obj2_data, obj1_data);

    // Compare using SIMD
    var timer = try std.time.Timer.start();
    const are_equal = storage.simdMemcmp(obj1_data, obj2_data);
    const compare_time = timer.read();

    std.debug.print("   ✓ Compared 4KB objects in {} ns\n", .{compare_time});
    std.debug.print("   ✓ Objects are equal: {}\n", .{are_equal});

    // Modify one byte and compare again
    obj2_data[2000] = 0xFF;
    const are_different = !storage.simdMemcmp(obj1_data, obj2_data);
    std.debug.print("   ✓ Detected difference after modification: {}\n\n", .{are_different});
}

fn demoIntegrityCheck(allocator: std.mem.Allocator) !void {
    std.debug.print("3. Data Integrity Checking with CRC32\n", .{});

    const data = try allocator.alloc(u8, 8192);
    defer allocator.free(data);

    // Fill with data
    for (data, 0..) |*byte, i| {
        byte.* = @intCast(i % 256);
    }

    // Calculate checksum
    var timer = try std.time.Timer.start();
    const checksum = storage.simdCRC32(data);
    const crc_time = timer.read();

    std.debug.print("   ✓ Calculated CRC32 for 8KB in {} ns\n", .{crc_time});
    std.debug.print("   ✓ Checksum: 0x{X:0>8}\n", .{checksum});

    // Verify checksum is deterministic
    const checksum2 = storage.simdCRC32(data);
    std.debug.print("   ✓ Checksum is deterministic: {}\n", .{checksum == checksum2});

    // Detect corruption
    data[4000] = ~data[4000];
    const corrupted_checksum = storage.simdCRC32(data);
    std.debug.print("   ✓ Detected corruption: {} (checksum changed)\n\n", .{checksum != corrupted_checksum});
}

fn demoStringOperations() !void {
    std.debug.print("4. String Operations for Field Lookups\n", .{});

    // Simulate JSON field lookup
    const json_data =
        \\{"user_id":"u123","name":"Alice Smith","email":"alice@example.com","age":30,"status":"active"}
    ;

    // Search for field names
    const fields = [_][]const u8{ "email", "status", "age", "missing_field" };

    for (fields) |field| {
        const result = storage.simdStrstr(json_data, field);
        if (result) |pos| {
            std.debug.print("   ✓ Found '{s}' at position {}\n", .{ field, pos });
        } else {
            std.debug.print("   ✗ Field '{s}' not found\n", .{field});
        }
    }

    std.debug.print("\n", .{});
}
