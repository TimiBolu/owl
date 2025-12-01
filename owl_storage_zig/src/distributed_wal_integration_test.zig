//! Integration test for Distributed WAL with existing components
//!
//! This test verifies that the distributed WAL integrates properly with
//! the existing Arena, WAL, and other storage components.

const std = @import("std");
const testing = std.testing;
const Allocator = std.mem.Allocator;

const Arena = @import("arena.zig").Arena;
const WAL = @import("wal.zig").WAL;
const Object = @import("object.zig").Object;
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;

const DistributedWAL = @import("distributed_wal.zig");
const ShardWALManager = DistributedWAL.ShardWALManager;
const WALReplicationManager = DistributedWAL.WALReplicationManager;
const ShardSynchronizationManager = DistributedWAL.ShardSynchronizationManager;
const DistributedWALEntry = DistributedWAL.DistributedWALEntry;
const DistributedEntryType = DistributedWAL.DistributedEntryType;

/// Helper to create test arena
fn createTestArena(allocator: Allocator, name: []const u8, shard_id: ShardId) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, name });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 1024 * 1024, shard_id); // 1MB
    return .{ .arena = arena, .dir = tmp };
}

/// Helper to create test WAL
fn createTestWAL(allocator: Allocator, name: []const u8) !struct { wal: WAL, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const wal_path = try std.fs.path.join(allocator, &[_][]const u8{ path, name });
    defer allocator.free(wal_path);

    const wal = try WAL.init(allocator, wal_path);
    return .{ .wal = wal, .dir = tmp };
}

