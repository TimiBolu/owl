//! Comprehensive tests for crash recovery functionality
//!
//! These tests verify the crash recovery implementation including WAL replay,
//! corruption detection, automatic repair, and consistency checks.

const std = @import("std");
const testing = std.testing;
const Arena = @import("arena.zig").Arena;
const WAL = @import("wal.zig").WAL;
const WALEntry = @import("wal.zig").WALEntry;
const EntryType = @import("wal.zig").EntryType;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const ObjRef = @import("objref.zig").ObjRef;
const BTree = @import("btree.zig").BTree;
const BTreeConfig = @import("btree.zig").BTreeConfig;
const CrashRecovery = @import("crash_recovery.zig").CrashRecovery;
const RecoveryConfig = @import("crash_recovery.zig").RecoveryConfig;
const CorruptionSeverity = @import("crash_recovery.zig").CorruptionSeverity;

/// Helper to create test arena
fn createTestArena(allocator: std.mem.Allocator, name: []const u8) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, name });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 1024 * 1024, 0); // 1MB, shard 0
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

/// Helper to create test B-tree
fn createTestBTree(allocator: std.mem.Allocator) !*BTree {
    const config = BTreeConfig{ .order = 3 };
    return try BTree.init(allocator, config, 0); // shard 0
}

test "CrashRecovery: basic initialization" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_basic_test.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_basic_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    const config = RecoveryConfig{};
    var recovery = try CrashRecovery.init(allocator, &test_arena.arena, &test_wal.wal, config);
    defer recovery.deinit();

    const stats = recovery.getStats();
    try testing.expectEqual(@as(u64, 0), stats.operations_performed);
    try testing.expectEqual(@as(u64, 0), stats.corruptions_detected);
}

test "CrashRecovery: arena corruption detection" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_arena_corruption.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_arena_corruption_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    // Create a valid object first
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, "test data", &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try test_arena.arena.alloc(@intCast(data.len));
    try test_arena.arena.write(objref, data);

    // Corrupt the object checksum
    var corrupted_data = try allocator.dupe(u8, data);
    defer allocator.free(corrupted_data);

    // Corrupt checksum bytes (assuming they're at offset 28-31 in ObjectHeader)
    corrupted_data[28] = 0xFF;
    corrupted_data[29] = 0xFF;
    corrupted_data[30] = 0xFF;
    corrupted_data[31] = 0xFF;

    try test_arena.arena.writeAt(objref.getOffset(), corrupted_data);

    const config = RecoveryConfig{ .auto_repair_enabled = false };
    var recovery = try CrashRecovery.init(allocator, &test_arena.arena, &test_wal.wal, config);
    defer recovery.deinit();

    try recovery.detectCorruptions();

    const stats = recovery.getStats();
    try testing.expect(stats.corruptions_detected > 0);

    const corruptions = recovery.getCorruptions();
    try testing.expect(corruptions.len > 0);

    // Should detect checksum mismatch
    var found_checksum_error = false;
    for (corruptions) |corruption| {
        if (std.mem.eql(u8, corruption.corruption_type, "object_checksum_mismatch")) {
            found_checksum_error = true;
            try testing.expectEqual(CorruptionSeverity.minor, corruption.severity);
            try testing.expect(corruption.can_auto_repair);
            break;
        }
    }
    try testing.expect(found_checksum_error);
}

test "CrashRecovery: automatic checksum repair" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_checksum_repair.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_checksum_repair_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    // Create a valid object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, "repair test", &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try test_arena.arena.alloc(@intCast(data.len));
    try test_arena.arena.write(objref, data);

    // Corrupt the checksum
    var corrupted_data = try allocator.dupe(u8, data);
    defer allocator.free(corrupted_data);
    corrupted_data[28] = 0xFF; // Corrupt checksum
    try test_arena.arena.writeAt(objref.getOffset(), corrupted_data);

    const config = RecoveryConfig{ .auto_repair_enabled = true };
    var recovery = try CrashRecovery.init(allocator, &test_arena.arena, &test_wal.wal, config);
    defer recovery.deinit();

    try recovery.performRecovery();

    const stats = recovery.getStats();
    try testing.expect(stats.corruptions_detected > 0);
    try testing.expect(stats.corruptions_repaired > 0);

    // Verify the object can now be read without error
    var repaired_obj = try test_arena.arena.read(objref, allocator);
    defer repaired_obj.deinit();

    try repaired_obj.verify(); // Should not throw checksum error
}

