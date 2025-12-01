// Comprehensive tests for distributed arena management functionality
// Task 8.8: Implement distributed arena management (Zig)

const std = @import("std");
const testing = std.testing;
const Arena = @import("arena.zig").Arena;
const Object = @import("object.zig").Object;
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const DistributedArenaManager = @import("distributed_arena.zig").DistributedArenaManager;
const ShardStatus = @import("distributed_arena.zig").ShardStatus;
const MigrationState = @import("distributed_arena.zig").MigrationState;

fn createTestArena(allocator: std.mem.Allocator, shard_id: ShardId) !struct { arena: *Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_name = try std.fmt.allocPrint(allocator, "test_arena_{}.dat", .{shard_id});
    defer allocator.free(file_name);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, file_name });
    defer allocator.free(file_path);

    const arena = try allocator.create(Arena);
    arena.* = try Arena.init(allocator, file_path, 2 * 1024 * 1024, shard_id); // 2MB
    return .{ .arena = arena, .dir = tmp };
}

test "Distributed Arena: comprehensive shard management" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    // Create multiple test arenas
    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();
    var test_arena3 = try createTestArena(allocator, 3);
    defer test_arena3.dir.cleanup();

    // Add shards to manager
    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);
    try manager.addShard(3, test_arena3.arena);

    // Verify all shards are active
    try testing.expectEqual(ShardStatus.active, manager.getShardStatus(1));
    try testing.expectEqual(ShardStatus.active, manager.getShardStatus(2));
    try testing.expectEqual(ShardStatus.active, manager.getShardStatus(3));

    // Test distributed allocation across shards
    const ref1 = try manager.allocateDistributed(1, 1000);
    const ref2 = try manager.allocateDistributed(2, 2000);
    const ref3 = try manager.allocateDistributed(3, 3000);

    try testing.expectEqual(@as(ShardId, 1), ref1.getShardId());
    try testing.expectEqual(@as(ShardId, 2), ref2.getShardId());
    try testing.expectEqual(@as(ShardId, 3), ref3.getShardId());

    // Test shard status changes
    try manager.setShardStatus(2, .migrating);
    try testing.expectEqual(ShardStatus.migrating, manager.getShardStatus(2));

    // Allocation should fail on migrating shard
    const result = manager.allocateDistributed(2, 1000);
    try testing.expectError(error.ShardMigrating, result);

    // Remove a shard
    try manager.removeShard(3);
    try testing.expectEqual(ShardStatus.offline, manager.getShardStatus(3));

    // Allocation should fail on removed shard
    const result2 = manager.allocateDistributed(3, 1000);
    try testing.expectError(error.ShardNotFound, result2);
}

test "Distributed Arena: cross-shard object operations" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    // Create objects in different shards
    const id1 = [_]u8{1} ** 16;
    const id2 = [_]u8{2} ** 16;

    var obj1 = try Object.init(allocator, 1, id1, "Object in shard 1", &[_]u8{}, &[_]ObjRef{});
    defer obj1.deinit();
    var obj2 = try Object.init(allocator, 2, id2, "Object in shard 2", &[_]u8{}, &[_]ObjRef{});
    defer obj2.deinit();

    // Serialize objects
    const data1 = try obj1.toBytes(allocator);
    defer allocator.free(data1);
    const data2 = try obj2.toBytes(allocator);
    defer allocator.free(data2);

    // Allocate and store objects
    const global_ref1 = try manager.allocateDistributed(1, @intCast(data1.len));
    const global_ref2 = try manager.allocateDistributed(2, @intCast(data2.len));

    try test_arena1.arena.writeGlobal(global_ref1, data1);
    try test_arena2.arena.writeGlobal(global_ref2, data2);

    // Test cross-shard navigation
    const nav_result1 = manager.navigateDistributed(global_ref1);
    try testing.expect(nav_result1.isLocal());
    var retrieved_obj1 = nav_result1.getLocal().?;
    defer retrieved_obj1.deinit();
    try testing.expectEqualStrings("Object in shard 1", retrieved_obj1.fixed);

    const nav_result2 = manager.navigateDistributed(global_ref2);
    try testing.expect(nav_result2.isLocal());
    var retrieved_obj2 = nav_result2.getLocal().?;
    defer retrieved_obj2.deinit();
    try testing.expectEqualStrings("Object in shard 2", retrieved_obj2.fixed);

    // Test cross-shard reference resolution
    const resolved1 = try manager.resolveCrossShardRef(global_ref1);
    try testing.expect(resolved1 != null);
    var resolved_obj1 = resolved1.?;
    defer resolved_obj1.deinit();
    try testing.expectEqualStrings("Object in shard 1", resolved_obj1.fixed);

    const resolved2 = try manager.resolveCrossShardRef(global_ref2);
    try testing.expect(resolved2 != null);
    var resolved_obj2 = resolved2.?;
    defer resolved_obj2.deinit();
    try testing.expectEqualStrings("Object in shard 2", resolved_obj2.fixed);
}