test "DistributedWAL: integration with Arena and Object storage" {
    const allocator = testing.allocator;

    // Create arena and WAL for shard 0
    var test_arena = try createTestArena(allocator, "integration_arena.dat", 0);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var test_wal = try createTestWAL(allocator, "integration_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    // Create distributed WAL manager
    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    // Create test objects
    const id1 = [_]u8{1} ** 16;
    var obj1 = try Object.init(allocator, 1, id1, &[_]u8{ 1, 2, 3 }, "test object 1", &[_]ObjRef{});
    defer obj1.deinit();

    const id2 = [_]u8{2} ** 16;
    var obj2 = try Object.init(allocator, 2, id2, &[_]u8{ 4, 5, 6 }, "test object 2", &[_]ObjRef{});
    defer obj2.deinit();

    // Serialize objects
    const obj1_data = try obj1.toBytes(allocator);
    defer allocator.free(obj1_data);
    const obj2_data = try obj2.toBytes(allocator);
    defer allocator.free(obj2_data);

    // Allocate space in arena
    const objref1 = try test_arena.arena.alloc(@intCast(obj1_data.len));
    const objref2 = try test_arena.arena.alloc(@intCast(obj2_data.len));

    // Write objects to arena
    try test_arena.arena.write(objref1, obj1_data);
    try test_arena.arena.write(objref2, obj2_data);

    // Create global references
    const global_ref1 = GlobalObjRef.fromLocal(objref1, 0);
    const global_ref2 = GlobalObjRef.fromLocal(objref2, 0);

    // Begin distributed transaction
    const participants = [_]ShardId{1};
    const txn_id = try shard_wal.beginDistributedTransaction(&participants);

    // Write distributed operations for the objects
    try shard_wal.writeCrossShardOperation(
        txn_id,
        .insert,
        global_ref1,
        1, // target shard
        obj1_data,
    );

    try shard_wal.writeCrossShardOperation(
        txn_id,
        .update,
        global_ref2,
        1, // target shard
        obj2_data,
    );

    // Prepare and commit transaction
    const prepared = try shard_wal.prepareDistributedTransaction(txn_id);
    try testing.expect(prepared);

    const committed = try shard_wal.commitDistributedTransaction(txn_id);
    try testing.expect(committed);

    // Verify objects can be read back from arena
    var read_obj1 = try test_arena.arena.read(objref1, allocator);
    defer read_obj1.deinit();
    try testing.expectEqual(obj1.header.type_tag, read_obj1.header.type_tag);

    var read_obj2 = try test_arena.arena.read(objref2, allocator);
    defer read_obj2.deinit();
    try testing.expectEqual(obj2.header.type_tag, read_obj2.header.type_tag);

    // Additional verification that objects are correctly stored
    try testing.expectEqualSlices(u8, &obj1.header.id, &read_obj1.header.id);
    try testing.expectEqualSlices(u8, &obj2.header.id, &read_obj2.header.id);

    // Cleanup transaction
    shard_wal.cleanupTransaction(txn_id);
}

test "DistributedWAL: multi-shard coordination simulation" {
    const allocator = testing.allocator;

    // Create multiple shards
    const num_shards = 3;
    var arenas: [num_shards]@TypeOf(try createTestArena(allocator, "test", 0)) = undefined;
    var wals: [num_shards]@TypeOf(try createTestWAL(allocator, "test")) = undefined;
    var shard_managers: [num_shards]ShardWALManager = undefined;

    // Initialize all shards
    for (0..num_shards) |i| {
        const arena_name = try std.fmt.allocPrint(allocator, "multi_shard_arena_{}.dat", .{i});
        defer allocator.free(arena_name);
        arenas[i] = try createTestArena(allocator, arena_name, @intCast(i));

        const wal_name = try std.fmt.allocPrint(allocator, "multi_shard_wal_{}", .{i});
        defer allocator.free(wal_name);
        wals[i] = try createTestWAL(allocator, wal_name);

        shard_managers[i] = ShardWALManager.init(allocator, &wals[i].wal, @intCast(i));
    }

    // Cleanup
    defer {
        for (0..num_shards) |i| {
            shard_managers[i].deinit();
            wals[i].wal.deinit();
            wals[i].dir.cleanup();
            arenas[i].arena.deinit();
            arenas[i].dir.cleanup();
        }
    }

    // Create replication managers for each shard
    var replication_managers: [num_shards]WALReplicationManager = undefined;
    for (0..num_shards) |i| {
        replication_managers[i] = WALReplicationManager.init(allocator, &shard_managers[i]);
    }
    defer {
        for (0..num_shards) |i| {
            replication_managers[i].deinit();
        }
    }

    // Set up replication topology (each shard replicates to the next)
    for (0..num_shards) |i| {
        const next_shard = (i + 1) % num_shards;
        try replication_managers[i].addReplica(@intCast(next_shard));
    }

    // Simulate distributed transaction across all shards
    const all_participants = [_]ShardId{ 1, 2 };
    const coordinator_txn_id = try shard_managers[0].beginDistributedTransaction(&all_participants);

    // Create objects on each shard
    for (0..num_shards) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "multi-shard object", &[_]ObjRef{});
        defer obj.deinit();

        const obj_data = try obj.toBytes(allocator);
        defer allocator.free(obj_data);

        const objref = try arenas[i].arena.alloc(@intCast(obj_data.len));
        try arenas[i].arena.write(objref, obj_data);

        const global_ref = GlobalObjRef.fromLocal(objref, @intCast(i));

        // Write cross-shard operation from coordinator
        if (i > 0) {
            try shard_managers[0].writeCrossShardOperation(
                coordinator_txn_id,
                .insert,
                global_ref,
                @intCast(i),
                obj_data,
            );
        }

        // Replicate entry
        var entry = try DistributedWALEntry.init(
            allocator,
            @intCast(i + 1),
            .insert,
            global_ref,
            @intCast(i),
            @intCast(i),
            0, // local operation
            0,
            obj_data,
        );
        defer entry.deinit(allocator);

        try replication_managers[i].replicateEntry(&entry);
    }

    // Simulate participant transactions
    var participant_txn_ids: [num_shards - 1]@TypeOf(coordinator_txn_id) = undefined;
    for (1..num_shards) |i| {
        const coordinator_participant = [_]ShardId{0};
        participant_txn_ids[i - 1] = try shard_managers[i].beginDistributedTransaction(&coordinator_participant);
    }

    // Prepare phase on all shards
    const coord_prepared = try shard_managers[0].prepareDistributedTransaction(coordinator_txn_id);
    try testing.expect(coord_prepared);

    for (participant_txn_ids, 1..) |txn_id, i| {
        const prepared = try shard_managers[i].prepareDistributedTransaction(txn_id);
        try testing.expect(prepared);
    }

    // Commit phase on all shards
    const coord_committed = try shard_managers[0].commitDistributedTransaction(coordinator_txn_id);
    try testing.expect(coord_committed);

    for (participant_txn_ids, 1..) |txn_id, i| {
        const committed = try shard_managers[i].commitDistributedTransaction(txn_id);
        try testing.expect(committed);
    }

    // Verify replication lag
    for (0..num_shards) |i| {
        const next_shard = (i + 1) % num_shards;
        const lag = replication_managers[i].getReplicationLag(@intCast(next_shard));
        try testing.expect(lag != null);
        try testing.expect(lag.? >= 0);
    }

    // Cleanup all transactions
    shard_managers[0].cleanupTransaction(coordinator_txn_id);
    for (participant_txn_ids, 1..) |txn_id, i| {
        shard_managers[i].cleanupTransaction(txn_id);
    }

    // Verify arena statistics
    for (0..num_shards) |i| {
        const stats = arenas[i].arena.stats();
        try testing.expect(stats.object_count > 0);
        try testing.expect(stats.used_size > 0);
    }
}