test "CrashRecovery: WAL replay functionality" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_wal_replay.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_wal_replay_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    // Create some WAL entries
    const num_entries = 5;
    for (0..num_entries) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "wal test", &[_]ObjRef{});
        defer obj.deinit();

        const obj_data = try obj.toBytes(allocator);
        defer allocator.free(obj_data);

        const objref = ObjRef.init(1000 + i * 100);
        var entry = try WALEntry.init(allocator, test_wal.wal.nextLSN(), .insert, objref, obj_data);
        defer entry.deinit(allocator);

        try test_wal.wal.append(&entry);
    }

    const config = RecoveryConfig{};
    var recovery = try CrashRecovery.init(allocator, &test_arena.arena, &test_wal.wal, config);
    defer recovery.deinit();

    try recovery.performRecovery();

    const stats = recovery.getStats();
    try testing.expectEqual(@as(u64, num_entries), stats.wal_entries_replayed);
    try testing.expectEqual(@as(u64, num_entries), stats.objects_recovered);
}

test "CrashRecovery: B-tree index rebuild" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_btree_rebuild.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_btree_rebuild_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var btree = try createTestBTree(allocator);
    defer btree.deinit();

    // Create some objects in the arena
    const num_objects = 3;
    for (0..num_objects) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "index test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        const objref = try test_arena.arena.alloc(@intCast(data.len));
        try test_arena.arena.write(objref, data);
    }

    const config = RecoveryConfig{};
    var recovery = try CrashRecovery.initWithIndex(allocator, &test_arena.arena, &test_wal.wal, btree, config);
    defer recovery.deinit();

    try recovery.performRecovery();

    const stats = recovery.getStats();
    try testing.expect(stats.index_entries_rebuilt > 0);
    try testing.expect(btree.getSize() > 0);
}

test "CrashRecovery: consistency checks" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_consistency.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_consistency_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    // Create some valid objects
    for (0..3) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "consistency test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        const objref = try test_arena.arena.alloc(@intCast(data.len));
        try test_arena.arena.write(objref, data);
    }

    const config = RecoveryConfig{ .consistency_checks_enabled = true };
    var recovery = try CrashRecovery.init(allocator, &test_arena.arena, &test_wal.wal, config);
    defer recovery.deinit();

    try recovery.performRecovery();

    const stats = recovery.getStats();
    try testing.expect(stats.consistency_checks > 0);
    // Should have no consistency failures for valid data
    try testing.expectEqual(@as(u64, 0), stats.consistency_failures);
}

test "CrashRecovery: complete recovery process" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_complete.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_complete_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var btree = try createTestBTree(allocator);
    defer btree.deinit();

    // Create some objects and WAL entries
    const num_items = 4;
    for (0..num_items) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "complete test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        // Write to arena
        const objref = try test_arena.arena.alloc(@intCast(data.len));
        try test_arena.arena.write(objref, data);

        // Write to WAL
        var entry = try WALEntry.init(allocator, test_wal.wal.nextLSN(), .insert, objref, data);
        defer entry.deinit(allocator);
        try test_wal.wal.append(&entry);
    }

    // Corrupt one object's checksum
    const corrupt_offset: u64 = 64; // First object
    const corrupt_data = try test_arena.arena.readAt(corrupt_offset, ObjectHeader.headerSize());
    var corrupted = try allocator.dupe(u8, corrupt_data);
    defer allocator.free(corrupted);
    corrupted[28] = 0xFF; // Corrupt checksum
    try test_arena.arena.writeAt(corrupt_offset, corrupted);

    const config = RecoveryConfig{
        .auto_repair_enabled = true,
        .consistency_checks_enabled = true,
        .verbose_logging = false,
    };

    var recovery = try CrashRecovery.initWithIndex(allocator, &test_arena.arena, &test_wal.wal, btree, config);
    defer recovery.deinit();

    try recovery.performRecovery();

    const stats = recovery.getStats();

    // Verify recovery completed successfully
    try testing.expect(stats.recovery_start_time > 0);
    try testing.expect(stats.recovery_end_time >= stats.recovery_start_time);
    try testing.expect(stats.operations_performed > 0);
    try testing.expect(stats.wal_entries_replayed == num_items);
    try testing.expect(stats.objects_recovered == num_items);
    try testing.expect(stats.corruptions_detected > 0);
    try testing.expect(stats.corruptions_repaired > 0);
    try testing.expect(stats.index_entries_rebuilt > 0);
    try testing.expect(stats.consistency_checks > 0);

    // Verify recovery rate calculation
    const rate = stats.recoveryRate();
    try testing.expect(rate > 0.0);

    // Verify duration calculation
    const duration = stats.recoveryDurationMs();
    try testing.expect(duration >= 0);
}