test "Distributed Arena: co-location across shards" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    // Allocate first object in shard 1
    const ref1 = try manager.allocateDistributed(1, 500);

    // Allocate near the first object (should be in same shard)
    const ref2 = try manager.allocateNearDistributed(1, 500, ref1);

    try testing.expectEqual(ref1.getShardId(), ref2.getShardId());
    try testing.expectEqual(ref1.pageId(), ref2.pageId()); // Should be co-located

    // Try to allocate near an object from a different shard
    const ref3 = try manager.allocateDistributed(2, 500);
    const ref4 = try manager.allocateNearDistributed(1, 500, ref3); // Different shard

    try testing.expectEqual(@as(ShardId, 1), ref4.getShardId()); // Should be in target shard
    try testing.expect(ref3.getShardId() != ref4.getShardId());
}

test "Distributed Arena: migration state management" {

    // Test migration state creation and progress tracking
    var migration = MigrationState.init(1, 2, 1000);

    try testing.expectEqual(@as(ShardId, 1), migration.source_shard);
    try testing.expectEqual(@as(ShardId, 2), migration.target_shard);
    try testing.expectEqual(@as(u64, 1000), migration.total_objects);
    try testing.expectEqual(@as(f64, 0.0), migration.progress());
    try testing.expect(!migration.isComplete());

    // Simulate migration progress
    _ = migration.migrated_objects.fetchAdd(250, .seq_cst);
    try testing.expectEqual(@as(f64, 25.0), migration.progress());
    try testing.expect(!migration.isComplete());

    _ = migration.migrated_objects.fetchAdd(750, .seq_cst);
    try testing.expectEqual(@as(f64, 100.0), migration.progress());

    // Mark as complete
    migration.completed.store(true, .seq_cst);
    try testing.expect(migration.isComplete());
}

test "Distributed Arena: distributed garbage collection" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    // Create and delete some objects to generate garbage
    const id1 = [_]u8{1} ** 16;
    var obj1 = try Object.init(allocator, 1, id1, "Garbage object", &[_]u8{}, &[_]ObjRef{});
    defer obj1.deinit();

    const data1 = try obj1.toBytes(allocator);
    defer allocator.free(data1);

    const global_ref1 = try manager.allocateDistributed(1, @intCast(data1.len));
    try test_arena1.arena.writeGlobal(global_ref1, data1);

    // Delete the object to create garbage
    try test_arena1.arena.deleteGlobal(global_ref1);

    // Run distributed GC
    try manager.runDistributedGC();

    // GC should complete without errors
    // In a real implementation, we would verify that garbage was collected
}

test "Distributed Arena: shard statistics" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    // Allocate some objects
    _ = try manager.allocateDistributed(1, 1000);
    _ = try manager.allocateDistributed(1, 2000);
    _ = try manager.allocateDistributed(2, 1500);

    // Get distributed statistics
    var stats = try manager.getDistributedStats(allocator);
    defer stats.deinit(allocator);

    try testing.expectEqual(@as(usize, 2), stats.items.len);

    // Verify stats contain expected shards
    var found_shard1 = false;
    var found_shard2 = false;

    for (stats.items) |stat| {
        if (stat.shard_id == 1) {
            found_shard1 = true;
            try testing.expectEqual(ShardStatus.active, stat.status);
            try testing.expectEqual(@as(u64, 2), stat.object_count); // 2 objects allocated
        } else if (stat.shard_id == 2) {
            found_shard2 = true;
            try testing.expectEqual(ShardStatus.active, stat.status);
            try testing.expectEqual(@as(u64, 1), stat.object_count); // 1 object allocated
        }
    }

    try testing.expect(found_shard1);
    try testing.expect(found_shard2);
}

test "Distributed Arena: local shard operations" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 5); // Local shard is 5
    defer manager.deinit();

    // Initially no local shard
    try testing.expect(!manager.isLocalShardAvailable());
    try testing.expect(manager.getLocalArena() == null);

    // Add local shard
    var test_arena = try createTestArena(allocator, 5);
    defer test_arena.dir.cleanup();

    try manager.addShard(5, test_arena.arena);

    // Now local shard should be available
    try testing.expect(manager.isLocalShardAvailable());

    const local_arena = manager.getLocalArena();
    try testing.expect(local_arena != null);
    try testing.expectEqual(@as(ShardId, 5), local_arena.?.getShardId());

    // Set local shard offline
    try manager.setShardStatus(5, .offline);
    try testing.expect(!manager.isLocalShardAvailable());
}