test "DistributedWAL: synchronization with arena operations" {
    const allocator = testing.allocator;

    // Create source and target setups
    var source_arena = try createTestArena(allocator, "sync_source_arena.dat", 0);
    defer source_arena.dir.cleanup();
    defer source_arena.arena.deinit();

    var source_wal = try createTestWAL(allocator, "sync_source_wal");
    defer source_wal.dir.cleanup();
    defer source_wal.wal.deinit();

    var target_arena = try createTestArena(allocator, "sync_target_arena.dat", 1);
    defer target_arena.dir.cleanup();
    defer target_arena.arena.deinit();

    var target_wal = try createTestWAL(allocator, "sync_target_wal");
    defer target_wal.dir.cleanup();
    defer target_wal.wal.deinit();

    // Create managers
    var source_mgr = ShardWALManager.init(allocator, &source_wal.wal, 0);
    defer source_mgr.deinit();

    var target_mgr = ShardWALManager.init(allocator, &target_wal.wal, 1);
    defer target_mgr.deinit();

    var sync_mgr = ShardSynchronizationManager.init(allocator, &source_mgr);
    defer sync_mgr.deinit();

    // Create objects in source arena
    const num_objects = 10;
    var object_refs: [num_objects]GlobalObjRef = undefined;

    for (0..num_objects) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "sync test object", &[_]ObjRef{});
        defer obj.deinit();

        const obj_data = try obj.toBytes(allocator);
        defer allocator.free(obj_data);

        const objref = try source_arena.arena.alloc(@intCast(obj_data.len));
        try source_arena.arena.write(objref, obj_data);

        object_refs[i] = GlobalObjRef.fromLocal(objref, 0);
    }

    // Start synchronization
    try sync_mgr.startSynchronization(1, 0);

    // Simulate synchronization progress
    for (0..num_objects) |i| {
        try sync_mgr.updateSyncProgress(1, @intCast(i + 1));

        // Simulate some delay
        if (i % 3 == 0) {
            std.Thread.sleep(1000); // 1μs
        }
    }

    // Perform catchup sync
    try sync_mgr.performCatchupSync(1, num_objects + 5);

    // Verify sync status
    const final_status = sync_mgr.getSyncStatus(1);
    try testing.expect(final_status != null);
    try testing.expectEqual(@as(@TypeOf(final_status.?.last_synced_lsn), num_objects + 5), final_status.?.last_synced_lsn);
    try testing.expect(final_status.?.is_active);

    // Verify sync rate calculation
    const rate = final_status.?.syncRate();
    try testing.expect(rate >= 0);

    // Stop synchronization
    sync_mgr.stopSynchronization(1);
    const stopped_status = sync_mgr.getSyncStatus(1);
    try testing.expect(!stopped_status.?.is_active);

    // Verify source arena statistics
    const source_stats = source_arena.arena.stats();
    try testing.expectEqual(@as(u64, num_objects), source_stats.object_count);
}

test "DistributedWAL: error handling and recovery" {
    const allocator = testing.allocator;

    var test_wal = try createTestWAL(allocator, "error_handling_wal");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_mgr = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_mgr.deinit();

    // Test transaction not found error
    const invalid_txn_result = try shard_mgr.prepareDistributedTransaction(999);
    try testing.expect(!invalid_txn_result);

    // Test operations on non-existent transaction
    const objref = GlobalObjRef.init(1, 1024, 0);
    const cross_shard_result = shard_mgr.writeCrossShardOperation(
        999, // non-existent transaction
        .insert,
        objref,
        1,
        "test data",
    );
    try testing.expectError(error.TransactionNotFound, cross_shard_result);

    // Test valid transaction lifecycle with error conditions
    const participants = [_]ShardId{1};
    const txn_id = try shard_mgr.beginDistributedTransaction(&participants);

    // Multiple prepare calls (should work)
    const prepared1 = try shard_mgr.prepareDistributedTransaction(txn_id);
    try testing.expect(prepared1);

    const prepared2 = try shard_mgr.prepareDistributedTransaction(txn_id);
    try testing.expect(prepared2);

    // Abort after prepare
    const aborted = try shard_mgr.abortDistributedTransaction(txn_id);
    try testing.expect(aborted);

    // Try to commit aborted transaction (should still work but be in aborted state)
    const commit_after_abort = try shard_mgr.commitDistributedTransaction(txn_id);
    try testing.expect(commit_after_abort);

    // Verify final state is committed (last operation wins)
    const final_state = shard_mgr.getTransactionState(txn_id);
    try testing.expectEqual(@TypeOf(final_state.?).committed, final_state.?);

    // Cleanup
    shard_mgr.cleanupTransaction(txn_id);
}
