//! Integration tests for crash simulation with storage components
//!
//! These tests demonstrate crash simulation working with Arena, WAL, and Object
//! components to verify crash recovery scenarios and corruption detection.

const std = @import("std");
const testing = std.testing;
const Arena = @import("arena.zig").Arena;
const DeterministicArena = @import("deterministic_arena.zig").DeterministicArena;
const WAL = @import("wal.zig").WAL;
const WALEntry = @import("wal.zig").WALEntry;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const ObjRef = @import("objref.zig").ObjRef;
const crash_sim = @import("crash_simulation.zig");

const CrashSimulator = crash_sim.CrashSimulator;
const CrashAwareArena = crash_sim.CrashAwareArena;
const CrashAwareWAL = crash_sim.CrashAwareWAL;
const FailureConfig = crash_sim.FailureConfig;
const CrashMode = crash_sim.CrashMode;
const CorruptionType = crash_sim.CorruptionType;

/// Helper to create test arena
fn createTestArena(allocator: std.mem.Allocator, name: []const u8) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, name });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 1024 * 1024); // 1MB
    return .{ .arena = arena, .dir = tmp };
}

/// Helper to create test WAL
fn createTestWAL(allocator: std.mem.Allocator, name: []const u8) !struct { wal: WAL, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const wal_path = try std.fs.path.join(allocator, &[_][]const u8{ path, name });
    defer allocator.free(wal_path);

    const wal = try WAL.init(allocator, wal_path);
    return .{ .wal = wal, .dir = tmp };
}