test "CrashRecovery: critical corruption handling" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_critical.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_critical_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    const config = RecoveryConfig{
        .stop_on_critical = true,
        .auto_repair_enabled = false,
    };

    var recovery = try CrashRecovery.init(allocator, &test_arena.arena, &test_wal.wal, config);
    defer recovery.deinit();

    // Manually add a critical corruption
    try recovery.recordCorruption("test_critical_corruption", .critical, 0, 100, false, "Simulated critical corruption for testing");

    // Recovery should fail due to critical corruption
    const result = recovery.performRecovery();
    try testing.expectError(error.CriticalCorruption, result);
}

test "CrashRecovery: arena integrity check" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_arena_integrity.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Create some valid objects
    for (0..3) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "integrity test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        const objref = try test_arena.arena.alloc(@intCast(data.len));
        try test_arena.arena.write(objref, data);
    }

    // Check integrity - should pass
    const integrity_ok = try test_arena.arena.checkIntegrity();
    try testing.expect(integrity_ok);
}

test "CrashRecovery: arena object scanning" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_arena_scan.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const num_objects = 3;

    // Create some objects
    for (0..num_objects) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "scan test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        const objref = try test_arena.arena.alloc(@intCast(data.len));
        try test_arena.arena.write(objref, data);
    }

    // Scan objects
    var objects = try test_arena.arena.scanObjects(allocator);
    defer objects.deinit(allocator);

    try testing.expectEqual(@as(usize, num_objects), objects.items.len);

    // Verify object info
    for (objects.items, 0..) |obj_info, i| {
        try testing.expectEqual(@as(u16, @intCast(i)), obj_info.type_tag);
        try testing.expect(!obj_info.is_deleted);
        try testing.expect(obj_info.checksum_valid);
        try testing.expect(obj_info.size > 0);
    }
}

test "CrashRecovery: B-tree integrity check" {
    const allocator = testing.allocator;

    var btree = try createTestBTree(allocator);
    defer btree.deinit();

    // Insert some data
    for (0..5) |i| {
        const key: u64 = i * 10;
        const value = ObjRef.init(i * 100);
        try btree.insert(key, value);
    }

    // Check integrity - should pass
    const integrity_ok = try btree.checkIntegrity();
    try testing.expect(integrity_ok);
}

test "CrashRecovery: WAL integrity check" {
    const allocator = testing.allocator;

    var test_wal = try createTestWAL(allocator, "recovery_wal_integrity");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    // Write some entries
    for (0..3) |i| {
        const data = try std.fmt.allocPrint(allocator, "integrity test {}", .{i});
        defer allocator.free(data);

        const objref = ObjRef.init(1000 + i * 100);
        var entry = try WALEntry.init(allocator, test_wal.wal.nextLSN(), .insert, objref, data);
        defer entry.deinit(allocator);

        try test_wal.wal.append(&entry);
    }

    // Check integrity - should pass
    const integrity_ok = try test_wal.wal.checkIntegrity();
    try testing.expect(integrity_ok);
}

test "CrashRecovery: recovery statistics" {
    const allocator = testing.allocator;

    var test_arena = try createTestArena(allocator, "recovery_stats.dat");
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "recovery_stats_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    const config = RecoveryConfig{};
    var recovery = try CrashRecovery.init(allocator, &test_arena.arena, &test_wal.wal, config);
    defer recovery.deinit();

    // Perform some recovery operations
    try recovery.detectCorruptions();

    const stats = recovery.getStats();

    // Test statistics methods
    const duration = stats.recoveryDurationMs();
    const rate = stats.recoveryRate();

    try testing.expect(duration >= 0);
    try testing.expect(rate >= 0.0);
}