test "Distributed Arena: error handling" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    // Test allocation to non-existent shard
    const result1 = manager.allocateDistributed(99, 1000);
    try testing.expectError(error.ShardNotFound, result1);

    // Test allocation near with non-existent shard
    const result2 = manager.allocateNearDistributed(99, 1000, null);
    try testing.expectError(error.ShardNotFound, result2);

    // Test cross-shard resolution with non-existent shard
    const remote_ref = GlobalObjRef.init(99, 1024, 0);
    const result3 = manager.resolveCrossShardRef(remote_ref);
    try testing.expectError(error.RemoteShardNotAvailable, result3);

    // Test navigation to non-existent shard
    const nav_result = manager.navigateDistributed(remote_ref);
    try testing.expect(nav_result.isRemote());

    const remote_info = nav_result.getRemoteInfo().?;
    try testing.expectEqual(@as(ShardId, 99), remote_info.shard_id);
}

test "Distributed Arena: migration workflow" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    // Create some objects in shard 1
    const id1 = [_]u8{1} ** 16;
    const id2 = [_]u8{2} ** 16;

    var obj1 = try Object.init(allocator, 1, id1, "Migration test 1", &[_]u8{}, &[_]ObjRef{});
    defer obj1.deinit();
    var obj2 = try Object.init(allocator, 2, id2, "Migration test 2", &[_]u8{}, &[_]ObjRef{});
    defer obj2.deinit();

    const data1 = try obj1.toBytes(allocator);
    defer allocator.free(data1);
    const data2 = try obj2.toBytes(allocator);
    defer allocator.free(data2);

    const ref1 = try manager.allocateDistributed(1, @intCast(data1.len));
    const ref2 = try manager.allocateDistributed(1, @intCast(data2.len));

    try test_arena1.arena.writeGlobal(ref1, data1);
    try test_arena1.arena.writeGlobal(ref2, data2);

    // Start migration from shard 1 to shard 2
    try manager.startMigration(1, 2);

    // Check that shard 1 is now in migrating status
    try testing.expectEqual(ShardStatus.migrating, manager.getShardStatus(1));

    // Check migration progress
    const progress = manager.getMigrationProgress(1);
    try testing.expect(progress != null);
    try testing.expect(progress.? >= 0.0);

    // Perform incremental migration
    try manager.performIncrementalMigration(10);

    // Migration progress should be tracked
    const new_progress = manager.getMigrationProgress(1);
    try testing.expect(new_progress != null);
}

test "Distributed Arena: rebalancing workflow" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();
    var test_arena3 = try createTestArena(allocator, 3);
    defer test_arena3.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);
    try manager.addShard(3, test_arena3.arena);

    // Create imbalanced load (many objects in shard 1, few in others)
    for (0..20) |i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, 1, id, "Load test object", &[_]u8{}, &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        const ref = try manager.allocateDistributed(1, @intCast(data.len));
        try test_arena1.arena.writeGlobal(ref, data);
    }

    // Trigger rebalancing
    try manager.rebalanceShards();

    // Rebalancing should start migrations (we can't easily test completion in unit tests)
    // In a real implementation, we would verify that migrations were started
}

test "Distributed Arena: concurrent operations" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    // Test concurrent allocations (simplified - in real tests we'd use threads)
    const ref1 = try manager.allocateDistributed(1, 1000);
    const ref2 = try manager.allocateDistributed(2, 1000);
    const ref3 = try manager.allocateDistributed(1, 1000);
    const ref4 = try manager.allocateDistributed(2, 1000);

    // All allocations should succeed
    try testing.expect(!ref1.isNull());
    try testing.expect(!ref2.isNull());
    try testing.expect(!ref3.isNull());
    try testing.expect(!ref4.isNull());

    // References should be in correct shards
    try testing.expectEqual(@as(ShardId, 1), ref1.getShardId());
    try testing.expectEqual(@as(ShardId, 2), ref2.getShardId());
    try testing.expectEqual(@as(ShardId, 1), ref3.getShardId());
    try testing.expectEqual(@as(ShardId, 2), ref4.getShardId());
}

test "Distributed Arena: null reference handling" {
    const allocator = testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    const null_ref = GlobalObjRef.nullRef();

    // Navigation should return not_found
    const nav_result = manager.navigateDistributed(null_ref);
    try testing.expect(!nav_result.isLocal());
    try testing.expect(!nav_result.isRemote());

    // Cross-shard resolution should return null
    const resolved = try manager.resolveCrossShardRef(null_ref);
    try testing.expect(resolved == null);
}