test "Crash simulation: Arena write crash before sync" {
    const allocator = testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .after_write_before_sync,
        .crash_probability = 1.0, // Always crash
        .seed = 12345,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var test_arena = try createTestArena(allocator, "crash_test_arena.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &simulator);

    // Create test object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3, 4 }, "test data", &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try test_arena.arena.alloc(@intCast(data.len));

    // Write should crash after write but before sync
    const result = crash_arena.write(objref, data);
    try testing.expectError(error.SystemCrashed, result);
    try testing.expect(simulator.isCrashed());

    // Verify crash statistics
    const stats = simulator.getStats();
    try testing.expectEqual(@as(u64, 1), stats.crashes_simulated);
    try testing.expect(stats.operations_attempted > 0);
}

test "Crash simulation: WAL corruption injection" {
    const allocator = testing.allocator;

    const config = FailureConfig{
        .corruption_enabled = true,
        .corruption_type = .wal_corruption,
        .corruption_probability = 1.0, // Always corrupt
        .seed = 23456,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var test_wal = try createTestWAL(allocator, "crash_test_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var crash_wal = CrashAwareWAL.init(allocator, &test_wal.wal, &simulator);

    // Create WAL entry
    const data = "test wal entry data";
    const objref = ObjRef.init(1024);
    var entry = try WALEntry.init(allocator, test_wal.wal.nextLSN(), .insert, objref, data);
    defer entry.deinit(allocator);

    // Append should inject corruption
    try crash_wal.append(&entry);

    // Verify corruption was injected
    const stats = simulator.getStats();
    try testing.expectEqual(@as(u64, 1), stats.corruptions_injected);
}

test "Crash simulation: Object header corruption detection" {
    const allocator = testing.allocator;

    const config = FailureConfig{
        .corruption_enabled = true,
        .corruption_type = .header_corruption,
        .corruption_probability = 1.0,
        .seed = 34567,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var test_arena = try createTestArena(allocator, "header_corruption_test.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &simulator);

    // Create object with valid header
    const id = [_]u8{ 0x12, 0x34, 0x56, 0x78 } ++ [_]u8{0xAB} ** 12;
    var obj = try Object.init(allocator, 42, id, &[_]u8{ 1, 2, 3, 4, 5 }, "valid data", &[_]ObjRef{});
    defer obj.deinit();

    try obj.updateChecksum();
    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try test_arena.arena.alloc(@intCast(data.len));

    // Write with header corruption
    try crash_arena.write(objref, data);

    // Try to read - should detect corruption
    const read_result = crash_arena.read(objref, allocator);

    if (read_result) |read_obj| {
        // If read succeeded, the object might still be corrupted
        var read_obj_mut = read_obj;
        defer read_obj_mut.deinit();

        // Verify checksum should fail on corrupted object
        const verify_result = read_obj_mut.verify();
        try testing.expectError(error.ChecksumMismatch, verify_result);
    } else |err| {
        // Expected - corruption should cause read failure
        try testing.expect(err == error.InsufficientData or
            err == error.ChecksumMismatch or
            err == error.SystemCrashed);
    }

    // Verify corruption statistics
    const stats = simulator.getStats();
    try testing.expectEqual(@as(u64, 1), stats.corruptions_injected);
}

test "Crash simulation: Partial write recovery" {
    const allocator = testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .during_write,
        .crash_probability = 1.0,
        .seed = 45678,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var test_arena = try createTestArena(allocator, "partial_write_test.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &simulator);

    // Create a proper object with large data
    const id = [_]u8{1} ** 16;
    const large_var_data = "This is a large piece of data that should be partially written during crash simulation testing";
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, large_var_data, &[_]ObjRef{});
    defer obj.deinit();

    const large_data = try obj.toBytes(allocator);
    defer allocator.free(large_data);

    const objref = try test_arena.arena.alloc(@intCast(large_data.len));

    // Should crash during write, creating partial write
    const result = crash_arena.write(objref, large_data);
    try testing.expectError(error.SystemCrashed, result);
    try testing.expect(simulator.isCrashed());

    // Verify partial write statistics
    const stats = simulator.getStats();
    try testing.expectEqual(@as(u64, 1), stats.partial_writes);
    try testing.expectEqual(@as(u64, 1), stats.crashes_simulated);

    // Recover from crash
    simulator.recover();
    try testing.expect(!simulator.isCrashed());

    // Disable crash for successful recovery write
    simulator.config.crash_enabled = false;
    try crash_arena.write(objref, large_data);

    // Verify data was written correctly after recovery
    const read_data = try test_arena.arena.readAt(objref.getOffset(), large_data.len);
    try testing.expectEqualStrings(large_data, read_data);
}

test "Crash simulation: Multiple corruption types" {
    const allocator = testing.allocator;

    const corruption_types = [_]CorruptionType{
        .bit_flip,
        .zero_bytes,
        .checksum_corruption,
    };

    for (corruption_types, 0..) |corruption_type, i| {
        const config = FailureConfig{
            .corruption_enabled = true,
            .corruption_type = corruption_type,
            .corruption_probability = 1.0,
            .seed = 56789 + i,
        };

        var simulator = CrashSimulator.init(allocator, config);
        defer simulator.deinit();

        const arena_name = try std.fmt.allocPrint(allocator, "multi_corruption_test_{}.dat", .{i});
        defer allocator.free(arena_name);

        var test_arena = try createTestArena(allocator, arena_name);
        defer test_arena.dir.cleanup();
        defer test_arena.arena.deinit();

        var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &simulator);

        // Create test object
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{ 1, 2, 3, 4 }, "corruption test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        const objref = try test_arena.arena.alloc(@intCast(data.len));

        // Write with specific corruption type
        try crash_arena.write(objref, data);

        // Verify corruption was injected
        const stats = simulator.getStats();
        try testing.expectEqual(@as(u64, 1), stats.corruptions_injected);
    }
}

test "Crash simulation: Deterministic behavior with same seed" {
    const allocator = testing.allocator;

    const seed: u64 = 98765;
    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .random_timing,
        .crash_probability = 0.3,
        .corruption_enabled = true,
        .corruption_type = .bit_flip,
        .corruption_probability = 0.2,
        .seed = seed,
    };

    // Run simulation twice with same seed
    var results1: [100]bool = undefined;
    var results2: [100]bool = undefined;

    // First run
    {
        var simulator = CrashSimulator.init(allocator, config);
        defer simulator.deinit();

        for (&results1) |*result| {
            result.* = simulator.shouldCrash("test_op");
        }
    }

    // Second run with same seed
    {
        var simulator = CrashSimulator.init(allocator, config);
        defer simulator.deinit();

        for (&results2) |*result| {
            result.* = simulator.shouldCrash("test_op");
        }
    }

    // Results should be identical
    for (results1, results2) |r1, r2| {
        try testing.expectEqual(r1, r2);
    }
}

test "Crash simulation: WAL recovery with corruption" {
    const allocator = testing.allocator;

    const config = FailureConfig{
        .corruption_enabled = true,
        .corruption_type = .wal_corruption,
        .corruption_probability = 0.5, // 50% corruption rate
        .seed = 11111,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var test_wal = try createTestWAL(allocator, "wal_recovery_corruption_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var crash_wal = CrashAwareWAL.init(allocator, &test_wal.wal, &simulator);

    // Write multiple WAL entries
    var entries_written: u32 = 0;
    for (0..10) |i| {
        const data = try std.fmt.allocPrint(allocator, "wal entry {}", .{i});
        defer allocator.free(data);

        const objref = ObjRef.init(1000 + i * 100);
        var entry = try WALEntry.init(allocator, test_wal.wal.nextLSN(), .insert, objref, data);
        defer entry.deinit(allocator);

        // Some entries may be corrupted
        crash_wal.append(&entry) catch |err| {
            if (err == error.SystemCrashed) continue;
            return err;
        };
        entries_written += 1;
    }

    // Verify some corruption occurred
    const stats = simulator.getStats();
    try testing.expect(stats.corruptions_injected > 0);
    try testing.expect(entries_written > 0);

    // WAL should still be functional for non-corrupted entries
    const wal_stats = test_wal.wal.getStats();
    try testing.expect(wal_stats.entry_count > 0);
}

test "Crash simulation: Arena allocation with crash recovery" {
    const allocator = testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .before_write,
        .crash_probability = 0.7, // 70% crash rate
        .seed = 22222,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var test_arena = try createTestArena(allocator, "allocation_crash_test.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &simulator);

    var successful_allocs: u32 = 0;
    var crashed_allocs: u32 = 0;

    // Try multiple allocations
    for (0..20) |_| {
        const result = crash_arena.alloc(100);

        if (result) |_| {
            successful_allocs += 1;
        } else |err| {
            if (err == error.SystemCrashed) {
                crashed_allocs += 1;
                simulator.recover(); // Recover for next attempt
            } else {
                return err;
            }
        }
    }

    // Should have both successful and crashed allocations
    try testing.expect(successful_allocs > 0);
    try testing.expect(crashed_allocs > 0);

    // Verify crash statistics
    const stats = simulator.getStats();
    try testing.expectEqual(crashed_allocs, stats.crashes_simulated);
}

test "Crash simulation: Complex scenario with Arena and WAL" {
    const allocator = testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .random_timing,
        .crash_probability = 0.2,
        .corruption_enabled = true,
        .corruption_type = .bit_flip,
        .corruption_probability = 0.1,
        .seed = 33333,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var test_arena = try createTestArena(allocator, "complex_scenario_arena.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "complex_scenario_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &simulator);
    var crash_wal = CrashAwareWAL.init(allocator, &test_wal.wal, &simulator);

    var operations_completed: u32 = 0;
    var operations_failed: u32 = 0;

    // Perform complex operations involving both Arena and WAL
    for (0..50) |i| {
        // Create object
        const id = [_]u8{@intCast(i % 256)} ** 16;
        var obj = try Object.init(allocator, @intCast(i % 10), id, &[_]u8{ @intCast(i), @intCast(i + 1) }, "complex test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        // Try to allocate and write to arena
        const alloc_result = crash_arena.alloc(@intCast(data.len));
        if (alloc_result) |objref| {
            const write_result = crash_arena.write(objref, data);
            if (write_result) |_| {
                // Try to write WAL entry
                const wal_data = try std.fmt.allocPrint(allocator, "operation {}", .{i});
                defer allocator.free(wal_data);

                var entry = try WALEntry.init(allocator, test_wal.wal.nextLSN(), .insert, objref, wal_data);
                defer entry.deinit(allocator);

                const wal_result = crash_wal.append(&entry);
                if (wal_result) |_| {
                    operations_completed += 1;
                } else |_| {
                    operations_failed += 1;
                    simulator.recover();
                }
            } else |_| {
                operations_failed += 1;
                simulator.recover();
            }
        } else |_| {
            operations_failed += 1;
            simulator.recover();
        }
    }

    // Should have completed some operations despite crashes and corruption
    try testing.expect(operations_completed > 0);
    try testing.expect(operations_failed > 0);

    // Verify statistics
    const stats = simulator.getStats();
    try testing.expect(stats.operations_attempted > 0);
    try testing.expect(stats.crashes_simulated + stats.corruptions_injected > 0);
}
